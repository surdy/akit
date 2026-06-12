//! ckit — Copilot Kit core engine.
//!
//! On-demand personal Copilot customizations: pull skills / custom agents from a central
//! collection into a project's `.github/{skills,agents}` via symlink (copy fallback),
//! keep them personal + gitignored (`.git/info/exclude`), tracked by a per-project lockfile.
//!
//! This crate is the harness-agnostic engine; the CLI in `main.rs` and any GUI (pterm) are
//! thin wrappers over the operations in [`ops`].

pub mod collection;
pub mod fsops;
pub mod gitexclude;
pub mod lockfile;
pub mod ops;
pub mod project;
