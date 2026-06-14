//! High-level engine operations. The CLI and any GUI call these; they own the end-to-end
//! pipeline (resolve → materialize/remove → gitignore → record in lockfile).

use anyhow::{Context, Result};
use serde::Serialize;
use std::io::ErrorKind;

use crate::bundle;
use crate::collection::Collection;
use crate::fsops;
use crate::gitexclude;
use crate::lockfile::{ItemType, LockItem, Lockfile, Mode};
use crate::manifest;
use crate::project::Project;
use crate::remote::{self, SourceSpec};

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
    /// `local` for collection items, or `owner/repo/path` for remote items.
    pub source: String,
    /// Source ref, when applicable.
    #[serde(rename = "ref", skip_serializing_if = "Option::is_none")]
    pub git_ref: Option<String>,
    /// Bundle this item was installed as part of, if any.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bundle: Option<String>,
    /// Whether the link/copy was created (vs already present).
    pub link_created: bool,
    /// Whether a new line was added to `.git/info/exclude`.
    pub exclude_added: bool,
    /// Whether a new lockfile entry was added (vs replaced).
    pub lock_added: bool,
    /// True if the project is not a git repo (pulls cannot be auto-ignored).
    pub not_a_git_repo: bool,
}

/// Outcome of an `add --bundle` operation.
#[derive(Debug, Serialize)]
pub struct BundleAddReport {
    pub bundle: String,
    pub items: Vec<AddReport>,
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

/// Outcome of an `rm --bundle` operation.
#[derive(Debug, Serialize)]
pub struct BundleRemoveReport {
    pub bundle: String,
    pub items: Vec<RemoveReport>,
}

/// Health status for an installed lockfile item.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum HealthStatus {
    Ok,
    Orphaned,
    Missing,
    Drifted,
}

/// One row returned by `ls`.
#[derive(Debug, Serialize)]
pub struct ListItem {
    pub id: String,
    #[serde(rename = "type")]
    pub item_type: ItemType,
    pub mode: Mode,
    pub target: String,
    pub bundle: Option<String>,
    pub status: HealthStatus,
}

/// Pull an item from the collection into the project (symlink, gitignore, record).
pub fn add_item(
    project: &Project,
    collection: &Collection,
    item_type: ItemType,
    name: &str,
    mode: Mode,
    bundle_name: Option<&str>,
) -> Result<AddReport> {
    let src = match item_type {
        ItemType::Skill => collection.resolve_skill(name)?,
        ItemType::Agent => collection.resolve_agent(name)?,
    };
    let target_rel = target_for(item_type, name);
    record_materialized(
        project,
        MaterializeRecord {
            item_type,
            id: name,
            target_rel,
            src: &src,
            mode,
            source: "local".to_string(),
            git_ref: None,
            bundle_name,
        },
    )
}

/// Outcome of a `pull` operation (fetch a remote source into the local collection).
#[derive(Debug, Serialize)]
pub struct PullReport {
    pub id: String,
    #[serde(rename = "type")]
    pub item_type: ItemType,
    /// `owner/repo/path` source the item was fetched from.
    pub source: String,
    /// Source ref, when one was supplied.
    #[serde(rename = "ref", skip_serializing_if = "Option::is_none")]
    pub git_ref: Option<String>,
    /// Absolute path written in the collection.
    pub path: String,
    /// Whether files were written (false when an identical copy was already present).
    pub created: bool,
    /// Whether an existing, differing item was overwritten (requires `force`).
    pub overwritten: bool,
}

/// Fetch a remote `owner/repo/path[#ref]` source and copy it into the local collection.
///
/// Unlike [`add_remote`], which materializes a remote source straight into a project, this
/// seeds a reusable **collection** item (`skills/<id>/` or `agents/<id>.agent.md`) so it can
/// later be added, searched, and previewed like any other local item. The copy is standalone,
/// independent of the git-fetch cache.
///
/// The remote provenance is recorded in the collection manifest ([`manifest`]) so the item can
/// be re-fetched on a new machine with [`restore_collection`].
pub fn pull_into_collection(
    collection: &Collection,
    spec: &SourceSpec,
    item_type: ItemType,
    as_id: Option<&str>,
    base_url: &str,
    force: bool,
) -> Result<PullReport> {
    let report = pull_copy(collection, spec, item_type, as_id, base_url, force)?;
    manifest::record(
        collection,
        &manifest::ManifestEntry {
            spec: spec.clone(),
            item_type,
            id: report.id.clone(),
        },
    )?;
    Ok(report)
}

