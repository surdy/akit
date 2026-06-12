//! ckit CLI — a thin wrapper over the `ckit` engine.

use anyhow::Result;
use clap::{Parser, Subcommand};
use std::path::PathBuf;

use ckit::collection::Collection;
use ckit::lockfile::{ItemType, Mode};
use ckit::ops;
use ckit::ops::{HealthStatus, ListItem};
use ckit::project::Project;

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
    /// Pull a skill from your collection into this project.
    Add {
        /// Name of the skill to add.
        name: String,
    },
    /// Remove a skill from this project.
    Rm {
        /// Name of the skill to remove.
        name: String,
    },
    /// List installed items and their health.
    #[command(alias = "status")]
    Ls,
}

fn main() {
    if let Err(e) = run() {
        eprintln!("error: {e:#}");
        std::process::exit(1);
    }
}

fn run() -> Result<()> {
    let cli = Cli::parse();
    let project = Project::locate(cli.project.clone())?;

    match &cli.command {
        Commands::Add { name } => {
            let collection = Collection::locate()?;
            let report = ops::add_skill(&project, &collection, name)?;
            if cli.json {
                println!("{}", serde_json::to_string(&report)?);
            } else {
                if report.not_a_git_repo {
                    eprintln!(
                        "warning: {} is not a git repository; pulled files will NOT be git-ignored",
                        project.root.display()
                    );
                }
                let link = if report.link_created {
                    "linked"
                } else {
                    "already linked"
                };
                println!(
                    "Added {} '{}' -> {} ({link})",
                    type_name(report.item_type),
                    report.id,
                    report.target
                );
            }
        }
        Commands::Rm { name } => {
            let report = ops::remove_skill(&project, name)?;
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
            let items = ops::list_items(&project)?;
            if cli.json {
                println!("{}", serde_json::to_string(&items)?);
            } else {
                print_table(&items);
            }
        }
    }
    Ok(())
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
