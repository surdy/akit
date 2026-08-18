use std::fs;
use std::path::Path;

use akit::catalog::Catalog;
use akit::lockfile::ItemType;
use akit::search;

fn make_skill(catalog_root: &Path, dir_name: &str, body: &str) {
    let dir = catalog_root.join("skills").join(dir_name);
    fs::create_dir_all(&dir).unwrap();
    fs::write(dir.join("SKILL.md"), body).unwrap();
}

/// A leftover legacy flat agent file (`agents/<id>.agent.md`), a shape removed in
/// v0.32.0. Used only to assert that search ignores it.
fn make_legacy_flat_agent(catalog_root: &Path, file_name: &str, body: &str) {
    let dir = catalog_root.join("agents");
    fs::create_dir_all(&dir).unwrap();
    fs::write(dir.join(format!("{file_name}.agent.md")), body).unwrap();
}

fn make_agent_pkg(catalog_root: &Path, id: &str, description: &str, variants: &[(&str, &str)]) {
    let dir = catalog_root.join("agents").join(id);
    fs::create_dir_all(&dir).unwrap();
    let mut yml = format!("id: {id}\ndescription: {description}\nvariants:\n");
    for (harness, file) in variants {
        yml.push_str(&format!("  {harness}: {file}\n"));
        fs::write(dir.join(file), "---\nname: a\n---\nprompt\n").unwrap();
    }
    fs::write(dir.join("agent.yml"), yml).unwrap();
}

#[test]
fn search_surfaces_agent_packages_with_harnesses() {
    let tmp = tempfile::tempdir().unwrap();
    let catalog_root = tmp.path().join("catalog");
    make_agent_pkg(
        &catalog_root,
        "code-reviewer",
        "Reviews pull requests",
        &[("copilot", "c.md"), ("claude", "cl.md")],
    );

    let catalog = Catalog::with_root(&catalog_root);
    let hits = search::search(&catalog, "review").unwrap();

    let hit = hits.iter().find(|h| h.id == "code-reviewer").unwrap();
    assert_eq!(hit.item_type, ItemType::Agent);
    assert_eq!(hit.description, "Reviews pull requests");
    assert_eq!(
        hit.harnesses,
        vec![
            akit::harness::HarnessId::Copilot,
            akit::harness::HarnessId::Claude
        ]
    );
    assert!(hit.score > 0);
}

#[test]
fn partial_query_ranks_matching_item_first() {
    let tmp = tempfile::tempdir().unwrap();
    let catalog_root = tmp.path().join("catalog");
    make_skill(
        &catalog_root,
        "deploy-helper",
        "---\nname: Deploy Helper\ndescription: Ship apps safely\ncategory: ops\n---\nbody\n",
    );
    make_skill(
        &catalog_root,
        "docs-helper",
        "---\nname: Docs Helper\ndescription: Write project docs\ncategory: writing\n---\nbody\n",
    );

    let catalog = Catalog::with_root(&catalog_root);
    let hits = search::search(&catalog, "depl").unwrap();

    assert!(!hits.is_empty());
    assert_eq!(hits[0].item_type, ItemType::Skill);
    assert_eq!(hits[0].id, "deploy-helper");
    assert_eq!(hits[0].name, "Deploy Helper");
    assert!(hits[0].score > 0);
}

#[test]
fn query_matches_the_catalog_id_handle() {
    let tmp = tempfile::tempdir().unwrap();
    let catalog_root = tmp.path().join("catalog");
    // Frontmatter name deliberately differs from the directory handle so this
    // only matches via `id`.
    make_skill(
        &catalog_root,
        "deploy-helper",
        "---\nname: Rocket\ndescription: Ship apps safely\ncategory: ops\n---\nbody\n",
    );

    let catalog = Catalog::with_root(&catalog_root);
    let hits = search::search(&catalog, "deploy-helper").unwrap();

    assert!(!hits.is_empty());
    assert_eq!(hits[0].id, "deploy-helper");
    assert_eq!(hits[0].name, "Rocket");
    assert!(hits[0].score > 0);
}

#[test]
fn empty_query_returns_all_items() {
    let tmp = tempfile::tempdir().unwrap();
    let catalog_root = tmp.path().join("catalog");
    make_skill(
        &catalog_root,
        "deploy-helper",
        "---\nname: Deploy Helper\ndescription: Ship apps safely\ncategory: ops\n---\nbody\n",
    );
    make_agent_pkg(
        &catalog_root,
        "reviewer",
        "Review code",
        &[("claude", "cl.md")],
    );
    // A leftover flat file is not an item and must not appear.
    make_legacy_flat_agent(
        &catalog_root,
        "legacy",
        "---\nname: Legacy\ndescription: old shape\n---\nbody\n",
    );

    let catalog = Catalog::with_root(&catalog_root);
    let hits = search::search(&catalog, "").unwrap();

    assert_eq!(hits.len(), 2);
    assert!(hits.iter().all(|hit| hit.score == 0));
    assert!(hits.iter().any(|hit| hit.name == "Deploy Helper"));
    assert!(hits.iter().any(|hit| hit.id == "reviewer"));
    assert!(
        hits.iter().all(|hit| hit.id != "legacy"),
        "flat .agent.md must not be searchable"
    );
}

#[test]
fn missing_or_malformed_frontmatter_is_included_without_panicking() {
    let tmp = tempfile::tempdir().unwrap();
    let catalog_root = tmp.path().join("catalog");
    make_skill(&catalog_root, "plain", "body without frontmatter\n");
    // An agent package whose `agent.yml` is unparseable stays visible with the
    // load error, rather than breaking the whole scan.
    let broken = catalog_root.join("agents").join("broken");
    fs::create_dir_all(&broken).unwrap();
    fs::write(broken.join("agent.yml"), "variants: [not, a, mapping\n").unwrap();

    let catalog = Catalog::with_root(&catalog_root);
    let hits = search::search(&catalog, "").unwrap();

    assert_eq!(hits.len(), 2);
    assert!(hits.iter().any(|hit| hit.name == "plain"));
    assert!(hits.iter().any(|hit| hit.id == "broken"));
}
