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
use crate::materialize::{self, MaterializeItem, remove_materialization};
use crate::ownership::{AKIT_LOCKFILE_REL, AkitLockfile, Installation, MaterializationRecord};
use crate::plan::{self, Plan, PlanIssue, PlannedMaterialization};
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

/// Tunables for an install/plan operation. Defaults reproduce the historical
/// behavior, so this is an additive, backward-compatible knob set.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct InstallOptions {
    /// Prefer symlink materialization (`install --symlink`). Honored per
    /// materialization, only where safe — a path is symlinked when every harness
    /// it serves is a confirmed symlink-follower, else it stays a copy. Silently
    /// downgrades to copy on a transport that can't symlink. Skills only; agents
    /// are always copied (no harness is a confirmed agent-file symlink-follower).
    pub force_symlink: bool,
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

/// Outcome of installing a whole catalog bundle (issue #45).
///
/// Each member is installed independently (its own atomic reconcile), in
/// manifest order — skills first, then agents — and tagged with the bundle name
/// in the lockfile. A member that serves no harnesses (all selected skipped)
/// still appears here with an empty `harnesses` and its `issues` populated.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BundleInstallReport {
    /// The bundle name (matches `<catalog>/bundles/<name>.yml`).
    pub bundle: String,
    /// Harnesses selected for the whole bundle install (sorted, deduped).
    pub harnesses: Vec<HarnessId>,
    /// Per-member outcomes, in manifest order.
    pub items: Vec<InstallReport>,
}

/// A read-only preview of what [`install_bundle`] would do (`--dry-run`), and the
/// basis for the partial-install confirmation: aggregated per-member previews.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BundleInstallPreview {
    /// The bundle name (matches `<catalog>/bundles/<name>.yml`).
    pub bundle: String,
    /// Harnesses selected for the whole bundle install (sorted, deduped).
    pub harnesses: Vec<HarnessId>,
    /// Per-member previews, in manifest order.
    pub items: Vec<InstallPreview>,
}

impl BundleInstallPreview {
    /// True when any member can't be served for every selected harness — i.e.
    /// applying would be a *partial* install (the CLI confirms before doing so).
    pub fn is_partial(&self) -> bool {
        self.items.iter().any(|p| !p.issues.is_empty())
    }
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

/// Outcome of uninstalling every member tagged with a bundle (issue #45).
///
/// Driven by the lockfile bundle tag, **not** the current manifest — so a member
/// dropped from `bundles/<name>.yml` after install is still uninstalled here
/// (matching legacy `rm --bundle`). `items` is empty when nothing carries the tag.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BundleRemoveReport {
    /// The bundle name whose tagged members were removed.
    pub bundle: String,
    /// Per-member removal outcomes, in lockfile order.
    pub items: Vec<RemoveReport>,
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

/// [`install`] with explicit [`InstallOptions`] (e.g. `--symlink`), local transport.
pub fn install_opts(
    project: &Project,
    catalog: &Catalog,
    item_type: ItemType,
    id: &str,
    ctx: &HarnessContext,
    opts: InstallOptions,
) -> Result<InstallReport> {
    install_with_opts(&LocalFs, project, catalog, item_type, id, ctx, opts)
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
    install_with_opts(
        fs,
        project,
        catalog,
        item_type,
        id,
        ctx,
        InstallOptions::default(),
    )
}

/// [`install_with`] with explicit [`InstallOptions`].
pub fn install_with_opts(
    fs: &dyn FsTransport,
    project: &Project,
    catalog: &Catalog,
    item_type: ItemType,
    id: &str,
    ctx: &HarnessContext,
    opts: InstallOptions,
) -> Result<InstallReport> {
    let (plan, resolver) = build_plan(catalog, item_type, id, ctx.harnesses(), opts)?;
    reconcile(fs, project, item_type, id, "local", None, &plan, &resolver)
}

/// A read-only preview of what [`install`] would do, without touching the project.
///
/// Diffs the freshly-computed plan against the current `.akit/kit.lock.json`, so
/// the caller can show exactly which materializations would be created, which
/// already exist unchanged, which would be removed (a reshape to a smaller
/// harness set), and which selected harnesses would be skipped.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InstallPreview {
    pub id: String,
    pub item_type: ItemType,
    /// Harnesses selected for this install (sorted, deduped).
    pub harnesses: Vec<HarnessId>,
    /// Planned materializations not currently present for this item.
    pub create: Vec<PlannedMaterialization>,
    /// Planned materializations already owned by this item at the same path.
    pub unchanged: Vec<PlannedMaterialization>,
    /// Paths this item currently owns that the new plan drops (reshape removals).
    pub remove: Vec<String>,
    /// Selected harnesses that could not be served, with reasons.
    pub issues: Vec<PlanIssue>,
    /// Whether an existing installation of this item would be replaced/reshaped.
    pub replaces: bool,
}

