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
//! longer has an `.akit` lockfile, is filtered out of every read (in memory —
//! reads never rewrite the file, so a project on a temporarily unmounted volume
//! survives a `doctor --all`); the file itself is compacted on the next install.
//! An index file that is unreadable or of an unknown schema is treated as empty.
//! None of these is ever an error — the index is a rebuildable cache, not
//! ownership state.
//!
//! Transport: index I/O uses `std::fs` directly rather than the
//! [`crate::transport`] seam. That seam exists so an embedding host can drive a
//! *remote project root*; the index is host-local state about the machine akit
//! runs on (like the catalog itself and the remote cache), so it is always local.

use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use crate::catalog::Catalog;
use crate::harness::HarnessId;
use crate::install::{self, InstallOptions};
use crate::lockfile::{ItemType, Mode};
use crate::materialize::{
    self, Drift, MaterializeItem, check_drift, content_hash, materialize_one,
};
use crate::ownership::{AkitLockfile, Installation, MaterializationRecord};
use crate::project::Project;
use crate::reconcile::{self, ItemHealth, MaterializationHealth};
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
///
/// This is the one place the index is compacted: entries that no longer look
/// like akit projects are dropped as part of the rewrite an install performs
/// anyway. Reads never prune on disk (see [`known_projects_at`]).
pub fn record_install_at(index: &Path, root: &Path) -> Result<()> {
    let mut doc = InstallIndex::load(index);
    doc.projects
        .retain(|e| looks_like_akit_project(Path::new(&e.path)));
    doc.upsert(&canonical_key(root), now_secs());
    doc.save(index)
}

/// Every recorded project that still looks like an akit project.
///
/// A recorded path is filtered out when it no longer exists or has no
/// `.akit/kit.lock.json`. A path that exists but cannot be read is *kept*: that
/// may be a transient permission problem, and the consumer reports it as a
/// skipped project instead.
///
/// The filter is **in memory only**. `doctor --all` and `where` are documented
/// as strictly read-only, and "the root is not a directory right now" is not the
/// durable fact it looks like — a project on an unmounted volume or a detached
/// network share would be permanently forgotten by a command that promised to
/// only look. The index is compacted on the next [`record_install`] instead,
/// which is already writing it.
pub fn known_projects() -> Result<Vec<PathBuf>> {
    Ok(known_projects_at(&index_path()?))
}

/// [`known_projects`] against an explicit index file.
///
/// One project is returned once even if the index spells its path two ways (a
/// hand-written entry, or a symlinked root recorded before canonicalization) —
/// walking it twice would let a single project's own copies look like a
/// cross-project disagreement.
pub fn known_projects_at(index: &Path) -> Vec<PathBuf> {
    let mut seen: HashSet<String> = HashSet::new();
    InstallIndex::load(index)
        .projects
        .iter()
        .map(|e| PathBuf::from(&e.path))
        .filter(|p| looks_like_akit_project(p))
        .filter(|p| seen.insert(canonical_key(p)))
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
    /// Projects whose lockfile, health pass, or on-disk content could not be
    /// read. A skipped project contributes nothing else to this report.
    pub skipped: Vec<SkippedProject>,
    /// This item's **copy** installs across the projects above, one
    /// [`VariantGroup`] per comparable family of copies (issue #41).
    #[serde(default)]
    pub variants: Vec<VariantGroup>,
    /// True when any group holds different content in different projects.
    #[serde(default)]
    pub diverged: bool,
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
    let mut groups: BTreeMap<GroupKey, Vec<HashedCopy>> = BTreeMap::new();

    for root in known_projects_at(index) {
        let project = Project::at(&root);
        // Cheap filter first: only projects that actually record the item pay
        // for a health pass (which hashes every materialization).
        let lock = match AkitLockfile::load(&project.akit_lockfile_path()) {
            Ok(lock) => lock,
            Err(e) => {
                skipped.push(skip(&root, e));
                continue;
            }
        };
        let Some(inst) = lock.get(item_type, id).cloned() else {
            continue;
        };
        // The health pass first, and only a project that survives it contributes
        // anything: a project reported as skipped must not also be able to flip
        // `diverged` from content nobody could vouch for.
        let health = match reconcile::health(&project, catalog) {
            Ok(report) => report
                .items
                .into_iter()
                .find(|i| i.item_type == item_type && i.id == id),
            Err(e) => {
                skipped.push(skip(&root, e));
                continue;
            }
        };
        let Some(health) = health else { continue };

        // Health just determined drift for every materialization, so a clean
        // copy's recorded hash *is* its on-disk hash — no second walk of the tree.
        let candidates = candidates_of(&root, &inst, Some(&health.materializations));
        let (hashed, errors) = hash_candidates(&candidates);
        if let Some(error) = errors.into_iter().next() {
            skipped.push(error);
            continue;
        }
        for (key, copy) in hashed {
            groups.entry(key).or_default().push(copy);
        }
        projects.push(WhereProject {
            project: root.display().to_string(),
            health,
        });
    }

    projects.sort_by(|a, b| a.project.cmp(&b.project));
    skipped.sort_by(|a, b| a.project.cmp(&b.project));
    let variants: Vec<VariantGroup> = groups
        .into_iter()
        .map(|((_, _, harness), copies)| build_group(harness, &copies))
        .collect();
    Ok(WhereReport {
        id: id.to_string(),
        item_type,
        projects,
        skipped,
        diverged: variants.iter().any(|g| g.diverged),
        variants,
    })
}

