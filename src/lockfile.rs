//! The per-project lockfile: the record of what has been pulled into a project.
//!
//! Path: `<project>/.copilot/kit.lock.json` (itself gitignored). Schema (frozen by #1):
//! ```json
//! { "version": 1, "items": [
//!   { "id", "type": "skill|agent", "source": "local|<owner/repo/path>",
//!     "ref"?, "mode": "symlink|copy", "target", "bundle"? } ] }
//! ```

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::path::Path;

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

/// A single pulled item recorded in the lockfile.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LockItem {
    /// Item name, e.g. `deploy-to-vercel`.
    pub id: String,
    #[serde(rename = "type")]
    pub item_type: ItemType,
    /// `local` for the local catalog, or an `owner/repo/path` source spec (future).
    pub source: String,
    /// Source ref (branch/tag/sha), when applicable.
    #[serde(rename = "ref", skip_serializing_if = "Option::is_none", default)]
    pub git_ref: Option<String>,
    pub mode: Mode,
    /// Project-relative target path, e.g. `.github/skills/<name>`.
    pub target: String,
    /// Bundle this item was installed as part of, if any.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub bundle: Option<String>,
}

fn default_version() -> u32 {
    1
}

/// The lockfile document.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Lockfile {
    #[serde(default = "default_version")]
    pub version: u32,
    #[serde(default)]
    pub items: Vec<LockItem>,
}

impl Default for Lockfile {
    fn default() -> Self {
        Self {
            version: 1,
            items: Vec::new(),
        }
    }
}

impl Lockfile {
    /// Load the lockfile, returning an empty default if it does not exist.
    pub fn load(path: &Path) -> Result<Self> {
        if !path.exists() {
            return Ok(Self::default());
        }
        let data = std::fs::read_to_string(path)
            .with_context(|| format!("reading lockfile {}", path.display()))?;
        if data.trim().is_empty() {
            return Ok(Self::default());
        }
        serde_json::from_str(&data).with_context(|| format!("parsing lockfile {}", path.display()))
    }

    /// Persist the lockfile (pretty-printed, trailing newline), creating parent dirs.
    pub fn save(&self, path: &Path) -> Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("creating {}", parent.display()))?;
        }
        let mut s = serde_json::to_string_pretty(self).context("serializing lockfile")?;
        s.push('\n');
        std::fs::write(path, s).with_context(|| format!("writing lockfile {}", path.display()))?;
        Ok(())
    }

    /// Insert or replace the item identified by `(item_type, id)`.
    /// Returns `true` if it was newly added, `false` if it replaced an existing entry.
    pub fn upsert(&mut self, item: LockItem) -> bool {
        if let Some(existing) = self
            .items
            .iter_mut()
            .find(|i| i.item_type == item.item_type && i.id == item.id)
        {
            *existing = item;
            false
        } else {
            self.items.push(item);
            true
        }
    }

    /// Remove the item identified by `(item_type, id)`, returning it if present.
    pub fn remove(&mut self, item_type: ItemType, id: &str) -> Option<LockItem> {
        self.items
            .iter()
            .position(|i| i.item_type == item_type && i.id == id)
            .map(|pos| self.items.remove(pos))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn upsert_is_keyed_by_type_and_id() {
        let mut lf = Lockfile::default();
        let item = LockItem {
            id: "demo".into(),
            item_type: ItemType::Skill,
            source: "local".into(),
            git_ref: None,
            mode: Mode::Symlink,
            target: ".github/skills/demo".into(),
            bundle: None,
        };
        assert!(lf.upsert(item.clone()));
        assert!(!lf.upsert(item.clone())); // replace, not add
        assert_eq!(lf.items.len(), 1);
        assert!(lf.remove(ItemType::Skill, "demo").is_some());
        assert!(lf.items.is_empty());
    }

    #[test]
    fn serializes_ref_as_ref_and_omits_none() {
        let lf = Lockfile {
            version: 1,
            items: vec![LockItem {
                id: "demo".into(),
                item_type: ItemType::Skill,
                source: "local".into(),
                git_ref: None,
                mode: Mode::Symlink,
                target: ".github/skills/demo".into(),
                bundle: None,
            }],
        };
        let json = serde_json::to_string(&lf).unwrap();
        assert!(json.contains("\"type\":\"skill\""));
        assert!(json.contains("\"mode\":\"symlink\""));
        assert!(!json.contains("\"ref\""));
        assert!(!json.contains("\"bundle\""));
    }
}
