//! Reconcile the akit lockfile with materialized files and git's local exclude file.

use anyhow::{Context, Result};
use serde::Serialize;
use std::collections::BTreeSet;
use std::io::ErrorKind;

use crate::catalog::Catalog;
use crate::fsops;
use crate::gitexclude;
use crate::lockfile::{ItemType, LockItem, Lockfile, Mode};
use crate::ops::{self, HealthStatus, LOCKFILE_REL};
use crate::project::Project;

/// Read-only health report for lockfile items and managed git-exclude lines.
#[derive(Debug, Serialize)]
pub struct DoctorReport {
    pub items: Vec<DoctorItem>,
    pub exclude: ExcludeHealth,
    pub summary: DoctorSummary,
}

/// One lockfile item as seen by `doctor`.
#[derive(Debug, Serialize)]
pub struct DoctorItem {
    pub id: String,
    #[serde(rename = "type")]
    pub item_type: ItemType,
    pub mode: Mode,
    pub target: String,
    pub bundle: Option<String>,
    pub status: HealthStatus,
    pub source_present: bool,
    pub target_present: bool,
    pub exclude_present: bool,
}

/// Health of `.git/info/exclude` lines managed by akit.
#[derive(Debug, Serialize)]
pub struct ExcludeHealth {
    pub checked: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    pub lockfile_present: bool,
    pub missing: Vec<String>,
    pub stale: Vec<String>,
}

/// Aggregate counts for a doctor report.
#[derive(Debug, Serialize)]
pub struct DoctorSummary {
    pub total: usize,
    pub ok: usize,
    pub orphaned: usize,
    pub missing: usize,
    pub drifted: usize,
    pub missing_exclude_lines: usize,
    pub stale_exclude_lines: usize,
    pub not_a_git_repo: bool,
    pub healthy: bool,
}

/// Repair report for `sync`.
#[derive(Debug, Serialize)]
pub struct SyncReport {
    pub items: Vec<SyncItem>,
    pub exclude: SyncExcludeReport,
    pub summary: SyncSummary,
}

/// One lockfile item as handled by `sync`.
#[derive(Debug, Serialize)]
pub struct SyncItem {
    pub id: String,
    #[serde(rename = "type")]
    pub item_type: ItemType,
    pub mode: Mode,
    pub target: String,
    pub bundle: Option<String>,
    pub status_before: HealthStatus,
    pub status_after: HealthStatus,
    pub source_present: bool,
    pub restored: bool,
    pub exclude_added: bool,
    pub skipped_orphan: bool,
    pub drifted: bool,
}

/// Exclude-file actions and remaining findings from `sync`.
#[derive(Debug, Serialize)]
pub struct SyncExcludeReport {
    pub checked: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    pub lockfile_added: bool,
    pub target_lines_added: Vec<String>,
    pub missing_after: Vec<String>,
    pub stale: Vec<String>,
}

/// Aggregate counts for a sync report.
#[derive(Debug, Serialize)]
pub struct SyncSummary {
    pub total: usize,
    pub restored: usize,
    pub exclude_added: usize,
    pub skipped_orphan: usize,
    pub drifted: usize,
    pub missing_after: usize,
    pub missing_exclude_lines: usize,
    pub stale_exclude_lines: usize,
    pub not_a_git_repo: bool,
    pub healthy: bool,
}

struct ExcludeInspection {
    health: ExcludeHealth,
    lines: Option<BTreeSet<String>>,
}

/// Inspect lockfile items, materialized files, catalog sources, and git-exclude lines.
pub fn diagnose(project: &Project, catalog: &Catalog) -> Result<DoctorReport> {
    let lockfile_path = project.lockfile_path();
    let lockfile_exists = lockfile_path.exists();
    let lockfile = Lockfile::load(&lockfile_path)?;
    let exclude = inspect_exclude(project, &lockfile.items, lockfile_exists)?;

    let items = lockfile
        .items
        .iter()
        .map(|item| doctor_item(project, catalog, item, exclude.lines.as_ref()))
        .collect::<Result<Vec<_>>>()?;
    let summary = doctor_summary(&items, &exclude.health);

    Ok(DoctorReport {
        items,
        exclude: exclude.health,
        summary,
    })
}

