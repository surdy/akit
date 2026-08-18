//! akit CLI — a thin wrapper over the `akit` engine.

use anyhow::{Result, bail};
use clap::{Parser, Subcommand};
use std::path::PathBuf;

use akit::catalog::Catalog;
use akit::config::LocalConfig;
use akit::harness::{HarnessId, Primitive};
use akit::install::{self, HarnessContext, RemoveScope};
use akit::lockfile::{ItemType, Mode};
use akit::ops;
use akit::ops::{BundleHealth, BundleState, CatalogItem};
use akit::project::Project;
use akit::remote::{self, SourceSpec};
use akit::search::{self, SearchHit};
use akit::show;

#[derive(Parser)]
#[command(
    name = "akit",
    version,
    about = "akit (agent kit) — on-demand personal agent customizations"
)]
struct Cli {
    /// Project directory (defaults to the enclosing git repo root, else the current dir).
    #[arg(long, global = true)]
    project: Option<PathBuf>,

    /// Emit machine-readable JSON.
    #[arg(long, global = true)]
    json: bool,

    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// List every skill and agent in your catalog.
    #[command(alias = "catalog")]
    Ls,
    /// Harness-aware project overview: installed items, health, and bundle completeness (.akit).
    Status,
    /// Re-materialize missing akit-owned files and resync git-excludes (.akit; same as `repair`).
    Sync,
    /// Read-only harness-aware diagnosis: item drift, bundle completeness, exclude drift (.akit).
    Doctor,
    /// Search your catalog by skill/agent frontmatter.
    Search {
        /// Query to fuzzy-match against name and description (empty lists everything).
        query: Option<String>,
    },
    /// Print a read-only preview of a catalog item (frontmatter + content).
    Show {
        /// Show an agent package (`agents/<id>/`) instead of a skill.
        #[arg(long)]
        agent: bool,
        /// Item id: a skill directory name, or an agent file stem.
        id: String,
    },
    /// Fetch a remote owner/repo/path[#ref] source into your local catalog.
    Pull {
        /// Pull an agent package (`agents/<id>/`) instead of a skill.
        #[arg(long)]
        agent: bool,
        /// Store under this id instead of the source's last path segment.
        #[arg(long = "as")]
        as_id: Option<String>,
        /// Overwrite an existing catalog item that differs from the source.
        #[arg(long)]
        force: bool,
        /// Remote source: owner/repo/path[#ref].
        source: String,
    },
    /// Re-fetch every remote item recorded in the catalog manifest (akit.yml).
    Restore {
        /// Overwrite catalog items that differ from their recorded source.
        #[arg(long)]
        force: bool,
        /// Follow each item's symbolic ref to its latest commit instead of the
        /// recorded one, and rewrite the recorded commit.
        #[arg(long)]
        latest: bool,
    },
    /// Update pulled catalog items to the latest upstream commit of their recorded ref.
    Update {
        /// Update an agent package instead of a skill (only meaningful with `id`).
        #[arg(long)]
        agent: bool,
        /// Report what would change without writing anything.
        #[arg(long)]
        check: bool,
        /// After refreshing the catalog, re-sync copy-mode installs of the updated
        /// items in every project in the global install index.
        #[arg(long)]
        propagate: bool,
        /// Roll back (or forward-pin) `<id>` to this exact commit of its recorded ref.
        #[arg(long, value_name = "SHA")]
        to: Option<String>,
        /// Catalog id to update; omit to update every pulled item.
        id: Option<String>,
    },
    /// Show the upstream commit history of a pulled catalog item (newest first).
    Log {
        /// Inspect an agent package instead of a skill.
        #[arg(long)]
        agent: bool,
        /// Catalog id of the pulled item.
        id: String,
    },
    /// Remove a skill or agent from the catalog (prunes its manifest entry if it was pulled).
    Drop {
        /// Drop an agent package instead of a skill.
        #[arg(long)]
        agent: bool,
        /// Catalog id to drop.
        id: String,
    },
    /// Install a skill or agent for one or more agent harnesses (harness-aware).
    ///
    /// Files land in each harness's own discovery paths, sharing a path across
    /// harnesses when that path is discoverable by all of them. Re-running with a
    /// different `--harness` set reshapes the install to exactly that set.
    ///
    /// `<id>` may be a local catalog id, or a remote `owner/repo/path[#ref]` — a
    /// remote source is pulled into your catalog first, then installed.
    Install {
        /// Install an agent instead of a skill.
        #[arg(long)]
        agent: bool,
        /// Target harness (repeatable). Overrides `AKIT_HARNESSES` and `.akit/config.json`.
        #[arg(long = "harness", short = 'H', value_name = "ID")]
        harnesses: Vec<String>,
        /// Preview the plan without changing anything.
        #[arg(long)]
        dry_run: bool,
        /// Install every item listed by `bundles/<name>.yml` (harness-aware).
        #[arg(long)]
        bundle: Option<String>,
        /// Skip the partial-install confirmation prompt.
        #[arg(long)]
        yes: bool,
        /// Re-pull a remote `<id>` whose catalog copy differs from the source.
        #[arg(long)]
        force: bool,
        /// Symlink skills to the catalog instead of copying, where every served
        /// harness is a confirmed symlink-follower (else that path stays a copy).
        #[arg(long)]
        symlink: bool,
        /// Catalog id, or a remote owner/repo/path[#ref] to pull then install (omit with --bundle).
        id: Option<String>,
    },
    /// Uninstall a harness-aware install from some or all harnesses.
    ///
    /// Locally modified copies are never deleted silently: the removal plan is
    /// shown and confirmed first (`--yes` skips the prompt, and is required
    /// non-interactively).
    Uninstall {
        /// Uninstall an agent instead of a skill.
        #[arg(long)]
        agent: bool,
        /// Only remove from these harnesses (repeatable); omit to fully uninstall.
        #[arg(long = "harness", short = 'H', value_name = "ID")]
        harnesses: Vec<String>,
        /// Preview the removal plan without changing anything.
        #[arg(long)]
        dry_run: bool,
        /// Uninstall every installed item tagged with this bundle.
        #[arg(long)]
        bundle: Option<String>,
        /// Remove locally modified copies without confirming.
        #[arg(long)]
        yes: bool,
        /// Catalog id of the skill/agent to uninstall (omit with --bundle).
        id: Option<String>,
    },
    /// List harness-aware installs recorded in `.akit/kit.lock.json`.
    Installed,
    /// Show every known project where a catalog item is installed (global index).
    ///
    /// Driven by the global install index, so it works from any directory. Only
    /// projects akit has installed into are known; a project it never touched (or
    /// one whose index entry was deleted) simply does not appear.
    Where {
        /// Look for an agent instead of a skill.
        #[arg(long)]
        agent: bool,
        /// Catalog id to look for.
        id: String,
    },
    /// Remove every akit-owned file and clear the harness-aware lockfile.
    Reset {
        /// Skip the confirmation prompt.
        #[arg(long)]
        yes: bool,
    },
    /// Probe + statically verify harness support on this host (no model/LLM).
    Verify,
    /// Re-materialize missing akit-owned files and prune stale git-exclude lines.
    ///
    /// Operates only on paths recorded in `.akit/kit.lock.json`. Locally modified
    /// copies are conflicts and are never overwritten; items whose catalog source
    /// is gone are reported, not touched.
    Repair,
    /// Stop managing an install but keep its files on disk (make them git-visible).
    Detach {
        /// Detach an agent instead of a skill.
        #[arg(long)]
        agent: bool,
        /// Catalog id of the installed skill/agent to detach.
        id: String,
    },
    /// Drop an orphaned ownership record whose files are already gone.
    Forget {
        /// Forget an agent instead of a skill.
        #[arg(long)]
        agent: bool,
        /// Catalog id of the ownership record to forget.
        id: String,
    },
    /// Claim already-present, exact-content files as akit-owned (lost-lockfile recovery).
    Adopt {
        /// Adopt an agent instead of a skill.
        #[arg(long)]
        agent: bool,
        /// Target harness (repeatable). Overrides `AKIT_HARNESSES` and `.akit/config.json`.
        #[arg(long = "harness", short = 'H', value_name = "ID")]
        harnesses: Vec<String>,
        /// Catalog id of the skill/agent to adopt.
        id: String,
    },
}

fn main() {
    if let Err(e) = run() {
        eprintln!("error: {e:#}");
        std::process::exit(1);
    }
}

