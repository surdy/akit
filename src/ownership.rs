//! The `.akit` ownership schema (issue #32).
//!
//! The harness-aware successor to the legacy `.copilot/kit.lock.json`. It records
//! each **logical installation** — a catalog item plus the set of harnesses it
//! was installed for — together with the **physical materializations** that back
//! it (the concrete files, their coverage sets, mode, and a content hash used
//! for copy drift detection by the materializer, #31).
//!
//! Path: `<project>/.akit/kit.lock.json` (local-only, git-excluded). This is a
//! clean break from the v1 schema — there is no in-place migration.

use std::path::Path;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use crate::harness::HarnessId;
use crate::lockfile::{ItemType, Mode};
use crate::transport::{FsTransport, LocalFs};

/// Current on-disk schema version for `.akit/kit.lock.json`.
pub const AKIT_LOCKFILE_VERSION: u32 = 2;

/// Project-relative path of the `.akit` lockfile, for the git-exclude entry.
pub const AKIT_LOCKFILE_REL: &str = ".akit/kit.lock.json";

/// One physical file/directory backing a logical installation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MaterializationRecord {
    /// Project-relative path of the materialized skill dir or agent file.
    pub path: String,
    /// How it was materialized.
    pub mode: Mode,
    /// The selected harnesses this materialization makes the item discoverable
    /// by (sorted). Non-empty.
    pub covers: Vec<HarnessId>,
    /// Content hash recorded at materialization time, used to detect drift for
    /// copies. `None` for symlinks (which reflect the source directly).
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub hash: Option<String>,
}

/// A single logical installation: an item installed for a set of harnesses.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Installation {
    /// Catalog id, e.g. `deploy-to-vercel` or `reviewer`.
    pub id: String,
    #[serde(rename = "type")]
    pub item_type: ItemType,
    /// `local` for the local catalog, or an `owner/repo/path` source spec.
    pub source: String,
    /// Source ref (branch/tag/sha), when applicable.
    #[serde(rename = "ref", skip_serializing_if = "Option::is_none", default)]
    pub git_ref: Option<String>,
    /// Bundle this item was installed as part of, if any.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub bundle: Option<String>,
    /// The harnesses this item is logically installed for (sorted, deduped).
    pub harnesses: Vec<HarnessId>,
    /// The physical materializations backing the installation.
    pub materializations: Vec<MaterializationRecord>,
}

impl Installation {
    /// Whether this installation currently serves `harness`.
    pub fn serves(&self, harness: HarnessId) -> bool {
        self.harnesses.contains(&harness)
    }
}

fn default_version() -> u32 {
    AKIT_LOCKFILE_VERSION
}

/// The `.akit` lockfile document.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AkitLockfile {
    #[serde(default = "default_version")]
    pub version: u32,
    #[serde(default)]
    pub items: Vec<Installation>,
}

impl Default for AkitLockfile {
    fn default() -> Self {
        Self {
            version: AKIT_LOCKFILE_VERSION,
            items: Vec::new(),
        }
    }
}

impl AkitLockfile {
    /// Load the lockfile at `path`, or a fresh empty document if absent.
    ///
    /// A version mismatch is a hard error rather than a silent reset: the caller
    /// must decide how to handle an unknown schema (there is no v1 migration).
    pub fn load(path: &Path) -> Result<Self> {
        Self::load_with(&LocalFs, path)
    }

    /// [`load`] against an explicit transport (for embedding hosts / remote roots).
    pub fn load_with(fs: &dyn FsTransport, path: &Path) -> Result<Self> {
        if !fs.exists(path)? {
            return Ok(Self::default());
        }
        let bytes = fs
            .read(path)
            .with_context(|| format!("reading {}", path.display()))?;
        let text = String::from_utf8(bytes)
            .with_context(|| format!("{} is not valid UTF-8", path.display()))?;
        let doc: AkitLockfile =
            serde_json::from_str(&text).with_context(|| format!("parsing {}", path.display()))?;
        if doc.version != AKIT_LOCKFILE_VERSION {
            anyhow::bail!(
                "{} is schema version {} but this akit understands version {AKIT_LOCKFILE_VERSION}",
                path.display(),
                doc.version
            );
        }
        Ok(doc)
    }

    /// Persist the lockfile to `path`, creating the `.akit` directory as needed.
    /// Written pretty + trailing newline for readable diffs.
    pub fn save(&self, path: &Path) -> Result<()> {
        self.save_with(&LocalFs, path)
    }

    /// [`save`] against an explicit transport.
    pub fn save_with(&self, fs: &dyn FsTransport, path: &Path) -> Result<()> {
        let mut text = serde_json::to_string_pretty(self).context("serializing lockfile")?;
        text.push('\n');
        fs.write(path, text.as_bytes())
            .with_context(|| format!("writing {}", path.display()))
    }

