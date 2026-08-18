//! High-level catalog engine operations: fetch remote sources into the catalog
//! (`pull`/`restore`/`update`/`log`/`drop`) and list catalog contents (`ls`).
//! Project materialization is the harness-aware engine's job ([`crate::install`]).

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::io::ErrorKind;
use std::path::PathBuf;

use crate::catalog::Catalog;
use crate::fsops;
use crate::harness::HarnessId;
use crate::lockfile::{ItemType, Mode};
use crate::manifest;
use crate::remote::{self, SourceSpec};

/// Whether an installed bundle has all its manifest members present.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum BundleState {
    /// Every member declared by `bundles/<name>.yml` is installed.
    Complete,
    /// Some declared members are not installed.
    Partial,
    /// The bundle manifest could not be read, so completeness is undetermined.
    Unknown,
}

/// Completeness of one installed bundle, comparing the project lockfile's
/// bundle-tagged entries against the catalog `bundles/<name>.yml` manifest.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BundleHealth {
    pub name: String,
    /// Members declared by the manifest; `None` when the manifest is unreadable.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub expected: Option<usize>,
    /// Manifest members currently present in the lockfile.
    pub installed: usize,
    /// Declared members not installed (ids only); empty for `Complete`/`Unknown`.
    pub missing: Vec<String>,
    pub state: BundleState,
}

/// One skill or agent present in the catalog, as listed by `catalog ls`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CatalogItem {
    /// Catalog id: the handle used by `add`, `show`, and `unpull`.
    pub id: String,
    #[serde(rename = "type")]
    pub item_type: ItemType,
    /// Frontmatter description, or empty when absent.
    pub description: String,
    /// Remote provenance (`owner/repo/path[#ref]`) when the item was pulled and
    /// recorded in the manifest; `None` for hand-authored (local) items.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
    /// Harnesses an agent *package* supports. Empty for skills, and for an
    /// invalid package (which has no resolvable per-harness contract).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub harnesses: Vec<HarnessId>,
    /// True when this agent is an invalid package (surfaced but not installable);
    /// `description` then carries the diagnostic. Never set for valid items.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub disabled: bool,
}

/// Outcome of a `pull` operation (fetch a remote source into the local catalog).
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
    /// Absolute path written in the catalog.
    pub path: String,
    /// Whether files were written (false when an identical copy was already present).
    pub created: bool,
    /// Whether an existing, differing item was overwritten (requires `force`).
    pub overwritten: bool,
    /// Commit SHA the source resolved to, when it could be determined.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub commit: Option<String>,
}

/// Fetch a remote `owner/repo/path[#ref]` source and copy it into the local catalog.
///
/// This seeds a reusable **catalog** item (`skills/<id>/`, or an `agents/<id>/` agent
/// package) so it can later be installed, searched, and previewed like any other local
/// item. The copy is standalone, independent of the git-fetch cache.
///
/// The remote provenance is recorded in the catalog manifest ([`manifest`]) so the item can
/// be re-fetched on a new machine with [`restore_catalog`].
pub fn pull_into_catalog(
    catalog: &Catalog,
    spec: &SourceSpec,
    item_type: ItemType,
    as_id: Option<&str>,
    base_url: &str,
    force: bool,
) -> Result<PullReport> {
    let report = pull_copy(
        catalog,
        spec,
        item_type,
        as_id,
        base_url,
        FetchMode::Cached,
        force,
    )?;
    manifest::record(
        catalog,
        &manifest::ManifestEntry {
            spec: spec.clone(),
            item_type,
            id: report.id.clone(),
            commit: report.commit.clone(),
            agent_package: is_agent_package(catalog, item_type, &report.id),
        },
    )?;
    Ok(report)
}

/// Whether `(item_type, id)` is materialized in the catalog as a harness-aware
/// agent package directory. Every agent is one — this guards against recording a
/// manifest entry for an agent that did not land as a package.
fn is_agent_package(catalog: &Catalog, item_type: ItemType, id: &str) -> bool {
    item_type == ItemType::Agent && catalog.agent_package_dir(id).is_dir()
}