fn run() -> Result<()> {
    let cli = Cli::parse();

    match &cli.command {
        Commands::Ls => {
            let catalog = Catalog::locate()?;
            let items = ops::list_catalog(&catalog)?;
            if cli.json {
                println!("{}", serde_json::to_string(&items)?);
            } else {
                print_catalog_table(&items);
            }
        }
        Commands::Status => {
            let project = Project::locate(cli.project.clone())?;
            let catalog = Catalog::locate()?;
            // Harness-aware project overview over `.akit/kit.lock.json`: per-item
            // health (via reconcile) plus per-bundle completeness.
            let health = akit::reconcile::health(&project, &catalog)?;
            let bundles = akit::reconcile::akit_bundle_health(&project, &catalog)?;
            if cli.json {
                println!(
                    "{}",
                    serde_json::to_string(&StatusReport {
                        items: &health.items,
                        bundles: &bundles,
                    })?
                );
            } else {
                print_status_table(&health.items);
                print_bundle_health(&bundles);
            }
        }
        Commands::Sync => {
            let project = Project::locate(cli.project.clone())?;
            let catalog = Catalog::locate()?;
            // Harness-aware repair over `.akit`: re-materialize missing owned files
            // and resync the managed git-exclude block. Equivalent to `repair`.
            let report = akit::reconcile::repair(&project, &catalog)?;
            if cli.json {
                println!("{}", serde_json::to_string(&report)?);
            } else {
                print_repair_report(&report);
            }
        }
        Commands::Doctor => {
            let project = Project::locate(cli.project.clone())?;
            let catalog = Catalog::locate()?;
            // Read-only diagnosis over the harness-aware `.akit` lockfile.
            let report = akit::reconcile::diagnose(&project, &catalog)?;
            if cli.json {
                println!("{}", serde_json::to_string(&report)?);
            } else {
                print_diagnosis(&report);
            }
        }
        Commands::Search { query } => {
            let catalog = Catalog::locate()?;
            let hits = search::search(&catalog, query.as_deref().unwrap_or_default())?;
            if cli.json {
                println!("{}", serde_json::to_string(&hits)?);
            } else {
                print_search_hits(&hits);
            }
        }
        Commands::Show { agent, id } => {
            let catalog = Catalog::locate()?;
            let preview = show::show(&catalog, id, item_type(*agent))?;
            if cli.json {
                println!("{}", serde_json::to_string(&preview)?);
            } else {
                print_item_preview(&preview);
            }
        }
        Commands::Pull {
            agent,
            as_id,
            force,
            source,
        } => {
            let Some(spec) = SourceSpec::parse(source) else {
                bail!("invalid remote source spec '{source}'; expected owner/repo/path[#ref]")
            };
            let catalog = Catalog::locate()?;
            let report = ops::pull_into_catalog(
                &catalog,
                &spec,
                item_type(*agent),
                as_id.as_deref(),
                &remote_base_url(),
                *force,
            )?;
            if cli.json {
                println!("{}", serde_json::to_string(&report)?);
            } else {
                println!("{}", pull_report_line(&report));
            }
        }
        Commands::Restore { force, latest } => {
            let catalog = Catalog::locate()?;
            let report = ops::restore_catalog(&catalog, &remote_base_url(), *force, *latest)?;
            if cli.json {
                println!("{}", serde_json::to_string(&report)?);
            } else {
                print_restore_report(&report);
            }
            if report.summary.errors > 0 {
                std::process::exit(1);
            }
        }
        Commands::Update {
            agent,
            check,
            propagate,
            to,
            id,
        } => {
            let catalog = Catalog::locate()?;
            if *check && *propagate {
                bail!("update --propagate cannot be combined with --check (nothing was refreshed)");
            }
            let mut report = if let Some(to) = to.as_deref() {
                if *check {
                    bail!("update --to <sha> cannot be combined with --check");
                }
                let Some(id) = id.as_deref() else {
                    bail!("update --to <sha> requires an <id> to roll back");
                };
                ops::rollback_catalog(&catalog, item_type(*agent), id, to, &remote_base_url())?
            } else {
                let only = id.as_deref().map(|id| (item_type(*agent), id));
                ops::update_catalog(&catalog, only, &remote_base_url(), *check)?
            };
            if *propagate {
                // Every item the refresh actually resolved is a candidate; the
                // per-materialization content check decides what really needs
                // re-materializing, so an install left stale by an earlier update
                // is caught too. Items that errored are excluded.
                let targets: Vec<(ItemType, String)> = report
                    .items
                    .iter()
                    .filter(|i| !matches!(i.status, ops::UpdateStatus::Error))
                    .map(|i| (i.item_type, i.id.clone()))
                    .collect();
                report.propagation = Some(akit::index::propagate(&catalog, &targets)?);
            }
            if cli.json {
                println!("{}", serde_json::to_string(&report)?);
            } else {
                print_update_report(&report, *check);
                if let Some(propagation) = &report.propagation {
                    print_propagation_report(propagation);
                }
            }
            if report.summary.errors > 0 {
                std::process::exit(1);
            }
        }
        Commands::Log { agent, id } => {
            let catalog = Catalog::locate()?;
            let entries = ops::log_history(&catalog, item_type(*agent), id, &remote_base_url())?;
            if cli.json {
                println!("{}", serde_json::to_string(&entries)?);
            } else {
                print_log(&entries);
            }
        }
        Commands::Drop { agent, id } => {
            let catalog = Catalog::locate()?;
            let report = ops::drop_from_catalog(&catalog, item_type(*agent), id)?;
            if cli.json {
                println!("{}", serde_json::to_string(&report)?);
            } else {
                println!("{}", drop_report_line(&report));
            }
        }
        Commands::Install {
            agent,
            harnesses,
            dry_run,
            bundle,
            yes,
            force,
            symlink,
            id,
        } => {
            let project = Project::locate(cli.project.clone())?;
            let catalog = Catalog::locate()?;
            let ctx = resolve_install_harnesses(harnesses, &project)?;
            let opts = install::InstallOptions {
                force_symlink: *symlink,
            };
            match (bundle.as_deref(), id.as_deref()) {
                (Some(_), Some(_)) => {
                    bail!("install accepts either <id> or --bundle <name>, not both")
                }
                (None, None) => bail!("install requires <id> or --bundle <name>"),
                (Some(bundle), None) => {
                    if *agent {
                        bail!("install --bundle cannot be combined with --agent");
                    }
                    let preview =
                        install::plan_install_bundle_opts(&project, &catalog, bundle, &ctx, opts)?;
                    if *dry_run {
                        if cli.json {
                            println!("{}", serde_json::to_string(&preview)?);
                        } else {
                            print_bundle_install_preview(&preview);
                            println!(
                                "(dry run — nothing changed; re-run without --dry-run to apply)"
                            );
                        }
                        return Ok(());
                    }
                    // Partial install (some member can't be served for every selected
                    // harness): show the plan and confirm before applying.
                    if !*yes && !cli.json && preview.is_partial() {
                        print_bundle_install_preview(&preview);
                        if !confirm_partial_install()? {
                            println!("Aborted.");
                            return Ok(());
                        }
                    }
                    let report =
                        install::install_bundle_opts(&project, &catalog, bundle, &ctx, opts)?;
                    record_in_index(&project);
                    if cli.json {
                        println!("{}", serde_json::to_string(&report)?);
                    } else {
                        print_bundle_install_report(&project, &report);
                        if *symlink {
                            for item in &report.items {
                                print_symlink_notes(item);
                            }
                        }
                    }
                }
                (None, Some(id)) => {
                    if let Some(spec) = SourceSpec::parse(id) {
                        // Remote source: pull into the catalog first, then install by
                        // the resulting catalog id. A preview would require fetching,
                        // so `--dry-run` is refused here rather than pulling silently.
                        if *dry_run {
                            bail!(
                                "install --dry-run can't preview a remote source; pull it first, \
                                 then `install --dry-run <id>`"
                            );
                        }
                        let pulled = ops::pull_into_catalog(
                            &catalog,
                            &spec,
                            item_type(*agent),
                            None,
                            &remote_base_url(),
                            *force,
                        )?;
                        let report = install::install_opts(
                            &project,
                            &catalog,
                            pulled.item_type,
                            &pulled.id,
                            &ctx,
                            opts,
                        )?;
                        record_in_index(&project);
                        if cli.json {
                            // Monomorphic with a local install: emit the InstallReport.
                            // Pull provenance is recorded in the catalog manifest.
                            println!("{}", serde_json::to_string(&report)?);
                        } else {
                            println!("{}", pull_report_line(&pulled));
                            print_install_report(&project, &report);
                            if *symlink {
                                print_symlink_notes(&report);
                            }
                        }
                    } else if id.contains('/') {
                        bail!("invalid remote source spec '{id}'; expected owner/repo/path[#ref]");
                    } else if *dry_run {
                        let preview = install::plan_install_opts(
                            &project,
                            &catalog,
                            item_type(*agent),
                            id,
                            &ctx,
                            opts,
                        )?;
                        if cli.json {
                            println!("{}", serde_json::to_string(&preview)?);
                        } else {
                            print_install_preview(&preview);
                        }
                    } else {
                        let report = install::install_opts(
                            &project,
                            &catalog,
                            item_type(*agent),
                            id,
                            &ctx,
                            opts,
                        )?;
                        record_in_index(&project);
                        if cli.json {
                            println!("{}", serde_json::to_string(&report)?);
                        } else {
                            print_install_report(&project, &report);
                            if *symlink {
                                print_symlink_notes(&report);
                            }
                        }
                    }
                }
            }
        }
        Commands::Uninstall {
            agent,
            harnesses,
            dry_run,
            bundle,
            yes,
            id,
        } => {
            let project = Project::locate(cli.project.clone())?;
            let scope = if harnesses.is_empty() {
                RemoveScope::All
            } else {
                RemoveScope::Harnesses(parse_harnesses(harnesses)?)
            };
            // The plan is computed for a dry run, and for the drift gate — `--yes`
            // on a real uninstall waives both, so the hashing is skipped entirely.
            let need_plan = *dry_run || !*yes;
            match (bundle.as_deref(), id.as_deref()) {
                (Some(_), Some(_)) => {
                    bail!("uninstall accepts either <id> or --bundle <name>, not both")
                }
                (None, None) => bail!("uninstall requires <id> or --bundle <name>"),
                (Some(bundle), None) => {
                    if *agent {
                        bail!("uninstall --bundle cannot be combined with --agent");
                    }
                    if need_plan {
                        let preview = install::plan_remove_bundle(&project, bundle, scope.clone())?;
                        // One aggregate gate for the whole bundle: declining
                        // removes no member at all.
                        let gate = DriftGate {
                            drifted: preview.drifted(),
                            drifted_items: preview
                                .items
                                .iter()
                                .filter(|i| i.drifted() > 0)
                                .map(detach_target)
                                .collect(),
                            applies: !preview.items.is_empty(),
                        };
                        if !uninstall_gate(
                            cli.json,
                            *dry_run,
                            &preview,
                            gate,
                            print_bundle_uninstall_preview,
                        )? {
                            return Ok(());
                        }
                    }
                    let report = install::remove_bundle(&project, bundle, scope)?;
                    if cli.json {
                        println!("{}", serde_json::to_string(&report)?);
                    } else {
                        print_bundle_uninstall_report(&report);
                    }
                }
                (None, Some(id)) => {
                    if need_plan {
                        let preview =
                            install::plan_remove(&project, item_type(*agent), id, scope.clone())?;
                        let drifted = preview.drifted();
                        let gate = DriftGate {
                            drifted,
                            drifted_items: if drifted > 0 {
                                vec![detach_target(&preview)]
                            } else {
                                Vec::new()
                            },
                            applies: !preview.not_installed,
                        };
                        if !uninstall_gate(
                            cli.json,
                            *dry_run,
                            &preview,
                            gate,
                            print_uninstall_preview,
                        )? {
                            return Ok(());
                        }
                    }
                    let report = install::remove(&project, item_type(*agent), id, scope)?;
                    if cli.json {
                        println!("{}", serde_json::to_string(&report)?);
                    } else {
                        print_uninstall_report(&report);
                    }
                }
            }
        }
        Commands::Installed => {
            let project = Project::locate(cli.project.clone())?;
            // The catalog is needed to tell whether each install's source still
            // exists; health also reports per-materialization drift and degraded
            // (uncovered) harnesses.
            let catalog = Catalog::locate()?;
            let report = akit::reconcile::health(&project, &catalog)?;
            if cli.json {
                println!("{}", serde_json::to_string(&report)?);
            } else {
                print_installed_health(&report);
            }
        }
        Commands::Where { agent, id } => {
            // Index-driven: no project is located, so this works from any cwd.
            let catalog = Catalog::locate()?;
            let report = akit::index::locate(&catalog, item_type(*agent), id)?;
            if cli.json {
                println!("{}", serde_json::to_string(&report)?);
            } else {
                print_where_report(&report);
            }
        }
        Commands::Reset { yes } => {
            let project = Project::locate(cli.project.clone())?;
            let lock = akit::ownership::AkitLockfile::load(&project.akit_lockfile_path())?;
            let owned: usize = lock.items.iter().map(|i| i.materializations.len()).sum();
            if lock.items.is_empty() {
                if cli.json {
                    println!(
                        "{}",
                        serde_json::to_string(&install::ResetReport::default())?
                    );
                } else {
                    println!("Nothing to reset — no akit-owned files recorded.");
                }
                return Ok(());
            }
            if !yes && !cli.json {
                print_reset_preview(&lock.items);
                if !confirm_reset(lock.items.len(), owned)? {
                    println!("Aborted.");
                    return Ok(());
                }
            }
            let report = install::reset(&project)?;
            if cli.json {
                println!("{}", serde_json::to_string(&report)?);
            } else {
                println!(
                    "Reset complete — removed {} file(s) across {} install(s).",
                    report.removed_paths.len(),
                    report.cleared_items
                );
            }
        }
        Commands::Verify => {
            let report = akit::verify::verify_all(&akit::exec::LocalRunner, "local")?;
            if cli.json {
                println!("{}", serde_json::to_string(&report)?);
            } else {
                for v in &report {
                    let mark = if v.verified { "✓" } else { "✗" };
                    println!("{mark} {}", v.detail);
                }
            }
        }
        Commands::Repair => {
            let project = Project::locate(cli.project.clone())?;
            let catalog = Catalog::locate()?;
            let report = akit::reconcile::repair(&project, &catalog)?;
            if cli.json {
                println!("{}", serde_json::to_string(&report)?);
            } else {
                print_repair_report(&report);
            }
        }
        Commands::Detach { agent, id } => {
            let project = Project::locate(cli.project.clone())?;
            let report = akit::reconcile::detach(&project, item_type(*agent), id)?;
            if cli.json {
                println!("{}", serde_json::to_string(&report)?);
            } else {
                print_detach_report(&report, DropKind::Detach);
            }
        }
        Commands::Forget { agent, id } => {
            let project = Project::locate(cli.project.clone())?;
            let report = akit::reconcile::forget(&project, item_type(*agent), id)?;
            if cli.json {
                println!("{}", serde_json::to_string(&report)?);
            } else {
                print_detach_report(&report, DropKind::Forget);
            }
        }
        Commands::Adopt {
            agent,
            harnesses,
            id,
        } => {
            let project = Project::locate(cli.project.clone())?;
            let catalog = Catalog::locate()?;
            let ctx = resolve_install_harnesses(harnesses, &project)?;
            let report = akit::reconcile::adopt(&project, &catalog, item_type(*agent), id, &ctx)?;
            if cli.json {
                println!("{}", serde_json::to_string(&report)?);
            } else {
                print_adopt_report(&report);
            }
        }
    }
    Ok(())
}