    /// The installation matching `(item_type, id)`, if present.
    pub fn get(&self, item_type: ItemType, id: &str) -> Option<&Installation> {
        self.items
            .iter()
            .find(|i| i.item_type == item_type && i.id == id)
    }

    /// Insert or replace the installation keyed by `(item_type, id)`.
    ///
    /// Returns `true` if this replaced an existing entry, `false` if it was new.
    pub fn upsert(&mut self, installation: Installation) -> bool {
        if let Some(slot) = self
            .items
            .iter_mut()
            .find(|i| i.item_type == installation.item_type && i.id == installation.id)
        {
            *slot = installation;
            true
        } else {
            self.items.push(installation);
            false
        }
    }

    /// Remove the installation matching `(item_type, id)`. Returns the removed
    /// entry so the caller can clean up its materializations.
    pub fn remove(&mut self, item_type: ItemType, id: &str) -> Option<Installation> {
        let idx = self
            .items
            .iter()
            .position(|i| i.item_type == item_type && i.id == id)?;
        Some(self.items.remove(idx))
    }

    /// All materialization paths owned by any installation. Used by cleanup and
    /// orphan detection to know which files akit is responsible for.
    pub fn owned_paths(&self) -> Vec<&str> {
        self.items
            .iter()
            .flat_map(|i| i.materializations.iter().map(|m| m.path.as_str()))
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn sample() -> Installation {
        Installation {
            id: "deploy".to_string(),
            item_type: ItemType::Skill,
            source: "local".to_string(),
            git_ref: None,
            bundle: None,
            harnesses: vec![HarnessId::Copilot, HarnessId::Claude],
            materializations: vec![MaterializationRecord {
                path: ".claude/skills/deploy".to_string(),
                mode: Mode::Copy,
                covers: vec![HarnessId::Copilot, HarnessId::Claude],
                hash: Some("abc123".to_string()),
            }],
        }
    }

    #[test]
    fn roundtrips_through_disk() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join(".akit").join("kit.lock.json");
        let mut lock = AkitLockfile::default();
        lock.upsert(sample());
        lock.save(&path).unwrap();

        let loaded = AkitLockfile::load(&path).unwrap();
        assert_eq!(loaded, lock);
        assert_eq!(loaded.version, AKIT_LOCKFILE_VERSION);
    }

    #[test]
    fn absent_lockfile_loads_as_empty() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join(".akit").join("kit.lock.json");
        let loaded = AkitLockfile::load(&path).unwrap();
        assert!(loaded.items.is_empty());
    }

    #[test]
    fn version_mismatch_is_hard_error() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("kit.lock.json");
        std::fs::write(&path, r#"{"version":1,"items":[]}"#).unwrap();
        let err = AkitLockfile::load(&path).unwrap_err();
        assert!(err.to_string().contains("schema version 1"), "{err}");
    }

    #[test]
    fn upsert_replaces_by_type_and_id() {
        let mut lock = AkitLockfile::default();
        assert!(!lock.upsert(sample()));
        let mut updated = sample();
        updated.harnesses = vec![HarnessId::Claude];
        assert!(lock.upsert(updated.clone()));
        assert_eq!(lock.items.len(), 1);
        assert_eq!(lock.get(ItemType::Skill, "deploy").unwrap(), &updated);
    }

    #[test]
    fn same_id_different_type_coexist() {
        let mut lock = AkitLockfile::default();
        lock.upsert(sample());
        let mut agent = sample();
        agent.item_type = ItemType::Agent;
        lock.upsert(agent);
        assert_eq!(lock.items.len(), 2);
        assert!(lock.get(ItemType::Skill, "deploy").is_some());
        assert!(lock.get(ItemType::Agent, "deploy").is_some());
    }

    #[test]
    fn remove_returns_entry_for_cleanup() {
        let mut lock = AkitLockfile::default();
        lock.upsert(sample());
        let removed = lock.remove(ItemType::Skill, "deploy").unwrap();
        assert_eq!(removed.materializations.len(), 1);
        assert!(lock.items.is_empty());
        assert!(lock.remove(ItemType::Skill, "deploy").is_none());
    }

    #[test]
    fn owned_paths_lists_every_materialization() {
        let mut lock = AkitLockfile::default();
        lock.upsert(sample());
        let mut second = sample();
        second.id = "other".to_string();
        second.materializations[0].path = ".agents/skills/other".to_string();
        lock.upsert(second);
        let owned = lock.owned_paths();
        assert!(owned.contains(&".claude/skills/deploy"));
        assert!(owned.contains(&".agents/skills/other"));
    }

    #[test]
    fn symlink_materialization_omits_hash_in_json() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("kit.lock.json");
        let mut lock = AkitLockfile::default();
        let mut inst = sample();
        inst.materializations[0].mode = Mode::Symlink;
        inst.materializations[0].hash = None;
        lock.upsert(inst);
        lock.save(&path).unwrap();
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(!text.contains("hash"), "{text}");
    }
}
