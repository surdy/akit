//! Catalog-level manifest in an APM-compatible format, stored as `akit.yml`.
//!
//! Records the remote provenance of items pulled into the catalog so a new machine can be
//! rebootstrapped with `akit restore`. Only remotely-sourced items are tracked; hand-authored
//! skills/agents are content you keep under your own version control.
//!
//! The on-disk shape mirrors APM's [`apm.yml`](https://github.com/microsoft/apm) manifest (akit
//! stores it under its own filename, `akit.yml`):
//!
//! ```yaml
//! name: akit-catalog
//! version: 0.0.0
//! dependencies:
//!   apm:
//!     - vercel-labs/agent-skills/deploy-to-vercel#main   # skill (string shorthand)
//!     - acme/kits/reviewer.agent.md#main                 # agent (.agent.md file primitive)
//!     - git: acme/kits/deploy                            # custom id via object form
//!       ref: main
//!       alias: vercel
//! ```
//!
//! akit owns the `dependencies.apm` list; all other keys (`name`, `author`, `mcp`, …) are
//! preserved verbatim across rewrites (comments are not preserved).

use anyhow::{Context, Result};
use serde_yaml::{Mapping, Value};
use std::path::{Path, PathBuf};

use crate::catalog::Catalog;
use crate::lockfile::ItemType;
use crate::remote::SourceSpec;

/// Manifest filename at the catalog root (APM-compatible contents).
pub const MANIFEST_FILE: &str = "akit.yml";

/// Default manifest `name` when scaffolding a fresh file.
const DEFAULT_NAME: &str = "akit-catalog";
/// Default manifest `version` when scaffolding a fresh file.
const DEFAULT_VERSION: &str = "0.0.0";

/// A resolved manifest entry: a remote source plus the catalog id/type it materializes as.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ManifestEntry {
    pub spec: SourceSpec,
    pub item_type: ItemType,
    /// Catalog id the item is stored under (after any `--as`).
    pub id: String,
    /// Commit SHA the symbolic ref resolved to at pull/update time, when known.
    ///
    /// Recorded so `restore` can reproduce the exact commit and `update` can report precise
    /// `old → new` diffs. `None` for legacy entries written before SHA recording and for
    /// items that could not be resolved to a commit.
    pub commit: Option<String>,
}

/// Path to the catalog manifest (may not exist).
pub fn manifest_path(catalog: &Catalog) -> PathBuf {
    catalog.root.join(MANIFEST_FILE)
}

/// Whether the catalog has a manifest on disk.
pub fn exists(catalog: &Catalog) -> bool {
    manifest_path(catalog).is_file()
}

/// Record (upsert) a remote item in the catalog manifest, keyed by `(type, id)`.
pub fn record(catalog: &Catalog, entry: &ManifestEntry) -> Result<()> {
    let path = manifest_path(catalog);
    let mut root = load_value(&path)?;
    if root.is_null() {
        root = Value::Mapping(Mapping::new());
    }
    let map = root
        .as_mapping_mut()
        .with_context(|| format!("manifest root must be a mapping ({})", path.display()))?;

    scaffold(map);

    let deps = ensure_mapping(map, "dependencies")?;
    let apm = ensure_sequence(deps, "apm")?;
    apm.retain(|value| match parse_entry(value) {
        Some(existing) => !(existing.item_type == entry.item_type && existing.id == entry.id),
        None => true,
    });
    apm.push(entry_to_value(entry));

    write_value(&path, &root)
}

/// Remove the manifest entry matching `(item_type, id)`, if present.
///
/// Returns whether an entry was removed. Other keys and entries are preserved.
pub fn remove(catalog: &Catalog, item_type: ItemType, id: &str) -> Result<bool> {
    let path = manifest_path(catalog);
    let mut root = load_value(&path)?;
    let Some(map) = root.as_mapping_mut() else {
        return Ok(false);
    };
    let Some(apm) = map
        .get_mut("dependencies")
        .and_then(Value::as_mapping_mut)
        .and_then(|deps| deps.get_mut("apm"))
        .and_then(Value::as_sequence_mut)
    else {
        return Ok(false);
    };
    let before = apm.len();
    apm.retain(|value| match parse_entry(value) {
        Some(existing) => !(existing.item_type == item_type && existing.id == id),
        None => true,
    });
    if apm.len() == before {
        return Ok(false);
    }
    write_value(&path, &root)?;
    Ok(true)
}

