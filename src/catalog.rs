//! The catalog: a local directory holding the canonical set of skills and agents.
//!
//! Layout (shared contract, frozen by issue #1):
//! ```text
//! $KIT_CATALOG_DIR/          (default ~/.akit/catalog)
//!   skills/<name>/SKILL.md
//!   agents/<name>/agent.yml   (+ one native variant file per harness)
//! ```

use anyhow::{Context, Result, bail};
use std::path::PathBuf;

/// Environment variable that overrides the catalog location.
pub const ENV_CATALOG_DIR: &str = "KIT_CATALOG_DIR";

/// Filename suffix of the removed legacy flat catalog agent shape
/// (`agents/<id>.agent.md`). Retained only to recognize and report leftovers.
pub const LEGACY_FLAT_SUFFIX: &str = ".agent.md";

/// A handle to the on-disk catalog.
pub struct Catalog {
    pub root: PathBuf,
}

impl Catalog {
    /// Locate the catalog from `$KIT_CATALOG_DIR`, falling back to
    /// `~/.akit/catalog`.
    pub fn locate() -> Result<Self> {
        let root = match std::env::var_os(ENV_CATALOG_DIR) {
            Some(v) if !v.is_empty() => PathBuf::from(v),
            _ => {
                let home = dirs::home_dir().context("could not determine home directory")?;
                home.join(".akit").join("catalog")
            }
        };
        Ok(Self { root })
    }

    /// Construct a catalog rooted at an explicit path (used in tests / by callers).
    pub fn with_root(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    /// Path to a skill's source directory (may not exist).
    pub fn skill_source(&self, name: &str) -> PathBuf {
        self.root.join("skills").join(name)
    }

    /// Resolve a skill by name, validating it exists and has a `SKILL.md`.
    pub fn resolve_skill(&self, name: &str) -> Result<PathBuf> {
        let dir = self.skill_source(name);
        if !dir.is_dir() {
            bail!(
                "skill '{name}' not found in catalog (looked in {})",
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

    /// Path to a native-agent package directory (`agents/<id>/`, may not exist).
    pub fn agent_package_dir(&self, id: &str) -> PathBuf {
        self.root.join("agents").join(id)
    }

    /// Resolve a harness-aware native-agent package by id (#35), validating its
    /// `agent.yml` descriptor and every declared variant file.
    pub fn resolve_agent_package(&self, id: &str) -> Result<crate::agentpkg::AgentPackage> {
        let dir = self.agent_package_dir(id);
        if !dir.is_dir() {
            bail!(
                "agent package '{id}' not found in catalog (looked in {})",
                dir.display()
            );
        }
        crate::agentpkg::AgentPackage::load(id, &dir)
    }

    /// Load a skill's harness compatibility (#35). Portable when no `skill.yml`.
    pub fn skill_compat(&self, name: &str) -> Result<crate::agentpkg::SkillCompat> {
        crate::agentpkg::SkillCompat::load(&self.skill_source(name))
    }

    /// Discover every agent **package** in the catalog (`agents/<id>/` holding an
    /// `agent.yml`), sorted by id.
    ///
    /// This is the single source of truth for the read/browse surface
    /// (`ls` / `search` / `show`), so a package is never invisible to it.
    ///
    /// Legacy flat `agents/<id>.agent.md` files are **not** a catalog shape any
    /// more (removed in v0.32.0). They are skipped here rather than listed, but
    /// a one-line note naming them is written to stderr — silently dropping items
    /// a user pulled before would look like data loss, whereas a stderr note
    /// keeps `--json` stdout free of entries no command can act on.
    pub fn discover_agents(&self) -> Result<Vec<String>> {
        let dir = self.root.join("agents");
        let entries = match std::fs::read_dir(&dir) {
            Ok(entries) => entries,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(e) => return Err(e).with_context(|| format!("reading {}", dir.display())),
        };
        let mut ids = Vec::new();
        let mut legacy = Vec::new();
        for entry in entries {
            let entry = entry.with_context(|| format!("reading {}", dir.display()))?;
            let path = entry.path();
            let name = entry.file_name().to_string_lossy().into_owned();
            if path.is_dir() {
                if path.join(crate::agentpkg::AGENT_DESCRIPTOR).is_file() {
                    ids.push(name);
                }
            } else if let Some(id) = name.strip_suffix(LEGACY_FLAT_SUFFIX) {
                legacy.push(id.to_string());
            }
        }
        ids.sort();
        if !legacy.is_empty() {
            legacy.sort();
            eprintln!(
                "warning: ignoring {} legacy flat agent file(s) in {} ({}). \
                 Flat `.agent.md` agents are no longer a catalog shape — convert each to \
                 `agents/<id>/agent.yml` plus a native variant file.",
                legacy.len(),
                dir.display(),
                legacy.join(", ")
            );
        }
        Ok(ids)
    }
}
