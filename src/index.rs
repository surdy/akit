//! The global install index (issue #40).
//!
//! Each project's `.akit/kit.lock.json` stays the single source of truth for
//! *what* is installed there. This index records only **which project
//! directories akit has installed into**, which is what turns a set of isolated
//! per-project lockfiles into two cross-project views usable from any cwd:
//!
//!   - [`locate`] — `akit where <id>`: every known project holding an item, with
//!     that item's per-harness health in each.
//!   - [`propagate`] — `akit update --propagate`: after the catalog is refreshed,
//!     re-materialize copy-mode installs of the refreshed items in those projects.
//!
//! Scope + privacy: the file lives next to the catalog under `~/.akit` (or
//! `$AKIT_STATE_DIR`), never inside a project and never in git. It holds project
//! paths and a timestamp — no item ids, no file contents. Deleting it is safe:
//! akit re-learns paths on the next install, and `where`/`propagate` just see
//! fewer projects until then.
//!
//! Reads are self-healing. A recorded path that has disappeared, or that no
//! longer has an `.akit` lockfile, is pruned on the next read; an index file that
//! is unreadable or of an unknown schema is treated as empty. None of these is
//! ever an error — the index is a rebuildable cache, not ownership state.
//!
//! Transport: index I/O uses `std::fs` directly rather than the
//! [`crate::transport`] seam. That seam exists so an embedding host can drive a
//! *remote project root*; the index is host-local state about the machine akit
//! runs on (like the catalog itself and the remote cache), so it is always local.

use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use crate::catalog::Catalog;
use crate::install::{self, InstallOptions};
use crate::lockfile::{ItemType, Mode};
use crate::materialize::{Drift, MaterializeItem, check_drift, content_hash, materialize_one};
use crate::ownership::AkitLockfile;
use crate::project::Project;
use crate::reconcile::{self, ItemHealth};
use crate::transport::LocalFs;

/// Environment variable that overrides the host state directory.
pub const ENV_STATE_DIR: &str = "AKIT_STATE_DIR";

/// Filename of the index inside the state directory.
pub const INDEX_FILE: &str = "installs.json";

/// Current on-disk schema version of the index.
pub const INDEX_VERSION: u32 = 1;

/// The host state directory: `$AKIT_STATE_DIR`, else `~/.akit/state`.
pub fn state_dir() -> Result<PathBuf> {
    match std::env::var_os(ENV_STATE_DIR) {
        Some(v) if !v.is_empty() => Ok(PathBuf::from(v)),
        _ => {
            let home = dirs::home_dir().context("could not determine home directory")?;
            Ok(home.join(".akit").join("state"))
        }
    }
}

/// Absolute path of the index file (`<state dir>/installs.json`).
pub fn index_path() -> Result<PathBuf> {
    Ok(state_dir()?.join(INDEX_FILE))
}

/// One recorded project directory.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProjectEntry {
    /// Absolute project root (canonicalized when it could be resolved).
    pub path: String,
    /// Unix timestamp (seconds) of the most recent install recorded here.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub last_install: Option<u64>,
}

fn default_version() -> u32 {
    INDEX_VERSION
}

/// The `installs.json` document: project paths only.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InstallIndex {
    #[serde(default = "default_version")]
    pub version: u32,
    #[serde(default)]
    pub projects: Vec<ProjectEntry>,
}

impl Default for InstallIndex {
    fn default() -> Self {
        Self {
            version: INDEX_VERSION,
            projects: Vec::new(),
        }
    }
}

impl InstallIndex {
    /// Load the index at `path`.
    ///
    /// Unlike the project lockfile, nothing here is authoritative, so every
    /// failure mode — absent, unreadable, malformed, unknown schema version —
    /// degrades to an empty index instead of erroring. The worst case is that
    /// akit forgets some project paths and re-learns them on the next install.
    pub fn load(path: &Path) -> Self {
        let Ok(text) = std::fs::read_to_string(path) else {
            return Self::default();
        };
        match serde_json::from_str::<Self>(&text) {
            Ok(doc) if doc.version == INDEX_VERSION => doc,
            _ => Self::default(),
        }
    }

