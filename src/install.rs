//! Harness-aware install orchestration — the v0.10 embedding API (issue #34).
//!
//! This is the high-level surface an embedding host (madari) and the CLI call to
//! install, adjust, and remove kits **for an explicit set of harnesses**. It ties
//! the pieces together:
//!
//! plan ([`crate::plan`]) → materialize ([`crate::materialize`]) →
//! record ownership ([`crate::ownership`]) → git-exclude ([`crate::gitexclude`]).
//!
//! The harness selection is passed as an explicit, immutable [`HarnessContext`]
//! — never read from a process-global env inside the library — so concurrent
//! callers (e.g. two madari panes) never race on ambient state.
//!
//! [`install`] is *absolute*: it makes an item's installed harness set exactly
//! the context's set, reconciling materializations (adding newly needed files,
//! removing now-unneeded ones). Adding or dropping a harness is therefore just
//! an install with the new set, which re-runs the planner and reshapes the files
//! optimally.

use std::path::PathBuf;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use crate::catalog::Catalog;
use crate::gitexclude;
use crate::harness::HarnessId;
use crate::lockfile::ItemType;
use crate::materialize::{MaterializeItem, materialize_one, remove_materialization};
use crate::ownership::{AKIT_LOCKFILE_REL, AkitLockfile, Installation, MaterializationRecord};
use crate::plan::{self, Plan, PlanIssue};
use crate::project::Project;
use crate::transport::{FsTransport, LocalFs};

/// The explicit, immutable set of harnesses an operation targets.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HarnessContext {
    harnesses: Vec<HarnessId>,
}

impl HarnessContext {
    /// Build a context from a non-empty set of harnesses (deduped + sorted).
    pub fn new(harnesses: impl IntoIterator<Item = HarnessId>) -> Result<Self> {
        let mut harnesses: Vec<HarnessId> = harnesses.into_iter().collect();
        harnesses.sort();
        harnesses.dedup();
        if harnesses.is_empty() {
            anyhow::bail!("at least one target harness is required");
        }
        Ok(Self { harnesses })
    }

    /// The targeted harnesses (sorted, deduped, non-empty).
    pub fn harnesses(&self) -> &[HarnessId] {
        &self.harnesses
    }
}

/// Which materializations a remove touches.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RemoveScope {
    /// Uninstall the item from every harness (drops the installation).
    All,
    /// Uninstall only from these harnesses, keeping the rest (re-plans + reshapes).
    Harnesses(Vec<HarnessId>),
}

/// Outcome of an [`install`] (or a partial [`remove`] that re-plans).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InstallReport {
    pub id: String,
    pub item_type: ItemType,
    /// The harnesses actually served after the operation (sorted).
    pub harnesses: Vec<HarnessId>,
    /// Physical materializations now backing the installation.
    pub materializations: Vec<MaterializationRecord>,
    /// Selected harnesses that could not be served, with reasons.
    pub issues: Vec<PlanIssue>,
    /// Whether an existing installation was replaced.
    pub replaced: bool,
    /// True if the project is not a git repo (materializations can't be excluded).
    pub not_a_git_repo: bool,
}

/// Outcome of a full or scoped [`remove`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RemoveReport {
    pub id: String,
    pub item_type: ItemType,
    /// Materialization paths physically removed.
    pub removed_paths: Vec<String>,
    /// The harnesses still served afterwards (empty when fully uninstalled).
    pub remaining_harnesses: Vec<HarnessId>,
    /// Whether the item had no installation to begin with.
    pub not_installed: bool,
}

/// Outcome of [`reset`].
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct ResetReport {
    /// Every materialization path removed, across all installations.
    pub removed_paths: Vec<String>,
    /// Number of logical installations cleared.
    pub cleared_items: usize,
}

