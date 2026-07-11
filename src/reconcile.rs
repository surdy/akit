//! Ownership reconciliation for the harness-aware `.akit` model (issue #61).
//!
//! Everything here operates strictly on files akit *owns* (recorded in
//! `.akit/kit.lock.json`) or on its own managed git-exclude block. It never
//! overwrites, deletes, or claims unmanaged bytes without an explicit,
//! exact-content match. The embedding host (madari) renders these read-only
//! reports and drives the safe mutations behind confirmation UX — all ownership
//! logic lives here.
//!
//! Operations:
//!   - [`health`]              — read-only per-item drift + stale-exclude report.
//!   - [`repair`]              — re-materialize *missing* records; never touches modified copies.
//!   - [`detach`]              — drop ownership, keep bytes, make the file git-visible.
//!   - [`forget`]              — drop an orphaned/missing ownership record only.
//!   - [`remove_stale_excludes`] — prune managed exclude lines with no owner.
//!   - [`adopt`]               — claim existing *exact-content* files as owned.

use anyhow::Result;
use serde::{Deserialize, Serialize};

use crate::catalog::Catalog;
use crate::gitexclude;
use crate::harness::HarnessId;
use crate::install::{self, HarnessContext};
use crate::lockfile::{ItemType, Mode};
use crate::materialize::{self, Drift, MaterializeItem, content_hash, materialize_one};
use crate::ownership::{AkitLockfile, Installation, MaterializationRecord};
use crate::project::Project;
use crate::transport::{FsTransport, LocalFs};

/// Drift health of one owned materialization.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MaterializationHealth {
    pub path: String,
    pub mode: Mode,
    pub covers: Vec<HarnessId>,
    /// `clean` | `missing` | `modified`.
    pub drift: Drift,
}

/// Health of one logical installed item.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ItemHealth {
    pub id: String,
    #[serde(rename = "type")]
    pub item_type: ItemType,
    pub source: String,
    pub harnesses: Vec<HarnessId>,
    pub materializations: Vec<MaterializationHealth>,
    /// Whether the catalog still provides this item's source.
    pub source_present: bool,
    /// True when a selected harness lacks a clean covering materialization.
    pub degraded: bool,
}

/// A read-only ownership + drift report over the whole project.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct HealthReport {
    pub items: Vec<ItemHealth>,
    /// Managed exclude lines that no longer correspond to an owned path.
    pub stale_excludes: Vec<String>,
    /// Whether an `.akit/kit.lock.json` exists.
    pub lockfile_present: bool,
    /// True when every item is fully clean and there are no stale excludes.
    pub healthy: bool,
}

/// Read-only health of every installed item plus stale-exclude detection.
pub fn health(project: &Project, catalog: &Catalog) -> Result<HealthReport> {
    health_with(&LocalFs, project, catalog)
}

/// [`health`] against an explicit transport.
pub fn health_with(
    fs: &dyn FsTransport,
    project: &Project,
    catalog: &Catalog,
) -> Result<HealthReport> {
    let lf_path = project.akit_lockfile_path();
    let lockfile_present = fs.exists(&lf_path)?;
    let lock = AkitLockfile::load_with(fs, &lf_path)?;

    let mut items = Vec::new();
    let mut healthy = true;
    for inst in &lock.items {
        let source_present = source_exists(catalog, inst);
        let mut healthy_covers: Vec<HarnessId> = Vec::new();
        let mut mats = Vec::new();
        for m in &inst.materializations {
            let drift = check_drift_safe(fs, project, m);
            if drift == Drift::Clean {
                for h in &m.covers {
                    if !healthy_covers.contains(h) {
                        healthy_covers.push(*h);
                    }
                }
            } else {
                healthy = false;
            }
            mats.push(MaterializationHealth {
                path: m.path.clone(),
                mode: m.mode,
                covers: m.covers.clone(),
                drift,
            });
        }
        let degraded = inst.harnesses.iter().any(|h| !healthy_covers.contains(h));
        items.push(ItemHealth {
            id: inst.id.clone(),
            item_type: inst.item_type,
            source: inst.source.clone(),
            harnesses: inst.harnesses.clone(),
            materializations: mats,
            source_present,
            degraded,
        });
    }

    let stale_excludes = stale_exclude_lines(fs, project, &lock);
    if !stale_excludes.is_empty() {
        healthy = false;
    }

    Ok(HealthReport {
        items,
        stale_excludes,
        lockfile_present,
        healthy,
    })
}