    /// Persist the index to `path`, creating the state directory as needed.
    ///
    /// Written to a sibling temp and renamed into place so a concurrent reader
    /// never observes a half-written index.
    pub fn save(&self, path: &Path) -> Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("creating {}", parent.display()))?;
        }
        let mut text = serde_json::to_string_pretty(self).context("serializing install index")?;
        text.push('\n');
        let temp = path.with_extension("json.tmp");
        std::fs::write(&temp, text.as_bytes())
            .with_context(|| format!("writing {}", temp.display()))?;
        std::fs::rename(&temp, path).with_context(|| format!("writing {}", path.display()))
    }

    /// Insert `path`, or refresh the timestamp of the entry already holding it.
    pub fn upsert(&mut self, path: &str, now: Option<u64>) {
        match self.projects.iter_mut().find(|e| e.path == path) {
            Some(entry) => entry.last_install = now,
            None => self.projects.push(ProjectEntry {
                path: path.to_string(),
                last_install: now,
            }),
        }
        self.projects.sort_by(|a, b| a.path.cmp(&b.path));
    }
}

/// Record `root` as a project akit has installed into.
///
/// Called after a successful install of any shape (local id, remote source, or
/// `--bundle`). Uninstall/reset deliberately do **not** remove the entry: a
/// project whose lockfile is emptied simply stops matching `where` and
/// `propagate`, and a re-install would only have to re-learn the path.
pub fn record_install(root: &Path) -> Result<()> {
    record_install_at(&index_path()?, root)
}

/// [`record_install`] against an explicit index file (tests / embedding hosts
/// that keep their own state directory).
pub fn record_install_at(index: &Path, root: &Path) -> Result<()> {
    let mut doc = InstallIndex::load(index);
    doc.upsert(&canonical_key(root), now_secs());
    doc.save(index)
}

/// Every recorded project that still looks like an akit project, pruning the
/// rest from the index in place.
///
/// A recorded path is dropped when it no longer exists or has no
/// `.akit/kit.lock.json` — both durable facts (the project was deleted, or akit
/// was fully removed from it). A path that exists but cannot be read is *kept*:
/// that may be a transient permission problem, and the consumer reports it as a
/// skipped project instead. Pruning is best-effort: if the rewrite fails, the
/// pruned view is still returned.
pub fn known_projects() -> Result<Vec<PathBuf>> {
    Ok(known_projects_at(&index_path()?))
}

/// [`known_projects`] against an explicit index file.
pub fn known_projects_at(index: &Path) -> Vec<PathBuf> {
    let mut doc = InstallIndex::load(index);
    let before = doc.projects.len();
    doc.projects
        .retain(|e| looks_like_akit_project(Path::new(&e.path)));
    if doc.projects.len() != before {
        // Best-effort: a failed prune must never fail the read.
        let _ = doc.save(index);
    }
    doc.projects
        .iter()
        .map(|e| PathBuf::from(&e.path))
        .collect()
}

/// Whether a recorded path still has an akit lockfile to consult.
fn looks_like_akit_project(root: &Path) -> bool {
    root.is_dir() && Project::at(root).akit_lockfile_path().is_file()
}

/// The index key for a project root: its canonical path when resolvable, else
/// the path as given (a project that vanished still matches its old entry).
fn canonical_key(root: &Path) -> String {
    root.canonicalize()
        .unwrap_or_else(|_| root.to_path_buf())
        .to_string_lossy()
        .into_owned()
}

fn now_secs() -> Option<u64> {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .ok()
        .map(|d| d.as_secs())
}

// ── where ────────────────────────────────────────────────────────────────────

/// One known project that has the item installed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WhereProject {
    /// Absolute project root.
    pub project: String,
    /// The item's health in that project: harnesses, per-materialization mode +
    /// drift, catalog source presence, and whether coverage is degraded.
    pub health: ItemHealth,
}

/// A known project that could not be inspected (kept in the index, skipped here).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SkippedProject {
    pub project: String,
    pub error: String,
}