/// Repair safely restorable drift: missing materializations and missing exclude lines.
pub fn sync(project: &Project, catalog: &Catalog) -> Result<SyncReport> {
    let lockfile_path = project.lockfile_path();
    let lockfile_exists = lockfile_path.exists();
    let lockfile = Lockfile::load(&lockfile_path)?;
    let exclude_path = project.git_info_exclude_path();
    let mut target_lines_added = Vec::new();
    let mut items = Vec::with_capacity(lockfile.items.len());

    for item in &lockfile.items {
        let target_line = exclude_line(&item.target);
        let exclude_added = if let Some(path) = exclude_path.as_ref() {
            let added = gitexclude::add_line(&crate::transport::LocalFs, path, &target_line)?;
            if added {
                target_lines_added.push(target_line);
            }
            added
        } else {
            false
        };

        items.push(sync_item(project, catalog, item, exclude_added)?);
    }

    let lockfile_added = if should_expect_lockfile_line(&lockfile.items, lockfile_exists) {
        if let Some(path) = exclude_path.as_ref() {
            gitexclude::add_line(&crate::transport::LocalFs, path, &lockfile_exclude_line())?
        } else {
            false
        }
    } else {
        false
    };

    let exclude_after = inspect_exclude(project, &lockfile.items, lockfile_exists)?;
    let summary = sync_summary(&items, lockfile_added, &exclude_after.health);

    Ok(SyncReport {
        items,
        exclude: SyncExcludeReport {
            checked: exclude_after.health.checked,
            path: exclude_after.health.path,
            lockfile_added,
            target_lines_added,
            missing_after: exclude_after.health.missing,
            stale: exclude_after.health.stale,
        },
        summary,
    })
}

fn doctor_item(
    project: &Project,
    catalog: &Catalog,
    item: &LockItem,
    exclude_lines: Option<&BTreeSet<String>>,
) -> Result<DoctorItem> {
    let source_present = ops::source_for_item(catalog, item).exists();
    let target_present = std::fs::symlink_metadata(project.root.join(&item.target)).is_ok();
    let status = item_health(project, catalog, item, source_present, target_present)?;
    let exclude_present =
        exclude_lines.is_some_and(|lines| lines.contains(&exclude_line(&item.target)));

    Ok(DoctorItem {
        id: item.id.clone(),
        item_type: item.item_type,
        mode: item.mode,
        target: item.target.clone(),
        bundle: item.bundle.clone(),
        status,
        source_present,
        target_present,
        exclude_present,
    })
}

fn sync_item(
    project: &Project,
    catalog: &Catalog,
    item: &LockItem,
    exclude_added: bool,
) -> Result<SyncItem> {
    let src = ops::source_for_item(catalog, item);
    let source_present = src.exists();
    let target_present = std::fs::symlink_metadata(project.root.join(&item.target)).is_ok();
    let status_before = item_health(project, catalog, item, source_present, target_present)?;
    let mut restored = false;
    let mut skipped_orphan = false;
    let mut drifted = false;

    match status_before {
        HealthStatus::Missing => {
            if source_present {
                let dst = project.root.join(&item.target);
                restored = fsops::materialize(item.mode, &src, &dst)?.created();
            } else {
                skipped_orphan = true;
            }
        }
        HealthStatus::Orphaned => skipped_orphan = true,
        HealthStatus::Drifted => drifted = true,
        HealthStatus::Ok => {}
    }

    let target_present_after = std::fs::symlink_metadata(project.root.join(&item.target)).is_ok();
    let status_after = item_health(project, catalog, item, source_present, target_present_after)?;
    Ok(SyncItem {
        id: item.id.clone(),
        item_type: item.item_type,
        mode: item.mode,
        target: item.target.clone(),
        bundle: item.bundle.clone(),
        status_before,
        status_after,
        source_present,
        restored,
        exclude_added,
        skipped_orphan,
        drifted,
    })
}

fn item_health(
    project: &Project,
    catalog: &Catalog,
    item: &LockItem,
    source_present: bool,
    target_present: bool,
) -> Result<HealthStatus> {
    let status = ops::health(project, item, Some(catalog))?;
    if !source_present && target_present {
        Ok(HealthStatus::Orphaned)
    } else {
        Ok(status)
    }
}