fn item_type(agent: bool) -> ItemType {
    if agent {
        ItemType::Agent
    } else {
        ItemType::Skill
    }
}

/// Env var holding a comma/space separated default harness list.
const ENV_HARNESSES: &str = "AKIT_HARNESSES";

/// Parse a list of `--harness` tokens into deduped [`HarnessId`]s.
fn parse_harnesses(tokens: &[String]) -> Result<Vec<HarnessId>> {
    let mut out = Vec::new();
    for tok in tokens {
        for part in split_harness_list(tok) {
            let id: HarnessId = part
                .parse()
                .map_err(|e: akit::harness::UnknownHarness| anyhow::anyhow!("{e}"))?;
            if !out.contains(&id) {
                out.push(id);
            }
        }
    }
    Ok(out)
}

/// Split a token on commas/whitespace, dropping empties.
fn split_harness_list(s: &str) -> Vec<String> {
    s.split([',', ' ', '\t', '\n'])
        .map(str::trim)
        .filter(|p| !p.is_empty())
        .map(str::to_string)
        .collect()
}

/// Resolve the target harness set for an install: `--harness` flags, else the
/// `AKIT_HARNESSES` env var, else `.akit/config.json`, else an interactive prompt.
fn resolve_install_harnesses(flags: &[String], project: &Project) -> Result<HarnessContext> {
    if !flags.is_empty() {
        return HarnessContext::new(parse_harnesses(flags)?);
    }
    if let Ok(value) = std::env::var(ENV_HARNESSES) {
        let toks = split_harness_list(&value);
        if !toks.is_empty() {
            return HarnessContext::new(parse_harnesses(&toks)?);
        }
    }
    let cfg = LocalConfig::load(&project.akit_config_path())?;
    let defaults = cfg.default_harnesses();
    if !defaults.is_empty() {
        return HarnessContext::new(defaults);
    }
    prompt_for_harnesses()
}

