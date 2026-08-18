//! Shared item primitives: what kind of customization an item is (`ItemType`)
//! and how it is materialized into a project (`Mode`). Used across the catalog
//! and harness-aware engines. (The harness-aware ownership record lives in
//! [`crate::ownership`]; the legacy `.copilot` lockfile has been retired.)

use serde::{Deserialize, Serialize};

/// The kind of customization an item represents.
///
/// The derived ordering is the one every listing sorts by — skills before
/// agents — so declaration order is load-bearing: `ls`, `search`, and the
/// divergence sweep all rely on it instead of re-deriving a private rank.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ItemType {
    Skill,
    Agent,
}

/// How an item was materialized into the project.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Mode {
    Symlink,
    Copy,
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Declaration order is the sort order every listing uses (`ls`, `search`,
    /// the divergence sweep), so pin it here rather than in each caller.
    #[test]
    fn item_type_sorts_skills_before_agents() {
        assert!(ItemType::Skill < ItemType::Agent);
        let mut v = vec![ItemType::Agent, ItemType::Skill, ItemType::Agent];
        v.sort();
        assert_eq!(v, vec![ItemType::Skill, ItemType::Agent, ItemType::Agent]);
    }
}
