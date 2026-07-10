//! Harness-aware catalog contracts (issue #35).
//!
//! Extends the catalog with two harness-aware shapes that the install planner
//! (#32) and materializer (#31) consume:
//!
//! 1. **Native agent variant packages** — a directory `agents/<id>/` containing
//!    an `agent.yml` descriptor plus one native file per harness the agent
//!    supports. akit copies a variant's bytes **verbatim** into that harness's
//!    proprietary destination; it never converts one format into another. The
//!    descriptor maps each [`HarnessId`] to a source file inside the package and
//!    declares only a destination *basename* — the [`crate::harness`] registry
//!    owns the destination directory and extension.
//!
//! 2. **Skill compatibility metadata** — an optional `skill.yml` beside a
//!    skill's `SKILL.md`. A skill is portable by default (works on every
//!    harness); `skill.yml` may narrow that to an explicit allow-list when a
//!    skill genuinely depends on a subset of harnesses.
//!
//! This is a clean break from the legacy flat `agents/<name>.agent.md` file: an
//! agent is now always a package with explicit per-harness variants. There is no
//! transformation and no implicit single-format agent.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use serde::Deserialize;

use crate::harness::{self, AgentFormat, HarnessId};

/// Descriptor filename inside an agent package directory.
pub const AGENT_DESCRIPTOR: &str = "agent.yml";
/// Optional skill compatibility descriptor beside `SKILL.md`.
pub const SKILL_DESCRIPTOR: &str = "skill.yml";

/// Raw `agent.yml` as authored in the catalog. Field validation happens in
/// [`AgentPackage::load`]; this is only the deserialization shape.
#[derive(Debug, Clone, Deserialize)]
struct RawAgentDescriptor {
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    description: Option<String>,
    #[serde(default)]
    category: Option<String>,
    /// Destination filename stem for every harness (defaults to the package id).
    #[serde(default)]
    basename: Option<String>,
    /// Map of harness id → source file (relative to the package directory).
    #[serde(default)]
    variants: BTreeMap<String, String>,
}

/// A single harness's native variant within an agent package.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentVariant {
    /// The harness this variant targets.
    pub harness: HarnessId,
    /// Source file, project/package-relative (the file that gets copied).
    pub source_file: String,
    /// The native on-disk format this file is authored in (from the registry).
    pub format: AgentFormat,
}

/// A validated native-agent package resolved from `agents/<id>/`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentPackage {
    /// Catalog id (the package directory name).
    pub id: String,
    /// Frontmatter-style display name (defaults to `id`).
    pub name: String,
    /// Short description for search/preview.
    pub description: String,
    /// Optional category for search/preview.
    pub category: String,
    /// Destination filename stem used for every harness's materialized file.
    pub basename: String,
    /// Absolute package directory.
    pub dir: PathBuf,
    /// One variant per harness the agent supports, keyed by harness.
    pub variants: BTreeMap<HarnessId, AgentVariant>,
}

impl AgentPackage {
    /// Harnesses this agent provides a native variant for.
    pub fn supported_harnesses(&self) -> impl Iterator<Item = HarnessId> + '_ {
        self.variants.keys().copied()
    }

    /// Whether the agent can be installed for `harness`.
    pub fn supports(&self, harness: HarnessId) -> bool {
        self.variants.contains_key(&harness)
    }

    /// The absolute source file for `harness`'s variant, if provided.
    pub fn source_path(&self, harness: HarnessId) -> Option<PathBuf> {
        self.variants
            .get(&harness)
            .map(|v| self.dir.join(&v.source_file))
    }

    /// Load and validate the package rooted at `dir` (the `agents/<id>` dir).
    pub fn load(id: &str, dir: &Path) -> Result<Self> {
        let descriptor = dir.join(AGENT_DESCRIPTOR);
        let text = std::fs::read_to_string(&descriptor).with_context(|| {
            format!(
                "agent package '{id}' is missing {} ({})",
                AGENT_DESCRIPTOR,
                descriptor.display()
            )
        })?;
        let raw: RawAgentDescriptor = serde_yaml::from_str(&text)
            .with_context(|| format!("parsing {}", descriptor.display()))?;

        if raw.variants.is_empty() {
            bail!(
                "agent package '{id}' declares no variants in {} — an agent must \
                 provide at least one harness variant",
                descriptor.display()
            );
        }

        let basename = raw.basename.unwrap_or_else(|| id.to_string());
        validate_basename(id, &basename)?;

        let mut variants = BTreeMap::new();
        for (raw_harness, source_file) in &raw.variants {
            let harness: HarnessId = raw_harness
                .parse()
                .with_context(|| format!("agent package '{id}' variant key"))?;

            // The variant file must live inside the package (no traversal / abs paths).
            validate_variant_path(id, raw_harness, source_file)?;
            let source = dir.join(source_file);
            if !source.is_file() {
                bail!(
                    "agent package '{id}' variant '{raw_harness}' points at missing file '{source_file}' ({})",
                    source.display()
                );
            }

            let target = harness::agent_target(harness);
            variants.insert(
                harness,
                AgentVariant {
                    harness,
                    source_file: source_file.clone(),
                    format: target.format,
                },
            );
        }

        Ok(Self {
            id: id.to_string(),
            name: raw.name.unwrap_or_else(|| id.to_string()),
            description: raw.description.unwrap_or_default(),
            category: raw.category.unwrap_or_default(),
            basename,
            dir: dir.to_path_buf(),
            variants,
        })
    }
}