/// Outcome of a [`repair`].
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct RepairReport {
    /// Missing materializations that were re-created from their source.
    pub restored_paths: Vec<String>,
    /// Modified copies left untouched (a conflict; never overwritten).
    pub skipped_modified: Vec<String>,
    /// Owned items whose catalog source is gone (cannot be repaired).
    pub missing_source: Vec<String>,
}

/// Re-materialize *missing* owned materializations from their catalog source and
/// resync the managed exclude block. Modified copies are conflicts and are never
/// overwritten; items whose source is gone are reported, not touched.
pub fn repair(project: &Project, catalog: &Catalog) -> Result<RepairReport> {
    repair_with(&LocalFs, project, catalog)
}

/// [`repair`] against an explicit transport.
pub fn repair_with(
    fs: &dyn FsTransport,
    project: &Project,
    catalog: &Catalog,
) -> Result<RepairReport> {
    let lf_path = project.akit_lockfile_path();
    let lock = AkitLockfile::load_with(fs, &lf_path)?;
    let mut report = RepairReport::default();

    for inst in &lock.items {
        if !source_exists(catalog, inst) {
            report.missing_source.push(inst.id.clone());
            continue;
        }
        // Re-plan for this item's recorded harness set to resolve sources.
        let (plan, resolver) =
            match install::build_plan(catalog, inst.item_type, &inst.id, &inst.harnesses) {
                Ok(pr) => pr,
                Err(_) => {
                    report.missing_source.push(inst.id.clone());
                    continue;
                }
            };
        for planned in &plan.materializations {
            // Only act on paths this item already owns.
            let owned = inst
                .materializations
                .iter()
                .find(|m| m.path == planned.path);
            let Some(record) = owned else { continue };
            match check_drift_safe(fs, project, record) {
                Drift::Missing => {
                    let source = resolver(planned);
                    materialize_one(fs, &project.root, &MaterializeItem { source, planned })?;
                    report.restored_paths.push(planned.path.clone());
                }
                Drift::Modified => report.skipped_modified.push(planned.path.clone()),
                Drift::Clean => {}
            }
        }
    }

    // Restore any missing managed exclude lines (and prune stale ones).
    install::sync_excludes(fs, project, &lock)?;
    Ok(report)
}

/// Outcome of a [`detach`] or [`forget`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DetachReport {
    pub id: String,
    #[serde(rename = "type")]
    pub item_type: ItemType,
    /// Paths whose ownership was dropped (bytes preserved on disk).
    pub paths: Vec<String>,
    /// Whether the item had no ownership record to begin with.
    pub not_installed: bool,
}

/// Drop ownership of an item while **preserving its bytes on disk**, and remove
/// its managed exclude lines so Git can see the now-unmanaged files. Use when a
/// user wants to keep a materialized skill/agent but stop akit from managing it.
pub fn detach(project: &Project, item_type: ItemType, id: &str) -> Result<DetachReport> {
    detach_with(&LocalFs, project, item_type, id)
}

/// [`detach`] against an explicit transport.
pub fn detach_with(
    fs: &dyn FsTransport,
    project: &Project,
    item_type: ItemType,
    id: &str,
) -> Result<DetachReport> {
    drop_ownership(fs, project, item_type, id)
}

/// Drop an ownership record without touching files or excludes-of-others. Use to
/// clear a stale/orphaned record (e.g. its files were manually deleted). The
/// managed exclude block is resynced so the item's now-unowned lines are pruned.
pub fn forget(project: &Project, item_type: ItemType, id: &str) -> Result<DetachReport> {
    forget_with(&LocalFs, project, item_type, id)
}