/// Install (or reshape) `id` for exactly the harnesses in `ctx`, using the local
/// filesystem transport.
pub fn install(
    project: &Project,
    catalog: &Catalog,
    item_type: ItemType,
    id: &str,
    ctx: &HarnessContext,
) -> Result<InstallReport> {
    install_with(&LocalFs, project, catalog, item_type, id, ctx)
}

/// [`install`] against an explicit destination transport (for embedding hosts).
pub fn install_with(
    fs: &dyn FsTransport,
    project: &Project,
    catalog: &Catalog,
    item_type: ItemType,
    id: &str,
    ctx: &HarnessContext,
) -> Result<InstallReport> {
    let (plan, resolver) = build_plan(catalog, item_type, id, ctx.harnesses())?;
    reconcile(fs, project, item_type, id, "local", &plan, &resolver)
}

/// Remove `id` from some or all harnesses.
pub fn remove(
    project: &Project,
    item_type: ItemType,
    id: &str,
    scope: RemoveScope,
) -> Result<RemoveReport> {
    remove_with(&LocalFs, project, item_type, id, scope)
}

/// [`remove`] against an explicit transport.
pub fn remove_with(
    fs: &dyn FsTransport,
    project: &Project,
    item_type: ItemType,
    id: &str,
    scope: RemoveScope,
) -> Result<RemoveReport> {
    let lf_path = project.akit_lockfile_path();
    let lock = AkitLockfile::load(&lf_path)?;
    let Some(existing) = lock.get(item_type, id).cloned() else {
        return Ok(RemoveReport {
            id: id.to_string(),
            item_type,
            removed_paths: Vec::new(),
            remaining_harnesses: Vec::new(),
            not_installed: true,
        });
    };

    let remaining: Vec<HarnessId> = match &scope {
        RemoveScope::All => Vec::new(),
        RemoveScope::Harnesses(drop) => existing
            .harnesses
            .iter()
            .copied()
            .filter(|h| !drop.contains(h))
            .collect(),
    };

    if remaining.is_empty() {
        // Full uninstall: remove every materialization, drop entry, resync excludes.
        let mut lock = lock;
        let removed = lock.remove(item_type, id).expect("checked present");
        let mut removed_paths = Vec::new();
        for m in &removed.materializations {
            if remove_materialization(fs, &project.root, &m.path)? {
                removed_paths.push(m.path.clone());
            }
        }
        prune_empty_owned_dirs(fs, project, &removed.materializations);
        lock.save(&lf_path)?;
        sync_excludes(project, &lock)?;
        Ok(RemoveReport {
            id: id.to_string(),
            item_type,
            removed_paths,
            remaining_harnesses: Vec::new(),
            not_installed: false,
        })
    } else {
        // Partial: re-plan for the reduced set and reshape.
        let cat = Catalog::locate()?;
        let ctx = HarnessContext::new(remaining)?;
        let (plan, resolver) = build_plan(&cat, item_type, id, ctx.harnesses())?;
        let report = reconcile(
            fs,
            project,
            item_type,
            id,
            &existing.source,
            &plan,
            &resolver,
        )?;
        let removed_paths = existing
            .materializations
            .iter()
            .map(|m| m.path.clone())
            .filter(|p| !report.materializations.iter().any(|m| &m.path == p))
            .collect();
        Ok(RemoveReport {
            id: id.to_string(),
            item_type,
            removed_paths,
            remaining_harnesses: report.harnesses,
            not_installed: false,
        })
    }
}

/// Remove *every* akit-owned materialization in the project and clear the
/// lockfile. Only files akit recorded are touched.
pub fn reset(project: &Project) -> Result<ResetReport> {
    reset_with(&LocalFs, project)
}