// ── cross-project divergence ─────────────────────────────────────────────────

/// One distinct on-disk content of a copy install, and every path holding it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ContentVariant {
    /// The content hash — the same sha256 `drift` compares a copy against.
    pub hash: String,
    /// Absolute paths whose content hashes to `hash`, sorted.
    pub paths: Vec<String>,
}

/// One family of **comparable** copy installs of a single catalog id, with the
/// distinct contents found across them.
///
/// Comparability is what makes divergence meaningful, and it is not "same id":
///
///   - A **skill** materializes the whole skill directory, so every copy of it
///     is a copy of the same source no matter which destination a project's set
///     cover picked (`.agents/skills/x` here, `.claude/skills/x` there). One
///     group per skill, [`harness`](Self::harness) `None`.
///   - An **agent** materializes one *native per-harness variant file*
///     (`claude.md` vs `codex.toml` vs `github.agent.md`), so only copies
///     covering the same harness are copies of the same bytes. Pooling them by
///     id would report a perfectly clean multi-harness install as diverged
///     forever, and `update --propagate` could never resolve it (issue #41).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct VariantGroup {
    /// The harness whose native variant these copies came from, for agents.
    /// `None` for skills, whose copies all come from the one skill directory.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub harness: Option<HarnessId>,
    /// The distinct contents in this family, one entry per content hash.
    pub contents: Vec<ContentVariant>,
    /// True when two *different projects* hold different content here. Two
    /// copies inside a single project differing is ordinary local drift, which
    /// `status`/`doctor` already report as `modified` — and which
    /// `update --propagate` deliberately will not touch, so calling it a
    /// cross-project divergence would be an unactionable false positive.
    pub diverged: bool,
}

/// One comparable family of copy installs that **has** diverged across projects
/// (issue #41).
///
/// Divergence is a *conflict report*, not drift: each project may be perfectly
/// clean against its own recorded hash and still hold different bytes from the
/// next one (a copy installed before a catalog update, or edited in place). akit
/// never resolves this for you — `update --propagate` refreshes clean copies,
/// and an edited copy stays the user's.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Divergence {
    pub id: String,
    #[serde(rename = "type")]
    pub item_type: ItemType,
    /// The agent variant's harness; absent for skills (see [`VariantGroup`]).
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub harness: Option<HarnessId>,
    /// The distinct contents, always two or more.
    pub contents: Vec<ContentVariant>,
}

/// Outcome of the index-wide divergence sweep.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct DivergenceReport {
    /// Diverged families, sorted by type, then id, then harness.
    pub items: Vec<Divergence>,
    /// Projects that could not be read (unreadable lockfile, or a present but
    /// unhashable materialization). Such a project contributes nothing to
    /// `items` — its content is unknown, not proof of agreement.
    #[serde(default)]
    pub skipped: Vec<SkippedProject>,
}

/// Every comparable family of **copy** installs holding different content in
/// different known projects.
pub fn divergences() -> Result<DivergenceReport> {
    divergences_with(None)
}

/// [`divergences`], reusing drift a caller already computed for one project.
///
/// `doctor --all` has just diagnosed the local project, so passing
/// `(root, &diagnosis.items)` lets the sweep take the recorded hash of every
/// copy that pass proved clean instead of walking those trees a second time.
pub fn divergences_with(diagnosed: Option<(&Path, &[ItemHealth])>) -> Result<DivergenceReport> {
    Ok(divergences_at_with(&index_path()?, diagnosed))
}