/// Read all remote items recorded in the catalog manifest.
pub fn entries(catalog: &Catalog) -> Result<Vec<ManifestEntry>> {
    let root = load_value(&manifest_path(catalog))?;
    let mut out = Vec::new();
    let Some(map) = root.as_mapping() else {
        return Ok(out);
    };
    let Some(apm) = map
        .get("dependencies")
        .and_then(Value::as_mapping)
        .and_then(|deps| deps.get("apm"))
        .and_then(Value::as_sequence)
    else {
        return Ok(out);
    };
    for value in apm {
        match parse_entry(value) {
            Some(entry) => out.push(entry),
            None => eprintln!(
                "warning: skipping unrecognized manifest entry: {}",
                serde_yaml::to_string(value).unwrap_or_default().trim()
            ),
        }
    }
    Ok(out)
}

fn load_value(path: &Path) -> Result<Value> {
    match std::fs::read_to_string(path) {
        Ok(text) => serde_yaml::from_str(&text)
            .with_context(|| format!("parsing manifest {}", path.display())),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Value::Mapping(Mapping::new())),
        Err(e) => Err(e).with_context(|| format!("reading manifest {}", path.display())),
    }
}

fn write_value(path: &Path, value: &Value) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("creating {}", parent.display()))?;
    }
    let text = serde_yaml::to_string(value).context("serializing manifest")?;
    std::fs::write(path, text).with_context(|| format!("writing manifest {}", path.display()))
}

fn scaffold(map: &mut Mapping) {
    if map.get("name").is_none() {
        map.insert(string("name"), string(DEFAULT_NAME));
    }
    if map.get("version").is_none() {
        map.insert(string("version"), string(DEFAULT_VERSION));
    }
}

fn ensure_mapping<'a>(map: &'a mut Mapping, key: &str) -> Result<&'a mut Mapping> {
    if map.get(key).is_none() {
        map.insert(string(key), Value::Mapping(Mapping::new()));
    }
    map.get_mut(key)
        .expect("just inserted")
        .as_mapping_mut()
        .with_context(|| format!("`{key}` must be a mapping"))
}

fn ensure_sequence<'a>(map: &'a mut Mapping, key: &str) -> Result<&'a mut Vec<Value>> {
    if map.get(key).is_none() {
        map.insert(string(key), Value::Sequence(Vec::new()));
    }
    map.get_mut(key)
        .expect("just inserted")
        .as_sequence_mut()
        .with_context(|| format!("`{key}` must be a sequence"))
}

/// Canonical repo path for an entry, with the `.agent.md` suffix forced for agents so the
/// APM file-primitive classification round-trips.
fn canonical_path(entry: &ManifestEntry) -> String {
    let path = &entry.spec.path;
    if entry.item_type == ItemType::Agent && !path.ends_with(".agent.md") {
        format!("{path}.agent.md")
    } else {
        path.clone()
    }
}

fn entry_to_value(entry: &ManifestEntry) -> Value {
    let path = canonical_path(entry);
    let default_id = default_id_for(entry.item_type, &path);
    let is_default_id = entry.id == default_id;

    // String shorthand stays the canonical form when there is nothing extra to record: the id is
    // the default and no resolved commit needs persisting. A recorded commit forces the object
    // form because a single string can't carry both the symbolic ref and the commit.
    if is_default_id && entry.commit.is_none() {
        let base = format!("{}/{}/{}", entry.spec.owner, entry.spec.repo, path);
        let shorthand = match &entry.spec.ref_ {
            Some(git_ref) => format!("{base}#{git_ref}"),
            None => base,
        };
        return Value::String(shorthand);
    }

    // Object form: APM `git`/`path`/`ref`, plus our `commit` and (for `--as`) `alias`.
    let mut object = Mapping::new();
    object.insert(
        string("git"),
        string(&format!("{}/{}", entry.spec.owner, entry.spec.repo)),
    );
    object.insert(string("path"), string(&path));
    if let Some(git_ref) = &entry.spec.ref_ {
        object.insert(string("ref"), string(git_ref));
    }
    if let Some(commit) = &entry.commit {
        object.insert(string("commit"), string(commit));
    }
    if !is_default_id {
        object.insert(string("alias"), string(&entry.id));
    }
    Value::Mapping(object)
}

