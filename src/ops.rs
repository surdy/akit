//! High-level engine operations. The CLI and any GUI call these; they own the end-to-end
//! pipeline (resolve → materialize → gitignore → record in lockfile).

use anyhow::Result;
use serde::Serialize;

use crate::collection::Collection;
use crate::fsops;
use crate::gitexclude;
use crate::lockfile::{ItemType, LockItem, Lockfile, Mode};
use crate::project::Project;

/// Project-relative path of the lockfile, used for the git-exclude entry.
const LOCKFILE_REL: &str = ".copilot/kit.lock.json";

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

/// Pull a skill from the collection into the project (symlink, gitignore, record).
pub fn add_skill(project: &Project, collection: &Collection, name: &str) -> Result<AddReport> {
    let src = collection.resolve_skill(name)?;
    let target_rel = format!(".github/skills/{name}");
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
        item_type: ItemType::Skill,
        source: "local".to_string(),
        git_ref: None,
        mode: Mode::Symlink,
        target: target_rel.clone(),
        bundle: None,
    });
    lockfile.save(&lf_path)?;

    Ok(AddReport {
        id: name.to_string(),
        item_type: ItemType::Skill,
        mode: Mode::Symlink,
        target: target_rel,
        link_created: outcome.created(),
        exclude_added,
        lock_added,
        not_a_git_repo,
    })
}