/// Interactively pick target harnesses. Errors (with guidance) when stdin is not
/// a terminal, so scripts get an actionable message instead of hanging.
fn prompt_for_harnesses() -> Result<HarnessContext> {
    use std::io::{IsTerminal, Write};
    if !std::io::stdin().is_terminal() {
        bail!(
            "no target harness specified; pass --harness <id> (repeatable), set {ENV_HARNESSES}, \
             or add \"harnesses\" to .akit/config.json"
        );
    }
    println!("Select target harness(es) for this install:");
    for (i, h) in HarnessId::ALL.iter().enumerate() {
        println!("  {}) {} ({})", i + 1, h.as_str(), h.label());
    }
    print!("Enter numbers or names (comma/space separated): ");
    std::io::stdout().flush().ok();
    let mut line = String::new();
    std::io::stdin().read_line(&mut line)?;
    let mut chosen: Vec<HarnessId> = Vec::new();
    for tok in split_harness_list(&line) {
        let id = if let Ok(n) = tok.parse::<usize>() {
            *HarnessId::ALL
                .get(n.wrapping_sub(1))
                .ok_or_else(|| anyhow::anyhow!("selection '{tok}' out of range"))?
        } else {
            tok.parse()
                .map_err(|e: akit::harness::UnknownHarness| anyhow::anyhow!("{e}"))?
        };
        if !chosen.contains(&id) {
            chosen.push(id);
        }
    }
    HarnessContext::new(chosen)
}

/// List the akit-owned files a reset would remove, before the confirm prompt.
fn print_reset_preview(items: &[akit::ownership::Installation]) {
    println!("Reset would remove these akit-owned files:");
    for item in items {
        for m in &item.materializations {
            println!("  {}", m.path);
        }
    }
}

/// Confirm a destructive reset at an interactive prompt.
fn confirm_reset(installs: usize, files: usize) -> Result<bool> {
    use std::io::{IsTerminal, Write};
    if !std::io::stdin().is_terminal() {
        bail!("refusing to reset non-interactively; re-run with --yes to confirm");
    }
    print!("Remove {files} akit-owned file(s) across {installs} install(s)? [y/N] ");
    std::io::stdout().flush().ok();
    let mut line = String::new();
    std::io::stdin().read_line(&mut line)?;
    Ok(matches!(line.trim(), "y" | "Y" | "yes" | "Yes"))
}

/// Record the project in the global install index (#40) after a successful
/// install, so `where` and `update --propagate` can find it from anywhere.
///
/// Host-level bookkeeping, not ownership state — it lives under `~/.akit` (or
/// `$AKIT_STATE_DIR`), never inside the project — so a failure here warns and is
/// never allowed to fail an install that already succeeded.
fn record_in_index(project: &Project) {
    if let Err(e) = akit::index::record_install(&project.root) {
        eprintln!("warning: could not update the global install index: {e:#}");
    }
}

fn print_install_report(project: &Project, report: &install::InstallReport) {
    if report.not_a_git_repo {
        warn_not_git(project);
    }
    print_install_report_body(report);
    print_reload_guidance(report.item_type, &report.harnesses);
}

/// Print one install outcome (verb line, materializations, skipped) with no
/// not-a-git-repo warning and no reload hint — the caller emits those once, for
/// the whole run (so a bundle shows one aggregated reload block, not one per item).
fn print_install_report_body(report: &install::InstallReport) {
    let harnesses = report
        .harnesses
        .iter()
        .map(|h| h.as_str())
        .collect::<Vec<_>>()
        .join(", ");
    let verb = if report.replaced {
        "Reshaped"
    } else {
        "Installed"
    };
    if report.harnesses.is_empty() {
        println!(
            "{} '{}' installed for no harnesses (all selected were skipped)",
            title_case(type_name(report.item_type)),
            report.id
        );
    } else {
        println!(
            "{verb} {} '{}' for {harnesses}",
            type_name(report.item_type),
            report.id
        );
    }
    for m in &report.materializations {
        let covers = m
            .covers
            .iter()
            .map(|h| h.as_str())
            .collect::<Vec<_>>()
            .join(", ");
        println!("  {}  ({covers})", m.path);
    }
    if !report.issues.is_empty() {
        println!("skipped:");
        for issue in &report.issues {
            println!("  {}: {}", issue.harness.as_str(), issue.reason.message());
        }
    }
}

/// After a `--symlink` install, note any materialization that was copied anyway,
/// with the reason — so the user is never misled that everything symlinked.
///
/// A skill path copies when some harness it serves isn't a confirmed
/// symlink-follower; agents are always copied (agent-file symlinks are unverified
/// for every harness). Materializations that did symlink produce no note.
fn print_symlink_notes(report: &install::InstallReport) {
    match report.item_type {
        ItemType::Agent => {
            if !report.materializations.is_empty() {
                println!(
                    "note: agent '{}' was copied — symlink-following for agent files is not \
                     confirmed for any harness",
                    report.id
                );
            }
        }
        ItemType::Skill => {
            for m in &report.materializations {
                if m.mode != Mode::Symlink {
                    let blockers: Vec<&str> = m
                        .covers
                        .iter()
                        .filter(|h| !h.follows_skill_symlink())
                        .map(|h| h.as_str())
                        .collect();
                    if !blockers.is_empty() {
                        println!(
                            "note: {} copied — symlink-following not confirmed for: {}",
                            m.path,
                            blockers.join(", ")
                        );
                    }
                }
            }
        }
    }
}

/// Print post-install reload/restart guidance per served harness.
///
/// Both primitives now have per-harness reload data in the registry (#46), so
/// skills get the same precise per-harness line agents always had — Copilot's
/// `/skills reload` is a *command*, Claude/Codex watch the directory, OpenCode
/// needs a restart. Cells no primary source establishes are `Reload::Unknown`
/// and degrade to the honest "restart if it does not appear" hint.
fn print_reload_guidance(item_type: ItemType, harnesses: &[HarnessId]) {
    if harnesses.is_empty() {
        return;
    }
    let primitive = match item_type {
        ItemType::Skill => Primitive::Skill,
        ItemType::Agent => Primitive::Agent,
    };
    println!("reload:");
    for &h in harnesses {
        println!(
            "  {} {}: {}",
            h.as_str(),
            type_name(item_type),
            akit::harness::reload_for(primitive, h).guidance()
        );
    }
}

fn print_install_preview(preview: &install::InstallPreview) {
    let harnesses = preview
        .harnesses
        .iter()
        .map(|h| h.as_str())
        .collect::<Vec<_>>()
        .join(", ");
    let suffix = if preview.replaces {
        "  (reshapes an existing install)"
    } else {
        ""
    };
    println!(
        "Plan: {} '{}' for {harnesses}{suffix}",
        type_name(preview.item_type),
        preview.id
    );
    print_preview_sections(preview, "  ");
    println!("(dry run — nothing changed; re-run without --dry-run to apply)");
}

/// Print the create/unchanged/remove/skipped sections of one preview at the given
/// indent. Shared by single-item and per-bundle-member preview printing.
fn print_preview_sections(preview: &install::InstallPreview, indent: &str) {
    if !preview.create.is_empty() {
        println!("{indent}create:");
        for m in &preview.create {
            println!(
                "{indent}  {}  ({})  [{}]",
                m.path,
                covers_str(&m.covers),
                mode_name(m.mode)
            );
        }
    }
    if !preview.unchanged.is_empty() {
        println!("{indent}unchanged:");
        for m in &preview.unchanged {
            println!("{indent}  {}  ({})", m.path, covers_str(&m.covers));
        }
    }
    if !preview.remove.is_empty() {
        println!("{indent}remove (reshape):");
        for p in &preview.remove {
            println!("{indent}  {p}");
        }
    }
    if !preview.issues.is_empty() {
        println!("{indent}skipped:");
        for issue in &preview.issues {
            println!(
                "{indent}  {}: {}",
                issue.harness.as_str(),
                issue.reason.message()
            );
        }
    }
}