fn parse_entry(value: &Value) -> Option<ManifestEntry> {
    match value {
        Value::String(shorthand) => {
            let spec = SourceSpec::parse(shorthand)?;
            let item_type = type_from_path(&spec.path);
            let id = default_id(item_type, &spec);
            Some(ManifestEntry {
                spec,
                item_type,
                id,
                commit: None,
            })
        }
        Value::Mapping(object) => {
            let git = object.get("git").and_then(Value::as_str)?;
            let path = object.get("path").and_then(Value::as_str);
            let git_ref = object
                .get("ref")
                .and_then(Value::as_str)
                .map(str::to_string);
            let commit = object
                .get("commit")
                .and_then(Value::as_str)
                .map(str::to_string);
            let alias = object
                .get("alias")
                .and_then(Value::as_str)
                .map(str::to_string);
            let source = match path {
                Some(path) => format!(
                    "{}/{}",
                    git.trim_end_matches('/'),
                    path.trim_start_matches('/')
                ),
                None => git.to_string(),
            };
            let spec = SourceSpec::from_source_and_ref(&source, git_ref)?;
            let item_type = type_from_path(&spec.path);
            let id = alias.unwrap_or_else(|| default_id(item_type, &spec));
            Some(ManifestEntry {
                spec,
                item_type,
                id,
                commit,
            })
        }
        _ => None,
    }
}

fn type_from_path(path: &str) -> ItemType {
    let leaf = path.rsplit('/').next().unwrap_or(path);
    if leaf.ends_with(".agent.md") {
        ItemType::Agent
    } else {
        ItemType::Skill
    }
}

fn default_id(item_type: ItemType, spec: &SourceSpec) -> String {
    default_id_for(item_type, spec.path.as_str())
}

fn default_id_for(item_type: ItemType, path: &str) -> String {
    let leaf = path.rsplit('/').next().unwrap_or(path);
    match item_type {
        ItemType::Agent => leaf.strip_suffix(".agent.md").unwrap_or(leaf).to_string(),
        ItemType::Skill => leaf.to_string(),
    }
}