/// Skill compatibility, loaded from an optional `skill.yml`. A skill is portable
/// (all harnesses) unless it declares an explicit `harnesses` allow-list.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SkillCompat {
    /// Works on every harness (default when no `skill.yml`, or an empty list).
    Portable,
    /// Only compatible with the listed harnesses.
    Only(Vec<HarnessId>),
}

#[derive(Debug, Clone, Deserialize)]
struct RawSkillDescriptor {
    #[serde(default)]
    harnesses: Vec<String>,
}

impl SkillCompat {
    /// Whether the skill may be installed for `harness`.
    pub fn allows(&self, harness: HarnessId) -> bool {
        match self {
            SkillCompat::Portable => true,
            SkillCompat::Only(list) => list.contains(&harness),
        }
    }

    /// The concrete set of harnesses this skill supports, given the full
    /// supported set.
    pub fn resolve(&self, all: &[HarnessId]) -> Vec<HarnessId> {
        all.iter().copied().filter(|h| self.allows(*h)).collect()
    }

    /// Load compatibility from a skill directory. Absence of `skill.yml` (or an
    /// empty `harnesses` list) means [`SkillCompat::Portable`].
    pub fn load(skill_dir: &Path) -> Result<Self> {
        let descriptor = skill_dir.join(SKILL_DESCRIPTOR);
        let text = match std::fs::read_to_string(&descriptor) {
            Ok(text) => text,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(SkillCompat::Portable),
            Err(e) => {
                return Err(e).with_context(|| format!("reading {}", descriptor.display()));
            }
        };
        let raw: RawSkillDescriptor = serde_yaml::from_str(&text)
            .with_context(|| format!("parsing {}", descriptor.display()))?;
        if raw.harnesses.is_empty() {
            return Ok(SkillCompat::Portable);
        }
        let mut list = Vec::new();
        for token in &raw.harnesses {
            let harness: HarnessId = token
                .parse()
                .with_context(|| format!("skill compat in {}", descriptor.display()))?;
            if !list.contains(&harness) {
                list.push(harness);
            }
        }
        Ok(SkillCompat::Only(list))
    }
}

fn validate_basename(id: &str, basename: &str) -> Result<()> {
    if basename.is_empty()
        || basename.contains('/')
        || basename.contains('\\')
        || basename.contains("..")
    {
        bail!("agent package '{id}' has an invalid basename '{basename}'");
    }
    Ok(())
}