/// [`forget`] against an explicit transport.
pub fn forget_with(
    fs: &dyn FsTransport,
    project: &Project,
    item_type: ItemType,
    id: &str,
) -> Result<DetachReport> {
    drop_ownership(fs, project, item_type, id)
}

fn drop_ownership(
    fs: &dyn FsTransport,
    project: &Project,
    item_type: ItemType,
    id: &str,
) -> Result<DetachReport> {
    let lf_path = project.akit_lockfile_path();
    let mut lock = AkitLockfile::load_with(fs, &lf_path)?;
    let Some(removed) = lock.remove(item_type, id) else {
        return Ok(DetachReport {
            id: id.to_string(),
            item_type,
            paths: Vec::new(),
            not_installed: true,
        });
    };
    let paths = removed
        .materializations
        .iter()
        .map(|m| m.path.clone())
        .collect();
    lock.save_with(fs, &lf_path)?;
    // Recompute the managed exclude block: the dropped item's lines disappear,
    // so its (preserved) files become visible to Git.
    install::sync_excludes(fs, project, &lock)?;
    Ok(DetachReport {
        id: id.to_string(),
        item_type,
        paths,
        not_installed: false,
    })
}

/// Prune managed exclude lines that no longer correspond to an owned path,
/// returning the removed lines. Never touches user-authored exclude entries.
pub fn remove_stale_excludes(project: &Project) -> Result<Vec<String>> {
    remove_stale_excludes_with(&LocalFs, project)
}

/// [`remove_stale_excludes`] against an explicit transport.
pub fn remove_stale_excludes_with(fs: &dyn FsTransport, project: &Project) -> Result<Vec<String>> {
    let lock = AkitLockfile::load_with(fs, &project.akit_lockfile_path())?;
    let stale = stale_exclude_lines(fs, project, &lock);
    if !stale.is_empty() {
        install::sync_excludes(fs, project, &lock)?;
    }
    Ok(stale)
}

/// Outcome of an [`adopt`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AdoptReport {
    pub id: String,
    #[serde(rename = "type")]
    pub item_type: ItemType,
    /// The harnesses now served by adopted materializations.
    pub harnesses: Vec<HarnessId>,
    /// Paths adopted (existing files with exact-content match to the source).
    pub adopted_paths: Vec<String>,
    /// Existing files that differ from the source and were NOT overwritten.
    pub conflicts: Vec<String>,
}

/// Establish ownership of **already-present, exact-content** files without
/// rewriting bytes — the safe recovery when a lockfile is missing but the
/// materialized files still match the catalog. A destination that exists but
/// differs is reported as a conflict and never overwritten; an absent
/// destination is simply not adopted (use a normal install to create it).
pub fn adopt(
    project: &Project,
    catalog: &Catalog,
    item_type: ItemType,
    id: &str,
    ctx: &HarnessContext,
) -> Result<AdoptReport> {
    adopt_with(&LocalFs, project, catalog, item_type, id, ctx)
}

/// [`adopt`] against an explicit transport.
pub fn adopt_with(
    fs: &dyn FsTransport,
    project: &Project,
    catalog: &Catalog,
    item_type: ItemType,
    id: &str,
    ctx: &HarnessContext,
) -> Result<AdoptReport> {
    let (plan, resolver) = install::build_plan(catalog, item_type, id, ctx.harnesses())?;

    let mut adopted = Vec::new();
    let mut conflicts = Vec::new();
    for planned in &plan.materializations {
        let abs = project.root.join(&planned.path);
        if fs.symlink_kind(&abs)?.is_none() {
            // Absent: nothing to adopt (a normal install would create it).
            continue;
        }
        let source = resolver(planned);
        let use_symlink = planned.mode == Mode::Symlink && fs.supports_symlink();
        if use_symlink {
            // A symlink destination is adopted if it resolves to the source.
            match (abs.canonicalize(), source.canonicalize()) {
                (Ok(a), Ok(b)) if a == b => adopted.push(MaterializationRecord {
                    path: planned.path.clone(),
                    mode: Mode::Symlink,
                    covers: sorted(planned.covers.clone()),
                    hash: None,
                }),
                _ => conflicts.push(planned.path.clone()),
            }
            continue;
        }
        // Copy destination: adopt only on exact content match.
        let expected = content_hash(fs, &source)?;
        let actual = content_hash(fs, &abs)?;
        if expected == actual {
            adopted.push(MaterializationRecord {
                path: planned.path.clone(),
                mode: Mode::Copy,
                covers: sorted(planned.covers.clone()),
                hash: Some(actual),
            });
        } else {
            conflicts.push(planned.path.clone());
        }
    }

    let harnesses = served_by(&adopted);
    if !adopted.is_empty() {
        let lf_path = project.akit_lockfile_path();
        let mut lock = AkitLockfile::load_with(fs, &lf_path)?;
        lock.upsert(Installation {
            id: id.to_string(),
            item_type,
            source: "local".to_string(),
            git_ref: None,
            bundle: None,
            harnesses: harnesses.clone(),
            materializations: adopted.clone(),
        });
        lock.save_with(fs, &lf_path)?;
        install::sync_excludes(fs, project, &lock)?;
    }

    Ok(AdoptReport {
        id: id.to_string(),
        item_type,
        harnesses,
        adopted_paths: adopted.into_iter().map(|m| m.path).collect(),
        conflicts,
    })
}