/// Print an aggregated bundle-install plan (dry-run and partial-install confirm).
/// The caller adds the trailing dry-run note or confirm prompt.
fn print_bundle_install_preview(preview: &install::BundleInstallPreview) {
    let harnesses = preview
        .harnesses
        .iter()
        .map(|h| h.as_str())
        .collect::<Vec<_>>()
        .join(", ");
    println!("Plan: bundle '{}' for {harnesses}", preview.bundle);
    for item in &preview.items {
        let suffix = if item.replaces {
            "  (reshapes an existing install)"
        } else {
            ""
        };
        println!("  {} '{}'{suffix}:", type_name(item.item_type), item.id);
        print_preview_sections(item, "    ");
    }
    if preview.is_partial() {
        println!("(partial — some items can't be served for every selected harness)");
    }
}

/// Print the outcome of a bundle install: a header, then each member's report.
fn print_bundle_install_report(project: &Project, report: &install::BundleInstallReport) {
    if report.items.iter().any(|i| i.not_a_git_repo) {
        warn_not_git(project);
    }
    let harnesses = report
        .harnesses
        .iter()
        .map(|h| h.as_str())
        .collect::<Vec<_>>()
        .join(", ");
    println!(
        "Installed bundle '{}' for {harnesses} ({} item(s))",
        report.bundle,
        report.items.len()
    );
    for item in &report.items {
        println!();
        print_install_report_body(item);
    }
    // One aggregated reload block for the whole bundle: union the served harnesses
    // per item type, so the (identical) skills hint isn't repeated per member and
    // agent hints are consolidated.
    for item_type in [ItemType::Skill, ItemType::Agent] {
        let mut served: Vec<HarnessId> = Vec::new();
        for item in report.items.iter().filter(|i| i.item_type == item_type) {
            for h in &item.harnesses {
                if !served.contains(h) {
                    served.push(*h);
                }
            }
        }
        served.sort();
        if !served.is_empty() {
            print_reload_guidance(item_type, &served);
        }
    }
}

/// Confirm proceeding with a partial bundle install at an interactive prompt.
fn confirm_partial_install() -> Result<bool> {
    use std::io::{IsTerminal, Write};
    if !std::io::stdin().is_terminal() {
        bail!(
            "refusing a partial bundle install non-interactively; re-run with --yes to install \
             the servable items and skip the rest"
        );
    }
    print!("Proceed with a partial install (skipping the items above)? [y/N] ");
    std::io::stdout().flush().ok();
    let mut line = String::new();
    std::io::stdin().read_line(&mut line)?;
    Ok(matches!(line.trim(), "y" | "Y" | "yes" | "Yes"))
}

fn covers_str(covers: &[HarnessId]) -> String {
    covers
        .iter()
        .map(|h| h.as_str())
        .collect::<Vec<_>>()
        .join(", ")
}

/// Print the removal plan for one item (`uninstall --dry-run`, and the preview
/// shown before the drift confirmation). The caller adds the trailing dry-run
/// note or the prompt.
fn print_uninstall_preview(preview: &install::RemovePreview) {
    if preview.not_installed {
        println!(
            "{} '{}' is not installed",
            title_case(type_name(preview.item_type)),
            preview.id
        );
        return;
    }
    let kind = type_name(preview.item_type);
    if preview.reshape() {
        let remaining = preview
            .remaining_harnesses
            .iter()
            .map(|h| h.as_str())
            .collect::<Vec<_>>()
            .join(", ");
        println!(
            "Plan: uninstall {kind} '{}' from selected harness(es); would stay installed for \
             {remaining}",
            preview.id
        );
    } else {
        println!("Plan: uninstall {kind} '{}' (full removal)", preview.id);
    }
    print_uninstall_preview_sections(preview, "  ");
}

/// Print the remove/create/unchanged sections of one removal plan at the given
/// indent. Shared by the single-item and per-bundle-member printing.
fn print_uninstall_preview_sections(preview: &install::RemovePreview, indent: &str) {
    if !preview.remove.is_empty() {
        println!("{indent}remove:");
        for r in &preview.remove {
            println!("{indent}  {}{}", r.path, removal_drift_note(r.drift));
        }
    }
    if !preview.create.is_empty() {
        println!("{indent}create (reshape):");
        for m in &preview.create {
            println!(
                "{indent}  {}  ({})  [{}]",
                m.path,
                covers_str(&m.covers),
                mode_name(m.mode)
            );
        }
    }
    if !preview.keep.is_empty() {
        println!("{indent}keep (rewritten by the reshape):");
        for k in &preview.keep {
            println!(
                "{indent}  {}  ({}){}",
                k.materialization.path,
                covers_str(&k.materialization.covers),
                kept_drift_note(k.drift)
            );
        }
    }
}

/// The suffix flagging an owned copy that no longer matches what akit recorded.
/// A locally modified copy is the one deletion the user is asked to confirm.
fn removal_drift_note(drift: akit::materialize::Drift) -> &'static str {
    match drift {
        akit::materialize::Drift::Clean => "",
        akit::materialize::Drift::Modified => "  (locally modified)",
        akit::materialize::Drift::Missing => "  (already gone)",
    }
}

/// The suffix for a path the reshape *keeps*. Keeping is not leaving alone — the
/// reshape rewrites the path from the catalog — so a locally modified copy here
/// loses its edits just as a deleted one does, and the gate counts it.
fn kept_drift_note(drift: akit::materialize::Drift) -> &'static str {
    match drift {
        akit::materialize::Drift::Clean => "",
        akit::materialize::Drift::Modified => "  (locally modified — will be reverted)",
        akit::materialize::Drift::Missing => "  (missing — will be restored)",
    }
}

/// Print the aggregated removal plan for `uninstall --bundle`.
fn print_bundle_uninstall_preview(preview: &install::BundleRemovePreview) {
    if preview.items.is_empty() {
        println!("Bundle '{}' has no harness-aware installs", preview.bundle);
        return;
    }
    println!(
        "Plan: uninstall bundle '{}' ({} item(s))",
        preview.bundle,
        preview.items.len()
    );
    for item in &preview.items {
        let suffix = if item.reshape() { "  (reshape)" } else { "" };
        println!("  {} '{}'{suffix}:", type_name(item.item_type), item.id);
        print_uninstall_preview_sections(item, "    ");
    }
    let drifted = preview.drifted();
    if drifted > 0 {
        println!("({drifted} locally modified file(s) would be deleted or reverted)");
    }
}

/// The shared `uninstall` gate: the dry-run output, and the confirmation that
/// stands between a locally modified akit-owned copy and its deletion (or, for a
/// path a scoped reshape keeps, its reversion to catalog content).
///
/// Returns `Ok(false)` when the caller must stop without applying anything — a
/// dry run, or a declined confirmation. Both `uninstall <id>` and
/// `uninstall --bundle <name>` go through here, so the two paths cannot drift
/// apart on what they print, count, or refuse.
fn uninstall_gate<P: serde::Serialize>(
    json: bool,
    dry_run: bool,
    preview: &P,
    gate: DriftGate,
    print: impl FnOnce(&P),
) -> Result<bool> {
    use std::io::{IsTerminal, Write};
    if dry_run {
        if json {
            println!("{}", serde_json::to_string(preview)?);
        } else {
            print(preview);
            // Nothing to apply (not installed, or a bundle nobody tagged): the
            // "re-run without --dry-run" nudge would be a dead end.
            if gate.applies {
                println!("(dry run — nothing removed; re-run without --dry-run to apply)");
            }
        }
        return Ok(false);
    }
    if gate.drifted == 0 {
        return Ok(true);
    }
    if !json {
        print(preview);
    }
    if json || !std::io::stdin().is_terminal() {
        if json {
            // Structured refusal: a machine consumer gets the same preview object
            // `--dry-run` emits, so it can see *what* drifted and tell a refusal
            // apart from an infrastructure failure. The reason goes to stderr.
            println!("{}", serde_json::to_string(preview)?);
        }
        bail!(
            "refusing to discard local edits to {} akit-owned file(s) without confirmation; \
             re-run with --yes to delete/revert them anyway, or {} to keep them and stop \
             managing the install",
            gate.drifted,
            gate.detach_hint()
        );
    }
    // Prompt on stderr: stdout may well be a redirected log, and an invisible
    // prompt is an unexplained hang.
    eprint!(
        "Discard local edits to {} akit-owned file(s)? [y/N] ",
        gate.drifted
    );
    std::io::stderr().flush().ok();
    let mut line = String::new();
    std::io::stdin().read_line(&mut line)?;
    if matches!(line.trim(), "y" | "Y" | "yes" | "Yes") {
        return Ok(true);
    }
    eprintln!("Aborted.");
    Ok(false)
}