/// Outcome of `akit where <id>`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WhereReport {
    pub id: String,
    #[serde(rename = "type")]
    pub item_type: ItemType,
    /// Projects holding the item, sorted by path. A known project without it
    /// simply does not appear.
    pub projects: Vec<WhereProject>,
    /// Projects whose lockfile could not be read.
    pub skipped: Vec<SkippedProject>,
}

/// Find every known project whose `.akit` lockfile records `(item_type, id)`,
/// with the item's health in each.
///
/// Index-driven, so it works from any cwd and never needs a project of its own.
/// An unreadable project is reported in `skipped` rather than failing the run.
pub fn locate(catalog: &Catalog, item_type: ItemType, id: &str) -> Result<WhereReport> {
    locate_at(&index_path()?, catalog, item_type, id)
}

/// [`locate`] against an explicit index file.
pub fn locate_at(
    index: &Path,
    catalog: &Catalog,
    item_type: ItemType,
    id: &str,
) -> Result<WhereReport> {
    let mut projects = Vec::new();
    let mut skipped = Vec::new();

    for root in known_projects_at(index) {
        let project = Project::at(&root);
        // Cheap filter first: only projects that actually record the item pay
        // for a health pass (which hashes every materialization).
        match AkitLockfile::load(&project.akit_lockfile_path()) {
            Ok(lock) => {
                if lock.get(item_type, id).is_none() {
                    continue;
                }
            }
            Err(e) => {
                skipped.push(SkippedProject {
                    project: root.display().to_string(),
                    error: format!("{e:#}"),
                });
                continue;
            }
        }
        match reconcile::health(&project, catalog) {
            Ok(report) => {
                if let Some(health) = report
                    .items
                    .into_iter()
                    .find(|i| i.item_type == item_type && i.id == id)
                {
                    projects.push(WhereProject {
                        project: root.display().to_string(),
                        health,
                    });
                }
            }
            Err(e) => skipped.push(SkippedProject {
                project: root.display().to_string(),
                error: format!("{e:#}"),
            }),
        }
    }

    projects.sort_by(|a, b| a.project.cmp(&b.project));
    skipped.sort_by(|a, b| a.project.cmp(&b.project));
    Ok(WhereReport {
        id: id.to_string(),
        item_type,
        projects,
        skipped,
    })
}

// ── propagate ────────────────────────────────────────────────────────────────

/// What propagation did to one materialization.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum PropagateStatus {
    /// A clean copy that lagged the refreshed catalog was re-materialized.
    Updated,
    /// The copy already matches the catalog source; nothing to do.
    UpToDate,
    /// The copy was edited locally (bytes ≠ recorded hash) — a conflict, left
    /// untouched, exactly like `repair`'s no-clobber policy.
    Drifted,
    /// A symlink install: it already tracks the catalog live, so it is skipped.
    Symlink,
    /// The materialization is gone; restoring it is `akit repair`'s job.
    Missing,
    /// This materialization could not be processed (see `error`).
    Error,
}

/// One materialization considered during propagation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PropagatedPath {
    /// Project-relative materialization path.
    pub path: String,
    pub mode: Mode,
    pub status: PropagateStatus,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub error: Option<String>,
}

/// One installed item considered during propagation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PropagatedItem {
    pub id: String,
    #[serde(rename = "type")]
    pub item_type: ItemType,
    pub materializations: Vec<PropagatedPath>,
    /// Set when the whole item was skipped (e.g. its catalog source is gone).
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub error: Option<String>,
}

/// Propagation outcome for one known project.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProjectPropagation {
    /// Absolute project root.
    pub project: String,
    pub items: Vec<PropagatedItem>,
    /// Set when the project was skipped (unreadable lockfile) or its lockfile
    /// could not be rewritten after re-materializing.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub error: Option<String>,
}

/// Aggregate counts for a propagation run.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct PropagateSummary {
    /// Known projects inspected (including ones with nothing to do).
    pub projects: usize,
    pub updated: usize,
    pub up_to_date: usize,
    pub drifted: usize,
    pub symlink: usize,
    pub missing: usize,
    /// Item- and path-level failures plus skipped projects.
    pub errors: usize,
}

/// Outcome of `akit update --propagate`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct PropagationReport {
    /// Only projects with something to report appear here.
    pub projects: Vec<ProjectPropagation>,
    pub summary: PropagateSummary,
}

