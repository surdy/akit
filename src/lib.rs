//! akit — agent kit core engine.
//!
//! On-demand personal agent customizations: pull skills / custom agents from a central
//! catalog into a project's `.github/{skills,agents}` via symlink (copy fallback),
//! keep them personal + gitignored (`.git/info/exclude`), tracked by a per-project lockfile.
//!
//! This crate is the harness-agnostic engine; the CLI in `main.rs` and any GUI (pterm) are
//! thin wrappers over the operations in [`ops`].

pub mod agentpkg;
pub mod bundle;
pub mod catalog;
pub mod doctor;
pub mod fsops;
pub mod gitexclude;
pub mod harness;
pub mod lockfile;
pub mod manifest;
pub mod ops;
pub mod ownership;
pub mod plan;
pub mod project;
pub mod remote;
pub mod search;
pub mod show;