fn inspect_exclude(
    project: &Project,
    items: &[LockItem],
    lockfile_exists: bool,
) -> Result<ExcludeInspection> {
    let Some(path) = project.git_info_exclude_path() else {
        return Ok(ExcludeInspection {
            health: ExcludeHealth {
                checked: false,
                path: None,
                lockfile_present: false,
                missing: Vec::new(),
                stale: Vec::new(),
            },
            lines: None,
        });
    };

    let lines = read_exclude_lines(&path)?;
    let expected = expected_exclude_lines(items, lockfile_exists);
    let missing = expected
        .iter()
        .filter(|line| !lines.contains(*line))
        .cloned()
        .collect();
    let stale = lines
        .iter()
        .filter(|line| is_managed_exclude_line(line) && !expected.contains(*line))
        .cloned()
        .collect();

    Ok(ExcludeInspection {
        health: ExcludeHealth {
            checked: true,
            path: Some(path.display().to_string()),
            lockfile_present: lines.contains(&lockfile_exclude_line()),
            missing,
            stale,
        },
        lines: Some(lines),
    })
}

fn read_exclude_lines(path: &std::path::Path) -> Result<BTreeSet<String>> {
    let content = match std::fs::read_to_string(path) {
        Ok(content) => content,
        Err(err) if err.kind() == ErrorKind::NotFound => String::new(),
        Err(err) => return Err(err).with_context(|| format!("reading {}", path.display())),
    };
    Ok(content
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(str::to_string)
        .collect())
}

fn expected_exclude_lines(items: &[LockItem], lockfile_exists: bool) -> BTreeSet<String> {
    let mut expected = items
        .iter()
        .map(|item| exclude_line(&item.target))
        .collect::<BTreeSet<_>>();
    if should_expect_lockfile_line(items, lockfile_exists) {
        expected.insert(lockfile_exclude_line());
    }
    expected
}

fn should_expect_lockfile_line(items: &[LockItem], lockfile_exists: bool) -> bool {
    lockfile_exists || !items.is_empty()
}

fn exclude_line(target: &str) -> String {
    format!("/{target}")
}

fn lockfile_exclude_line() -> String {
    format!("/{LOCKFILE_REL}")
}

fn is_managed_exclude_line(line: &str) -> bool {
    line == lockfile_exclude_line()
        || line.starts_with("/.github/skills/")
        || line.starts_with("/.github/agents/")
}

fn doctor_summary(items: &[DoctorItem], exclude: &ExcludeHealth) -> DoctorSummary {
    let ok = items
        .iter()
        .filter(|item| item.status == HealthStatus::Ok)
        .count();
    let orphaned = items
        .iter()
        .filter(|item| item.status == HealthStatus::Orphaned)
        .count();
    let missing = items
        .iter()
        .filter(|item| item.status == HealthStatus::Missing)
        .count();
    let drifted = items
        .iter()
        .filter(|item| item.status == HealthStatus::Drifted)
        .count();
    let missing_exclude_lines = exclude.missing.len();
    let stale_exclude_lines = exclude.stale.len();
    DoctorSummary {
        total: items.len(),
        ok,
        orphaned,
        missing,
        drifted,
        missing_exclude_lines,
        stale_exclude_lines,
        not_a_git_repo: !exclude.checked,
        healthy: orphaned == 0
            && missing == 0
            && drifted == 0
            && missing_exclude_lines == 0
            && stale_exclude_lines == 0,
    }
}

fn sync_summary(items: &[SyncItem], lockfile_added: bool, exclude: &ExcludeHealth) -> SyncSummary {
    let restored = items.iter().filter(|item| item.restored).count();
    let target_excludes_added = items.iter().filter(|item| item.exclude_added).count();
    let skipped_orphan = items.iter().filter(|item| item.skipped_orphan).count();
    let drifted = items.iter().filter(|item| item.drifted).count();
    let missing_after = items
        .iter()
        .filter(|item| item.status_after == HealthStatus::Missing)
        .count();
    let missing_exclude_lines = exclude.missing.len();
    let stale_exclude_lines = exclude.stale.len();

    SyncSummary {
        total: items.len(),
        restored,
        exclude_added: target_excludes_added + usize::from(lockfile_added),
        skipped_orphan,
        drifted,
        missing_after,
        missing_exclude_lines,
        stale_exclude_lines,
        not_a_git_repo: !exclude.checked,
        healthy: items
            .iter()
            .all(|item| item.status_after == HealthStatus::Ok)
            && missing_exclude_lines == 0
            && stale_exclude_lines == 0,
    }
}