/// Copy a remote source into the collection without touching the manifest.
fn pull_copy(
    collection: &Collection,
    spec: &SourceSpec,
    item_type: ItemType,
    as_id: Option<&str>,
    base_url: &str,
    force: bool,
) -> Result<PullReport> {
    let src = remote::fetch(spec, base_url)?;
    let default_id = remote_id(item_type, spec);
    let id = as_id.unwrap_or(&default_id);
    ensure_simple_id(id)?;
    validate_remote_source(item_type, id, &src)?;

    let dst = match item_type {
        ItemType::Skill => collection.skill_source(id),
        ItemType::Agent => collection.agent_source(id),
    };

    let existed = std::fs::symlink_metadata(&dst).is_ok();
    let mut overwritten = false;
    let created;
    if existed {
        if fsops::drifted(&src, &dst)? {
            if !force {
                anyhow::bail!(
                    "collection already has {} '{id}' at {} and it differs from the source; \
                     pass --force to overwrite",
                    type_label(item_type),
                    dst.display()
                );
            }
            fsops::remove(&dst)?;
            fsops::materialize(Mode::Copy, &src, &dst)?;
            overwritten = true;
            created = true;
        } else {
            created = false;
        }
    } else {
        fsops::materialize(Mode::Copy, &src, &dst)?;
        created = true;
    }

    Ok(PullReport {
        id: id.to_string(),
        item_type,
        source: spec.source(),
        git_ref: spec.ref_.clone(),
        path: dst.display().to_string(),
        created,
        overwritten,
    })
}

/// Status of a single item processed by [`restore_collection`].
#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum RestoreStatus {
    /// Newly fetched and written into the collection.
    Pulled,
    /// Already present and identical; nothing changed.
    AlreadyPresent,
    /// Present but differed; overwritten because `force` was set.
    Overwritten,
    /// Could not be restored (see `error`).
    Error,
}

/// Per-item result of a restore.
#[derive(Debug, Serialize)]
pub struct RestoreItem {
    pub id: String,
    #[serde(rename = "type")]
    pub item_type: ItemType,
    /// `owner/repo/path` source the item is fetched from.
    pub source: String,
    #[serde(rename = "ref", skip_serializing_if = "Option::is_none")]
    pub git_ref: Option<String>,
    pub status: RestoreStatus,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// Aggregate counts for a restore.
#[derive(Debug, Default, Serialize)]
pub struct RestoreSummary {
    pub pulled: usize,
    pub already_present: usize,
    pub overwritten: usize,
    pub errors: usize,
}

/// Outcome of a `restore` operation.
#[derive(Debug, Serialize)]
pub struct RestoreReport {
    pub items: Vec<RestoreItem>,
    pub summary: RestoreSummary,
}

/// Re-fetch every item recorded in the collection manifest.
///
/// Each entry is pulled with its recorded id (`--as` semantics) for an exact reproduction.
/// Per-item failures are collected rather than aborting the whole run; the caller decides how
/// to react to a non-zero `summary.errors`.
pub fn restore_collection(
    collection: &Collection,
    base_url: &str,
    force: bool,
) -> Result<RestoreReport> {
    let entries = manifest::entries(collection)?;
    let mut items = Vec::with_capacity(entries.len());
    let mut summary = RestoreSummary::default();

    for entry in entries {
        let result = pull_copy(
            collection,
            &entry.spec,
            entry.item_type,
            Some(&entry.id),
            base_url,
            force,
        );
        let item = match result {
            Ok(report) => {
                let status = if report.overwritten {
                    summary.overwritten += 1;
                    RestoreStatus::Overwritten
                } else if report.created {
                    summary.pulled += 1;
                    RestoreStatus::Pulled
                } else {
                    summary.already_present += 1;
                    RestoreStatus::AlreadyPresent
                };
                RestoreItem {
                    id: report.id,
                    item_type: report.item_type,
                    source: report.source,
                    git_ref: report.git_ref,
                    status,
                    error: None,
                }
            }
            Err(e) => {
                summary.errors += 1;
                RestoreItem {
                    id: entry.id,
                    item_type: entry.item_type,
                    source: entry.spec.source(),
                    git_ref: entry.spec.ref_.clone(),
                    status: RestoreStatus::Error,
                    error: Some(format!("{e:#}")),
                }
            }
        };
        items.push(item);
    }

    Ok(RestoreReport { items, summary })
}

fn ensure_simple_id(id: &str) -> Result<()> {
    if id.is_empty() || id == "." || id == ".." || id.contains('/') || id.contains('\\') {
        anyhow::bail!("invalid collection id '{id}'; expected a single path segment");
    }
    Ok(())
}

fn type_label(item_type: ItemType) -> &'static str {
    match item_type {
        ItemType::Skill => "skill",
        ItemType::Agent => "agent",
    }
}