/// Re-sync installs of `targets` across every known project against the current
/// (just-refreshed) catalog.
///
/// Policy, mirroring `repair`'s no-clobber rules:
///   - a **copy** whose bytes still match its recorded hash but whose catalog
///     source has moved is re-materialized through the atomic stage-and-rename
///     transaction, and its recorded hash is updated;
///   - a **drifted** copy (bytes ≠ recorded hash) is reported and never touched;
///   - a **symlink** already resolves to the catalog, so it is reported as live
///     and skipped;
///   - a **missing** materialization is reported (that is `akit repair`'s job).
///
/// A project that cannot be read, or an item whose catalog source disappeared, is
/// reported and skipped — never fatal. Local projects only: the index records
/// paths on this host, so this runs against [`LocalFs`].
pub fn propagate(catalog: &Catalog, targets: &[(ItemType, String)]) -> Result<PropagationReport> {
    propagate_at(&index_path()?, catalog, targets)
}

/// [`propagate`] against an explicit index file.
pub fn propagate_at(
    index: &Path,
    catalog: &Catalog,
    targets: &[(ItemType, String)],
) -> Result<PropagationReport> {
    let mut report = PropagationReport::default();
    if targets.is_empty() {
        return Ok(report);
    }

    for root in known_projects_at(index) {
        report.summary.projects += 1;
        let project = Project::at(&root);
        let lf_path = project.akit_lockfile_path();
        let mut lock = match AkitLockfile::load(&lf_path) {
            Ok(lock) => lock,
            Err(e) => {
                report.summary.errors += 1;
                report.projects.push(ProjectPropagation {
                    project: root.display().to_string(),
                    items: Vec::new(),
                    error: Some(format!("{e:#}")),
                });
                continue;
            }
        };

        let mut items = Vec::new();
        let mut lock_dirty = false;
        for inst in &mut lock.items {
            if !targets
                .iter()
                .any(|(ty, id)| *ty == inst.item_type && id == &inst.id)
            {
                continue;
            }
            // Re-plan for the item's recorded harness set to resolve each
            // materialization back to its catalog source.
            let planned = install::build_plan(
                catalog,
                inst.item_type,
                &inst.id,
                &inst.harnesses,
                InstallOptions::default(),
            );
            let (plan, resolver) = match planned {
                Ok(pair) => pair,
                Err(e) => {
                    report.summary.errors += 1;
                    items.push(PropagatedItem {
                        id: inst.id.clone(),
                        item_type: inst.item_type,
                        materializations: Vec::new(),
                        error: Some(format!("{e:#}")),
                    });
                    continue;
                }
            };

            let mut paths = Vec::new();
            for record in &mut inst.materializations {
                // Only act on paths the current plan still describes; a recorded
                // path the plan no longer produces is a reshape/repair concern.
                let Some(target) = plan.materializations.iter().find(|p| p.path == record.path)
                else {
                    continue;
                };
                let item = MaterializeItem {
                    source: resolver(target),
                    planned: target,
                };
                let (status, error, new_hash) = propagate_one(&project, record, &item);
                if let Some(hash) = new_hash {
                    record.hash = Some(hash);
                    lock_dirty = true;
                }
                match status {
                    PropagateStatus::Updated => report.summary.updated += 1,
                    PropagateStatus::UpToDate => report.summary.up_to_date += 1,
                    PropagateStatus::Drifted => report.summary.drifted += 1,
                    PropagateStatus::Symlink => report.summary.symlink += 1,
                    PropagateStatus::Missing => report.summary.missing += 1,
                    PropagateStatus::Error => report.summary.errors += 1,
                }
                paths.push(PropagatedPath {
                    path: record.path.clone(),
                    mode: record.mode,
                    status,
                    error,
                });
            }

            items.push(PropagatedItem {
                id: inst.id.clone(),
                item_type: inst.item_type,
                materializations: paths,
                error: None,
            });
        }

        // Only the recorded hashes changed; owned paths (and therefore the
        // managed exclude block) are untouched. A failed write is reported
        // against the project — the files are already refreshed, so the rest of
        // the run must still complete.
        let mut project_error = None;
        if lock_dirty && let Err(e) = lock.save(&lf_path) {
            report.summary.errors += 1;
            project_error = Some(format!("{e:#}"));
        }
        if !items.is_empty() {
            report.projects.push(ProjectPropagation {
                project: root.display().to_string(),
                items,
                error: project_error,
            });
        }
    }

    report.projects.sort_by(|a, b| a.project.cmp(&b.project));
    Ok(report)
}