/// What the uninstall gate needs to know about a plan it did not compute.
struct DriftGate {
    /// Locally modified akit-owned copies the uninstall would delete or revert.
    drifted: usize,
    /// The drifted items, as `detach` would address them (`--agent <id>` for an
    /// agent), so the refusal names the real way out rather than a placeholder.
    drifted_items: Vec<String>,
    /// False when there is nothing to apply at all — an item that is not
    /// installed, or a bundle with no tagged members.
    applies: bool,
}

impl DriftGate {
    /// The `akit detach` invocation the refusal points at.
    fn detach_hint(&self) -> String {
        match self.drifted_items.as_slice() {
            [] => "`akit detach <id>`".to_string(),
            [one] => format!("`akit detach {one}`"),
            many => format!("`akit detach` on each of: {}", many.join(", ")),
        }
    }
}

/// How `akit detach` addresses one previewed item.
fn detach_target(preview: &install::RemovePreview) -> String {
    match preview.item_type {
        ItemType::Agent => format!("--agent {}", preview.id),
        ItemType::Skill => preview.id.clone(),
    }
}

fn print_uninstall_report(report: &install::RemoveReport) {
    if report.not_installed {
        println!(
            "{} '{}' is not installed",
            title_case(type_name(report.item_type)),
            report.id
        );
        return;
    }
    if report.remaining_harnesses.is_empty() {
        println!(
            "Uninstalled {} '{}' ({} file(s) removed)",
            type_name(report.item_type),
            report.id,
            report.removed_paths.len()
        );
    } else {
        let remaining = report
            .remaining_harnesses
            .iter()
            .map(|h| h.as_str())
            .collect::<Vec<_>>()
            .join(", ");
        println!(
            "Removed {} '{}' from selected harness(es); still installed for {remaining}",
            type_name(report.item_type),
            report.id
        );
    }
}

/// Print the outcome of `uninstall --bundle`: a header, then one line per member.
fn print_bundle_uninstall_report(report: &install::BundleRemoveReport) {
    if report.items.is_empty() {
        println!("Bundle '{}' has no harness-aware installs", report.bundle);
        return;
    }
    let files: usize = report.items.iter().map(|i| i.removed_paths.len()).sum();
    println!(
        "Uninstalled bundle '{}' ({} item(s), {files} file(s) removed)",
        report.bundle,
        report.items.len()
    );
    for item in &report.items {
        if item.remaining_harnesses.is_empty() {
            println!(
                "  {} '{}' — removed ({} file(s))",
                type_name(item.item_type),
                item.id,
                item.removed_paths.len()
            );
        } else {
            let remaining = item
                .remaining_harnesses
                .iter()
                .map(|h| h.as_str())
                .collect::<Vec<_>>()
                .join(", ");
            println!(
                "  {} '{}' — still installed for {remaining}",
                type_name(item.item_type),
                item.id
            );
        }
    }
}

/// Per-item health label + the harnesses left uncovered when degraded.
fn item_health_label(item: &akit::reconcile::ItemHealth) -> String {
    if !item.source_present {
        return "missing-source".to_string();
    }
    if !item.degraded {
        return "ok".to_string();
    }
    // Harnesses covered by at least one clean materialization.
    let mut clean: Vec<HarnessId> = Vec::new();
    for m in &item.materializations {
        if m.drift == akit::materialize::Drift::Clean {
            for h in &m.covers {
                if !clean.contains(h) {
                    clean.push(*h);
                }
            }
        }
    }
    let uncovered: Vec<&str> = item
        .harnesses
        .iter()
        .filter(|h| !clean.contains(h))
        .map(|h| h.as_str())
        .collect();
    if uncovered.is_empty() {
        "degraded".to_string()
    } else {
        format!("degraded (uncovered: {})", uncovered.join(", "))
    }
}

fn print_installed_health(report: &akit::reconcile::HealthReport) {
    if report.items.is_empty() {
        println!("No harness-aware installs in this project.");
        if !report.stale_excludes.is_empty() {
            print_stale_excludes(&report.stale_excludes);
        }
        return;
    }
    println!("{:<28} {:<7} {:<24} HEALTH", "ID", "TYPE", "HARNESSES");
    let mut degraded = 0usize;
    let mut missing_source = 0usize;
    for item in &report.items {
        let harnesses = item
            .harnesses
            .iter()
            .map(|h| h.as_str())
            .collect::<Vec<_>>()
            .join(", ");
        let health = item_health_label(item);
        if !item.source_present {
            missing_source += 1;
        } else if item.degraded {
            degraded += 1;
        }
        println!(
            "{:<28} {:<7} {:<24} {health}",
            item.id,
            type_name(item.item_type),
            harnesses
        );
    }
    if !report.stale_excludes.is_empty() {
        print_stale_excludes(&report.stale_excludes);
    }
    if report.healthy {
        println!("Health: ok");
    } else {
        let mut parts = Vec::new();
        if degraded > 0 {
            parts.push(format!("{degraded} degraded"));
        }
        if missing_source > 0 {
            parts.push(format!("{missing_source} missing-source"));
        }
        if !report.stale_excludes.is_empty() {
            parts.push(format!(
                "{} stale exclude line(s)",
                report.stale_excludes.len()
            ));
        }
        println!("Health: {}", parts.join(", "));
    }
}

/// Print `where`: one block per known project holding the item, listing its
/// harness coverage, health, and every materialization with mode + drift.
fn print_where_report(report: &akit::index::WhereReport) {
    let kind = type_name(report.item_type);
    if report.projects.is_empty() {
        println!(
            "{} '{}' is not installed in any known project.",
            title_case(kind),
            report.id
        );
    } else {
        println!(
            "{} '{}' — installed in {} project(s):",
            title_case(kind),
            report.id,
            report.projects.len()
        );
        for p in &report.projects {
            println!();
            println!("{}", p.project);
            println!(
                "  harnesses: {}   health: {}",
                covers_str(&p.health.harnesses),
                item_health_label(&p.health)
            );
            for m in &p.health.materializations {
                println!(
                    "  {}  [{}]  ({})  {}",
                    m.path,
                    mode_name(m.mode),
                    covers_str(&m.covers),
                    drift_name(m.drift)
                );
            }
        }
    }
    if !report.skipped.is_empty() {
        println!();
        println!("Skipped {} unreadable project(s):", report.skipped.len());
        for s in &report.skipped {
            println!("  {}: {}", s.project, s.error);
        }
    }
}

fn drift_name(drift: akit::materialize::Drift) -> &'static str {
    match drift {
        akit::materialize::Drift::Clean => "clean",
        akit::materialize::Drift::Missing => "missing",
        akit::materialize::Drift::Modified => "modified",
    }
}

fn print_stale_excludes(stale: &[String]) {
    println!("Stale exclude lines (not owned by any install):");
    for line in stale {
        println!("  {line}");
    }
}

fn print_repair_report(report: &akit::reconcile::RepairReport) {
    if report.restored_paths.is_empty()
        && report.skipped_modified.is_empty()
        && report.missing_source.is_empty()
    {
        println!("Nothing to repair — all akit-owned files are present.");
        return;
    }
    if !report.restored_paths.is_empty() {
        println!("Restored {} missing file(s):", report.restored_paths.len());
        for p in &report.restored_paths {
            println!("  {p}");
        }
    }
    if !report.skipped_modified.is_empty() {
        println!(
            "Skipped {} locally-modified file(s) (not overwritten):",
            report.skipped_modified.len()
        );
        for p in &report.skipped_modified {
            println!("  {p}");
        }
    }
    if !report.missing_source.is_empty() {
        println!(
            "Cannot repair {} item(s) — catalog source is gone:",
            report.missing_source.len()
        );
        for id in &report.missing_source {
            println!("  {id}");
        }
    }
}

/// Which ownership-drop verb produced a [`DetachReport`], for phrasing output.
enum DropKind {
    Detach,
    Forget,
}