/// [`divergences`] against an explicit index file.
pub fn divergences_at(index: &Path) -> DivergenceReport {
    divergences_at_with(index, None)
}

/// [`divergences_with`] against an explicit index file.
///
/// Read-only and lockfile-driven: a project that cannot be read is reported in
/// [`DivergenceReport::skipped`] rather than failing the scan, exactly as it is
/// skipped by `where`.
///
/// Cost: the lockfile pass is pure bookkeeping — nothing is hashed until a
/// family is known to hold copies in two or more projects, which is the only
/// shape that can be diverged. A single-project id (the common case) costs no
/// I/O beyond reading its lockfile.
pub fn divergences_at_with(
    index: &Path,
    diagnosed: Option<(&Path, &[ItemHealth])>,
) -> DivergenceReport {
    let mut groups: BTreeMap<GroupKey, Vec<Candidate>> = BTreeMap::new();
    let mut skipped: Vec<SkippedProject> = Vec::new();

    for root in known_projects_at(index) {
        let project = Project::at(&root);
        let lock = match AkitLockfile::load(&project.akit_lockfile_path()) {
            Ok(lock) => lock,
            Err(e) => {
                skipped.push(skip(&root, e));
                continue;
            }
        };
        let known = diagnosed
            .filter(|(diagnosed_root, _)| same_root(diagnosed_root, &root))
            .map(|(_, items)| items);
        for inst in &lock.items {
            let drift = known.and_then(|items| {
                items
                    .iter()
                    .find(|i| i.item_type == inst.item_type && i.id == inst.id)
                    .map(|i| i.materializations.as_slice())
            });
            for candidate in candidates_of(&root, inst, drift) {
                groups
                    .entry(candidate.key.clone())
                    .or_default()
                    .push(candidate);
            }
        }
    }

    let mut items = Vec::new();
    for ((item_type, id, harness), candidates) in groups {
        // Only a family spread over two or more projects can be diverged, so
        // everything else is dropped before any tree is hashed.
        if distinct_projects(&candidates) < 2 {
            continue;
        }
        let (hashed, errors) = hash_candidates(&candidates);
        skipped.extend(errors);
        let copies: Vec<HashedCopy> = hashed.into_iter().map(|(_, copy)| copy).collect();
        let group = build_group(harness, &copies);
        if group.diverged {
            items.push(Divergence {
                id,
                item_type,
                harness,
                contents: group.contents,
            });
        }
    }

    skipped.sort_by(|a, b| a.project.cmp(&b.project));
    skipped.dedup_by(|a, b| a.project == b.project);
    DivergenceReport { items, skipped }
}

/// What makes two copies comparable: the item they belong to plus which source
/// variant they were materialized from (see [`VariantGroup`]).
type GroupKey = (ItemType, String, Option<HarnessId>);

/// One **copy** materialization that could take part in a divergence.
#[derive(Debug, Clone)]
struct Candidate {
    key: GroupKey,
    /// The known project root it belongs to, as recorded in the index.
    project: PathBuf,
    /// Absolute path of the materialization.
    abs: PathBuf,
    /// The hash akit recorded at materialization time — the on-disk content
    /// only when a drift pass has vouched for it (`clean`).
    recorded: Option<String>,
    /// Whether a drift pass already proved the bytes match `recorded`.
    clean: bool,
}

/// One hashed copy: which project holds it, where, and what it contains.
#[derive(Debug, Clone)]
struct HashedCopy {
    project: String,
    path: String,
    hash: String,
}

/// Every **copy** materialization of `inst` as a divergence candidate, with the
/// drift a health pass already computed (when the caller has one).
///
/// Symlink installs are deliberately excluded: they resolve to the catalog, so
/// they always hold whatever the catalog holds and cannot diverge from it or
/// from each other.
fn candidates_of(
    root: &Path,
    inst: &Installation,
    drift: Option<&[MaterializationHealth]>,
) -> Vec<Candidate> {
    inst.materializations
        .iter()
        .filter(|m| m.mode == Mode::Copy)
        .map(|m| Candidate {
            key: (
                inst.item_type,
                inst.id.clone(),
                variant_key(inst.item_type, m),
            ),
            project: root.to_path_buf(),
            abs: root.join(&m.path),
            recorded: m.hash.clone(),
            clean: drift.is_some_and(|ms| {
                ms.iter()
                    .any(|h| h.path == m.path && h.drift == Drift::Clean)
            }),
        })
        .collect()
}

