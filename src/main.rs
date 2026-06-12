//! ckit CLI — a thin wrapper over the `ckit` engine.

use anyhow::Result;
use clap::{Parser, Subcommand};
use std::path::PathBuf;

use ckit::collection::Collection;
use ckit::ops;
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
    let collection = Collection::locate()?;

    match &cli.command {
        Commands::Add { name } => {
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
                println!("Added skill '{}' -> {} ({link})", report.id, report.target);
            }
        }
    }
    Ok(())
}
