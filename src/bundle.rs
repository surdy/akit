//! Bundle manifests: named sets of skills and agents in a catalog.
//!
//! A bundle lives at `<catalog>/bundles/<name>.yml` and uses this schema:
//!
//! ```yaml
//! skills: [deploy-to-vercel, lint-fix]
//! agents: [code-reviewer]
//! ```
//!
//! Either key may be omitted and is treated as an empty list. Loading validates every referenced
//! item exists before any add operation starts, so a bad bundle fails as a whole.

use anyhow::{Context, Result, bail};
use serde::Deserialize;

use crate::catalog::Catalog;
use crate::lockfile::ItemType;

/// One bundle item resolved from a manifest.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BundleItem {
    pub item_type: ItemType,
    pub id: String,
}

/// A loaded bundle manifest.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Bundle {
    pub name: String,
    pub items: Vec<BundleItem>,
}

#[derive(Debug, Default, Deserialize)]
struct Manifest {
    #[serde(default)]
    skills: Vec<String>,
    #[serde(default)]
    agents: Vec<String>,
}

/// Load and validate `<catalog>/bundles/<name>.yml`.
pub fn load(catalog: &Catalog, name: &str) -> Result<Bundle> {
    let path = catalog.root.join("bundles").join(format!("{name}.yml"));
    if !path.is_file() {
        bail!(
            "bundle '{name}' not found in catalog (looked in {})",
            path.display()
        );
    }

    let data = std::fs::read_to_string(&path)
        .with_context(|| format!("reading bundle manifest {}", path.display()))?;
    let manifest: Manifest = serde_yaml::from_str(&data)
        .with_context(|| format!("parsing bundle manifest {}", path.display()))?;

    let mut items = Vec::with_capacity(manifest.skills.len() + manifest.agents.len());
    for skill in manifest.skills {
        catalog
            .resolve_skill(&skill)
            .with_context(|| format!("bundle '{name}' references skill '{skill}'"))?;
        items.push(BundleItem {
            item_type: ItemType::Skill,
            id: skill,
        });
    }
    for agent in manifest.agents {
        // Accept either on-disk shape: a legacy flat `agents/<id>.agent.md` file
        // or a harness-aware `agents/<id>/` package. The existence check is
        // engine-agnostic; each engine resolves its own shape when installing.
        if catalog.resolve_agent(&agent).is_err() {
            catalog
                .resolve_agent_package(&agent)
                .with_context(|| format!("bundle '{name}' references agent '{agent}'"))?;
        }
        items.push(BundleItem {
            item_type: ItemType::Agent,
            id: agent,
        });
    }

    Ok(Bundle {
        name: name.to_string(),
        items,
    })
}