/// Compute an [`InstallPreview`] for `install` without applying it (`--dry-run`).
pub fn plan_install(
    project: &Project,
    catalog: &Catalog,
    item_type: ItemType,
    id: &str,
    ctx: &HarnessContext,
) -> Result<InstallPreview> {
    plan_install_with(&LocalFs, project, catalog, item_type, id, ctx)
}

/// [`plan_install`] with explicit [`InstallOptions`] (e.g. `--symlink`), local transport.
pub fn plan_install_opts(
    project: &Project,
    catalog: &Catalog,
    item_type: ItemType,
    id: &str,
    ctx: &HarnessContext,
    opts: InstallOptions,
) -> Result<InstallPreview> {
    plan_install_with_opts(&LocalFs, project, catalog, item_type, id, ctx, opts)
}

/// [`plan_install`] against an explicit transport.
pub fn plan_install_with(
    fs: &dyn FsTransport,
    project: &Project,
    catalog: &Catalog,
    item_type: ItemType,
    id: &str,
    ctx: &HarnessContext,
) -> Result<InstallPreview> {
    plan_install_with_opts(
        fs,
        project,
        catalog,
        item_type,
        id,
        ctx,
        InstallOptions::default(),
    )
}

/// [`plan_install_with`] with explicit [`InstallOptions`].
pub fn plan_install_with_opts(
    fs: &dyn FsTransport,
    project: &Project,
    catalog: &Catalog,
    item_type: ItemType,
    id: &str,
    ctx: &HarnessContext,
    opts: InstallOptions,
) -> Result<InstallPreview> {
    let (plan, _resolver) = build_plan(catalog, item_type, id, ctx.harnesses(), opts)?;

    let lock = AkitLockfile::load_with(fs, &project.akit_lockfile_path())?;
    let existing = lock.get(item_type, id);
    let existing_paths: std::collections::HashSet<&str> = existing
        .map(|e| e.materializations.iter().map(|m| m.path.as_str()).collect())
        .unwrap_or_default();
    let planned_paths: std::collections::HashSet<&str> = plan
        .materializations
        .iter()
        .map(|m| m.path.as_str())
        .collect();

    let mut create = Vec::new();
    let mut unchanged = Vec::new();
    for m in &plan.materializations {
        if existing_paths.contains(m.path.as_str()) {
            unchanged.push(m.clone());
        } else {
            create.push(m.clone());
        }
    }
    let mut remove: Vec<String> = existing_paths
        .iter()
        .filter(|p| !planned_paths.contains(*p))
        .map(|p| p.to_string())
        .collect();
    remove.sort();

    Ok(InstallPreview {
        id: id.to_string(),
        item_type,
        harnesses: ctx.harnesses().to_vec(),
        create,
        unchanged,
        remove,
        issues: plan.issues,
        replaces: existing.is_some(),
    })
}

/// Install every member of a catalog bundle for exactly the harnesses in `ctx`,
/// tagging each installation with the bundle name.
///
/// Members are installed in manifest order (skills, then agents), each as its own
/// atomic reconcile: a member that can't be served for a selected harness is
/// skipped (reported in that member's `issues`) rather than failing the bundle.
/// A missing/invalid manifest, or a member whose catalog source is gone, fails
/// the whole call before any member is installed (validated by [`crate::bundle::load`]).
pub fn install_bundle(
    project: &Project,
    catalog: &Catalog,
    bundle: &str,
    ctx: &HarnessContext,
) -> Result<BundleInstallReport> {
    install_bundle_with(&LocalFs, project, catalog, bundle, ctx)
}

/// [`install_bundle`] with explicit [`InstallOptions`] (e.g. `--symlink`), local transport.
pub fn install_bundle_opts(
    project: &Project,
    catalog: &Catalog,
    bundle: &str,
    ctx: &HarnessContext,
    opts: InstallOptions,
) -> Result<BundleInstallReport> {
    install_bundle_with_opts(&LocalFs, project, catalog, bundle, ctx, opts)
}

/// [`install_bundle`] against an explicit destination transport (for embedding hosts).
pub fn install_bundle_with(
    fs: &dyn FsTransport,
    project: &Project,
    catalog: &Catalog,
    bundle: &str,
    ctx: &HarnessContext,
) -> Result<BundleInstallReport> {
    install_bundle_with_opts(fs, project, catalog, bundle, ctx, InstallOptions::default())
}