/// Which source variant a materialization is a copy of (see [`VariantGroup`]).
fn variant_key(item_type: ItemType, m: &MaterializationRecord) -> Option<HarnessId> {
    match item_type {
        ItemType::Skill => None,
        // An agent materialization is one harness's native variant file, and the
        // planner gives it exactly that harness as its coverage.
        ItemType::Agent => m.covers.first().copied(),
    }
}

/// How many distinct projects hold `candidates`.
fn distinct_projects(candidates: &[Candidate]) -> usize {
    candidates
        .iter()
        .map(|c| c.project.as_path())
        .collect::<HashSet<_>>()
        .len()
}

/// Hash every candidate, plus the projects that could not be read.
///
/// A project whose copy is present but unhashable (a dangling symlink inside the
/// tree, a permission wall) is reported *and* has all of its candidates dropped:
/// unknown content is not evidence of agreement, and must never be able to flip
/// a verdict either way. A candidate that is simply *gone* is not an error —
/// that is `repair`'s concern, already reported as `missing` drift.
fn hash_candidates(candidates: &[Candidate]) -> (Vec<(GroupKey, HashedCopy)>, Vec<SkippedProject>) {
    let mut hashed = Vec::new();
    let mut errors = Vec::new();
    let mut unreadable: Vec<&Path> = Vec::new();

    for c in candidates {
        match candidate_hash(c) {
            Ok(Some(hash)) => hashed.push((
                c.key.clone(),
                HashedCopy {
                    project: c.project.display().to_string(),
                    path: c.abs.display().to_string(),
                    hash,
                },
            )),
            Ok(None) => {}
            Err(e) => {
                if !unreadable.contains(&c.project.as_path()) {
                    unreadable.push(c.project.as_path());
                    errors.push(skip(&c.project, e));
                }
            }
        }
    }

    if !unreadable.is_empty() {
        let dropped: Vec<String> = unreadable.iter().map(|p| p.display().to_string()).collect();
        hashed.retain(|(_, copy)| !dropped.contains(&copy.project));
    }
    (hashed, errors)
}

/// The on-disk content hash of a candidate: the recorded hash when a drift pass
/// already proved the copy clean (no second walk of the tree), otherwise a fresh
/// hash. `None` when nothing is there any more.
fn candidate_hash(c: &Candidate) -> Result<Option<String>> {
    if c.clean
        && let Some(hash) = &c.recorded
    {
        return Ok(Some(hash.clone()));
    }
    if !materialize::is_occupied(&LocalFs, &c.abs)? {
        return Ok(None);
    }
    content_hash(&LocalFs, &c.abs).map(Some)
}

/// Fold hashed copies into deterministic content groups and decide whether they
/// disagree *across projects*.
fn build_group(harness: Option<HarnessId>, copies: &[HashedCopy]) -> VariantGroup {
    let mut contents: BTreeMap<&str, Vec<String>> = BTreeMap::new();
    let mut by_project: BTreeMap<&str, BTreeSet<&str>> = BTreeMap::new();
    for copy in copies {
        contents
            .entry(copy.hash.as_str())
            .or_default()
            .push(copy.path.clone());
        by_project
            .entry(copy.project.as_str())
            .or_default()
            .insert(copy.hash.as_str());
    }
    // Two projects disagree exactly when they hold different sets of content;
    // several copies of one item inside a single project are local drift.
    let distinct: BTreeSet<&BTreeSet<&str>> = by_project.values().collect();
    VariantGroup {
        harness,
        contents: contents
            .into_iter()
            .map(|(hash, mut paths)| {
                paths.sort();
                paths.dedup();
                ContentVariant {
                    hash: hash.to_string(),
                    paths,
                }
            })
            .collect(),
        diverged: distinct.len() > 1,
    }
}

/// Whether two recorded roots name the same project, tolerating symlinked and
/// non-canonical index entries.
fn same_root(a: &Path, b: &Path) -> bool {
    a == b
        || matches!(
            (a.canonicalize(), b.canonicalize()),
            (Ok(x), Ok(y)) if x == y
        )
}

/// A project the caller could not inspect.
fn skip(root: &Path, error: anyhow::Error) -> SkippedProject {
    SkippedProject {
        project: root.display().to_string(),
        error: format!("{error:#}"),
    }
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