/// Whether to reuse the source cache as-is or re-fetch the latest commit first.
#[derive(Debug, Clone, Copy)]
enum FetchMode {
    /// Reuse the cached checkout (clone on first use); the default for `pull`/`restore`.
    Cached,
    /// Force a re-fetch of the latest commit of the ref before copying; used by `update`.
    Refresh,
}

impl FetchMode {
    fn fetch(self, spec: &SourceSpec, base_url: &str) -> Result<PathBuf> {
        match self {
            FetchMode::Cached => remote::fetch(spec, base_url),
            FetchMode::Refresh => remote::refresh(spec, base_url),
        }
    }
}

/// Copy a remote source into the catalog without touching the manifest.
fn pull_copy(
    catalog: &Catalog,
    spec: &SourceSpec,
    item_type: ItemType,
    as_id: Option<&str>,
    base_url: &str,
    fetch_mode: FetchMode,
    force: bool,
) -> Result<PullReport> {
    let src = fetch_mode.fetch(spec, base_url)?;
    let default_id = remote_id(item_type, spec);
    let id = as_id.unwrap_or(&default_id);
    ensure_simple_id(id)?;
    let dst = catalog_dst_for_source(catalog, item_type, id, &src)?;

    let existed = std::fs::symlink_metadata(&dst).is_ok();
    let mut overwritten = false;
    let created;
    if existed {
        if fsops::drifted(&src, &dst)? {
            if !force {
                anyhow::bail!(
                    "catalog already has {} '{id}' at {} and it differs from the source; \
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
        commit: remote::resolved_commit(spec),
    })
}

/// Status of a single item processed by [`restore_catalog`].
#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum RestoreStatus {
    /// Newly fetched and written into the catalog.
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

/// Re-fetch every item recorded in the catalog manifest.
///
/// Each entry is pulled under its recorded id (`--as` semantics) for an exact reproduction. When
/// the entry records a resolved `commit` and `latest` is false, that exact commit is checked out
/// so restores are reproducible across machines; with `latest = true` (or for legacy entries
/// without a recorded commit) the head of the symbolic ref is fetched instead, and the freshly
/// resolved commit is written back to the manifest.
///
/// Per-item failures are collected rather than aborting the whole run; the caller decides how to
/// react to a non-zero `summary.errors`.
pub fn restore_catalog(
    catalog: &Catalog,
    base_url: &str,
    force: bool,
    latest: bool,
) -> Result<RestoreReport> {
    let entries = manifest::entries(catalog)?;
    let mut items = Vec::with_capacity(entries.len());
    let mut summary = RestoreSummary::default();

    for entry in entries {
        // A pre-v0.32.0 manifest may still record a legacy flat `.agent.md` agent. That
        // shape no longer exists, so it cannot be re-fetched — report it as a per-item
        // error with a migration hint and carry on, leaving the entry in place so it can
        // be fixed and retried.
        if entry.is_legacy_flat_agent() {
            summary.errors += 1;
            items.push(RestoreItem {
                id: entry.id.clone(),
                item_type: entry.item_type,
                source: entry.spec.source(),
                git_ref: entry.spec.ref_.clone(),
                status: RestoreStatus::Error,
                error: Some(entry.legacy_flat_hint()),
            });
            continue;
        }

        // Pin to the recorded commit for reproducibility unless the caller asked for the latest
        // (or no commit was ever recorded), in which case follow the symbolic ref.
        let pin_to_commit = !latest && entry.commit.is_some();
        let fetch_spec = match &entry.commit {
            Some(commit) if pin_to_commit => SourceSpec {
                ref_: Some(commit.clone()),
                ..entry.spec.clone()
            },
            _ => entry.spec.clone(),
        };
        let fetch_mode = if latest {
            FetchMode::Refresh
        } else {
            FetchMode::Cached
        };

        let result = pull_copy(
            catalog,
            &fetch_spec,
            entry.item_type,
            Some(&entry.id),
            base_url,
            fetch_mode,
            force,
        );
        let item = match result {
            Ok(report) => {
                // When following the ref, persist the commit it resolved to (records SHAs for
                // legacy entries and advances them under `--latest`).
                if !pin_to_commit && report.commit != entry.commit {
                    manifest::record(
                        catalog,
                        &manifest::ManifestEntry {
                            spec: entry.spec.clone(),
                            item_type: entry.item_type,
                            id: entry.id.clone(),
                            commit: report.commit.clone(),
                            agent_package: entry.agent_package,
                        },
                    )?;
                }
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
                    git_ref: entry.spec.ref_.clone(),
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

/// Status of a single item processed by [`update_catalog`].
#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum UpdateStatus {
    /// The catalog copy was refreshed to a newer upstream commit.
    Updated,
    /// Upstream has newer content (check mode only; nothing was written).
    Outdated,
    /// Already at the latest upstream content; nothing changed.
    UpToDate,
    /// Pinned to an immutable full commit SHA; never re-fetched.
    Pinned,
    /// Could not be updated (see `error`).
    Error,
}

/// Per-item result of an update.
#[derive(Debug, Serialize)]
pub struct UpdateItem {
    pub id: String,
    #[serde(rename = "type")]
    pub item_type: ItemType,
    /// `owner/repo/path` source the item is fetched from.
    pub source: String,
    #[serde(rename = "ref", skip_serializing_if = "Option::is_none")]
    pub git_ref: Option<String>,
    pub status: UpdateStatus,
    /// Commit recorded before the update, when one was known.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub previous_commit: Option<String>,
    /// Commit the source resolves to now, when it could be determined.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub commit: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// Aggregate counts for an update.
#[derive(Debug, Default, Serialize)]
pub struct UpdateSummary {
    pub updated: usize,
    pub outdated: usize,
    pub up_to_date: usize,
    pub pinned: usize,
    pub errors: usize,
}

/// Outcome of an `update` operation.
#[derive(Debug, Serialize)]
pub struct UpdateReport {
    pub items: Vec<UpdateItem>,
    pub summary: UpdateSummary,
}

/// Whether a ref is an immutable full commit SHA (40 hex for SHA-1, 64 for SHA-256).
///
/// Such refs can never move upstream, so `update` reports them as `Pinned` and skips the
/// network. Tags are also immutable but indistinguishable from branches without a probe;
/// re-fetching a tag is harmless (it reports `up-to-date`), so only full SHAs get the skip.
fn is_full_sha(ref_: &str) -> bool {
    matches!(ref_.len(), 40 | 64) && ref_.bytes().all(|b| b.is_ascii_hexdigit())
}

/// Fetch the latest source and report whether the catalog copy is outdated, without writing.
///
/// A missing destination counts as outdated so `update --check` flags items that need a pull.
fn pull_check(
    catalog: &Catalog,
    spec: &SourceSpec,
    item_type: ItemType,
    as_id: Option<&str>,
    base_url: &str,
    fetch_mode: FetchMode,
) -> Result<bool> {
    let src = fetch_mode.fetch(spec, base_url)?;
    let default_id = remote_id(item_type, spec);
    let id = as_id.unwrap_or(&default_id);
    ensure_simple_id(id)?;
    let dst = catalog_dst_for_source(catalog, item_type, id, &src)?;
    if std::fs::symlink_metadata(&dst).is_err() {
        return Ok(true);
    }
    fsops::drifted(&src, &dst)
}

/// Re-fetch the latest upstream content for catalog items recorded in the manifest.
///
/// With `only = None`, every recorded item is considered; otherwise only the entry matching the
/// given `(type, id)` is, and a non-match is an error. Items sharing an `(owner, repo, ref)` are
/// network-refreshed once (the rest reuse the warmed cache). Full-SHA refs are immutable and
/// reported as `Pinned` without contacting the remote.
///
/// In `check` mode nothing is written; items are reported as `Outdated`/`UpToDate`. Otherwise the
/// catalog copy is overwritten in place when upstream moved (`Updated`) or left as-is (`UpToDate`).
/// Per-item failures are collected rather than aborting; the caller reacts to `summary.errors`.
pub fn update_catalog(
    catalog: &Catalog,
    only: Option<(ItemType, &str)>,
    base_url: &str,
    check: bool,
) -> Result<UpdateReport> {
    let entries = manifest::entries(catalog)?;
    let mut items = Vec::new();
    let mut summary = UpdateSummary::default();
    let mut matched = false;
    let mut refreshed: HashSet<(String, String, Option<String>)> = HashSet::new();

    for entry in entries {
        if let Some((ty, id)) = only
            && (entry.item_type != ty || entry.id != id)
        {
            continue;
        }
        matched = true;

        // A legacy flat `.agent.md` entry from a pre-v0.32.0 manifest can no longer be
        // materialized; report it and continue rather than failing the whole run.
        if entry.is_legacy_flat_agent() {
            summary.errors += 1;
            items.push(UpdateItem {
                id: entry.id.clone(),
                item_type: entry.item_type,
                source: entry.spec.source(),
                git_ref: entry.spec.ref_.clone(),
                status: UpdateStatus::Error,
                previous_commit: entry.commit.clone(),
                commit: entry.commit.clone(),
                error: Some(entry.legacy_flat_hint()),
            });
            continue;
        }

        if entry.spec.ref_.as_deref().is_some_and(is_full_sha) {
            summary.pinned += 1;
            items.push(UpdateItem {
                id: entry.id,
                item_type: entry.item_type,
                source: entry.spec.source(),
                git_ref: entry.spec.ref_.clone(),
                status: UpdateStatus::Pinned,
                previous_commit: entry.commit.clone(),
                commit: entry.commit.clone(),
                error: None,
            });
            continue;
        }

        // Refresh each shared checkout from the network only once.
        let key = (
            entry.spec.owner.clone(),
            entry.spec.repo.clone(),
            entry.spec.ref_.clone(),
        );
        let mode = if refreshed.insert(key) {
            FetchMode::Refresh
        } else {
            FetchMode::Cached
        };

        // Each branch resolves the status plus the commit the ref now points at.
        let outcome: Result<(UpdateStatus, Option<String>)> = if check {
            pull_check(
                catalog,
                &entry.spec,
                entry.item_type,
                Some(&entry.id),
                base_url,
                mode,
            )
            .map(|outdated| {
                let status = if outdated {
                    UpdateStatus::Outdated
                } else {
                    UpdateStatus::UpToDate
                };
                (status, remote::resolved_commit(&entry.spec))
            })
        } else {
            pull_copy(
                catalog,
                &entry.spec,
                entry.item_type,
                Some(&entry.id),
                base_url,
                mode,
                true,
            )
            .map(|report| {
                // Prefer the recorded SHA for the verdict; fall back to content drift for legacy
                // entries (or when the commit couldn't be resolved).
                let advanced = match (&entry.commit, &report.commit) {
                    (Some(old), Some(new)) => old != new,
                    _ => report.created,
                };
                let status = if advanced {
                    UpdateStatus::Updated
                } else {
                    UpdateStatus::UpToDate
                };
                (status, report.commit)
            })
        };

        let item = match outcome {
            Ok((status, new_commit)) => {
                match status {
                    UpdateStatus::Updated => summary.updated += 1,
                    UpdateStatus::Outdated => summary.outdated += 1,
                    UpdateStatus::UpToDate => summary.up_to_date += 1,
                    UpdateStatus::Pinned => summary.pinned += 1,
                    UpdateStatus::Error => summary.errors += 1,
                }
                // Persist the resolved commit when applying (records SHAs for legacy entries and
                // advances them when the ref moved). `--check` never writes.
                if !check && new_commit != entry.commit {
                    manifest::record(
                        catalog,
                        &manifest::ManifestEntry {
                            spec: entry.spec.clone(),
                            item_type: entry.item_type,
                            id: entry.id.clone(),
                            commit: new_commit.clone(),
                            agent_package: entry.agent_package,
                        },
                    )?;
                }
                UpdateItem {
                    id: entry.id,
                    item_type: entry.item_type,
                    source: entry.spec.source(),
                    git_ref: entry.spec.ref_.clone(),
                    status,
                    previous_commit: entry.commit.clone(),
                    commit: new_commit,
                    error: None,
                }
            }
            Err(e) => {
                summary.errors += 1;
                UpdateItem {
                    id: entry.id,
                    item_type: entry.item_type,
                    source: entry.spec.source(),
                    git_ref: entry.spec.ref_.clone(),
                    status: UpdateStatus::Error,
                    previous_commit: entry.commit.clone(),
                    commit: None,
                    error: Some(format!("{e:#}")),
                }
            }
        };
        items.push(item);
    }

    if let Some((_, id)) = only
        && !matched
    {
        anyhow::bail!("nothing to update: no catalog item with id '{id}' was pulled from a source");
    }

    Ok(UpdateReport { items, summary })
}

/// One row returned by `log`: an upstream commit of a pulled item's recorded ref.
#[derive(Debug, Serialize)]
pub struct LogEntry {
    /// Full commit SHA.
    pub commit: String,
    /// Recorded symbolic ref the history was walked from, when one was recorded.
    #[serde(rename = "ref", skip_serializing_if = "Option::is_none")]
    pub git_ref: Option<String>,
    /// Author date, `YYYY-MM-DD`.
    pub date: String,
    /// Commit subject.
    pub subject: String,
    /// Whether this is the commit currently recorded in the manifest (installed).
    pub current: bool,
}

/// List the upstream commit history of a pulled catalog item, newest first.
///
/// History is read from the git-fetch cache's clone of the item's recorded ref (deepening it once
/// when online); the manifest keeps only the current commit. The row whose commit matches the
/// manifest's recorded `commit` is flagged `current`. Errors when `id` was never pulled.
pub fn log_history(
    catalog: &Catalog,
    item_type: ItemType,
    id: &str,
    base_url: &str,
) -> Result<Vec<LogEntry>> {
    ensure_simple_id(id)?;
    let entry = find_pulled_entry(catalog, item_type, id)?;
    let commits = remote::history(&entry.spec, base_url)?;
    Ok(commits
        .into_iter()
        .map(|c| LogEntry {
            current: entry.commit.as_deref() == Some(c.commit.as_str()),
            commit: c.commit,
            git_ref: entry.spec.ref_.clone(),
            date: c.date,
            subject: c.subject,
        })
        .collect())
}

/// Roll back (or forward-pin) a pulled catalog item to an exact commit of its recorded ref.
///
/// The target `to` (a full SHA or an unambiguous prefix) must be reachable from the item's recorded
/// ref; an unreachable or unknown commit is rejected without touching the manifest. On success the
/// catalog copy is re-materialized at that commit and the manifest is pinned to the resolved full
/// SHA (so `update --check` reports it as `pinned`). Returns the same shape as [`update_catalog`].
pub fn rollback_catalog(
    catalog: &Catalog,
    item_type: ItemType,
    id: &str,
    to: &str,
    base_url: &str,
) -> Result<UpdateReport> {
    ensure_simple_id(id)?;
    let entry = find_pulled_entry(catalog, item_type, id)?;
    if entry.is_legacy_flat_agent() {
        anyhow::bail!("{}", entry.legacy_flat_hint());
    }

    // Resolve `to` against the recorded ref's history: this both expands a prefix to the full SHA
    // and enforces reachability (git log lists exactly the ancestors of the ref tip).
    let commits = remote::history(&entry.spec, base_url)?;
    let full = commits
        .iter()
        .find(|c| c.commit == to || c.commit.starts_with(to))
        .map(|c| c.commit.clone());
    let Some(full) = full else {
        let ref_label = entry.spec.ref_.as_deref().unwrap_or("the default branch");
        anyhow::bail!(
            "commit '{to}' is not reachable from {ref_label} of {}; \
             run `akit log {id}` to list valid commits",
            entry.spec.source()
        );
    };

    // Re-materialize pinned to the resolved SHA. A SHA ref caches under its own checkout dir, so
    // this fetches and checks out that exact commit (like a SHA-pinned pull).
    let pinned_spec = SourceSpec {
        ref_: Some(full.clone()),
        ..entry.spec.clone()
    };
    pull_copy(
        catalog,
        &pinned_spec,
        item_type,
        Some(&entry.id),
        base_url,
        FetchMode::Cached,
        true,
    )?;

    manifest::record(
        catalog,
        &manifest::ManifestEntry {
            spec: pinned_spec,
            item_type,
            id: entry.id.clone(),
            commit: Some(full.clone()),
            agent_package: entry.agent_package,
        },
    )?;

    let mut summary = UpdateSummary::default();
    let status = if entry.commit.as_deref() == Some(full.as_str()) {
        summary.up_to_date += 1;
        UpdateStatus::UpToDate
    } else {
        summary.updated += 1;
        UpdateStatus::Updated
    };
    Ok(UpdateReport {
        items: vec![UpdateItem {
            id: entry.id,
            item_type,
            source: entry.spec.source(),
            git_ref: entry.spec.ref_.clone(),
            status,
            previous_commit: entry.commit.clone(),
            commit: Some(full),
            error: None,
        }],
        summary,
    })
}

/// Find the manifest entry for a pulled `(type, id)`, or bail with `update`-style guidance.
fn find_pulled_entry(
    catalog: &Catalog,
    item_type: ItemType,
    id: &str,
) -> Result<manifest::ManifestEntry> {
    manifest::entries(catalog)?
        .into_iter()
        .find(|e| e.item_type == item_type && e.id == id)
        .with_context(|| {
            format!("no catalog item with id '{id}' was pulled from a source (nothing recorded in the manifest)")
        })
}

/// Outcome of a `drop` operation (removing an item from the catalog).
#[derive(Debug, Serialize)]
pub struct DropReport {
    pub id: String,
    #[serde(rename = "type")]
    pub item_type: ItemType,
    /// `owner/repo/path` source, when the item had been pulled and recorded in
    /// the manifest; `None` for hand-authored (local) items.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
    #[serde(rename = "ref", skip_serializing_if = "Option::is_none")]
    pub git_ref: Option<String>,
    /// Catalog path that was (or would have been) removed.
    pub path: String,
    /// Whether files were actually removed from disk (false if already absent).
    pub item_removed: bool,
    /// Whether a manifest entry was pruned (false for items not recorded as pulled).
    pub manifest_pruned: bool,
}

/// Remove an item from the catalog, pruning its manifest entry when present.
///
/// Deletes the catalog copy (`skills/<id>/` or `agents/<id>/`) and, if the item was
/// recorded as a pull, removes its manifest entry so `restore` won't bring it back.
/// Works on both pulled and hand-authored (local) items. Errors only when `id` exists
/// neither on disk nor in the manifest.
///
/// This is also how a stale manifest entry for a removed legacy flat agent is cleaned
/// up: nothing is deleted from disk (`item_removed: false`) but the entry is pruned.
pub fn drop_from_catalog(catalog: &Catalog, item_type: ItemType, id: &str) -> Result<DropReport> {
    ensure_simple_id(id)?;
    let entry = manifest::entries(catalog)?
        .into_iter()
        .find(|e| e.item_type == item_type && e.id == id);

    let dst = drop_target(catalog, item_type, id);
    let item_removed = fsops::remove(&dst)?;
    let manifest_pruned = manifest::remove(catalog, item_type, id)?;

    if !item_removed && !manifest_pruned {
        anyhow::bail!(
            "no {} '{id}' in the catalog; nothing to drop",
            type_label(item_type)
        );
    }

    let (source, git_ref) = match entry {
        Some(e) => (Some(e.spec.source()), e.spec.ref_),
        None => (None, None),
    };

    Ok(DropReport {
        id: id.to_string(),
        item_type,
        source,
        git_ref,
        path: dst.display().to_string(),
        item_removed,
        manifest_pruned,
    })
}

/// The catalog path `drop` should remove for `(item_type, id)`: a skill directory or
/// an agent **package** directory. Those are the only two catalog shapes.
fn drop_target(catalog: &Catalog, item_type: ItemType, id: &str) -> PathBuf {
    match item_type {
        ItemType::Skill => catalog.skill_source(id),
        ItemType::Agent => catalog.agent_package_dir(id),
    }
}

fn ensure_simple_id(id: &str) -> Result<()> {
    if id.is_empty() || id == "." || id == ".." || id.contains('/') || id.contains('\\') {
        anyhow::bail!("invalid catalog id '{id}'; expected a single path segment");
    }
    Ok(())
}

fn type_label(item_type: ItemType) -> &'static str {
    match item_type {
        ItemType::Skill => "skill",
        ItemType::Agent => "agent",
    }
}

fn validate_remote_skill(id: &str, src: &std::path::Path) -> Result<()> {
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
    Ok(())
}

/// Resolve where a fetched remote item is stored **in the catalog**, validating
/// its on-disk shape. Skills are always directories; an agent is always a
/// harness-aware **package** directory (`agent.yml` + variants) stored at
/// `agents/<id>/`. A remote that resolves to a legacy flat `.agent.md` file is
/// rejected with a migration hint — it is no longer a catalog shape.
fn catalog_dst_for_source(
    catalog: &Catalog,
    item_type: ItemType,
    id: &str,
    src: &std::path::Path,
) -> Result<PathBuf> {
    match item_type {
        ItemType::Skill => {
            validate_remote_skill(id, src)?;
            Ok(catalog.skill_source(id))
        }
        ItemType::Agent if src.is_dir() => {
            // A directory must be a valid agent package (validates agent.yml + variants).
            crate::agentpkg::AgentPackage::load(id, src)
                .with_context(|| format!("remote agent '{id}' is not a valid agent package"))?;
            Ok(catalog.agent_package_dir(id))
        }
        ItemType::Agent if src.is_file() => anyhow::bail!(
            "remote agent '{id}' resolved to a legacy flat .agent.md file ({}), which is no \
             longer a supported catalog shape — akit needs an agent *package*: a directory \
             `agents/{id}/` holding an `agent.yml` descriptor plus one native variant file \
             per harness",
            src.display()
        ),
        ItemType::Agent => anyhow::bail!(
            "remote agent '{id}' did not resolve to an agent package directory ({}) — an agent \
             must be a directory holding an `agent.yml` descriptor plus one native variant file \
             per harness",
            src.display()
        ),
    }
}

fn remote_id(item_type: ItemType, spec: &SourceSpec) -> String {
    let leaf = spec.leaf();
    match item_type {
        ItemType::Skill => leaf.to_string(),
        // A `.agent.md` leaf no longer resolves to anything installable, but stripping
        // the suffix still yields the id the rejection message should name.
        ItemType::Agent => leaf
            .strip_suffix(crate::catalog::LEGACY_FLAT_SUFFIX)
            .unwrap_or(leaf)
            .to_string(),
    }
}

/// List every skill and agent present in the catalog, with provenance.
///
/// Each item is annotated with its remote `source` when the manifest records it
/// as pulled; hand-authored items have `source: None`. Sorted skills-first, then
/// by id.
pub fn list_catalog(catalog: &Catalog) -> Result<Vec<CatalogItem>> {
    let sources = manifest_sources(catalog)?;
    let mut items = Vec::new();
    scan_catalog_skills(catalog, &sources, &mut items)?;
    scan_catalog_agents(catalog, &sources, &mut items)?;
    items.sort_by(|a, b| {
        type_rank(a.item_type)
            .cmp(&type_rank(b.item_type))
            .then_with(|| a.id.cmp(&b.id))
    });
    Ok(items)
}

fn type_rank(item_type: ItemType) -> u8 {
    match item_type {
        ItemType::Skill => 0,
        ItemType::Agent => 1,
    }
}

/// Map each recorded `(type, id)` to its remote source string (`owner/repo/path[#ref]`).
fn manifest_sources(catalog: &Catalog) -> Result<HashMap<(ItemType, String), String>> {
    let mut sources = HashMap::new();
    for entry in manifest::entries(catalog)? {
        let source = match &entry.spec.ref_ {
            Some(git_ref) => format!("{}#{git_ref}", entry.spec.source()),
            None => entry.spec.source(),
        };
        sources.insert((entry.item_type, entry.id), source);
    }
    Ok(sources)
}

fn scan_catalog_skills(
    catalog: &Catalog,
    sources: &HashMap<(ItemType, String), String>,
    items: &mut Vec<CatalogItem>,
) -> Result<()> {
    let dir = catalog.root.join("skills");
    let entries = match std::fs::read_dir(&dir) {
        Ok(entries) => entries,
        Err(e) if e.kind() == ErrorKind::NotFound => return Ok(()),
        Err(e) => return Err(e).with_context(|| format!("reading {}", dir.display())),
    };
    for entry in entries {
        let entry = entry.with_context(|| format!("reading {}", dir.display()))?;
        let path = entry.path();
        if !path.is_dir() || !path.join("SKILL.md").is_file() {
            continue;
        }
        let id = entry.file_name().to_string_lossy().into_owned();
        items.push(catalog_item(
            ItemType::Skill,
            id,
            &path.join("SKILL.md"),
            sources,
        ));
    }
    Ok(())
}

fn scan_catalog_agents(
    catalog: &Catalog,
    sources: &HashMap<(ItemType, String), String>,
    items: &mut Vec<CatalogItem>,
) -> Result<()> {
    for id in catalog.discover_agents()? {
        items.push(catalog_package_item(catalog, id, sources));
    }
    Ok(())
}

/// Build a catalog row for an agent *package*, surfacing its supported harnesses.
/// An invalid package stays visible but `disabled`, carrying the load error as its
/// description — never silently dropped.
fn catalog_package_item(
    catalog: &Catalog,
    id: String,
    sources: &HashMap<(ItemType, String), String>,
) -> CatalogItem {
    let source = sources.get(&(ItemType::Agent, id.clone())).cloned();
    match catalog.resolve_agent_package(&id) {
        Ok(pkg) => CatalogItem {
            id,
            item_type: ItemType::Agent,
            harnesses: pkg.supported_harnesses().collect(),
            description: pkg.description,
            source,
            disabled: false,
        },
        Err(e) => CatalogItem {
            id,
            item_type: ItemType::Agent,
            description: format!("invalid package: {e}"),
            source,
            harnesses: Vec::new(),
            disabled: true,
        },
    }
}

fn catalog_item(
    item_type: ItemType,
    id: String,
    path: &std::path::Path,
    sources: &HashMap<(ItemType, String), String>,
) -> CatalogItem {
    let description = std::fs::read_to_string(path)
        .ok()
        .and_then(|content| crate::search::parse_frontmatter(path, &content).description)
        .unwrap_or_default();
    let source = sources.get(&(item_type, id.clone())).cloned();
    CatalogItem {
        id,
        item_type,
        description,
        source,
        harnesses: Vec::new(),
        disabled: false,
    }
}
