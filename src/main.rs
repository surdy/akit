//! ckit CLI — a thin wrapper over the `ckit` engine.

use anyhow::Result;
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
        /// Name of the item to add.
        name: String,
    },
    /// Remove a skill or agent from this project.
    Rm {
        /// Remove an agent (`agents/<name>.agent.md`) instead of a skill.
        #[arg(long)]
        agent: bool,
        /// Name of the item to remove.
        name: String,
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
        Commands::Add { agent, copy, name } => {
            let project = Project::locate(cli.project.clone())?;
            let collection = Collection::locate()?;
            let item_type = item_type(*agent);
            let mode = if *copy { Mode::Copy } else { Mode::Symlink };
            let report = ops::add_item(&project, &collection, item_type, name, mode)?;
            if cli.json {
                println!("{}", serde_json::to_string(&report)?);
            } else {
                if report.not_a_git_repo {
                    eprintln!(
                        "warning: {} is not a git repository; pulled files will NOT be git-ignored",
                        project.root.display()
                    );
                }
                let action = if report.link_created {
                    created_name(report.mode)
                } else {
                    already_present_name(report.mode)
                };
                println!(
                    "Added {} '{}' -> {} ({action})",
                    type_name(report.item_type),
                    report.id,
                    report.target
                );
            }
        }
        Commands::Rm { agent, name } => {
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
                let removed = if report.target_removed {
                    "removed"
                } else {
                    "target already missing"
                };
                println!(
                    "Removed {} '{}' -> {} ({removed})",
                    type_name(report.item_type),
                    report.id,
                    report.target
                );
            }
        }
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

fn print_table(items: &[ListItem]) {
    let mut type_width = "TYPE".len();
    let mut id_width = "ID".len();
    let mut mode_width = "MODE".len();
    let mut target_width = "TARGET".len();

    for item in items {
        type_width = type_width.max(type_name(item.item_type).len());
        id_width = id_width.max(item.id.len());
        mode_width = mode_width.max(mode_name(item.mode).len());
        target_width = target_width.max(item.target.len());
    }

    println!(
        "{:<type_width$}  {:<id_width$}  {:<mode_width$}  {:<target_width$}  STATUS",
        "TYPE", "ID", "MODE", "TARGET"
    );
    for item in items {
        println!(
            "{:<type_width$}  {:<id_width$}  {:<mode_width$}  {:<target_width$}  {}",
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