// ── internals ────────────────────────────────────────────────────────────────

/// Drift with any transport error treated as `Missing` (a conservative, safe
/// reading for a read-only health probe).
fn check_drift_safe(fs: &dyn FsTransport, project: &Project, m: &MaterializationRecord) -> Drift {
    materialize::check_drift(fs, &project.root, m).unwrap_or(Drift::Missing)
}

/// Whether the catalog still provides `inst`'s source (skill dir / agent package).
fn source_exists(catalog: &Catalog, inst: &Installation) -> bool {
    match inst.item_type {
        ItemType::Skill => catalog.resolve_skill(&inst.id).is_ok(),
        ItemType::Agent => catalog.resolve_agent_package(&inst.id).is_ok(),
    }
}

/// Managed exclude lines present on disk that no longer map to an owned path or
/// the lockfile itself.
fn stale_exclude_lines(
    fs: &dyn FsTransport,
    project: &Project,
    lock: &AkitLockfile,
) -> Vec<String> {
    let Some(excl) = project.git_info_exclude_path() else {
        return Vec::new();
    };
    let current = gitexclude::managed_lines(fs, &excl).unwrap_or_default();
    let desired = install::desired_excludes(lock);
    current
        .into_iter()
        .filter(|l| !desired.contains(l))
        .collect()
}

fn sorted(mut v: Vec<HarnessId>) -> Vec<HarnessId> {
    v.sort();
    v.dedup();
    v
}

