//! Optional local `.akit/config.json` defaults (issue #34).
//!
//! A project may record default target harnesses so `akit install <id>` does not
//! need `--harness` every time. This is the lowest-priority source in the CLI's
//! harness resolution (flags > `AKIT_HARNESSES` env > this config > interactive
//! prompt).

use std::path::Path;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use crate::harness::HarnessId;

/// The `.akit/config.json` document. All fields optional.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct LocalConfig {
    /// Default target harnesses for installs when none are specified.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub harnesses: Vec<HarnessId>,
}

impl LocalConfig {
    /// Load config from `path`, or a default (empty) config when absent.
    pub fn load(path: &Path) -> Result<Self> {
        match std::fs::read_to_string(path) {
            Ok(text) => {
                serde_json::from_str(&text).with_context(|| format!("parsing {}", path.display()))
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Self::default()),
            Err(e) => Err(e).with_context(|| format!("reading {}", path.display())),
        }
    }

    /// The default harnesses, deduped in registry order.
    pub fn default_harnesses(&self) -> Vec<HarnessId> {
        HarnessId::ALL
            .into_iter()
            .filter(|h| self.harnesses.contains(h))
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn absent_config_is_empty() {
        let tmp = TempDir::new().unwrap();
        let cfg = LocalConfig::load(&tmp.path().join("config.json")).unwrap();
        assert!(cfg.harnesses.is_empty());
        assert!(cfg.default_harnesses().is_empty());
    }

    #[test]
    fn loads_default_harnesses_in_registry_order() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("config.json");
        std::fs::write(&path, r#"{"harnesses":["claude","copilot"]}"#).unwrap();
        let cfg = LocalConfig::load(&path).unwrap();
        // Registry order (copilot before claude) regardless of file order.
        assert_eq!(
            cfg.default_harnesses(),
            vec![HarnessId::Copilot, HarnessId::Claude]
        );
    }

    #[test]
    fn rejects_unknown_harness_in_config() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("config.json");
        std::fs::write(&path, r#"{"harnesses":["cursor"]}"#).unwrap();
        assert!(LocalConfig::load(&path).is_err());
    }
}
