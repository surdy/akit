//! akit CLI — a thin wrapper over the `akit` engine.

use anyhow::{Result, bail};
use clap::{Parser, Subcommand};
use std::path::PathBuf;

use akit::catalog::Catalog;
use akit::config::LocalConfig;
use akit::doctor;
use akit::doctor::{DoctorReport, SyncReport};
use akit::harness::HarnessId;
use akit::install::{self, HarnessContext, RemoveScope};
use akit::lockfile::{ItemType, Mode};
use akit::ops;
use akit::ops::{BundleHealth, BundleState, CatalogItem, HealthStatus, ListItem};
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
    /// Pull a skill or agent from your catalog, or owner/repo/path[#ref], into this project.
    Add {
        /// Add an agent (`agents/<name>.agent.md`) instead of a skill.
        #[arg(long)]
        agent: bool,
        /// Copy files instead of symlinking them.
        #[arg(long)]
        copy: bool,
        /// Add every item listed by `bundles/<name>.yml`.
        #[arg(long)]
        bundle: Option<String>,
        /// Local item name, or remote owner/repo/path[#ref].
        name: Option<String>,
    },
    /// Remove a skill or agent from this project.
    Rm {
        /// Remove an agent (`agents/<name>.agent.md`) instead of a skill.
        #[arg(long)]
        agent: bool,
        /// Remove every installed item tagged with this bundle.
        #[arg(long)]
        bundle: Option<String>,
        /// Name of the item to remove.
        name: Option<String>,
    },
    /// List every skill and agent in your catalog.
    #[command(alias = "catalog")]
    Ls,
    /// List items installed in this project and their health.
    Status,
    /// Repair missing materializations and git-exclude lines from the lockfile.
    Sync,
    /// Read-only reconcile report for lockfile, files, and git-exclude lines.
    Doctor,
    /// Search your catalog by skill/agent frontmatter.
    Search {
        /// Query to fuzzy-match against name and description (empty lists everything).
        query: Option<String>,
    },
    /// Print a read-only preview of a catalog item (frontmatter + content).
    Show {
        /// Show an agent (`agents/<id>.agent.md`) instead of a skill.
        #[arg(long)]
        agent: bool,
        /// Item id: a skill directory name, or an agent file stem.
        id: String,
    },
    /// Fetch a remote owner/repo/path[#ref] source into your local catalog.
    Pull {
        /// Pull an agent (`.agent.md`) instead of a skill.
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
        /// Update an agent (`.agent.md`) instead of a skill (only meaningful with `id`).
        #[arg(long)]
        agent: bool,
        /// Report what would change without writing anything.
        #[arg(long)]
        check: bool,
        /// Roll back (or forward-pin) `<id>` to this exact commit of its recorded ref.
        #[arg(long, value_name = "SHA")]
        to: Option<String>,
        /// Catalog id to update; omit to update every pulled item.
        id: Option<String>,
    },
    /// Show the upstream commit history of a pulled catalog item (newest first).
    Log {
        /// Inspect an agent (`.agent.md`) instead of a skill.
        #[arg(long)]
        agent: bool,
        /// Catalog id of the pulled item.
        id: String,
    },
    /// Remove a skill or agent from the catalog (prunes its manifest entry if it was pulled).
    Drop {
        /// Drop an agent (`.agent.md`) instead of a skill.
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
        /// Catalog id of the skill/agent to install.
        id: String,
    },
    /// Uninstall a harness-aware install from some or all harnesses.
    Uninstall {
        /// Uninstall an agent instead of a skill.
        #[arg(long)]
        agent: bool,
        /// Only remove from these harnesses (repeatable); omit to fully uninstall.
        #[arg(long = "harness", short = 'H', value_name = "ID")]
        harnesses: Vec<String>,
        /// Catalog id of the skill/agent to uninstall.
        id: String,
    },
    /// List harness-aware installs recorded in `.akit/kit.lock.json`.
    Installed,
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
        Commands::Add {
            agent,
            copy,
            bundle,
            name,
        } => {
            let mode = if *copy { Mode::Copy } else { Mode::Symlink };
            match (bundle.as_deref(), name.as_deref()) {
                (Some(_), Some(_)) => {
                    bail!("add accepts either <name> or --bundle <name>, not both")
                }
                (None, None) => bail!("add requires <name> or --bundle <name>"),
                (Some(bundle), None) => {
                    if *agent {
                        bail!("add --bundle cannot be combined with --agent");
                    }
                    let project = Project::locate(cli.project.clone())?;
                    let catalog = Catalog::locate()?;
                    let report = ops::add_bundle(&project, &catalog, bundle, mode)?;
                    if cli.json {
                        println!("{}", serde_json::to_string(&report)?);
                    } else {
                        if report.items.iter().any(|item| item.not_a_git_repo) {
                            warn_not_git(&project);
                        }
                        println!(
                            "Added bundle '{}' ({} items)",
                            report.bundle,
                            report.items.len()
                        );
                        for item in &report.items {
                            println!("  {}", add_report_line(item));
                        }
                    }
                }
                (None, Some(name)) => {
                    let project = Project::locate(cli.project.clone())?;
                    let report = if let Some(spec) = SourceSpec::parse(name) {
                        ops::add_remote(
                            &project,
                            &spec,
                            item_type(*agent),
                            mode,
                            &remote_base_url(),
                        )?
                    } else if name.contains('/') {
                        bail!("invalid remote source spec '{name}'; expected owner/repo/path[#ref]")
                    } else {
                        let catalog = Catalog::locate()?;
                        ops::add_item(&project, &catalog, item_type(*agent), name, mode, None)?
                    };
                    if cli.json {
                        println!("{}", serde_json::to_string(&report)?);
                    } else {
                        if report.not_a_git_repo {
                            warn_not_git(&project);
                        }
                        println!("{}", add_report_line(&report));
                    }
                }
            }
        }
        Commands::Rm {
            agent,
            bundle,
            name,
        } => match (bundle.as_deref(), name.as_deref()) {
            (Some(_), Some(_)) => {
                bail!("rm accepts either <name> or --bundle <name>, not both")
            }
            (None, None) => bail!("rm requires <name> or --bundle <name>"),
            (Some(bundle), None) => {
                if *agent {
                    bail!("rm --bundle cannot be combined with --agent");
                }
                let project = Project::locate(cli.project.clone())?;
                let report = ops::remove_bundle(&project, bundle)?;
                if cli.json {
                    println!("{}", serde_json::to_string(&report)?);
                } else if report.items.is_empty() {
                    println!("Bundle '{}' is not installed", report.bundle);
                } else {
                    println!(
                        "Removed bundle '{}' ({} items)",
                        report.bundle,
                        report.items.len()
                    );
                    for item in &report.items {
                        println!("  {}", remove_report_line(item));
                    }
                }
            }
            (None, Some(name)) => {
                let project = Project::locate(cli.project.clone())?;
                let report = ops::remove_item(&project, item_type(*agent), name)?;
                if cli.json {
                    println!("{}", serde_json::to_string(&report)?);
                } else if report.not_installed {
                    println!(
                        "{} '{}' is not installed",
                        title_case(type_name(report.item_type)),
                        report.id
                    );
                } else {
                    println!("{}", remove_report_line(&report));
                }
            }
        },
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
            let items = ops::list_items(&project)?;
            // The catalog is only needed to read bundle manifests; a missing
            // catalog degrades bundles to `unknown` rather than failing status.
            let catalog = Catalog::locate().ok();
            let bundles = ops::bundle_health(&project, catalog.as_ref())?;
            if cli.json {
                println!(
                    "{}",
                    serde_json::to_string(&StatusReport {
                        items: &items,
                        bundles: &bundles,
                    })?
                );
            } else {
                print_table(&items);
                print_bundle_health(&bundles);
            }
        }
        Commands::Sync => {
            let project = Project::locate(cli.project.clone())?;
            let catalog = Catalog::locate()?;
            let report = doctor::sync(&project, &catalog)?;
            if cli.json {
                println!("{}", serde_json::to_string(&report)?);
            } else {
                print_sync_report(&report);
            }
        }
        Commands::Doctor => {
            let project = Project::locate(cli.project.clone())?;
            let catalog = Catalog::locate()?;
            let report = doctor::diagnose(&project, &catalog)?;
            if cli.json {
                println!("{}", serde_json::to_string(&report)?);
            } else {
                print_doctor_report(&report);
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
            to,
            id,
        } => {
            let catalog = Catalog::locate()?;
            let report = if let Some(to) = to.as_deref() {
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
            if cli.json {
                println!("{}", serde_json::to_string(&report)?);
            } else {
                print_update_report(&report, *check);
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
            id,
        } => {
            let project = Project::locate(cli.project.clone())?;
            let catalog = Catalog::locate()?;
            let ctx = resolve_install_harnesses(harnesses, &project)?;
            if *dry_run {
                let preview =
                    install::plan_install(&project, &catalog, item_type(*agent), id, &ctx)?;
                if cli.json {
                    println!("{}", serde_json::to_string(&preview)?);
                } else {
                    print_install_preview(&preview);
                }
            } else {
                let report = install::install(&project, &catalog, item_type(*agent), id, &ctx)?;
                if cli.json {
                    println!("{}", serde_json::to_string(&report)?);
                } else {
                    print_install_report(&project, &report);
                }
            }
        }
        Commands::Uninstall {
            agent,
            harnesses,
            id,
        } => {
            let project = Project::locate(cli.project.clone())?;
            let scope = if harnesses.is_empty() {
                RemoveScope::All
            } else {
                RemoveScope::Harnesses(parse_harnesses(harnesses)?)
            };
            let report = install::remove(&project, item_type(*agent), id, scope)?;
            if cli.json {
                println!("{}", serde_json::to_string(&report)?);
            } else {
                print_uninstall_report(&report);
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

fn print_install_report(project: &Project, report: &install::InstallReport) {
    if report.not_a_git_repo {
        warn_not_git(project);
    }
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
    print_reload_guidance(report.item_type, &report.harnesses);
}

/// Print post-install reload/restart guidance per served harness.
///
/// Agents have per-harness reload data in the registry; skills do not yet, so
/// they get one honest, harness-agnostic hint.
fn print_reload_guidance(item_type: ItemType, harnesses: &[HarnessId]) {
    if harnesses.is_empty() {
        return;
    }
    println!("reload:");
    match item_type {
        ItemType::Agent => {
            for h in harnesses {
                let target = akit::harness::agent_target(*h);
                println!("  {} agent: {}", h.as_str(), target.reload.guidance());
            }
        }
        ItemType::Skill => {
            println!(
                "  skills: start a new session (or run your harness's skills-reload command) if it does not appear"
            );
        }
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
    if !preview.create.is_empty() {
        println!("  create:");
        for m in &preview.create {
            println!(
                "    {}  ({})  [{}]",
                m.path,
                covers_str(&m.covers),
                mode_name(m.mode)
            );
        }
    }
    if !preview.unchanged.is_empty() {
        println!("  unchanged:");
        for m in &preview.unchanged {
            println!("    {}  ({})", m.path, covers_str(&m.covers));
        }
    }
    if !preview.remove.is_empty() {
        println!("  remove (reshape):");
        for p in &preview.remove {
            println!("    {p}");
        }
    }
    if !preview.issues.is_empty() {
        println!("  skipped:");
        for issue in &preview.issues {
            println!("    {}: {}", issue.harness.as_str(), issue.reason.message());
        }
    }
    println!("(dry run — nothing changed; re-run without --dry-run to apply)");
}

fn covers_str(covers: &[HarnessId]) -> String {
    covers
        .iter()
        .map(|h| h.as_str())
        .collect::<Vec<_>>()
        .join(", ")
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

fn status_name(status: HealthStatus) -> &'static str {
    match status {
        HealthStatus::Ok => "ok",
        HealthStatus::Orphaned => "orphaned",
        HealthStatus::Missing => "missing",
        HealthStatus::Drifted => "drifted",
    }
}

fn created_name(mode: Mode) -> &'static str {
    match mode {
        Mode::Symlink => "linked",
        Mode::Copy => "copied",
    }
}

fn already_present_name(mode: Mode) -> &'static str {
    match mode {
        Mode::Symlink => "already linked",
        Mode::Copy => "already present",
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

fn add_report_line(report: &ops::AddReport) -> String {
    let action = if report.link_created {
        created_name(report.mode)
    } else {
        already_present_name(report.mode)
    };
    format!(
        "Added {} '{}' -> {} ({action})",
        type_name(report.item_type),
        report.id,
        report.target
    )
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

fn remove_report_line(report: &ops::RemoveReport) -> String {
    let removed = if report.target_removed {
        "removed"
    } else {
        "target already missing"
    };
    format!(
        "Removed {} '{}' -> {} ({removed})",
        type_name(report.item_type),
        report.id,
        report.target
    )
}

/// JSON shape for `status`: installed items plus per-bundle completeness.
#[derive(serde::Serialize)]
struct StatusReport<'a> {
    items: &'a [ListItem],
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

fn print_table(items: &[ListItem]) {
    let mut bundle_width = "BUNDLE".len();
    let mut type_width = "TYPE".len();
    let mut id_width = "ID".len();
    let mut mode_width = "MODE".len();
    let mut target_width = "TARGET".len();

    for item in items {
        bundle_width = bundle_width.max(item.bundle.as_deref().unwrap_or("-").len());
        type_width = type_width.max(type_name(item.item_type).len());
        id_width = id_width.max(item.id.len());
        mode_width = mode_width.max(mode_name(item.mode).len());
        target_width = target_width.max(item.target.len());
    }

    println!(
        "{:<bundle_width$}  {:<type_width$}  {:<id_width$}  {:<mode_width$}  {:<target_width$}  STATUS",
        "BUNDLE", "TYPE", "ID", "MODE", "TARGET"
    );
    let mut ordered: Vec<&ListItem> = items.iter().collect();
    ordered.sort_by(|a, b| match (a.bundle.as_deref(), b.bundle.as_deref()) {
        (Some(a_bundle), Some(b_bundle)) => a_bundle.cmp(b_bundle),
        (Some(_), None) => std::cmp::Ordering::Less,
        (None, Some(_)) => std::cmp::Ordering::Greater,
        (None, None) => std::cmp::Ordering::Equal,
    });
    for item in ordered {
        println!(
            "{:<bundle_width$}  {:<type_width$}  {:<id_width$}  {:<mode_width$}  {:<target_width$}  {}",
            item.bundle.as_deref().unwrap_or("-"),
            type_name(item.item_type),
            item.id,
            mode_name(item.mode),
            item.target,
            status_name(item.status)
        );
    }
}

fn print_doctor_report(report: &DoctorReport) {
    print_doctor_table(report);
    print_bundle_health(&report.bundles);
    print_exclude_health(report);
    if report.summary.healthy {
        println!("Health: ok");
    } else {
        println!(
            "Health: {} issue(s): {} orphaned, {} missing, {} drifted, {} missing exclude, {} stale exclude",
            report.summary.total - report.summary.ok
                + report.summary.missing_exclude_lines
                + report.summary.stale_exclude_lines,
            report.summary.orphaned,
            report.summary.missing,
            report.summary.drifted,
            report.summary.missing_exclude_lines,
            report.summary.stale_exclude_lines
        );
    }
}

fn print_doctor_table(report: &DoctorReport) {
    let mut bundle_width = "BUNDLE".len();
    let mut type_width = "TYPE".len();
    let mut id_width = "ID".len();
    let mut mode_width = "MODE".len();
    let mut target_width = "TARGET".len();

    for item in &report.items {
        bundle_width = bundle_width.max(item.bundle.as_deref().unwrap_or("-").len());
        type_width = type_width.max(type_name(item.item_type).len());
        id_width = id_width.max(item.id.len());
        mode_width = mode_width.max(mode_name(item.mode).len());
        target_width = target_width.max(item.target.len());
    }

    println!(
        "{:<bundle_width$}  {:<type_width$}  {:<id_width$}  {:<mode_width$}  {:<target_width$}  {:<8}  EXCLUDE",
        "BUNDLE", "TYPE", "ID", "MODE", "TARGET", "STATUS"
    );
    let mut ordered: Vec<_> = report.items.iter().collect();
    ordered.sort_by(|a, b| match (a.bundle.as_deref(), b.bundle.as_deref()) {
        (Some(a_bundle), Some(b_bundle)) => a_bundle.cmp(b_bundle),
        (Some(_), None) => std::cmp::Ordering::Less,
        (None, Some(_)) => std::cmp::Ordering::Greater,
        (None, None) => std::cmp::Ordering::Equal,
    });
    for item in ordered {
        println!(
            "{:<bundle_width$}  {:<type_width$}  {:<id_width$}  {:<mode_width$}  {:<target_width$}  {:<8}  {}",
            item.bundle.as_deref().unwrap_or("-"),
            type_name(item.item_type),
            item.id,
            mode_name(item.mode),
            item.target,
            status_name(item.status),
            exclude_status_name(report.exclude.checked, item.exclude_present)
        );
    }
}

fn exclude_status_name(checked: bool, present: bool) -> &'static str {
    if !checked {
        "n/a"
    } else if present {
        "present"
    } else {
        "missing"
    }
}

fn print_exclude_health(report: &DoctorReport) {
    if !report.exclude.checked {
        println!("Exclude: not checked (not a git repository)");
        return;
    }
    if report.exclude.missing.is_empty() && report.exclude.stale.is_empty() {
        println!("Exclude: ok");
        return;
    }
    if !report.exclude.missing.is_empty() {
        println!("Missing exclude lines:");
        for line in &report.exclude.missing {
            println!("  {line}");
        }
    }
    if !report.exclude.stale.is_empty() {
        println!("Stale exclude lines (not removed):");
        for line in &report.exclude.stale {
            println!("  {line}");
        }
    }
}

fn print_sync_report(report: &SyncReport) {
    let mut printed = false;
    for item in &report.items {
        if item.restored {
            println!(
                "Restored {} '{}' -> {} ({})",
                type_name(item.item_type),
                item.id,
                item.target,
                mode_name(item.mode)
            );
            printed = true;
        }
        if item.exclude_added {
            println!("Added exclude /{}", item.target);
            printed = true;
        }
        if item.skipped_orphan {
            println!(
                "Skipped orphaned {} '{}' -> {} (source missing or unsafe to overwrite)",
                type_name(item.item_type),
                item.id,
                item.target
            );
            printed = true;
        }
        if item.drifted {
            println!(
                "Drifted {} '{}' -> {} (not overwritten)",
                type_name(item.item_type),
                item.id,
                item.target
            );
            printed = true;
        }
    }
    if report.exclude.lockfile_added {
        println!("Added exclude /.copilot/kit.lock.json");
        printed = true;
    }
    if !report.exclude.stale.is_empty() {
        println!("Stale exclude lines (not removed):");
        for line in &report.exclude.stale {
            println!("  {line}");
        }
        printed = true;
    }
    if !report.exclude.missing_after.is_empty() {
        println!("Missing exclude lines remaining:");
        for line in &report.exclude.missing_after {
            println!("  {line}");
        }
        printed = true;
    }
    if !printed {
        println!("Already in sync");
    }
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
/// invalid package as `disabled`, or `-` for skills and legacy flat agents.
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