/// [`reset`] against an explicit transport.
pub fn reset_with(fs: &dyn FsTransport, project: &Project) -> Result<ResetReport> {
    let lf_path = project.akit_lockfile_path();
    let mut lock = AkitLockfile::load(&lf_path)?;
    let mut report = ResetReport::default();
    let mut all_removed: Vec<MaterializationRecord> = Vec::new();
    for item in &lock.items {
        report.cleared_items += 1;
        for m in &item.materializations {
            if remove_materialization(fs, &project.root, &m.path)? {
                report.removed_paths.push(m.path.clone());
            }
            all_removed.push(m.clone());
        }
    }
    prune_empty_owned_dirs(fs, project, &all_removed);
    lock.items.clear();
    lock.save(&lf_path)?;
    // Empty lockfile → the managed exclude block is removed entirely.
    sync_excludes(project, &lock)?;
    Ok(report)
}

/// Read-only status of every installed item, with per-materialization drift.
pub fn status(project: &Project) -> Result<Vec<Installation>> {
    let lock = AkitLockfile::load(&project.akit_lockfile_path())?;
    Ok(lock.items)
}

// ── internals ────────────────────────────────────────────────────────────────

/// A closure that resolves a planned materialization to its absolute source.
pub(crate) type SourceResolver = Box<dyn Fn(&plan::PlannedMaterialization) -> PathBuf>;

pub(crate) fn build_plan(
    catalog: &Catalog,
    item_type: ItemType,
    id: &str,
    harnesses: &[HarnessId],
) -> Result<(Plan, SourceResolver)> {
    match item_type {
        ItemType::Skill => {
            let src = catalog.resolve_skill(id)?;
            let compat = catalog.skill_compat(id)?;
            let plan = plan::plan_skill(id, harnesses, &compat);
            let resolver: SourceResolver = Box::new(move |_planned| src.clone());
            Ok((plan, resolver))
        }
        ItemType::Agent => {
            let pkg = catalog.resolve_agent_package(id)?;
            let plan = plan::plan_agent(&pkg, harnesses);
            let dir = pkg.dir.clone();
            let resolver: SourceResolver = Box::new(move |planned| {
                let rel = planned
                    .source_file
                    .as_deref()
                    .expect("agent materializations carry a source file");
                dir.join(rel)
            });
            Ok((plan, resolver))
        }
    }
}

/// Materialize `plan`, remove stale materializations from a prior install, and
/// update ownership + git excludes accordingly.
fn reconcile(
    fs: &dyn FsTransport,
    project: &Project,
    item_type: ItemType,
    id: &str,
    source: &str,
    plan: &Plan,
    resolver: &SourceResolver,
) -> Result<InstallReport> {
    let lf_path = project.akit_lockfile_path();
    let mut lock = AkitLockfile::load(&lf_path)?;
    let previous = lock.get(item_type, id).cloned();

    // Materialize everything the plan asks for.
    let mut records = Vec::new();
    for planned in &plan.materializations {
        let source_path = resolver(planned);
        let record = materialize_one(
            fs,
            &project.root,
            &MaterializeItem {
                source: source_path,
                planned,
            },
        )
        .with_context(|| format!("installing '{id}' at {}", planned.path))?;
        records.push(record);
    }

    let new_paths: Vec<&str> = records.iter().map(|r| r.path.as_str()).collect();

    // Remove any prior materialization that the new plan no longer includes.
    if let Some(prev) = &previous {
        let mut stale = Vec::new();
        for m in &prev.materializations {
            if !new_paths.contains(&m.path.as_str()) {
                remove_materialization(fs, &project.root, &m.path)?;
                stale.push(m.clone());
            }
        }
        prune_empty_owned_dirs(fs, project, &stale);
    }

    let not_a_git_repo = project.git_dir.is_none();

    if records.is_empty() {
        // Nothing servable: drop any prior installation rather than keep an empty one.
        if previous.is_some() {
            lock.remove(item_type, id);
            lock.save(&lf_path)?;
            sync_excludes(project, &lock)?;
        }
        return Ok(InstallReport {
            id: id.to_string(),
            item_type,
            harnesses: Vec::new(),
            materializations: Vec::new(),
            issues: plan.issues.clone(),
            replaced: previous.is_some(),
            not_a_git_repo,
        });
    }

    let harnesses = plan.served();
    let installation = Installation {
        id: id.to_string(),
        item_type,
        source: source.to_string(),
        git_ref: None,
        bundle: None,
        harnesses: harnesses.clone(),
        materializations: records.clone(),
    };
    let replaced = lock.upsert(installation);
    lock.save(&lf_path)?;
    // Recompute the managed exclude block from the lockfile (adds new lines,
    // prunes ones the reshape dropped, and excludes the lockfile itself).
    sync_excludes(project, &lock)?;

    Ok(InstallReport {
        id: id.to_string(),
        item_type,
        harnesses,
        materializations: records,
        issues: plan.issues.clone(),
        replaced,
        not_a_git_repo,
    })
}

