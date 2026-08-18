//! Read-only preview of a single catalog item.
//!
//! Backs the CLI `akit show` command and the pterm kit-palette preview: given an
//! id and a kind, it resolves the source file, parses its frontmatter (reusing
//! [`crate::search`]'s parser), and returns the raw content alongside.

use anyhow::{Context, Result};
use serde::Serialize;
use std::path::PathBuf;

use crate::catalog::Catalog;
use crate::harness::HarnessId;
use crate::lockfile::ItemType;
use crate::search::parse_frontmatter;

/// A resolved, read-only view of a catalog item.
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
    /// Absolute path to the previewed source file: a skill's `SKILL.md`, or an
    /// agent package's `agent.yml`.
    pub path: PathBuf,
    /// Raw file content (frontmatter included).
    pub content: String,
    /// Harnesses an agent *package* supports; empty for skills.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub harnesses: Vec<HarnessId>,
}

/// Resolve and read a catalog item for preview.
///
/// Errors when the item (or its markdown file) is missing. Malformed
/// frontmatter is tolerated — the preview falls back to the id for `name` and
/// empty strings for the rest, matching [`crate::search`]'s behavior (a warning
/// is printed to stderr by the shared parser).
pub fn show(catalog: &Catalog, id: &str, kind: ItemType) -> Result<ItemPreview> {
    // An agent is always a *package* (`agents/<id>/agent.yml`) — the only agent
    // contract since v0.32.0.
    if kind == ItemType::Agent {
        return show_agent_package(catalog, id);
    }

    let path = catalog.resolve_skill(id)?.join("SKILL.md");

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
        harnesses: Vec::new(),
    })
}

/// Preview a harness-aware agent package: metadata + supported harnesses from
/// `agent.yml`, previewing the descriptor itself (the package has no single
/// canonical markdown file — each harness gets its own native variant).
fn show_agent_package(catalog: &Catalog, id: &str) -> Result<ItemPreview> {
    let pkg = catalog.resolve_agent_package(id)?;
    let descriptor = pkg.dir.join(crate::agentpkg::AGENT_DESCRIPTOR);
    let content = std::fs::read_to_string(&descriptor)
        .with_context(|| format!("reading {}", descriptor.display()))?;
    let harnesses = pkg.supported_harnesses().collect();
    Ok(ItemPreview {
        item_type: ItemType::Agent,
        id: id.to_string(),
        name: pkg.name,
        description: pkg.description,
        category: pkg.category,
        path: descriptor,
        content,
        harnesses,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::tempdir;

    fn catalog_with(skill: Option<&str>, agent: Option<&str>) -> (tempfile::TempDir, Catalog) {
        let tmp = tempdir().unwrap();
        let root = tmp.path().join("catalog");
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
        let catalog = Catalog::with_root(&root);
        (tmp, catalog)
    }

    #[test]
    fn previews_a_skill_with_frontmatter() {
        let (_tmp, catalog) = catalog_with(
            Some(
                "---\nname: Deploy Helper\ndescription: Ship safely\ncategory: ops\n---\nbody text\n",
            ),
            None,
        );

        let preview = show(&catalog, "deploy-helper", ItemType::Skill).unwrap();
        assert_eq!(preview.item_type, ItemType::Skill);
        assert_eq!(preview.id, "deploy-helper");
        assert_eq!(preview.name, "Deploy Helper");
        assert_eq!(preview.description, "Ship safely");
        assert_eq!(preview.category, "ops");
        assert!(preview.content.contains("body text"));
        assert!(preview.path.ends_with("SKILL.md"));
    }

    #[test]
    fn flat_agent_file_is_not_previewable() {
        // A leftover legacy flat `agents/<id>.agent.md` is not a catalog shape any
        // more: `show` reports the missing *package*, not the file.
        let (_tmp, catalog) = catalog_with(None, Some("---\nname: Reviewer\n---\nreview prompt\n"));

        let err = show(&catalog, "reviewer", ItemType::Agent).unwrap_err();
        assert!(err.to_string().contains("agent package"), "{err}");
    }

    /// Write a harness-aware agent package at `agents/<id>/`.
    fn make_agent_pkg(root: &std::path::Path, id: &str, yml: &str, variants: &[&str]) {
        let dir = root.join("agents").join(id);
        fs::create_dir_all(&dir).unwrap();
        for v in variants {
            fs::write(dir.join(v), "---\nname: a\n---\nprompt\n").unwrap();
        }
        fs::write(dir.join("agent.yml"), yml).unwrap();
    }

    #[test]
    fn previews_an_agent_package_with_harnesses() {
        let tmp = tempdir().unwrap();
        let root = tmp.path().join("catalog");
        make_agent_pkg(
            &root,
            "reviewer",
            "name: Reviewer\ndescription: Reviews PRs\nvariants:\n  copilot: c.md\n  claude: cl.md\n",
            &["c.md", "cl.md"],
        );
        let catalog = Catalog::with_root(&root);

        let preview = show(&catalog, "reviewer", ItemType::Agent).unwrap();
        assert_eq!(preview.item_type, ItemType::Agent);
        assert_eq!(preview.name, "Reviewer");
        assert_eq!(preview.description, "Reviews PRs");
        assert_eq!(
            preview.harnesses,
            vec![HarnessId::Copilot, HarnessId::Claude]
        );
        // The previewed file is the descriptor; content is the agent.yml.
        assert!(preview.path.ends_with("agent.yml"));
        assert!(preview.content.contains("variants:"));
    }

    #[test]
    fn stray_flat_file_never_shadows_a_package_of_the_same_id() {
        let tmp = tempdir().unwrap();
        let root = tmp.path().join("catalog");
        // A leftover flat file sits beside a package of the same id; only the
        // package exists as far as `show` is concerned.
        fs::create_dir_all(root.join("agents")).unwrap();
        fs::write(
            root.join("agents/dup.agent.md"),
            "---\nname: Flat\n---\nflat body\n",
        )
        .unwrap();
        make_agent_pkg(
            &root,
            "dup",
            "name: Package\ndescription: The package\nvariants:\n  codex: x.toml\n",
            &["x.toml"],
        );
        let catalog = Catalog::with_root(&root);

        let preview = show(&catalog, "dup", ItemType::Agent).unwrap();
        assert_eq!(preview.name, "Package");
        assert_eq!(preview.harnesses, vec![HarnessId::Codex]);
        assert!(preview.path.ends_with("agent.yml"));
    }

    #[test]
    fn falls_back_to_id_when_frontmatter_absent() {
        let (_tmp, catalog) = catalog_with(Some("no frontmatter here\n"), None);

        let preview = show(&catalog, "deploy-helper", ItemType::Skill).unwrap();
        assert_eq!(preview.name, "deploy-helper");
        assert_eq!(preview.description, "");
        assert_eq!(preview.category, "");
    }

    #[test]
    fn errors_on_missing_skill() {
        let (_tmp, catalog) = catalog_with(None, None);
        assert!(show(&catalog, "nope", ItemType::Skill).is_err());
    }

    #[test]
    fn errors_on_missing_agent() {
        let (_tmp, catalog) = catalog_with(None, None);
        assert!(show(&catalog, "nope", ItemType::Agent).is_err());
    }

    #[test]
    fn json_shape_is_stable() {
        let (_tmp, catalog) = catalog_with(
            Some("---\nname: Deploy Helper\ndescription: Ship safely\ncategory: ops\n---\nbody\n"),
            None,
        );
        let preview = show(&catalog, "deploy-helper", ItemType::Skill).unwrap();
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
