//! Shared item primitives: what kind of customization an item is (`ItemType`)
//! and how it is materialized into a project (`Mode`). Used across the catalog
//! and harness-aware engines. (The harness-aware ownership record lives in
//! [`crate::ownership`]; the legacy `.copilot` lockfile has been retired.)

use serde::{Deserialize, Serialize};

/// The kind of customization an item represents.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
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