/// The desired akit-managed exclude lines for `lock`: every owned materialization
/// path plus the lockfile itself, each as a `/`-anchored line. Empty when nothing
/// is installed (so the managed block is removed entirely).
pub(crate) fn desired_excludes(lock: &AkitLockfile) -> Vec<String> {
    if lock.items.is_empty() {
        return Vec::new();
    }
    let mut lines: Vec<String> = lock
        .owned_paths()
        .into_iter()
        .map(|p| format!("/{p}"))
        .collect();
    lines.push(format!("/{AKIT_LOCKFILE_REL}"));
    lines
}

/// Rewrite the project's akit-managed git-exclude block to match `lock`. This is
/// the single exclude mutation used across install, remove, reset, and cleanup:
/// the lockfile is the source of truth and the block is derived from it.
pub(crate) fn sync_excludes(project: &Project, lock: &AkitLockfile) -> Result<()> {
    if let Some(excl) = project.git_info_exclude_path() {
        gitexclude::set_managed_lines(&excl, &desired_excludes(lock))?;
    }
    Ok(())
}

/// After removing materializations, delete any now-empty ancestor directories
/// akit created for them (e.g. `.agents/skills`, then `.agents`), walking up
/// until a non-empty directory or the project root. Only *empty* directories are
/// removed, so user files are never touched. Best-effort: failures are ignored.
pub(crate) fn prune_empty_owned_dirs(
    fs: &dyn FsTransport,
    project: &Project,
    removed: &[MaterializationRecord],
) {
    for m in removed {
        let mut rel = PathBuf::from(&m.path);
        // Walk up from the materialization's parent to the project root.
        while rel.pop() {
            if rel.as_os_str().is_empty() {
                break;
            }
            let abs = project.root.join(&rel);
            match fs.dir_is_empty(&abs) {
                Ok(true) => {
                    if fs.remove_dir_all(&abs).is_err() {
                        break;
                    }
                }
                // Non-empty or not a directory: stop climbing this branch.
                _ => break,
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;
    use tempfile::TempDir;

    struct Fixtures {
        _tmp: TempDir,
        project: Project,
        catalog: Catalog,
    }

    fn setup() -> Fixtures {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path().join("project");
        std::fs::create_dir_all(root.join(".git/info")).unwrap();
        let project = Project {
            root: root.clone(),
            git_dir: Some(root.join(".git")),
        };
        let catalog = Catalog::with_root(tmp.path().join("catalog"));
        Fixtures {
            _tmp: tmp,
            project,
            catalog,
        }
    }

    fn write(path: &Path, contents: &str) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, contents).unwrap();
    }

    fn write_skill(catalog: &Catalog, id: &str, compat: Option<&str>) {
        let dir = catalog.skill_source(id);
        write(&dir.join("SKILL.md"), "---\nname: x\n---\nbody");
        if let Some(c) = compat {
            std::fs::write(dir.join("skill.yml"), c).unwrap();
        }
    }

    fn write_agent(catalog: &Catalog, id: &str) {
        let dir = catalog.agent_package_dir(id);
        write(&dir.join("copilot.agent.md"), "---\nname: r\n---\nbody");
        write(&dir.join("claude.md"), "---\nname: r\n---\nbody");
        write(
            &dir.join("agent.yml"),
            "variants:\n  copilot: copilot.agent.md\n  claude: claude.md\n",
        );
    }

    fn ctx(hs: &[HarnessId]) -> HarnessContext {
        HarnessContext::new(hs.to_vec()).unwrap()
    }

    #[test]
    fn empty_context_is_rejected() {
        assert!(HarnessContext::new([]).is_err());
    }

    #[test]
    fn install_skill_for_all_five_writes_two_dirs_and_excludes() {
        let f = setup();
        write_skill(&f.catalog, "deploy", None);
        let report = install(
            &f.project,
            &f.catalog,
            ItemType::Skill,
            "deploy",
            &ctx(&HarnessId::ALL),
        )
        .unwrap();

        assert_eq!(report.harnesses, HarnessId::ALL.to_vec());
        assert_eq!(report.materializations.len(), 2);
        assert!(!report.replaced);
        assert!(
            f.project
                .root
                .join(".agents/skills/deploy/SKILL.md")
                .is_file()
        );
        assert!(
            f.project
                .root
                .join(".claude/skills/deploy/SKILL.md")
                .is_file()
        );

        // Lockfile + both materializations are git-excluded.
        let excl = std::fs::read_to_string(f.project.root.join(".git/info/exclude")).unwrap();
        assert!(excl.contains("/.akit/kit.lock.json"), "{excl}");
        assert!(excl.contains("/.agents/skills/deploy"), "{excl}");
        assert!(excl.contains("/.claude/skills/deploy"), "{excl}");

        // Ownership recorded.
        let status = status(&f.project).unwrap();
        assert_eq!(status.len(), 1);
        assert_eq!(status[0].harnesses, HarnessId::ALL.to_vec());
    }

    #[test]
    fn reinstalling_with_fewer_harnesses_reshapes_and_removes_unneeded_paths() {
        let f = setup();
        write_skill(&f.catalog, "deploy", None);
        install(
            &f.project,
            &f.catalog,
            ItemType::Skill,
            "deploy",
            &ctx(&HarnessId::ALL),
        )
        .unwrap();
        assert!(f.project.root.join(".agents/skills/deploy").exists());

        // Reshape to Claude-only: the `.agents/skills` copy is now unneeded.
        let report = install(
            &f.project,
            &f.catalog,
            ItemType::Skill,
            "deploy",
            &ctx(&[HarnessId::Claude]),
        )
        .unwrap();
        assert!(report.replaced);
        assert_eq!(report.harnesses, vec![HarnessId::Claude]);
        assert!(f.project.root.join(".claude/skills/deploy").exists());
        assert!(!f.project.root.join(".agents/skills/deploy").exists());
        // The stale exclude line is gone too.
        let excl = std::fs::read_to_string(f.project.root.join(".git/info/exclude")).unwrap();
        assert!(!excl.contains("/.agents/skills/deploy"), "{excl}");
    }

    #[test]
    fn incompatible_skill_harness_is_reported_not_installed() {
        let f = setup();
        write_skill(&f.catalog, "clauded", Some("harnesses:\n  - claude\n"));
        let report = install(
            &f.project,
            &f.catalog,
            ItemType::Skill,
            "clauded",
            &ctx(&[HarnessId::Claude, HarnessId::Codex]),
        )
        .unwrap();
        assert_eq!(report.harnesses, vec![HarnessId::Claude]);
        assert_eq!(report.issues.len(), 1);
        assert_eq!(report.issues[0].harness, HarnessId::Codex);
    }

    #[test]
    fn install_agent_writes_native_file_per_harness() {
        let f = setup();
        write_agent(&f.catalog, "reviewer");
        let report = install(
            &f.project,
            &f.catalog,
            ItemType::Agent,
            "reviewer",
            &ctx(&[HarnessId::Copilot, HarnessId::Claude]),
        )
        .unwrap();
        assert_eq!(report.materializations.len(), 2);
        assert!(
            f.project
                .root
                .join(".github/agents/reviewer.agent.md")
                .is_file()
        );
        assert!(f.project.root.join(".claude/agents/reviewer.md").is_file());
    }

    #[test]
    fn remove_all_uninstalls_everything_and_prunes() {
        let f = setup();
        write_skill(&f.catalog, "deploy", None);
        install(
            &f.project,
            &f.catalog,
            ItemType::Skill,
            "deploy",
            &ctx(&HarnessId::ALL),
        )
        .unwrap();

        let report = remove(&f.project, ItemType::Skill, "deploy", RemoveScope::All).unwrap();
        assert!(!report.not_installed);
        assert_eq!(report.remaining_harnesses, vec![]);
        assert!(!f.project.root.join(".agents").exists());
        assert!(!f.project.root.join(".claude/skills/deploy").exists());
        assert!(status(&f.project).unwrap().is_empty());

        // Excludes for the materializations are gone.
        let excl = std::fs::read_to_string(f.project.root.join(".git/info/exclude")).unwrap();
        assert!(!excl.contains("/.agents/skills/deploy"), "{excl}");
    }

    #[test]
    fn remove_scoped_harness_reshapes_remaining() {
        // Install for copilot+claude (one shared `.claude/skills`), then drop claude:
        // copilot must remain, served by the neutral `.agents/skills` path.
        let f = setup();
        write_skill(&f.catalog, "deploy", None);
        // Point the process catalog env at our temp catalog so the partial-remove
        // re-plan (which relocates via Catalog::locate) resolves the source.
        unsafe { std::env::set_var(crate::catalog::ENV_CATALOG_DIR, &f.catalog.root) };
        install(
            &f.project,
            &f.catalog,
            ItemType::Skill,
            "deploy",
            &ctx(&[HarnessId::Copilot, HarnessId::Claude]),
        )
        .unwrap();

        let report = remove(
            &f.project,
            ItemType::Skill,
            "deploy",
            RemoveScope::Harnesses(vec![HarnessId::Claude]),
        )
        .unwrap();
        unsafe { std::env::remove_var(crate::catalog::ENV_CATALOG_DIR) };

        assert_eq!(report.remaining_harnesses, vec![HarnessId::Copilot]);
        assert!(f.project.root.join(".agents/skills/deploy").exists());
        assert!(!f.project.root.join(".claude/skills/deploy").exists());
    }

    #[test]
    fn remove_absent_item_is_not_installed() {
        let f = setup();
        let report = remove(&f.project, ItemType::Skill, "nope", RemoveScope::All).unwrap();
        assert!(report.not_installed);
    }

    #[test]
    fn reset_removes_all_owned_files_only() {
        let f = setup();
        write_skill(&f.catalog, "deploy", None);
        write_agent(&f.catalog, "reviewer");
        install(
            &f.project,
            &f.catalog,
            ItemType::Skill,
            "deploy",
            &ctx(&[HarnessId::Codex]),
        )
        .unwrap();
        install(
            &f.project,
            &f.catalog,
            ItemType::Agent,
            "reviewer",
            &ctx(&[HarnessId::Copilot]),
        )
        .unwrap();
        // An unrelated user file must survive reset.
        write(&f.project.root.join(".github/keep.md"), "mine");

        let report = reset(&f.project).unwrap();
        assert_eq!(report.cleared_items, 2);
        assert!(!f.project.root.join(".agents/skills/deploy").exists());
        assert!(
            !f.project
                .root
                .join(".github/agents/reviewer.agent.md")
                .exists()
        );
        assert!(f.project.root.join(".github/keep.md").is_file());
        assert!(status(&f.project).unwrap().is_empty());
    }
}