/// [`install_bundle_with`] with explicit [`InstallOptions`].
pub fn install_bundle_with_opts(
    fs: &dyn FsTransport,
    project: &Project,
    catalog: &Catalog,
    bundle: &str,
    ctx: &HarnessContext,
    opts: InstallOptions,
) -> Result<BundleInstallReport> {
    let loaded = crate::bundle::load(catalog, bundle)?;
    let mut items = Vec::with_capacity(loaded.items.len());
    for member in &loaded.items {
        let (plan, resolver) =
            build_plan(catalog, member.item_type, &member.id, ctx.harnesses(), opts)?;
        let report = reconcile(
            fs,
            project,
            member.item_type,
            &member.id,
            "local",
            Some(bundle),
            &plan,
            &resolver,
        )?;
        items.push(report);
    }
    Ok(BundleInstallReport {
        bundle: bundle.to_string(),
        harnesses: ctx.harnesses().to_vec(),
        items,
    })
}

/// Compute a [`BundleInstallPreview`] for `install_bundle` without applying it.
///
/// Powers both `--dry-run` and the partial-install confirmation. Validation of
/// the manifest and its members happens up front, so a bad bundle errors here
/// too (never a half-shown plan).
pub fn plan_install_bundle(
    project: &Project,
    catalog: &Catalog,
    bundle: &str,
    ctx: &HarnessContext,
) -> Result<BundleInstallPreview> {
    plan_install_bundle_with(&LocalFs, project, catalog, bundle, ctx)
}

/// [`plan_install_bundle`] with explicit [`InstallOptions`] (e.g. `--symlink`), local transport.
pub fn plan_install_bundle_opts(
    project: &Project,
    catalog: &Catalog,
    bundle: &str,
    ctx: &HarnessContext,
    opts: InstallOptions,
) -> Result<BundleInstallPreview> {
    plan_install_bundle_with_opts(&LocalFs, project, catalog, bundle, ctx, opts)
}

/// [`plan_install_bundle`] against an explicit transport.
pub fn plan_install_bundle_with(
    fs: &dyn FsTransport,
    project: &Project,
    catalog: &Catalog,
    bundle: &str,
    ctx: &HarnessContext,
) -> Result<BundleInstallPreview> {
    plan_install_bundle_with_opts(fs, project, catalog, bundle, ctx, InstallOptions::default())
}

