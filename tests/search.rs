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

fn make_agent(catalog_root: &Path, file_name: &str, body: &str) {
    let dir = catalog_root.join("agents");
    fs::create_dir_all(&dir).unwrap();
    fs::write(dir.join(format!("{file_name}.agent.md")), body).unwrap();
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
    make_agent(
        &catalog_root,
        "reviewer",
        "---\nname: Reviewer\ndescription: Review code\ncategory: quality\n---\nbody\n",
    );

    let catalog = Catalog::with_root(&catalog_root);
    let hits = search::search(&catalog, "").unwrap();

    assert_eq!(hits.len(), 2);
    assert!(hits.iter().all(|hit| hit.score == 0));
    assert!(hits.iter().any(|hit| hit.name == "Deploy Helper"));
    assert!(hits.iter().any(|hit| hit.name == "Reviewer"));
    assert!(hits.iter().any(|hit| hit.id == "deploy-helper"));
    assert!(hits.iter().any(|hit| hit.id == "reviewer"));
}

#[test]
fn missing_or_malformed_frontmatter_is_included_without_panicking() {
    let tmp = tempfile::tempdir().unwrap();
    let catalog_root = tmp.path().join("catalog");
    make_skill(&catalog_root, "plain", "body without frontmatter\n");
    make_agent(
        &catalog_root,
        "broken",
        "---\nname: Broken Agent\ndescription: unterminated frontmatter\n",
    );

    let catalog = Catalog::with_root(&catalog_root);
    let hits = search::search(&catalog, "").unwrap();

    assert_eq!(hits.len(), 2);
    assert!(hits.iter().any(|hit| hit.name == "plain"));
    assert!(hits.iter().any(|hit| hit.name == "broken"));
}
