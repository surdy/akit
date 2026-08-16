//! The catalog: a local directory holding the canonical set of skills and agents.
//!
//! Layout (shared contract, frozen by issue #1):
//! ```text
//! $KIT_CATALOG_DIR/          (default ~/.akit/catalog)
//!   skills/<name>/SKILL.md
//!   agents/<name>.agent.md
//! ```

use anyhow::{Context, Result, bail};
use std::path::PathBuf;

/// Environment variable that overrides the catalog location.
pub const ENV_CATALOG_DIR: &str = "KIT_CATALOG_DIR";

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

    /// Path to an agent's source file (may not exist).
    pub fn agent_source(&self, name: &str) -> PathBuf {
        self.root.join("agents").join(format!("{name}.agent.md"))
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

    /// Resolve an agent by name, validating `<name>.agent.md` exists.
    pub fn resolve_agent(&self, name: &str) -> Result<PathBuf> {
        let file = self.agent_source(name);
        if !file.is_file() {
            bail!(
                "agent '{name}' not found in catalog (looked in {})",
                file.display()
            );
        }
        Ok(file)
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

    /// Discover every agent in the catalog, across **both** on-disk shapes: legacy
    /// flat files (`agents/<id>.agent.md`) and harness-aware packages
    /// (`agents/<id>/` with an `agent.yml`). When both exist for one id, the
    /// package wins — it is the target contract. Results are sorted by id.
    ///
    /// This is the single source of truth for the read/browse surface
    /// (`ls` / `search` / `show`) so packages are never invisible to it.
    pub fn discover_agents(&self) -> Result<Vec<DiscoveredAgent>> {
        use std::collections::BTreeMap;
        let dir = self.root.join("agents");
        let entries = match std::fs::read_dir(&dir) {
            Ok(entries) => entries,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(e) => return Err(e).with_context(|| format!("reading {}", dir.display())),
        };
        // id → shape; a package overwrites a flat file of the same id, and a flat
        // file never overwrites a package (`or_insert`), so package always wins.
        let mut found: BTreeMap<String, AgentShape> = BTreeMap::new();
        for entry in entries {
            let entry = entry.with_context(|| format!("reading {}", dir.display()))?;
            let path = entry.path();
            let name = entry.file_name().to_string_lossy().into_owned();
            if path.is_dir() {
                if path.join(crate::agentpkg::AGENT_DESCRIPTOR).is_file() {
                    found.insert(name, AgentShape::Package);
                }
            } else if let Some(id) = name.strip_suffix(".agent.md") {
                found
                    .entry(id.to_string())
                    .or_insert(AgentShape::Flat(path));
            }
        }
        Ok(found
            .into_iter()
            .map(|(id, shape)| DiscoveredAgent { id, shape })
            .collect())
    }
}

/// The on-disk shape of a discovered catalog agent.
pub enum AgentShape {
    /// Legacy single file `agents/<id>.agent.md` (Copilot-shaped). Carries its path.
    Flat(PathBuf),
    /// Harness-aware package directory `agents/<id>/` (load via
    /// [`Catalog::resolve_agent_package`]).
    Package,
}

/// One agent discovered by [`Catalog::discover_agents`].
pub struct DiscoveredAgent {
    pub id: String,
    pub shape: AgentShape,
}