fn validate_variant_path(id: &str, harness: &str, path: &str) -> Result<()> {
    let invalid = path.is_empty()
        || path.starts_with('/')
        || path.starts_with('\\')
        || Path::new(path).components().any(|c| {
            matches!(
                c,
                std::path::Component::ParentDir | std::path::Component::RootDir
            )
        });
    if invalid {
        bail!(
            "agent package '{id}' variant '{harness}' has an unsafe file path '{path}' \
             (must be relative and inside the package)"
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn write(path: &Path, contents: &str) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, contents).unwrap();
    }

    fn agent_dir(tmp: &TempDir, id: &str) -> PathBuf {
        tmp.path().join("agents").join(id)
    }

    #[test]
    fn loads_multi_harness_agent_package() {
        let tmp = TempDir::new().unwrap();
        let dir = agent_dir(&tmp, "reviewer");
        write(
            &dir.join("copilot.agent.md"),
            "---\nname: reviewer\n---\nbody",
        );
        write(&dir.join("claude.md"), "---\nname: reviewer\n---\nbody");
        write(
            &dir.join("codex.toml"),
            "developer_instructions = \"review\"\n",
        );
        write(
            &dir.join(AGENT_DESCRIPTOR),
            "name: Reviewer\ndescription: Reviews code\ncategory: quality\n\
             variants:\n  copilot: copilot.agent.md\n  claude: claude.md\n  codex: codex.toml\n",
        );

        let pkg = AgentPackage::load("reviewer", &dir).unwrap();
        assert_eq!(pkg.name, "Reviewer");
        assert_eq!(pkg.description, "Reviews code");
        assert_eq!(pkg.basename, "reviewer");
        assert!(pkg.supports(HarnessId::Copilot));
        assert!(pkg.supports(HarnessId::Claude));
        assert!(pkg.supports(HarnessId::Codex));
        assert!(!pkg.supports(HarnessId::Gemini));
        assert_eq!(pkg.variants[&HarnessId::Codex].format, AgentFormat::Toml);
        assert_eq!(
            pkg.source_path(HarnessId::Claude).unwrap(),
            dir.join("claude.md")
        );
    }

    #[test]
    fn basename_defaults_to_package_id() {
        let tmp = TempDir::new().unwrap();
        let dir = agent_dir(&tmp, "deployer");
        write(&dir.join("claude.md"), "body");
        write(
            &dir.join(AGENT_DESCRIPTOR),
            "variants:\n  claude: claude.md\n",
        );
        let pkg = AgentPackage::load("deployer", &dir).unwrap();
        assert_eq!(pkg.basename, "deployer");
        assert_eq!(pkg.name, "deployer");
    }

    #[test]
    fn rejects_package_without_variants() {
        let tmp = TempDir::new().unwrap();
        let dir = agent_dir(&tmp, "empty");
        write(&dir.join(AGENT_DESCRIPTOR), "name: Empty\nvariants: {}\n");
        let err = AgentPackage::load("empty", &dir).unwrap_err();
        assert!(err.to_string().contains("no variants"), "{err}");
    }

    #[test]
    fn rejects_unknown_harness_variant_key() {
        let tmp = TempDir::new().unwrap();
        let dir = agent_dir(&tmp, "bad");
        write(&dir.join("x.md"), "body");
        write(&dir.join(AGENT_DESCRIPTOR), "variants:\n  cursor: x.md\n");
        let err = AgentPackage::load("bad", &dir).unwrap_err();
        assert!(format!("{err:#}").contains("cursor"), "{err:#}");
    }

    #[test]
    fn rejects_missing_variant_file() {
        let tmp = TempDir::new().unwrap();
        let dir = agent_dir(&tmp, "gone");
        write(
            &dir.join(AGENT_DESCRIPTOR),
            "variants:\n  claude: nope.md\n",
        );
        let err = AgentPackage::load("gone", &dir).unwrap_err();
        assert!(err.to_string().contains("missing file"), "{err}");
    }

    #[test]
    fn rejects_variant_path_traversal() {
        let tmp = TempDir::new().unwrap();
        let dir = agent_dir(&tmp, "evil");
        write(
            &dir.join(AGENT_DESCRIPTOR),
            "variants:\n  claude: ../escape.md\n",
        );
        let err = AgentPackage::load("evil", &dir).unwrap_err();
        assert!(err.to_string().contains("unsafe file path"), "{err}");
    }

    #[test]
    fn rejects_missing_descriptor() {
        let tmp = TempDir::new().unwrap();
        let dir = agent_dir(&tmp, "nodesc");
        std::fs::create_dir_all(&dir).unwrap();
        let err = AgentPackage::load("nodesc", &dir).unwrap_err();
        assert!(err.to_string().contains(AGENT_DESCRIPTOR), "{err}");
    }

    #[test]
    fn skill_is_portable_without_descriptor() {
        let tmp = TempDir::new().unwrap();
        let dir = tmp.path().join("skills").join("deploy");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("SKILL.md"), "---\nname: deploy\n---\n").unwrap();
        let compat = SkillCompat::load(&dir).unwrap();
        assert_eq!(compat, SkillCompat::Portable);
        for h in HarnessId::ALL {
            assert!(compat.allows(h));
        }
    }

    #[test]
    fn skill_compat_narrows_to_allow_list() {
        let tmp = TempDir::new().unwrap();
        let dir = tmp.path().join("skills").join("clauded");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("SKILL.md"), "body").unwrap();
        std::fs::write(
            dir.join(SKILL_DESCRIPTOR),
            "harnesses:\n  - claude\n  - opencode\n",
        )
        .unwrap();
        let compat = SkillCompat::load(&dir).unwrap();
        assert!(compat.allows(HarnessId::Claude));
        assert!(compat.allows(HarnessId::Opencode));
        assert!(!compat.allows(HarnessId::Copilot));
        assert_eq!(
            compat.resolve(&HarnessId::ALL),
            vec![HarnessId::Claude, HarnessId::Opencode]
        );
    }

    #[test]
    fn empty_skill_harness_list_is_portable() {
        let tmp = TempDir::new().unwrap();
        let dir = tmp.path().join("skills").join("x");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join(SKILL_DESCRIPTOR), "harnesses: []\n").unwrap();
        assert_eq!(SkillCompat::load(&dir).unwrap(), SkillCompat::Portable);
    }

    #[test]
    fn skill_compat_rejects_unknown_harness() {
        let tmp = TempDir::new().unwrap();
        let dir = tmp.path().join("skills").join("x");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join(SKILL_DESCRIPTOR), "harnesses:\n  - nope\n").unwrap();
        assert!(SkillCompat::load(&dir).is_err());
    }
}