/// [`plan_install_bundle_with`] with explicit [`InstallOptions`].
pub fn plan_install_bundle_with_opts(
    fs: &dyn FsTransport,
    project: &Project,
    catalog: &Catalog,
    bundle: &str,
    ctx: &HarnessContext,
    opts: InstallOptions,
) -> Result<BundleInstallPreview> {
    let loaded = crate::bundle::load(catalog, bundle)?;
    let mut items = Vec::with_capacity(loaded.items.len());
    for member in &loaded.items {
        items.push(plan_install_with_opts(
            fs,
            project,
            catalog,
            member.item_type,
            &member.id,
            ctx,
            opts,
        )?);
    }
    Ok(BundleInstallPreview {
        bundle: bundle.to_string(),
        harnesses: ctx.harnesses().to_vec(),
        items,
    })
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
    let lock = AkitLockfile::load_with(fs, &lf_path)?;
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
        lock.save_with(fs, &lf_path)?;
        sync_excludes(fs, project, &lock)?;
        Ok(RemoveReport {
            id: id.to_string(),
            item_type,
            removed_paths,
            remaining_harnesses: Vec::new(),
            not_installed: false,
        })
    } else {
        // Partial: re-plan for the reduced set and reshape. The reshape uses the
        // default (copy) mode — a scoped uninstall doesn't carry the original
        // `--symlink` intent, so a previously symlinked path may become a copy.
        let cat = Catalog::locate()?;
        let ctx = HarnessContext::new(remaining)?;
        let (plan, resolver) = build_plan(
            &cat,
            item_type,
            id,
            ctx.harnesses(),
            InstallOptions::default(),
        )?;
        // Preserve the bundle tag across a reshape, so a partial uninstall never
        // silently detaches a member from its bundle.
        let report = reconcile(
            fs,
            project,
            item_type,
            id,
            &existing.source,
            existing.bundle.as_deref(),
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

/// Uninstall every installed member tagged with `bundle`, applying `scope` to
/// each (`RemoveScope::All` drops them entirely; `Harnesses` reshapes each to
/// keep the rest). The set of members is read from the lockfile bundle tag, so
/// this needs neither the catalog nor the manifest for a full uninstall.
pub fn remove_bundle(
    project: &Project,
    bundle: &str,
    scope: RemoveScope,
) -> Result<BundleRemoveReport> {
    remove_bundle_with(&LocalFs, project, bundle, scope)
}

/// [`remove_bundle`] against an explicit transport.
pub fn remove_bundle_with(
    fs: &dyn FsTransport,
    project: &Project,
    bundle: &str,
    scope: RemoveScope,
) -> Result<BundleRemoveReport> {
    let lock = AkitLockfile::load_with(fs, &project.akit_lockfile_path())?;
    // Snapshot the tagged members up front: `remove_with` reloads and rewrites
    // the lockfile per item, so we can't iterate `lock.items` while removing.
    let targets: Vec<(ItemType, String)> = lock
        .items
        .iter()
        .filter(|i| i.bundle.as_deref() == Some(bundle))
        .map(|i| (i.item_type, i.id.clone()))
        .collect();
    let mut items = Vec::with_capacity(targets.len());
    for (item_type, id) in targets {
        items.push(remove_with(fs, project, item_type, &id, scope.clone())?);
    }
    Ok(BundleRemoveReport {
        bundle: bundle.to_string(),
        items,
    })
}

/// Remove *every* akit-owned materialization in the project and clear the
/// lockfile. Only files akit recorded are touched.
pub fn reset(project: &Project) -> Result<ResetReport> {
    reset_with(&LocalFs, project)
}

/// [`reset`] against an explicit transport.
pub fn reset_with(fs: &dyn FsTransport, project: &Project) -> Result<ResetReport> {
    let lf_path = project.akit_lockfile_path();
    let mut lock = AkitLockfile::load_with(fs, &lf_path)?;
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
    lock.save_with(fs, &lf_path)?;
    // Empty lockfile → the managed exclude block is removed entirely.
    sync_excludes(fs, project, &lock)?;
    Ok(report)
}

/// Read-only status of every installed item, with per-materialization drift.
pub fn status(project: &Project) -> Result<Vec<Installation>> {
    status_with(&LocalFs, project)
}

/// [`status`] against an explicit transport.
pub fn status_with(fs: &dyn FsTransport, project: &Project) -> Result<Vec<Installation>> {
    let lock = AkitLockfile::load_with(fs, &project.akit_lockfile_path())?;
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
    opts: InstallOptions,
) -> Result<(Plan, SourceResolver)> {
    match item_type {
        ItemType::Skill => {
            let src = catalog.resolve_skill(id)?;
            let compat = catalog.skill_compat(id)?;
            let plan = plan::plan_skill(id, harnesses, &compat, opts.force_symlink);
            let resolver: SourceResolver = Box::new(move |_planned| src.clone());
            Ok((plan, resolver))
        }
        ItemType::Agent => {
            // Agents are always copied — no harness is a confirmed agent-file
            // symlink-follower — so `opts.force_symlink` doesn't apply here.
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
#[allow(clippy::too_many_arguments)]
fn reconcile(
    fs: &dyn FsTransport,
    project: &Project,
    item_type: ItemType,
    id: &str,
    source: &str,
    bundle: Option<&str>,
    plan: &Plan,
    resolver: &SourceResolver,
) -> Result<InstallReport> {
    let lf_path = project.akit_lockfile_path();
    let mut lock = AkitLockfile::load_with(fs, &lf_path)?;
    let previous = lock.get(item_type, id).cloned();

    // Stage + atomically commit every planned materialization as one transaction
    // (all temps validated before any destination is renamed into place).
    let items: Vec<MaterializeItem<'_>> = plan
        .materializations
        .iter()
        .map(|planned| MaterializeItem {
            source: resolver(planned),
            planned,
        })
        .collect();
    // Paths akit already owns (this item's prior install plus every other item):
    // the materialize guard may only overwrite these, an absent path, or a
    // destination whose content already byte-matches the source. Any other
    // pre-existing file is foreign and the whole apply is refused.
    let owned: std::collections::HashSet<String> =
        lock.owned_paths().into_iter().map(str::to_string).collect();
    let records = materialize::materialize_all(fs, &project.root, &items, &owned)
        .with_context(|| format!("installing '{id}'"))?;

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
            lock.save_with(fs, &lf_path)?;
            sync_excludes(fs, project, &lock)?;
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
        bundle: bundle.map(str::to_string),
        harnesses: harnesses.clone(),
        materializations: records.clone(),
    };
    let replaced = lock.upsert(installation);
    lock.save_with(fs, &lf_path)?;
    // Recompute the managed exclude block from the lockfile (adds new lines,
    // prunes ones the reshape dropped, and excludes the lockfile itself).
    sync_excludes(fs, project, &lock)?;

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
pub(crate) fn sync_excludes(
    fs: &dyn FsTransport,
    project: &Project,
    lock: &AkitLockfile,
) -> Result<()> {
    if let Some(excl) = project.git_info_exclude_path() {
        gitexclude::set_managed_lines(fs, &excl, &desired_excludes(lock))?;
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
    use crate::lockfile::Mode;
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
            "description: Reviews code\nvariants:\n  copilot: copilot.agent.md\n  claude: claude.md\n",
        );
    }

    fn write_bundle(catalog: &Catalog, name: &str, manifest: &str) {
        let dir = catalog.root.join("bundles");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join(format!("{name}.yml")), manifest).unwrap();
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
    fn install_refuses_to_overwrite_a_preexisting_foreign_file() {
        let f = setup();
        write_skill(&f.catalog, "deploy", None);
        // A user file already sits where the Claude skill dir would materialize.
        let dest = f.project.root.join(".claude/skills/deploy");
        write(&dest.join("SKILL.md"), "hand-written user content");

        let err = install(
            &f.project,
            &f.catalog,
            ItemType::Skill,
            "deploy",
            &ctx(&[HarnessId::Claude]),
        )
        .unwrap_err();
        let msg = format!("{err:#}");
        assert!(msg.contains(".claude/skills/deploy"), "{msg}");

        // The pre-existing file is byte-for-byte untouched…
        assert_eq!(
            std::fs::read_to_string(dest.join("SKILL.md")).unwrap(),
            "hand-written user content"
        );
        // …and no ownership was recorded (no lockfile entry, none written at all).
        assert!(!f.project.root.join(".akit/kit.lock.json").exists());
        assert!(status(&f.project).unwrap().is_empty());
    }

    #[test]
    fn install_adopts_a_byte_identical_preexisting_file() {
        let f = setup();
        write_skill(&f.catalog, "deploy", None);
        // Pre-place content identical to what the skill materializes.
        let dest = f.project.root.join(".claude/skills/deploy");
        write(&dest.join("SKILL.md"), "---\nname: x\n---\nbody");

        let report = install(
            &f.project,
            &f.catalog,
            ItemType::Skill,
            "deploy",
            &ctx(&[HarnessId::Claude]),
        )
        .unwrap();
        assert_eq!(report.materializations.len(), 1);
        assert!(!report.replaced);
        assert_eq!(
            std::fs::read_to_string(dest.join("SKILL.md")).unwrap(),
            "---\nname: x\n---\nbody"
        );
        assert_eq!(status(&f.project).unwrap().len(), 1);
    }

    #[test]
    fn install_with_one_foreign_destination_fails_atomically() {
        let f = setup();
        write_skill(&f.catalog, "deploy", None);
        // Install for all harnesses would write both `.agents/skills/deploy` and
        // `.claude/skills/deploy`; only the latter is pre-occupied by a foreign file.
        let foreign = f.project.root.join(".claude/skills/deploy");
        write(&foreign.join("SKILL.md"), "foreign");

        let err = install(
            &f.project,
            &f.catalog,
            ItemType::Skill,
            "deploy",
            &ctx(&HarnessId::ALL),
        )
        .unwrap_err();
        assert!(format!("{err:#}").contains(".claude/skills/deploy"));

        // The other destination was never written; the foreign one is intact;
        // nothing recorded.
        assert!(!f.project.root.join(".agents/skills/deploy").exists());
        assert_eq!(
            std::fs::read_to_string(foreign.join("SKILL.md")).unwrap(),
            "foreign"
        );
        assert!(status(&f.project).unwrap().is_empty());
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
    fn install_symlink_symlinks_a_follower_only_install() {
        let f = setup();
        write_skill(&f.catalog, "deploy", None);
        let opts = InstallOptions {
            force_symlink: true,
        };
        // Claude is a confirmed follower → the sole materialization symlinks.
        let report = install_opts(
            &f.project,
            &f.catalog,
            ItemType::Skill,
            "deploy",
            &ctx(&[HarnessId::Claude]),
            opts,
        )
        .unwrap();
        assert_eq!(report.materializations.len(), 1);
        assert_eq!(report.materializations[0].mode, Mode::Symlink);
        // Symlinks carry no content hash.
        assert!(report.materializations[0].hash.is_none());
        // The on-disk entry is an actual symlink into the catalog.
        let dest = f.project.root.join(".claude/skills/deploy");
        assert!(std::fs::symlink_metadata(&dest).unwrap().is_symlink());
        assert_eq!(
            std::fs::read_link(&dest).unwrap(),
            f.catalog.skill_source("deploy")
        );
    }

    #[test]
    fn install_symlink_falls_back_to_copy_when_a_coverer_is_not_a_follower() {
        let f = setup();
        write_skill(&f.catalog, "deploy", None);
        let opts = InstallOptions {
            force_symlink: true,
        };
        // Copilot+Claude collapse onto the shared `.claude/skills` path; Copilot is
        // not a confirmed follower, so the path stays a copy despite --symlink.
        let report = install_opts(
            &f.project,
            &f.catalog,
            ItemType::Skill,
            "deploy",
            &ctx(&[HarnessId::Copilot, HarnessId::Claude]),
            opts,
        )
        .unwrap();
        assert_eq!(report.materializations.len(), 1);
        assert_eq!(report.materializations[0].mode, Mode::Copy);
        assert!(report.materializations[0].hash.is_some());
        assert!(
            !std::fs::symlink_metadata(f.project.root.join(".claude/skills/deploy"))
                .unwrap()
                .is_symlink()
        );
    }

    #[test]
    fn install_symlink_is_a_noop_for_agents() {
        let f = setup();
        write_agent(&f.catalog, "reviewer");
        let opts = InstallOptions {
            force_symlink: true,
        };
        // Agents are always copied — no harness is a confirmed agent-file follower.
        let report = install_opts(
            &f.project,
            &f.catalog,
            ItemType::Agent,
            "reviewer",
            &ctx(&[HarnessId::Claude]),
            opts,
        )
        .unwrap();
        assert!(report.materializations.iter().all(|m| m.mode == Mode::Copy));
    }

    #[test]
    fn install_bundle_installs_all_members_and_tags_lockfile() {
        let f = setup();
        write_skill(&f.catalog, "deploy", None);
        write_agent(&f.catalog, "reviewer");
        write_bundle(&f.catalog, "web", "skills: [deploy]\nagents: [reviewer]\n");

        let report =
            install_bundle(&f.project, &f.catalog, "web", &ctx(&[HarnessId::Claude])).unwrap();

        assert_eq!(report.bundle, "web");
        assert_eq!(report.items.len(), 2);
        assert!(report.items.iter().all(|i| i.issues.is_empty()));
        assert!(
            f.project
                .root
                .join(".claude/skills/deploy/SKILL.md")
                .is_file()
        );
        assert!(f.project.root.join(".claude/agents/reviewer.md").is_file());

        // Both installs are tagged with the bundle name in the lockfile.
        let items = status(&f.project).unwrap();
        assert_eq!(items.len(), 2);
        assert!(items.iter().all(|i| i.bundle.as_deref() == Some("web")));
    }

    #[test]
    fn install_bundle_skips_incompatible_member_but_installs_the_rest() {
        let f = setup();
        // A claude-only skill and a portable one, installed for claude+codex.
        write_skill(&f.catalog, "clauded", Some("harnesses:\n  - claude\n"));
        write_skill(&f.catalog, "portable", None);
        write_bundle(&f.catalog, "mix", "skills: [clauded, portable]\n");

        let report = install_bundle(
            &f.project,
            &f.catalog,
            "mix",
            &ctx(&[HarnessId::Claude, HarnessId::Codex]),
        )
        .unwrap();

        let clauded = report.items.iter().find(|i| i.id == "clauded").unwrap();
        assert_eq!(clauded.harnesses, vec![HarnessId::Claude]);
        assert_eq!(clauded.issues.len(), 1);
        assert_eq!(clauded.issues[0].harness, HarnessId::Codex);

        let portable = report.items.iter().find(|i| i.id == "portable").unwrap();
        assert!(portable.issues.is_empty());
        assert!(portable.harnesses.contains(&HarnessId::Codex));

        // Both members are tagged, and the partial one is still recorded.
        let items = status(&f.project).unwrap();
        assert_eq!(items.len(), 2);
        assert!(items.iter().all(|i| i.bundle.as_deref() == Some("mix")));
    }

    #[test]
    fn plan_install_bundle_flags_partial_without_touching_the_project() {
        let f = setup();
        write_skill(&f.catalog, "clauded", Some("harnesses:\n  - claude\n"));
        write_skill(&f.catalog, "portable", None);
        write_bundle(&f.catalog, "mix", "skills: [clauded, portable]\n");

        let preview = plan_install_bundle(
            &f.project,
            &f.catalog,
            "mix",
            &ctx(&[HarnessId::Claude, HarnessId::Codex]),
        )
        .unwrap();

        assert!(preview.is_partial());
        assert_eq!(preview.items.len(), 2);
        // Dry run: nothing was written and nothing recorded.
        assert!(!f.project.root.join(".claude/skills").exists());
        assert!(status(&f.project).unwrap().is_empty());
    }

    #[test]
    fn plan_install_bundle_is_not_partial_when_all_members_fit() {
        let f = setup();
        write_skill(&f.catalog, "deploy", None);
        write_skill(&f.catalog, "lint", None);
        write_bundle(&f.catalog, "clean", "skills: [deploy, lint]\n");

        let preview =
            plan_install_bundle(&f.project, &f.catalog, "clean", &ctx(&[HarnessId::Claude]))
                .unwrap();
        assert!(!preview.is_partial());
    }

    #[test]
    fn install_bundle_with_missing_member_fails_before_installing_anything() {
        let f = setup();
        write_skill(&f.catalog, "deploy", None);
        write_bundle(&f.catalog, "bad", "skills: [deploy, nope]\n");

        let err =
            install_bundle(&f.project, &f.catalog, "bad", &ctx(&[HarnessId::Claude])).unwrap_err();
        assert!(format!("{err:#}").contains("nope"), "{err:#}");
        // Up-front validation: the valid member was never installed.
        assert!(!f.project.root.join(".claude/skills/deploy").exists());
        assert!(status(&f.project).unwrap().is_empty());
    }

    #[test]
    fn remove_bundle_uninstalls_only_tagged_members() {
        let f = setup();
        write_skill(&f.catalog, "deploy", None);
        write_skill(&f.catalog, "lint", None);
        write_skill(&f.catalog, "solo", None);
        write_bundle(&f.catalog, "web", "skills: [deploy, lint]\n");
        install_bundle(&f.project, &f.catalog, "web", &ctx(&[HarnessId::Claude])).unwrap();
        // A standalone install (no bundle tag) must survive the bundle uninstall.
        install(
            &f.project,
            &f.catalog,
            ItemType::Skill,
            "solo",
            &ctx(&[HarnessId::Claude]),
        )
        .unwrap();

        let report = remove_bundle(&f.project, "web", RemoveScope::All).unwrap();
        assert_eq!(report.bundle, "web");
        assert_eq!(report.items.len(), 2);
        assert!(report.items.iter().all(|i| !i.not_installed));

        assert!(!f.project.root.join(".claude/skills/deploy").exists());
        assert!(!f.project.root.join(".claude/skills/lint").exists());
        assert!(f.project.root.join(".claude/skills/solo").exists());

        let items = status(&f.project).unwrap();
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].id, "solo");
    }

    #[test]
    fn remove_bundle_with_no_tagged_members_is_empty() {
        let f = setup();
        write_skill(&f.catalog, "solo", None);
        install(
            &f.project,
            &f.catalog,
            ItemType::Skill,
            "solo",
            &ctx(&[HarnessId::Claude]),
        )
        .unwrap();

        let report = remove_bundle(&f.project, "ghost", RemoveScope::All).unwrap();
        assert!(report.items.is_empty());
        // The untagged install is untouched.
        assert_eq!(status(&f.project).unwrap().len(), 1);
    }

    #[test]
    fn remove_bundle_scoped_reshapes_members_and_keeps_the_tag() {
        let f = setup();
        write_skill(&f.catalog, "deploy", None);
        write_bundle(&f.catalog, "web", "skills: [deploy]\n");
        // Point the process catalog env at our temp catalog so the scoped-remove
        // re-plan (which relocates via Catalog::locate) resolves the source.
        unsafe { std::env::set_var(crate::catalog::ENV_CATALOG_DIR, &f.catalog.root) };
        install_bundle(
            &f.project,
            &f.catalog,
            "web",
            &ctx(&[HarnessId::Copilot, HarnessId::Claude]),
        )
        .unwrap();

        let report = remove_bundle(
            &f.project,
            "web",
            RemoveScope::Harnesses(vec![HarnessId::Claude]),
        )
        .unwrap();
        unsafe { std::env::remove_var(crate::catalog::ENV_CATALOG_DIR) };

        assert_eq!(report.items.len(), 1);
        assert_eq!(
            report.items[0].remaining_harnesses,
            vec![HarnessId::Copilot]
        );
        // Member survives (reshaped) and keeps its bundle tag.
        let items = status(&f.project).unwrap();
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].bundle.as_deref(), Some("web"));
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

    /// A transport that redirects every path under `from` to a sibling `to`
    /// directory, delegating all real I/O to [`LocalFs`]. Paths outside `from`
    /// (e.g. the catalog source) pass through untouched. Used to prove that
    /// *all* engine I/O — materializations, the `.akit` lockfile, and the
    /// managed git-exclude block — flows through the transport rather than
    /// `std::fs`, which is the guarantee remote (SFTP) hosts depend on.
    struct RedirectFs {
        from: PathBuf,
        to: PathBuf,
    }

    impl RedirectFs {
        fn map(&self, p: &Path) -> PathBuf {
            match p.strip_prefix(&self.from) {
                Ok(rest) => self.to.join(rest),
                Err(_) => p.to_path_buf(),
            }
        }
    }

    impl FsTransport for RedirectFs {
        fn exists(&self, path: &Path) -> Result<bool> {
            LocalFs.exists(&self.map(path))
        }
        fn symlink_kind(&self, path: &Path) -> Result<Option<crate::transport::FileKind>> {
            LocalFs.symlink_kind(&self.map(path))
        }
        fn read(&self, path: &Path) -> Result<Vec<u8>> {
            LocalFs.read(&self.map(path))
        }
        fn read_dir(&self, dir: &Path) -> Result<Vec<String>> {
            LocalFs.read_dir(&self.map(dir))
        }
        fn create_dir_all(&self, path: &Path) -> Result<()> {
            LocalFs.create_dir_all(&self.map(path))
        }
        fn write(&self, path: &Path, bytes: &[u8]) -> Result<()> {
            LocalFs.write(&self.map(path), bytes)
        }
        fn remove_file(&self, path: &Path) -> Result<()> {
            LocalFs.remove_file(&self.map(path))
        }
        fn remove_dir_all(&self, path: &Path) -> Result<()> {
            LocalFs.remove_dir_all(&self.map(path))
        }
        fn rename(&self, from: &Path, to: &Path) -> Result<()> {
            LocalFs.rename(&self.map(from), &self.map(to))
        }
        fn dir_is_empty(&self, path: &Path) -> Result<bool> {
            LocalFs.dir_is_empty(&self.map(path))
        }
        fn symlink(&self, target: &Path, link: &Path) -> Result<()> {
            // Targets are content sources outside `from`; only the link is remapped.
            LocalFs.symlink(target, &self.map(link))
        }
        fn supports_symlink(&self) -> bool {
            true
        }
    }

    #[test]
    fn engine_io_flows_through_the_transport_not_std_fs() {
        let f = setup();
        write_skill(&f.catalog, "deploy", None);
        // Redirect the project root to a separate backing dir; the project's own
        // root stays empty if (and only if) every write went through the transport.
        let backing = f._tmp.path().join("backing");
        let fs = RedirectFs {
            from: f.project.root.clone(),
            to: backing.clone(),
        };

        let report = install_with(
            &fs,
            &f.project,
            &f.catalog,
            ItemType::Skill,
            "deploy",
            &ctx(&HarnessId::ALL),
        )
        .unwrap();
        assert_eq!(report.materializations.len(), 2);

        // Everything landed in the backing dir (via the transport)…
        assert!(backing.join(".agents/skills/deploy/SKILL.md").is_file());
        assert!(backing.join(".akit/kit.lock.json").is_file());
        let excl = std::fs::read_to_string(backing.join(".git/info/exclude")).unwrap();
        assert!(excl.contains("/.akit/kit.lock.json"), "{excl}");
        assert!(excl.contains("/.agents/skills/deploy"), "{excl}");

        // …and nothing was written to the real project root via std::fs.
        assert!(!f.project.root.join(".agents").exists());
        assert!(!f.project.root.join(".akit").exists());
        assert!(!f.project.root.join(".git/info/exclude").exists());

        // Health, status, and reset all read/write through the transport too.
        let health = crate::reconcile::health_with(&fs, &f.project, &f.catalog).unwrap();
        assert!(health.healthy, "{health:?}");
        assert_eq!(status_with(&fs, &f.project).unwrap().len(), 1);

        reset_with(&fs, &f.project).unwrap();
        assert!(!backing.join(".agents/skills/deploy").exists());
        assert!(status_with(&fs, &f.project).unwrap().is_empty());
        // The managed exclude block is gone once nothing is owned.
        let excl = std::fs::read_to_string(backing.join(".git/info/exclude")).unwrap();
        assert!(!excl.contains("/.akit/kit.lock.json"), "{excl}");
    }
}
