//! Read-only preview of a single collection item.
//!
//! Backs the CLI `akit show` command and the pterm kit-palette preview: given an
//! id and a kind, it resolves the source file, parses its frontmatter (reusing
//! [`crate::search`]'s parser), and returns the raw content alongside.

use anyhow::{Context, Result};
use serde::Serialize;
use std::path::PathBuf;

use crate::collection::Collection;
use crate::lockfile::ItemType;
use crate::search::parse_frontmatter;

/// A resolved, read-only view of a collection item.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ItemPreview {
    /// `"skill"` or `"agent"`.
    #[serde(rename = "type")]
    pub item_type: ItemType,
    /// The id the item was looked up by (skill dir name / agent file stem).
    pub id: String,
    /// Frontmatter `name`, or the id when absent.
    pub name: String,
    /// Frontmatter `description`, or empty.
    pub description: String,
    /// Frontmatter `category`, or empty.
    pub category: String,
    /// Absolute path to the source markdown file.
    pub path: PathBuf,
    /// Raw file content (frontmatter included).
    pub content: String,
}

/// Resolve and read a collection item for preview.
///
/// Errors when the item (or its markdown file) is missing. Malformed
/// frontmatter is tolerated — the preview falls back to the id for `name` and
/// empty strings for the rest, matching [`crate::search`]'s behavior (a warning
/// is printed to stderr by the shared parser).
pub fn show(collection: &Collection, id: &str, kind: ItemType) -> Result<ItemPreview> {
    let path = match kind {
        ItemType::Skill => collection.resolve_skill(id)?.join("SKILL.md"),
        ItemType::Agent => collection.resolve_agent(id)?,
    };

    let content =
        std::fs::read_to_string(&path).with_context(|| format!("reading {}", path.display()))?;
    let frontmatter = parse_frontmatter(&path, &content);

    Ok(ItemPreview {
        item_type: kind,
        id: id.to_string(),
        name: frontmatter.name.unwrap_or_else(|| id.to_string()),
        description: frontmatter.description.unwrap_or_default(),
        category: frontmatter.category.unwrap_or_default(),
        path,
        content,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::tempdir;

    fn collection_with(skill: Option<&str>, agent: Option<&str>) -> (tempfile::TempDir, Collection) {
        let tmp = tempdir().unwrap();
        let root = tmp.path().join("collection");
        if let Some(body) = skill {
            let dir = root.join("skills").join("deploy-helper");
            fs::create_dir_all(&dir).unwrap();
            fs::write(dir.join("SKILL.md"), body).unwrap();
        }
        if let Some(body) = agent {
            let dir = root.join("agents");
            fs::create_dir_all(&dir).unwrap();
            fs::write(dir.join("reviewer.agent.md"), body).unwrap();
        }
        let collection = Collection::with_root(&root);
        (tmp, collection)
    }

    #[test]
    fn previews_a_skill_with_frontmatter() {
        let (_tmp, collection) = collection_with(
            Some("---\nname: Deploy Helper\ndescription: Ship safely\ncategory: ops\n---\nbody text\n"),
            None,
        );

        let preview = show(&collection, "deploy-helper", ItemType::Skill).unwrap();
        assert_eq!(preview.item_type, ItemType::Skill);
        assert_eq!(preview.id, "deploy-helper");
        assert_eq!(preview.name, "Deploy Helper");
        assert_eq!(preview.description, "Ship safely");
        assert_eq!(preview.category, "ops");
        assert!(preview.content.contains("body text"));
        assert!(preview.path.ends_with("SKILL.md"));
    }

    #[test]
    fn previews_an_agent() {
        let (_tmp, collection) =
            collection_with(None, Some("---\nname: Reviewer\n---\nreview prompt\n"));

        let preview = show(&collection, "reviewer", ItemType::Agent).unwrap();
        assert_eq!(preview.item_type, ItemType::Agent);
        assert_eq!(preview.name, "Reviewer");
        assert!(preview.content.contains("review prompt"));
    }

    #[test]
    fn falls_back_to_id_when_frontmatter_absent() {
        let (_tmp, collection) = collection_with(Some("no frontmatter here\n"), None);

        let preview = show(&collection, "deploy-helper", ItemType::Skill).unwrap();
        assert_eq!(preview.name, "deploy-helper");
        assert_eq!(preview.description, "");
        assert_eq!(preview.category, "");
    }

    #[test]
    fn errors_on_missing_skill() {
        let (_tmp, collection) = collection_with(None, None);
        assert!(show(&collection, "nope", ItemType::Skill).is_err());
    }

    #[test]
    fn errors_on_missing_agent() {
        let (_tmp, collection) = collection_with(None, None);
        assert!(show(&collection, "nope", ItemType::Agent).is_err());
    }

    #[test]
    fn json_shape_is_stable() {
        let (_tmp, collection) = collection_with(
            Some("---\nname: Deploy Helper\ndescription: Ship safely\ncategory: ops\n---\nbody\n"),
            None,
        );
        let preview = show(&collection, "deploy-helper", ItemType::Skill).unwrap();
        let v = serde_json::to_value(&preview).unwrap();
        assert_eq!(v["type"], "skill");
        assert_eq!(v["id"], "deploy-helper");
        assert_eq!(v["name"], "Deploy Helper");
        assert_eq!(v["description"], "Ship safely");
        assert_eq!(v["category"], "ops");
        assert!(v["path"].as_str().unwrap().ends_with("SKILL.md"));
        assert!(v["content"].as_str().unwrap().contains("body"));
    }
}