fn print_detach_report(report: &akit::reconcile::DetachReport, kind: DropKind) {
    if report.not_installed {
        println!(
            "{} '{}' has no akit ownership record.",
            title_case(type_name(report.item_type)),
            report.id
        );
        return;
    }
    match kind {
        DropKind::Detach => println!(
            "Detached {} '{}' — {} file(s) kept on disk and made git-visible.",
            type_name(report.item_type),
            report.id,
            report.paths.len()
        ),
        DropKind::Forget => println!(
            "Forgot {} '{}' — dropped {} ownership record(s).",
            type_name(report.item_type),
            report.id,
            report.paths.len()
        ),
    }
    for p in &report.paths {
        println!("  {p}");
    }
}

fn print_adopt_report(report: &akit::reconcile::AdoptReport) {
    if report.adopted_paths.is_empty() && report.conflicts.is_empty() {
        println!(
            "Nothing to adopt for {} '{}' — no matching files on disk.",
            type_name(report.item_type),
            report.id
        );
        return;
    }
    if !report.adopted_paths.is_empty() {
        println!(
            "Adopted {} '{}' for {}:",
            type_name(report.item_type),
            report.id,
            covers_str(&report.harnesses)
        );
        for p in &report.adopted_paths {
            println!("  {p}");
        }
    }
    if !report.conflicts.is_empty() {
        println!(
            "Skipped {} file(s) that differ from the catalog source (not overwritten):",
            report.conflicts.len()
        );
        for p in &report.conflicts {
            println!("  {p}");
        }
    }
}

fn remote_base_url() -> String {
    std::env::var(remote::ENV_REMOTE_BASE_URL)
        .ok()
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| remote::DEFAULT_BASE_URL.to_string())
}

fn type_name(item_type: ItemType) -> &'static str {
    match item_type {
        ItemType::Skill => "skill",
        ItemType::Agent => "agent",
    }
}

fn mode_name(mode: Mode) -> &'static str {
    match mode {
        Mode::Symlink => "symlink",
        Mode::Copy => "copy",
    }
}

fn title_case(s: &str) -> String {
    let mut chars = s.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
        None => String::new(),
    }
}

fn warn_not_git(project: &Project) {
    eprintln!(
        "warning: {} is not a git repository; pulled files will NOT be git-ignored",
        project.root.display()
    );
}

fn pull_report_line(report: &ops::PullReport) -> String {
    let action = if report.overwritten {
        "overwritten"
    } else if report.created {
        "copied"
    } else {
        "already present"
    };
    let source = match &report.git_ref {
        Some(git_ref) => format!("{}#{git_ref}", report.source),
        None => report.source.clone(),
    };
    format!(
        "Pulled {} '{}' from {} -> {} ({action})",
        type_name(report.item_type),
        report.id,
        source,
        report.path
    )
}

fn drop_report_line(report: &ops::DropReport) -> String {
    let origin = match &report.source {
        Some(source) => match &report.git_ref {
            Some(git_ref) => format!(" (from {source}#{git_ref})"),
            None => format!(" (from {source})"),
        },
        None => String::new(),
    };
    let action = if report.item_removed {
        "removed"
    } else {
        "manifest entry pruned; files were already absent"
    };
    format!(
        "Dropped {} '{}'{origin} -> {} ({action})",
        type_name(report.item_type),
        report.id,
        report.path
    )
}

fn restore_status_label(status: ops::RestoreStatus) -> &'static str {
    match status {
        ops::RestoreStatus::Pulled => "pulled",
        ops::RestoreStatus::AlreadyPresent => "already present",
        ops::RestoreStatus::Overwritten => "overwritten",
        ops::RestoreStatus::Error => "error",
    }
}

fn print_restore_report(report: &ops::RestoreReport) {
    if report.items.is_empty() {
        println!("Nothing to restore; catalog manifest has no remote items.");
        return;
    }
    for item in &report.items {
        let source = match &item.git_ref {
            Some(git_ref) => format!("{}#{git_ref}", item.source),
            None => item.source.clone(),
        };
        match &item.error {
            Some(error) => eprintln!(
                "  {} {} '{}' from {}: {error}",
                restore_status_label(item.status),
                type_name(item.item_type),
                item.id,
                source
            ),
            None => println!(
                "  {} {} '{}' from {}",
                restore_status_label(item.status),
                type_name(item.item_type),
                item.id,
                source
            ),
        }
    }
    let s = &report.summary;
    println!(
        "Restored {} item(s): {} pulled, {} already present, {} overwritten, {} error(s).",
        report.items.len(),
        s.pulled,
        s.already_present,
        s.overwritten,
        s.errors
    );
}

fn update_status_label(status: ops::UpdateStatus) -> &'static str {
    match status {
        ops::UpdateStatus::Updated => "updated",
        ops::UpdateStatus::Outdated => "outdated",
        ops::UpdateStatus::UpToDate => "up to date",
        ops::UpdateStatus::Pinned => "pinned",
        ops::UpdateStatus::Error => "error",
    }
}

/// Short, display-friendly commit prefix.
fn short_sha(sha: &str) -> &str {
    sha.get(..7).unwrap_or(sha)
}

fn print_update_report(report: &ops::UpdateReport, check: bool) {
    if report.items.is_empty() {
        println!("Nothing to update; catalog manifest has no remote items.");
        return;
    }
    for item in &report.items {
        let source = match &item.git_ref {
            Some(git_ref) => format!("{}#{git_ref}", item.source),
            None => item.source.clone(),
        };
        // Append a short `old → new` (or `→ new`) commit hint when the SHA moved.
        let shas = match (&item.previous_commit, &item.commit) {
            (Some(old), Some(new)) if old != new => {
                format!(" ({} → {})", short_sha(old), short_sha(new))
            }
            (None, Some(new)) if matches!(item.status, ops::UpdateStatus::Updated) => {
                format!(" (→ {})", short_sha(new))
            }
            _ => String::new(),
        };
        match &item.error {
            Some(error) => eprintln!(
                "  {} {} '{}' from {}: {error}",
                update_status_label(item.status),
                type_name(item.item_type),
                item.id,
                source
            ),
            None => println!(
                "  {} {} '{}' from {}{shas}",
                update_status_label(item.status),
                type_name(item.item_type),
                item.id,
                source
            ),
        }
    }
    let s = &report.summary;
    if check {
        println!(
            "Checked {} item(s): {} outdated, {} up to date, {} pinned, {} error(s).",
            report.items.len(),
            s.outdated,
            s.up_to_date,
            s.pinned,
            s.errors
        );
    } else {
        println!(
            "Updated {} item(s): {} updated, {} up to date, {} pinned, {} error(s).",
            report.items.len(),
            s.updated,
            s.up_to_date,
            s.pinned,
            s.errors
        );
    }
}

fn propagate_status_label(status: akit::index::PropagateStatus) -> &'static str {
    use akit::index::PropagateStatus as S;
    match status {
        S::Updated => "updated",
        S::UpToDate => "up to date",
        S::Drifted => "drifted",
        S::Symlink => "symlink",
        S::Missing => "missing",
        S::Error => "error",
    }
}

/// Why a materialization was left alone, appended to its line so the symlink vs
/// copy distinction (and the no-clobber policy) is visible without the docs.
fn propagate_status_note(status: akit::index::PropagateStatus) -> &'static str {
    use akit::index::PropagateStatus as S;
    match status {
        S::Drifted => "  (locally modified — not overwritten)",
        S::Symlink => "  (symlink — already tracks the catalog)",
        S::Missing => "  (gone — run `akit repair` in that project)",
        _ => "",
    }
}

/// Print the `--propagate` section below the update report: per project, per
/// item, per materialization, then an aggregate line.
fn print_propagation_report(report: &akit::index::PropagationReport) {
    let s = &report.summary;
    println!();
    if report.projects.is_empty() {
        println!(
            "Propagate: nothing to re-sync ({} known project(s) checked).",
            s.projects
        );
        return;
    }
    println!("Propagate:");
    for p in &report.projects {
        println!("  {}", p.project);
        if let Some(error) = &p.error {
            println!("    skipped: {error}");
        }
        for item in &p.items {
            println!("    {} '{}'", type_name(item.item_type), item.id);
            if let Some(error) = &item.error {
                println!("      skipped: {error}");
            }
            for m in &item.materializations {
                let label = propagate_status_label(m.status);
                match &m.error {
                    Some(error) => println!("      {label:<10}  {}: {error}", m.path),
                    None => println!(
                        "      {label:<10}  {}{}",
                        m.path,
                        propagate_status_note(m.status)
                    ),
                }
            }
        }
    }
    println!(
        "Propagated across {} project(s): {} updated, {} up to date, {} drifted (skipped), \
         {} symlink (already live), {} missing, {} error(s).",
        s.projects, s.updated, s.up_to_date, s.drifted, s.symlink, s.missing, s.errors
    );
}

