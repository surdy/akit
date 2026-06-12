//! Bundle manifests: named sets of skills and agents in a collection.
//!
//! A bundle lives at `<collection>/bundles/<name>.yml` and uses this schema:
//!
//! ```yaml
//! skills: [deploy-to-vercel, lint-fix]
//! agents: [code-reviewer]
//! ```
//!
//! Either key may be omitted and is treated as an empty list. Loading validates every referenced
//! item exists before any add operation starts, so a bad bundle fails as a whole.

use anyhow::{bail, Context, Result};
use serde::Deserialize;

use crate::collection::Collection;
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

/// Load and validate `<collection>/bundles/<name>.yml`.
pub fn load(collection: &Collection, name: &str) -> Result<Bundle> {
    let path = collection.root.join("bundles").join(format!("{name}.yml"));
    if !path.is_file() {
        bail!(
            "bundle '{name}' not found in collection (looked in {})",
            path.display()
        );
    }

    let data = std::fs::read_to_string(&path)
        .with_context(|| format!("reading bundle manifest {}", path.display()))?;
    let manifest: Manifest = serde_yaml::from_str(&data)
        .with_context(|| format!("parsing bundle manifest {}", path.display()))?;

    let mut items = Vec::with_capacity(manifest.skills.len() + manifest.agents.len());
    for skill in manifest.skills {
        collection
            .resolve_skill(&skill)
            .with_context(|| format!("bundle '{name}' references skill '{skill}'"))?;
        items.push(BundleItem {
            item_type: ItemType::Skill,
            id: skill,
        });
    }
    for agent in manifest.agents {
        collection
            .resolve_agent(&agent)
            .with_context(|| format!("bundle '{name}' references agent '{agent}'"))?;
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
