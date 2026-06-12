//! ckit CLI — a thin wrapper over the `ckit` engine.

use anyhow::{Result, bail};
use clap::{Parser, Subcommand};
use std::path::PathBuf;

use ckit::collection::Collection;
use ckit::lockfile::{ItemType, Mode};
use ckit::ops;
use ckit::ops::{HealthStatus, ListItem};
use ckit::project::Project;
use ckit::search::{self, SearchHit};

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
    /// Pull a skill or agent from your collection into this project.
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
        /// Name of the item to add.
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
    /// Search your collection by skill/agent frontmatter.
    Search {
        /// Query to fuzzy-match against name and description (empty lists everything).
        query: Option<String>,
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
                    let collection = Collection::locate()?;
                    let report =
                        ops::add_item(&project, &collection, item_type(*agent), name, mode, None)?;
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
        Commands::Search { query } => {
            let collection = Collection::locate()?;
            let hits = search::search(&collection, query.as_deref().unwrap_or_default())?;
            if cli.json {
                println!("{}", serde_json::to_string(&hits)?);
            } else {
                print_search_hits(&hits);
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