fn print_log(entries: &[ops::LogEntry]) {
    if entries.is_empty() {
        println!("No commit history available for this item.");
        return;
    }
    for entry in entries {
        let mark = if entry.current { "*" } else { " " };
        println!(
            "{mark} {}  {}  {}",
            short_sha(&entry.commit),
            entry.date,
            entry.subject
        );
    }
}

/// JSON shape for `status`: harness-aware installed items (`.akit`) plus
/// per-bundle completeness. `items` are `reconcile::ItemHealth`.
#[derive(serde::Serialize)]
struct StatusReport<'a> {
    items: &'a [akit::reconcile::ItemHealth],
    bundles: &'a [BundleHealth],
}

/// Print a one-line-per-bundle completeness summary below the item table.
fn print_bundle_health(bundles: &[BundleHealth]) {
    if bundles.is_empty() {
        return;
    }
    println!();
    for b in bundles {
        match b.state {
            BundleState::Complete => {
                let expected = b.expected.unwrap_or(b.installed);
                println!(
                    "Bundle '{}': complete ({}/{})",
                    b.name, b.installed, expected
                );
            }
            BundleState::Partial => {
                let expected = b.expected.unwrap_or(b.installed);
                println!(
                    "Bundle '{}': partial ({}/{}) — missing: {}",
                    b.name,
                    b.installed,
                    expected,
                    b.missing.join(", ")
                );
            }
            BundleState::Unknown => {
                println!(
                    "Bundle '{}': unknown (manifest unavailable; {} installed)",
                    b.name, b.installed
                );
            }
        }
    }
}

/// Bundle-grouped project overview over the harness-aware `.akit` lockfile:
/// `BUNDLE  ID  TYPE  HARNESSES  HEALTH`, standalone (untagged) rows last.
fn print_status_table(items: &[akit::reconcile::ItemHealth]) {
    if items.is_empty() {
        println!("No items installed.");
        return;
    }
    let harnesses_of = |item: &akit::reconcile::ItemHealth| {
        item.harnesses
            .iter()
            .map(|h| h.as_str())
            .collect::<Vec<_>>()
            .join(", ")
    };

    let mut bundle_width = "BUNDLE".len();
    let mut id_width = "ID".len();
    let mut type_width = "TYPE".len();
    let mut harness_width = "HARNESSES".len();
    for item in items {
        bundle_width = bundle_width.max(item.bundle.as_deref().unwrap_or("-").len());
        id_width = id_width.max(item.id.len());
        type_width = type_width.max(type_name(item.item_type).len());
        harness_width = harness_width.max(harnesses_of(item).len());
    }

    println!(
        "{:<bundle_width$}  {:<id_width$}  {:<type_width$}  {:<harness_width$}  HEALTH",
        "BUNDLE", "ID", "TYPE", "HARNESSES"
    );
    let mut ordered: Vec<&akit::reconcile::ItemHealth> = items.iter().collect();
    ordered.sort_by(|a, b| match (a.bundle.as_deref(), b.bundle.as_deref()) {
        (Some(x), Some(y)) => x.cmp(y).then_with(|| a.id.cmp(&b.id)),
        (Some(_), None) => std::cmp::Ordering::Less,
        (None, Some(_)) => std::cmp::Ordering::Greater,
        (None, None) => a.id.cmp(&b.id),
    });
    for item in ordered {
        println!(
            "{:<bundle_width$}  {:<id_width$}  {:<type_width$}  {:<harness_width$}  {}",
            item.bundle.as_deref().unwrap_or("-"),
            item.id,
            type_name(item.item_type),
            harnesses_of(item),
            item_health_label(item)
        );
    }
}

/// Print a read-only `doctor` diagnosis over the harness-aware `.akit` state:
/// the item table, per-bundle completeness, exclude drift, then a verdict.
fn print_diagnosis(d: &akit::reconcile::Diagnosis) {
    print_status_table(&d.items);
    print_bundle_health(&d.bundles);

    if d.missing_excludes.is_empty() && d.stale_excludes.is_empty() {
        // No exclude drift — say so only when there is a lockfile to compare.
        if d.lockfile_present {
            println!("\nExclude: ok");
        }
    } else {
        println!("\ngit excludes:");
        for line in &d.missing_excludes {
            println!("  missing: {line}  (run `akit sync`)");
        }
        for line in &d.stale_excludes {
            println!("  stale:   {line}  (run `akit sync`)");
        }
    }

    if d.healthy {
        println!("\nDoctor: ok");
        return;
    }
    let degraded = d.items.iter().filter(|i| i.degraded).count();
    let missing_source = d.items.iter().filter(|i| !i.source_present).count();
    let partial = d
        .bundles
        .iter()
        .filter(|b| b.state == BundleState::Partial)
        .count();
    let mut parts = Vec::new();
    if degraded > 0 {
        parts.push(format!("{degraded} degraded"));
    }
    if missing_source > 0 {
        parts.push(format!("{missing_source} missing-source"));
    }
    if !d.missing_excludes.is_empty() {
        parts.push(format!(
            "{} missing exclude line(s)",
            d.missing_excludes.len()
        ));
    }
    if !d.stale_excludes.is_empty() {
        parts.push(format!("{} stale exclude line(s)", d.stale_excludes.len()));
    }
    if partial > 0 {
        parts.push(format!("{partial} partial bundle(s)"));
    }
    println!("\nDoctor: {}", parts.join(", "));
}

fn print_catalog_table(items: &[CatalogItem]) {
    if items.is_empty() {
        println!("Catalog is empty. Populate it by hand or with `akit pull`.");
        return;
    }

    let mut type_width = "TYPE".len();
    let mut id_width = "ID".len();
    let mut origin_width = "ORIGIN".len();
    let mut harness_width = "HARNESSES".len();
    for item in items {
        type_width = type_width.max(type_name(item.item_type).len());
        id_width = id_width.max(item.id.len());
        origin_width = origin_width.max(catalog_origin(item).len());
        harness_width = harness_width.max(catalog_harnesses(item).len());
    }

    println!(
        "{:<type_width$}  {:<id_width$}  {:<origin_width$}  {:<harness_width$}  DESCRIPTION",
        "TYPE", "ID", "ORIGIN", "HARNESSES"
    );
    for item in items {
        println!(
            "{:<type_width$}  {:<id_width$}  {:<origin_width$}  {:<harness_width$}  {}",
            type_name(item.item_type),
            item.id,
            catalog_origin(item),
            catalog_harnesses(item),
            item.description
        );
    }
}

/// The HARNESSES cell for a catalog row: an agent package's supported set, an
/// invalid package as `disabled`, or `-` for skills.
fn catalog_harnesses(item: &CatalogItem) -> String {
    if item.disabled {
        "disabled".to_string()
    } else if item.harnesses.is_empty() {
        "-".to_string()
    } else {
        covers_str(&item.harnesses)
    }
}

fn catalog_origin(item: &CatalogItem) -> String {
    item.source.clone().unwrap_or_else(|| "local".to_string())
}

fn print_search_hits(hits: &[SearchHit]) {
    for hit in hits {
        let mut details = hit.description.clone();
        if !hit.category.is_empty() {
            if !details.is_empty() {
                details.push(' ');
            }
            details.push('(');
            details.push_str(&hit.category);
            details.push(')');
        }
        if !hit.harnesses.is_empty() {
            if !details.is_empty() {
                details.push(' ');
            }
            details.push('[');
            details.push_str(&covers_str(&hit.harnesses));
            details.push(']');
        }

        if details.is_empty() {
            println!("{}  {}", type_name(hit.item_type), hit.name);
        } else {
            println!("{}  {}  — {}", type_name(hit.item_type), hit.name, details);
        }
    }
}

fn print_item_preview(preview: &show::ItemPreview) {
    let mut header = format!("{} · {}", type_name(preview.item_type), preview.name);
    if !preview.category.is_empty() {
        header.push_str(" · ");
        header.push_str(&preview.category);
    }
    println!("{header}");
    if !preview.description.is_empty() {
        println!("{}", preview.description);
    }
    if !preview.harnesses.is_empty() {
        println!("harnesses: {}", covers_str(&preview.harnesses));
    }
    println!("{}", preview.path.display());
    println!();
    print!("{}", preview.content);
    if !preview.content.ends_with('\n') {
        println!();
    }
}
