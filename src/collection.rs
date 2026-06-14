//! The collection: a local directory holding the canonical set of skills and agents.
//!
//! Layout (shared contract, frozen by issue #1):
//! ```text
//! $KIT_COLLECTION_DIR/          (default ~/.akit/collection)
//!   skills/<name>/SKILL.md
//!   agents/<name>.agent.md
//! ```

use anyhow::{Context, Result, bail};
use std::path::PathBuf;

/// Environment variable that overrides the collection location.
pub const ENV_COLLECTION_DIR: &str = "KIT_COLLECTION_DIR";

/// A handle to the on-disk collection.
pub struct Collection {
    pub root: PathBuf,
}

impl Collection {
    /// Locate the collection from `$KIT_COLLECTION_DIR`, falling back to
    /// `~/.akit/collection`.
    pub fn locate() -> Result<Self> {
        let root = match std::env::var_os(ENV_COLLECTION_DIR) {
            Some(v) if !v.is_empty() => PathBuf::from(v),
            _ => {
                let home = dirs::home_dir().context("could not determine home directory")?;
                home.join(".akit").join("collection")
            }
        };
        Ok(Self { root })
    }

    /// Construct a collection rooted at an explicit path (used in tests / by callers).
    pub fn with_root(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    /// Path to a skill's source directory (may not exist).
    pub fn skill_source(&self, name: &str) -> PathBuf {
        self.root.join("skills").join(name)
    }

    /// Path to an agent's source file (may not exist).
    pub fn agent_source(&self, name: &str) -> PathBuf {
        self.root.join("agents").join(format!("{name}.agent.md"))
    }

    /// Resolve a skill by name, validating it exists and has a `SKILL.md`.
    pub fn resolve_skill(&self, name: &str) -> Result<PathBuf> {
        let dir = self.skill_source(name);
        if !dir.is_dir() {
            bail!(
                "skill '{name}' not found in collection (looked in {})",
                dir.display()
            );
        }
        let skill_md = dir.join("SKILL.md");
        if !skill_md.is_file() {
            bail!(
                "skill '{name}' is missing SKILL.md ({})",
                skill_md.display()
            );
        }
        Ok(dir)
    }

    /// Resolve an agent by name, validating `<name>.agent.md` exists.
    pub fn resolve_agent(&self, name: &str) -> Result<PathBuf> {
        let file = self.agent_source(name);
        if !file.is_file() {
            bail!(
                "agent '{name}' not found in collection (looked in {})",
                file.display()
            );
        }
        Ok(file)
    }
}