fn string(value: &str) -> Value {
    Value::String(value.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn catalog(tmp: &TempDir) -> Catalog {
        Catalog::with_root(tmp.path().join("catalog"))
    }

    fn spec(s: &str) -> SourceSpec {
        SourceSpec::parse(s).unwrap()
    }

    #[test]
    fn skill_roundtrips_as_string_shorthand() {
        let tmp = TempDir::new().unwrap();
        let c = catalog(&tmp);
        let entry = ManifestEntry {
            spec: spec("acme/repo/deploy#main"),
            item_type: ItemType::Skill,
            id: "deploy".to_string(),
            commit: None,
        };
        record(&c, &entry).unwrap();

        let text = std::fs::read_to_string(manifest_path(&c)).unwrap();
        assert!(text.contains("acme/repo/deploy#main"), "{text}");
        assert!(!text.contains("alias:"), "{text}");
        assert_eq!(entries(&c).unwrap(), vec![entry]);
    }

    #[test]
    fn agent_roundtrips_with_agent_extension() {
        let tmp = TempDir::new().unwrap();
        let c = catalog(&tmp);
        let entry = ManifestEntry {
            spec: spec("acme/repo/reviewer#main"),
            item_type: ItemType::Agent,
            id: "reviewer".to_string(),
            commit: None,
        };
        record(&c, &entry).unwrap();

        let text = std::fs::read_to_string(manifest_path(&c)).unwrap();
        assert!(text.contains("reviewer.agent.md#main"), "{text}");

        let got = entries(&c).unwrap();
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].item_type, ItemType::Agent);
        assert_eq!(got[0].id, "reviewer");
    }

    #[test]
    fn custom_id_uses_object_form_with_alias() {
        let tmp = TempDir::new().unwrap();
        let c = catalog(&tmp);
        let entry = ManifestEntry {
            spec: spec("acme/repo/deploy#main"),
            item_type: ItemType::Skill,
            id: "vercel".to_string(),
            commit: None,
        };
        record(&c, &entry).unwrap();

        let text = std::fs::read_to_string(manifest_path(&c)).unwrap();
        assert!(text.contains("git: acme/repo"), "{text}");
        assert!(text.contains("alias: vercel"), "{text}");
        assert_eq!(entries(&c).unwrap(), vec![entry]);
    }

    #[test]
    fn commit_roundtrips_via_object_form() {
        let tmp = TempDir::new().unwrap();
        let c = catalog(&tmp);
        let entry = ManifestEntry {
            spec: spec("acme/repo/deploy#main"),
            item_type: ItemType::Skill,
            id: "deploy".to_string(),
            commit: Some("0123456789abcdef0123456789abcdef01234567".to_string()),
        };
        record(&c, &entry).unwrap();

        // A recorded commit forces the object form (a string can't carry ref + commit) and is
        // serialized without an `alias` since the id is the default.
        let text = std::fs::read_to_string(manifest_path(&c)).unwrap();
        assert!(text.contains("git: acme/repo"), "{text}");
        assert!(text.contains("ref: main"), "{text}");
        assert!(
            text.contains("commit: 0123456789abcdef0123456789abcdef01234567"),
            "{text}"
        );
        assert!(!text.contains("alias:"), "{text}");
        assert_eq!(entries(&c).unwrap(), vec![entry]);
    }

    #[test]
    fn legacy_string_entry_loads_without_commit() {
        let tmp = TempDir::new().unwrap();
        let c = catalog(&tmp);
        std::fs::create_dir_all(&c.root).unwrap();
        std::fs::write(
            manifest_path(&c),
            "name: akit-catalog\nversion: 0.0.0\ndependencies:\n  apm:\n    - acme/repo/deploy#main\n",
        )
        .unwrap();

        let got = entries(&c).unwrap();
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].id, "deploy");
        assert_eq!(got[0].commit, None);
    }

    #[test]
    fn record_upserts_by_id_and_type() {
        let tmp = TempDir::new().unwrap();
        let c = catalog(&tmp);
        record(
            &c,
            &ManifestEntry {
                spec: spec("acme/repo/deploy#main"),
                item_type: ItemType::Skill,
                id: "deploy".to_string(),
                commit: None,
            },
        )
        .unwrap();
        let updated = ManifestEntry {
            spec: spec("acme/repo/deploy#v2"),
            item_type: ItemType::Skill,
            id: "deploy".to_string(),
            commit: None,
        };
        record(&c, &updated).unwrap();

        assert_eq!(entries(&c).unwrap(), vec![updated]);
    }

    #[test]
    fn scaffold_sets_name_version_and_preserves_unknown_keys() {
        let tmp = TempDir::new().unwrap();
        let c = catalog(&tmp);
        std::fs::create_dir_all(&c.root).unwrap();
        std::fs::write(
            manifest_path(&c),
            "name: mine\nversion: 1.2.3\nauthor: surdy\n",
        )
        .unwrap();

        record(
            &c,
            &ManifestEntry {
                spec: spec("acme/repo/deploy#main"),
                item_type: ItemType::Skill,
                id: "deploy".to_string(),
                commit: None,
            },
        )
        .unwrap();

        let text = std::fs::read_to_string(manifest_path(&c)).unwrap();
        assert!(text.contains("name: mine"), "{text}");
        assert!(text.contains("version: 1.2.3"), "{text}");
        assert!(text.contains("author: surdy"), "{text}");
    }

    #[test]
    fn remove_prunes_matching_entry_and_preserves_others() {
        let tmp = TempDir::new().unwrap();
        let c = catalog(&tmp);
        let skill = ManifestEntry {
            spec: spec("acme/repo/deploy#main"),
            item_type: ItemType::Skill,
            id: "deploy".to_string(),
            commit: None,
        };
        let agent = ManifestEntry {
            spec: spec("acme/repo/reviewer#main"),
            item_type: ItemType::Agent,
            id: "reviewer".to_string(),
            commit: None,
        };
        record(&c, &skill).unwrap();
        record(&c, &agent).unwrap();

        // Removing by (type, id) prunes only the matching entry.
        assert!(remove(&c, ItemType::Skill, "deploy").unwrap());
        let remaining = entries(&c).unwrap();
        assert_eq!(remaining.len(), 1);
        assert_eq!(remaining[0].item_type, ItemType::Agent);
        assert_eq!(remaining[0].id, "reviewer");

        // A same-id but different-type entry is left untouched.
        assert!(!remove(&c, ItemType::Skill, "reviewer").unwrap());
        assert!(remove(&c, ItemType::Agent, "reviewer").unwrap());
        assert!(entries(&c).unwrap().is_empty());

        // Removing something absent is a no-op.
        assert!(!remove(&c, ItemType::Skill, "deploy").unwrap());
    }
}