/// Pull a remote item into the project through the same materialize/gitignore/lockfile pipeline.
pub fn add_remote(
    project: &Project,
    spec: &SourceSpec,
    item_type: ItemType,
    mode: Mode,
    base_url: &str,
) -> Result<AddReport> {
    let src = remote::fetch(spec, base_url)?;
    let id = remote_id(item_type, spec);
    validate_remote_source(item_type, &id, &src)?;
    let target_rel = target_for(item_type, &id);
    record_materialized(
        project,
        MaterializeRecord {
            item_type,
            id: &id,
            target_rel,
            src: &src,
            mode,
            source: spec.source(),
            git_ref: spec.ref_.clone(),
            bundle_name: None,
        },
    )
}

struct MaterializeRecord<'a> {
    item_type: ItemType,
    id: &'a str,
    target_rel: String,
    src: &'a std::path::Path,
    mode: Mode,
    source: String,
    git_ref: Option<String>,
    bundle_name: Option<&'a str>,
}

fn record_materialized(project: &Project, input: MaterializeRecord<'_>) -> Result<AddReport> {
    let MaterializeRecord {
        item_type,
        id,
        target_rel,
        src,
        mode,
        source,
        git_ref,
        bundle_name,
    } = input;
    let dst = project.root.join(&target_rel);

    let materialized = fsops::materialize_with_fallback(mode, src, &dst)?;

    let mut exclude_added = false;
    let not_a_git_repo = project.git_dir.is_none();
    if let Some(excl) = project.git_info_exclude_path() {
        exclude_added |= gitexclude::add_line(&excl, &format!("/{target_rel}"))?;
        exclude_added |= gitexclude::add_line(&excl, &format!("/{LOCKFILE_REL}"))?;
    }

    let lf_path = project.lockfile_path();
    let mut lockfile = Lockfile::load(&lf_path)?;
    let bundle = bundle_name.map(str::to_string);
    let lock_added = lockfile.upsert(LockItem {
        id: id.to_string(),
        item_type,
        source: source.clone(),
        git_ref: git_ref.clone(),
        mode: materialized.mode,
        target: target_rel.clone(),
        bundle: bundle.clone(),
    });
    lockfile.save(&lf_path)?;

    Ok(AddReport {
        id: id.to_string(),
        item_type,
        mode: materialized.mode,
        target: target_rel,
        source,
        git_ref,
        bundle,
        link_created: materialized.outcome.created(),
        exclude_added,
        lock_added,
        not_a_git_repo,
    })
}

fn validate_remote_source(item_type: ItemType, id: &str, src: &std::path::Path) -> Result<()> {
    match item_type {
        ItemType::Skill => {
            if !src.is_dir() {
                anyhow::bail!(
                    "remote skill '{id}' must be a directory (resolved {})",
                    src.display()
                );
            }
            let skill_md = src.join("SKILL.md");
            if !skill_md.is_file() {
                anyhow::bail!(
                    "remote skill '{id}' is missing SKILL.md ({})",
                    skill_md.display()
                );
            }
        }
        ItemType::Agent => {
            if !src.is_file() {
                anyhow::bail!(
                    "remote agent '{id}' must be a .agent.md file (resolved {})",
                    src.display()
                );
            }
        }
    }
    Ok(())
}

fn remote_id(item_type: ItemType, spec: &SourceSpec) -> String {
    let leaf = spec.leaf();
    match item_type {
        ItemType::Skill => leaf.to_string(),
        ItemType::Agent => leaf.strip_suffix(".agent.md").unwrap_or(leaf).to_string(),
    }
}

/// Pull a skill from the collection into the project (symlink, gitignore, record).
pub fn add_skill(project: &Project, collection: &Collection, name: &str) -> Result<AddReport> {
    add_item(
        project,
        collection,
        ItemType::Skill,
        name,
        Mode::Symlink,
        None,
    )
}

/// Pull every item in a named collection bundle into the project.
pub fn add_bundle(
    project: &Project,
    collection: &Collection,
    name: &str,
    mode: Mode,
) -> Result<BundleAddReport> {
    let bundle = bundle::load(collection, name)?;
    let mut items = Vec::with_capacity(bundle.items.len());
    for item in &bundle.items {
        items.push(add_item(
            project,
            collection,
            item.item_type,
            &item.id,
            mode,
            Some(&bundle.name),
        )?);
    }
    Ok(BundleAddReport {
        bundle: bundle.name,
        items,
    })
}

