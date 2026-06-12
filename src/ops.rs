//! High-level engine operations. The CLI and any GUI call these; they own the end-to-end
//! pipeline (resolve → materialize/remove → gitignore → record in lockfile).

use anyhow::{Context, Result};
use serde::Serialize;
use std::io::ErrorKind;

use crate::collection::Collection;
use crate::fsops;
use crate::gitexclude;
use crate::lockfile::{ItemType, LockItem, Lockfile, Mode};
use crate::project::Project;

/// Project-relative path of the lockfile, used for the git-exclude entry.
pub const LOCKFILE_REL: &str = ".copilot/kit.lock.json";

/// Outcome of an `add` operation.
#[derive(Debug, Serialize)]
pub struct AddReport {
    pub id: String,
    #[serde(rename = "type")]
    pub item_type: ItemType,
    pub mode: Mode,
    /// Project-relative materialized path.
    pub target: String,
    /// Whether the link/copy was created (vs already present).
    pub link_created: bool,
    /// Whether a new line was added to `.git/info/exclude`.
    pub exclude_added: bool,
    /// Whether a new lockfile entry was added (vs replaced).
    pub lock_added: bool,
    /// True if the project is not a git repo (pulls cannot be auto-ignored).
    pub not_a_git_repo: bool,
}

/// Outcome of an `rm` operation.
#[derive(Debug, Serialize)]
pub struct RemoveReport {
    pub id: String,
    #[serde(rename = "type")]
    pub item_type: ItemType,
    /// Project-relative materialized path.
    pub target: String,
    /// Whether a materialized target was removed.
    pub target_removed: bool,
    /// Whether the target line was removed from `.git/info/exclude`.
    pub exclude_removed: bool,
    /// Whether a lockfile entry was removed.
    pub lock_removed: bool,
    /// True when the item was not recorded as installed.
    pub not_installed: bool,
    /// True if the project is not a git repo.
    pub not_a_git_repo: bool,
}

/// Health status for an installed lockfile item.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum HealthStatus {
    Ok,
    Orphaned,
    Missing,
}

/// One row returned by `ls`.
#[derive(Debug, Serialize)]
pub struct ListItem {
    pub id: String,
    #[serde(rename = "type")]
    pub item_type: ItemType,
    pub mode: Mode,
    pub target: String,
    pub status: HealthStatus,
}

/// Pull an item from the collection into the project (symlink, gitignore, record).
pub fn add_item(
    project: &Project,
    collection: &Collection,
    item_type: ItemType,
    name: &str,
) -> Result<AddReport> {
    let src = match item_type {
        ItemType::Skill => collection.resolve_skill(name)?,
        ItemType::Agent => collection.resolve_agent(name)?,
    };
    let target_rel = target_for(item_type, name);
    let dst = project.root.join(&target_rel);

    let outcome = fsops::materialize(Mode::Symlink, &src, &dst)?;

    let mut exclude_added = false;
    let not_a_git_repo = project.git_dir.is_none();
    if let Some(excl) = project.git_info_exclude_path() {
        exclude_added |= gitexclude::add_line(&excl, &format!("/{target_rel}"))?;
        exclude_added |= gitexclude::add_line(&excl, &format!("/{LOCKFILE_REL}"))?;
    }

    let lf_path = project.lockfile_path();
    let mut lockfile = Lockfile::load(&lf_path)?;
    let lock_added = lockfile.upsert(LockItem {
        id: name.to_string(),
        item_type,
        source: "local".to_string(),
        git_ref: None,
        mode: Mode::Symlink,
        target: target_rel.clone(),
        bundle: None,
    });
    lockfile.save(&lf_path)?;

    Ok(AddReport {
        id: name.to_string(),
        item_type,
        mode: Mode::Symlink,
        target: target_rel,
        link_created: outcome.created(),
        exclude_added,
        lock_added,
        not_a_git_repo,
    })
}

/// Pull a skill from the collection into the project (symlink, gitignore, record).
pub fn add_skill(project: &Project, collection: &Collection, name: &str) -> Result<AddReport> {
    add_item(project, collection, ItemType::Skill, name)
}

/// Remove an installed item from the project.
pub fn remove_item(project: &Project, item_type: ItemType, name: &str) -> Result<RemoveReport> {
    let lf_path = project.lockfile_path();
    let mut lockfile = Lockfile::load(&lf_path)?;
    let removed_item = lockfile.remove(item_type, name);
    let lock_removed = removed_item.is_some();
    let target = removed_item
        .as_ref()
        .map(|item| item.target.clone())
        .unwrap_or_else(|| target_for(item_type, name));

    let target_removed = if lock_removed {
        fsops::remove(&project.root.join(&target))?
    } else {
        false
    };

    let mut exclude_removed = false;
    let not_a_git_repo = project.git_dir.is_none();
    if lock_removed {
        if let Some(excl) = project.git_info_exclude_path() {
            exclude_removed = gitexclude::remove_line(&excl, &format!("/{target}"))?;
        }
        lockfile.save(&lf_path)?;
    }

    Ok(RemoveReport {
        id: name.to_string(),
        item_type,
        target,
        target_removed,
        exclude_removed,
        lock_removed,
        not_installed: !lock_removed,
        not_a_git_repo,
    })
}

/// Remove an installed skill from the project.
pub fn remove_skill(project: &Project, name: &str) -> Result<RemoveReport> {
    remove_item(project, ItemType::Skill, name)
}

/// List lockfile items with their on-disk health.
pub fn list_items(project: &Project) -> Result<Vec<ListItem>> {
    let lockfile = Lockfile::load(&project.lockfile_path())?;
    lockfile
        .items
        .into_iter()
        .map(|item| {
            let status = health(project, &item)?;
            Ok(ListItem {
                id: item.id,
                item_type: item.item_type,
                mode: item.mode,
                target: item.target,
                status,
            })
        })
        .collect()
}

fn target_for(item_type: ItemType, name: &str) -> String {
    match item_type {
        ItemType::Skill => format!(".github/skills/{name}"),
        ItemType::Agent => format!(".github/agents/{name}.agent.md"),
    }
}

fn health(project: &Project, item: &LockItem) -> Result<HealthStatus> {
    let dst = project.root.join(&item.target);
    match std::fs::symlink_metadata(&dst) {
        Ok(meta) if meta.file_type().is_symlink() => {
            if dst.canonicalize().is_ok() {
                Ok(HealthStatus::Ok)
            } else {
                Ok(HealthStatus::Orphaned)
            }
        }
        Ok(_) => Ok(HealthStatus::Ok),
        Err(e) if e.kind() == ErrorKind::NotFound => Ok(HealthStatus::Missing),
        Err(e) => Err(e).with_context(|| format!("reading {}", dst.display())),
    }
}
