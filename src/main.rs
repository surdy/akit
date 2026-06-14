//! ckit CLI — a thin wrapper over the `ckit` engine.

use anyhow::{Result, bail};
use clap::{Parser, Subcommand};
use std::path::PathBuf;

use ckit::collection::Collection;
use ckit::doctor;
use ckit::doctor::{DoctorReport, SyncReport};
use ckit::lockfile::{ItemType, Mode};
use ckit::ops;
use ckit::ops::{HealthStatus, ListItem};
use ckit::project::Project;
use ckit::remote::{self, SourceSpec};
use ckit::search::{self, SearchHit};
use ckit::show;

#[derive(Parser)]
#[command(
    name = "ckit",
    version,
    about = "Copilot Kit — on-demand personal Copilot customizations"
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
    /// Pull a skill or agent from your collection, or owner/repo/path[#ref], into this project.
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
    /// List installed items and their health.
    #[command(alias = "status")]
    Ls,
    /// Repair missing materializations and git-exclude lines from the lockfile.
    Sync,
    /// Read-only reconcile report for lockfile, files, and git-exclude lines.
    Doctor,
    /// Search your collection by skill/agent frontmatter.
    Search {
        /// Query to fuzzy-match against name and description (empty lists everything).
        query: Option<String>,
    },
    /// Print a read-only preview of a collection item (frontmatter + content).
    Show {
        /// Show an agent (`agents/<id>.agent.md`) instead of a skill.
        #[arg(long)]
        agent: bool,
        /// Item id: a skill directory name, or an agent file stem.
        id: String,
    },
    /// Fetch a remote owner/repo/path[#ref] source into your local collection.
    Pull {
        /// Pull an agent (`.agent.md`) instead of a skill.
        #[arg(long)]
        agent: bool,
        /// Store under this id instead of the source's last path segment.
        #[arg(long = "as")]
        as_id: Option<String>,
        /// Overwrite an existing collection item that differs from the source.
        #[arg(long)]
        force: bool,
        /// Remote source: owner/repo/path[#ref].
        source: String,
    },
    /// Re-fetch every remote item recorded in the collection manifest (apm.yml).
    Restore {
        /// Overwrite collection items that differ from their recorded source.
        #[arg(long)]
        force: bool,
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
                    let collection = Collection::locate()?;
                    let report = ops::add_bundle(&project, &collection, bundle, mode)?;
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
                        let collection = Collection::locate()?;
                        ops::add_item(&project, &collection, item_type(*agent), name, mode, None)?
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
            let project = Project::locate(cli.project.clone())?;
            let items = ops::list_items(&project)?;
            if cli.json {
                println!("{}", serde_json::to_string(&items)?);
            } else {
                print_table(&items);
            }
        }
        Commands::Sync => {
            let project = Project::locate(cli.project.clone())?;
            let collection = Collection::locate()?;
            let report = doctor::sync(&project, &collection)?;
            if cli.json {
                println!("{}", serde_json::to_string(&report)?);
            } else {
                print_sync_report(&report);
            }
        }
        Commands::Doctor => {
            let project = Project::locate(cli.project.clone())?;
            let collection = Collection::locate()?;
            let report = doctor::diagnose(&project, &collection)?;
            if cli.json {
                println!("{}", serde_json::to_string(&report)?);
            } else {
                print_doctor_report(&report);
            }
        }
        Commands::Search { query } => {
            let collection = Collection::locate()?;
            let hits = search::search(&collection, query.as_deref().unwrap_or_default())?;
            if cli.json {
                println!("{}", serde_json::to_string(&hits)?);
            } else {
                print_search_hits(&hits);
            }
        }
        Commands::Show { agent, id } => {
            let collection = Collection::locate()?;
            let preview = show::show(&collection, id, item_type(*agent))?;
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
            let collection = Collection::locate()?;
            let report = ops::pull_into_collection(
                &collection,
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
        Commands::Restore { force } => {
            let collection = Collection::locate()?;
            let report = ops::restore_collection(&collection, &remote_base_url(), *force)?;
            if cli.json {
                println!("{}", serde_json::to_string(&report)?);
            } else {
                print_restore_report(&report);
            }
            if report.summary.errors > 0 {
                std::process::exit(1);
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
        println!("Nothing to restore; collection manifest has no remote items.");
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
    println!("{}", preview.path.display());
    println!();
    print!("{}", preview.content);
    if !preview.content.ends_with('\n') {
        println!();
    }
}