fn served_by(records: &[MaterializationRecord]) -> Vec<HarnessId> {
    let mut out = Vec::new();
    for r in records {
        for h in &r.covers {
            if !out.contains(h) {
                out.push(*h);
            }
        }
    }
    out.sort();
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::install::{HarnessContext, install};
    use crate::lockfile::ItemType;
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

    fn write_skill(catalog: &Catalog, id: &str) {
        let dir = catalog.skill_source(id);
        write(&dir.join("SKILL.md"), "---\nname: x\n---\nbody");
    }

    fn ctx(hs: &[HarnessId]) -> HarnessContext {
        HarnessContext::new(hs.to_vec()).unwrap()
    }

    fn install_skill(f: &Fixtures, id: &str, hs: &[HarnessId]) {
        write_skill(&f.catalog, id);
        install(&f.project, &f.catalog, ItemType::Skill, id, &ctx(hs)).unwrap();
    }

    fn excludes(project: &Project) -> Vec<String> {
        gitexclude::managed_lines(&LocalFs, &project.git_info_exclude_path().unwrap()).unwrap()
    }

    #[test]
    fn health_is_clean_after_install() {
        let f = setup();
        install_skill(&f, "deploy", &HarnessId::ALL);
        let r = health(&f.project, &f.catalog).unwrap();
        assert!(r.healthy, "{r:?}");
        assert!(r.lockfile_present);
        assert!(r.stale_excludes.is_empty());
        assert_eq!(r.items.len(), 1);
        let item = &r.items[0];
        assert!(item.source_present);
        assert!(!item.degraded);
        assert!(
            item.materializations
                .iter()
                .all(|m| m.drift == Drift::Clean)
        );
    }

    #[test]
    fn health_flags_missing_and_degraded() {
        let f = setup();
        install_skill(&f, "deploy", &[HarnessId::Copilot]);
        // Remove the materialized directory on disk.
        let mat = &health(&f.project, &f.catalog).unwrap().items[0].materializations[0].path;
        std::fs::remove_dir_all(f.project.root.join(mat)).unwrap();

        let r = health(&f.project, &f.catalog).unwrap();
        assert!(!r.healthy);
        let item = &r.items[0];
        assert!(item.degraded);
        assert_eq!(item.materializations[0].drift, Drift::Missing);
    }

    #[test]
    fn health_flags_modified() {
        let f = setup();
        install_skill(&f, "deploy", &[HarnessId::Copilot]);
        let path = health(&f.project, &f.catalog).unwrap().items[0].materializations[0]
            .path
            .clone();
        std::fs::write(f.project.root.join(&path).join("SKILL.md"), "tampered").unwrap();

        let r = health(&f.project, &f.catalog).unwrap();
        assert_eq!(r.items[0].materializations[0].drift, Drift::Modified);
        assert!(r.items[0].degraded);
    }

    #[test]
    fn health_flags_stale_exclude() {
        let f = setup();
        install_skill(&f, "deploy", &[HarnessId::Copilot]);
        // Inject a bogus managed line as if left behind by an old install.
        let excl = f.project.git_info_exclude_path().unwrap();
        let mut lines = gitexclude::managed_lines(&LocalFs, &excl).unwrap();
        lines.push("/.orphaned/skills/ghost".to_string());
        gitexclude::set_managed_lines(&LocalFs, &excl, &lines).unwrap();

        let r = health(&f.project, &f.catalog).unwrap();
        assert!(!r.healthy);
        assert!(
            r.stale_excludes
                .contains(&"/.orphaned/skills/ghost".to_string())
        );
    }

    #[test]
    fn repair_restores_missing_leaves_modified() {
        let f = setup();
        install_skill(&f, "a", &[HarnessId::Copilot]);
        install_skill(&f, "b", &[HarnessId::Copilot]);
        let items = health(&f.project, &f.catalog).unwrap().items;
        let a_path = items.iter().find(|i| i.id == "a").unwrap().materializations[0]
            .path
            .clone();
        let b_path = items.iter().find(|i| i.id == "b").unwrap().materializations[0]
            .path
            .clone();
        // a: missing; b: modified.
        std::fs::remove_dir_all(f.project.root.join(&a_path)).unwrap();
        std::fs::write(f.project.root.join(&b_path).join("SKILL.md"), "edited").unwrap();

        let rep = repair(&f.project, &f.catalog).unwrap();
        assert_eq!(rep.restored_paths, vec![a_path.clone()]);
        assert_eq!(rep.skipped_modified, vec![b_path.clone()]);
        assert!(rep.missing_source.is_empty());
        // a is back and clean; b remains modified (untouched).
        let after = health(&f.project, &f.catalog).unwrap();
        let a = after.items.iter().find(|i| i.id == "a").unwrap();
        let b = after.items.iter().find(|i| i.id == "b").unwrap();
        assert_eq!(a.materializations[0].drift, Drift::Clean);
        assert_eq!(b.materializations[0].drift, Drift::Modified);
        assert_eq!(
            std::fs::read_to_string(f.project.root.join(&b_path).join("SKILL.md")).unwrap(),
            "edited"
        );
    }

    #[test]
    fn repair_flags_missing_source() {
        let f = setup();
        install_skill(&f, "deploy", &[HarnessId::Copilot]);
        // Drop the catalog source.
        std::fs::remove_dir_all(f.catalog.skill_source("deploy")).unwrap();
        let rep = repair(&f.project, &f.catalog).unwrap();
        assert_eq!(rep.missing_source, vec!["deploy".to_string()]);
    }

    #[test]
    fn detach_keeps_bytes_and_prunes_excludes() {
        let f = setup();
        install_skill(&f, "deploy", &[HarnessId::Copilot]);
        let path = health(&f.project, &f.catalog).unwrap().items[0].materializations[0]
            .path
            .clone();
        assert!(excludes(&f.project).iter().any(|l| l.contains("deploy")));

        let rep = detach(&f.project, ItemType::Skill, "deploy").unwrap();
        assert!(!rep.not_installed);
        assert_eq!(rep.paths, vec![path.clone()]);
        // Bytes preserved, ownership + exclude gone.
        assert!(f.project.root.join(&path).join("SKILL.md").exists());
        assert!(!excludes(&f.project).iter().any(|l| l.contains("deploy")));
        assert!(health(&f.project, &f.catalog).unwrap().items.is_empty());
    }

    #[test]
    fn detach_reports_not_installed() {
        let f = setup();
        let rep = detach(&f.project, ItemType::Skill, "ghost").unwrap();
        assert!(rep.not_installed);
    }

    #[test]
    fn forget_drops_record() {
        let f = setup();
        install_skill(&f, "deploy", &[HarnessId::Copilot]);
        let rep = forget(&f.project, ItemType::Skill, "deploy").unwrap();
        assert!(!rep.not_installed);
        assert!(health(&f.project, &f.catalog).unwrap().items.is_empty());
    }

    #[test]
    fn remove_stale_excludes_prunes_orphans() {
        let f = setup();
        install_skill(&f, "deploy", &[HarnessId::Copilot]);
        let excl = f.project.git_info_exclude_path().unwrap();
        let mut lines = gitexclude::managed_lines(&LocalFs, &excl).unwrap();
        lines.push("/.orphaned/skills/ghost".to_string());
        gitexclude::set_managed_lines(&LocalFs, &excl, &lines).unwrap();

        let removed = remove_stale_excludes(&f.project).unwrap();
        assert_eq!(removed, vec!["/.orphaned/skills/ghost".to_string()]);
        assert!(!excludes(&f.project).contains(&"/.orphaned/skills/ghost".to_string()));
        // The real owned line survives.
        assert!(excludes(&f.project).iter().any(|l| l.contains("deploy")));
    }

    #[test]
    fn adopt_claims_exact_content_when_lockfile_missing() {
        let f = setup();
        install_skill(&f, "deploy", &[HarnessId::Copilot]);
        // Simulate a lost lockfile while files remain intact.
        std::fs::remove_file(f.project.akit_lockfile_path()).unwrap();
        assert!(health(&f.project, &f.catalog).unwrap().items.is_empty());

        let rep = adopt(
            &f.project,
            &f.catalog,
            ItemType::Skill,
            "deploy",
            &ctx(&[HarnessId::Copilot]),
        )
        .unwrap();
        assert!(!rep.adopted_paths.is_empty());
        assert!(rep.conflicts.is_empty());
        assert_eq!(rep.harnesses, vec![HarnessId::Copilot]);
        // Ownership re-established and clean.
        let after = health(&f.project, &f.catalog).unwrap();
        assert_eq!(after.items.len(), 1);
        assert!(!after.items[0].degraded);
    }

    #[test]
    fn adopt_reports_conflict_on_content_mismatch() {
        let f = setup();
        install_skill(&f, "deploy", &[HarnessId::Copilot]);
        let path = health(&f.project, &f.catalog).unwrap().items[0].materializations[0]
            .path
            .clone();
        std::fs::remove_file(f.project.akit_lockfile_path()).unwrap();
        std::fs::write(f.project.root.join(&path).join("SKILL.md"), "diverged").unwrap();

        let rep = adopt(
            &f.project,
            &f.catalog,
            ItemType::Skill,
            "deploy",
            &ctx(&[HarnessId::Copilot]),
        )
        .unwrap();
        assert!(rep.adopted_paths.is_empty());
        assert_eq!(rep.conflicts, vec![path]);
        // Nothing claimed → no ownership recorded.
        assert!(health(&f.project, &f.catalog).unwrap().items.is_empty());
    }
}