/// Decide and apply propagation for a single recorded materialization, returning
/// its status, any error text, and the new hash to record when it was rewritten.
fn propagate_one(
    project: &Project,
    record: &crate::ownership::MaterializationRecord,
    item: &MaterializeItem<'_>,
) -> (PropagateStatus, Option<String>, Option<String>) {
    if record.mode == Mode::Symlink {
        return (PropagateStatus::Symlink, None, None);
    }
    // A copy with no recorded hash cannot be shown to be unmodified, so it is
    // treated as a conflict rather than silently overwritten.
    let Some(recorded) = record.hash.as_deref() else {
        return (
            PropagateStatus::Drifted,
            Some("copy has no recorded hash; cannot verify it is unmodified".to_string()),
            None,
        );
    };
    match check_drift(&LocalFs, &project.root, record) {
        Ok(Drift::Missing) => (PropagateStatus::Missing, None, None),
        Ok(Drift::Modified) => (PropagateStatus::Drifted, None, None),
        Ok(Drift::Clean) => match content_hash(&LocalFs, &item.source) {
            Ok(source_hash) if source_hash == recorded => (PropagateStatus::UpToDate, None, None),
            Ok(source_hash) => match materialize_one(&LocalFs, &project.root, item) {
                Ok(_) => (PropagateStatus::Updated, None, Some(source_hash)),
                Err(e) => (PropagateStatus::Error, Some(format!("{e:#}")), None),
            },
            Err(e) => (PropagateStatus::Error, Some(format!("{e:#}")), None),
        },
        Err(e) => (PropagateStatus::Error, Some(format!("{e:#}")), None),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn load_tolerates_absent_corrupt_and_unknown_schema() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join(INDEX_FILE);
        assert!(InstallIndex::load(&path).projects.is_empty());

        std::fs::write(&path, "{ not json").unwrap();
        assert!(InstallIndex::load(&path).projects.is_empty());

        std::fs::write(&path, r#"{"version":99,"projects":[{"path":"/x"}]}"#).unwrap();
        assert!(InstallIndex::load(&path).projects.is_empty());
    }

    #[test]
    fn upsert_adds_once_then_refreshes_and_keeps_paths_sorted() {
        let mut index = InstallIndex::default();
        index.upsert("/b", Some(1));
        index.upsert("/a", Some(2));
        index.upsert("/b", Some(3));
        assert_eq!(index.projects.len(), 2);
        assert_eq!(index.projects[0].path, "/a");
        assert_eq!(index.projects[1].last_install, Some(3));
    }

    #[test]
    fn roundtrips_through_disk() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("state").join(INDEX_FILE);
        let mut index = InstallIndex::default();
        index.upsert("/some/project", Some(42));
        index.save(&path).unwrap();
        assert_eq!(InstallIndex::load(&path), index);
        // The temp used for the atomic write is not left behind.
        assert!(!path.with_extension("json.tmp").exists());
    }

    #[test]
    fn stale_paths_are_recognized_as_prunable() {
        let tmp = TempDir::new().unwrap();
        let live = tmp.path().join("live");
        std::fs::create_dir_all(live.join(".akit")).unwrap();
        std::fs::write(
            live.join(".akit/kit.lock.json"),
            r#"{"version":2,"items":[]}"#,
        )
        .unwrap();
        assert!(looks_like_akit_project(&live));

        // Exists but akit was fully removed from it.
        let no_lock = tmp.path().join("no-lock");
        std::fs::create_dir_all(&no_lock).unwrap();
        assert!(!looks_like_akit_project(&no_lock));

        // Gone entirely.
        assert!(!looks_like_akit_project(&tmp.path().join("vanished")));
    }
}