/// Remove an installed item from the project.
pub fn remove_item(project: &Project, item_type: ItemType, name: &str) -> Result<RemoveReport> {
    let lf_path = project.lockfile_path();
    let mut lockfile = Lockfile::load(&lf_path)?;
    let report = remove_item_from_lockfile(project, &mut lockfile, item_type, name)?;
    if report.lock_removed {
        lockfile.save(&lf_path)?;
    }
    Ok(report)
}

/// Remove an installed skill from the project.
pub fn remove_skill(project: &Project, name: &str) -> Result<RemoveReport> {
    remove_item(project, ItemType::Skill, name)
}

/// Remove every installed item tagged with a named bundle.
pub fn remove_bundle(project: &Project, name: &str) -> Result<BundleRemoveReport> {
    let lf_path = project.lockfile_path();
    let mut lockfile = Lockfile::load(&lf_path)?;
    let bundle_items: Vec<(ItemType, String)> = lockfile
        .items
        .iter()
        .filter(|item| item.bundle.as_deref() == Some(name))
        .map(|item| (item.item_type, item.id.clone()))
        .collect();

    let mut items = Vec::with_capacity(bundle_items.len());
    for (item_type, id) in bundle_items {
        items.push(remove_item_from_lockfile(
            project,
            &mut lockfile,
            item_type,
            &id,
        )?);
    }
    if items.iter().any(|item| item.lock_removed) {
        lockfile.save(&lf_path)?;
    }

    Ok(BundleRemoveReport {
        bundle: name.to_string(),
        items,
    })
}

fn remove_item_from_lockfile(
    project: &Project,
    lockfile: &mut Lockfile,
    item_type: ItemType,
    name: &str,
) -> Result<RemoveReport> {
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
    if lock_removed && let Some(excl) = project.git_info_exclude_path() {
        exclude_removed = gitexclude::remove_line(&excl, &format!("/{target}"))?;
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

/// List lockfile items with their on-disk health.
pub fn list_items(project: &Project) -> Result<Vec<ListItem>> {
    list_items_with_optional_collection(project, None)
}

/// List lockfile items using an explicit collection root for copy drift checks.
pub fn list_items_with_collection(
    project: &Project,
    collection: &Collection,
) -> Result<Vec<ListItem>> {
    list_items_with_optional_collection(project, Some(collection))
}

fn list_items_with_optional_collection(
    project: &Project,
    collection: Option<&Collection>,
) -> Result<Vec<ListItem>> {
    let lockfile = Lockfile::load(&project.lockfile_path())?;
    let needs_collection = lockfile.items.iter().any(|item| item.mode == Mode::Copy);
    let located_collection = if collection.is_none() && needs_collection {
        Some(Collection::locate()?)
    } else {
        None
    };
    let collection = collection.or(located_collection.as_ref());

    lockfile
        .items
        .into_iter()
        .map(|item| {
            let status = health(project, &item, collection)?;
            Ok(ListItem {
                id: item.id,
                item_type: item.item_type,
                mode: item.mode,
                target: item.target,
                bundle: item.bundle,
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

pub(crate) fn health(
    project: &Project,
    item: &LockItem,
    collection: Option<&Collection>,
) -> Result<HealthStatus> {
    let dst = project.root.join(&item.target);
    match std::fs::symlink_metadata(&dst) {
        Ok(_) if item.mode == Mode::Copy => {
            let collection = collection.context("collection is required to check copy drift")?;
            let src = source_for_item(collection, item);
            if !src.exists() {
                return Ok(HealthStatus::Orphaned);
            }
            if fsops::drifted(&src, &dst)? {
                Ok(HealthStatus::Drifted)
            } else {
                Ok(HealthStatus::Ok)
            }
        }
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

pub(crate) fn source_for(
    collection: &Collection,
    item_type: ItemType,
    id: &str,
) -> std::path::PathBuf {
    match item_type {
        ItemType::Skill => collection.skill_source(id),
        ItemType::Agent => collection.agent_source(id),
    }
}

pub(crate) fn source_for_item(collection: &Collection, item: &LockItem) -> std::path::PathBuf {
    if item.source == "local" {
        return source_for(collection, item.item_type, &item.id);
    }
    SourceSpec::from_source_and_ref(&item.source, item.git_ref.clone())
        .map(|spec| remote::cached_item_path(&spec))
        .unwrap_or_else(|| source_for(collection, item.item_type, &item.id))
}
