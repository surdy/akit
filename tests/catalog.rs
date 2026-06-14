use std::fs;
use std::path::Path;

use akit::catalog::Catalog;
use akit::lockfile::ItemType;
use akit::manifest;
use akit::ops;

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
fn list_catalog_reports_ids_provenance_and_descriptions() {
    let tmp = tempfile::tempdir().unwrap();
    let catalog_root = tmp.path().join("catalog");

    // A hand-authored skill (no manifest entry → local).
    make_skill(
        &catalog_root,
        "deploy-helper",
        "---\nname: Deploy Helper\ndescription: Ship apps safely\n---\nbody\n",
    );
    // A pulled skill, recorded in the manifest below.
    make_skill(
        &catalog_root,
        "grill-me",
        "---\nname: Grill Me\ndescription: Stress-test a plan\n---\nbody\n",
    );
    // A hand-authored agent (no manifest entry → local).
    make_agent(
        &catalog_root,
        "reviewer",
        "---\nname: Reviewer\ndescription: Review code\n---\nbody\n",
    );

    // Record only grill-me as a remote pull.
    fs::write(
        catalog_root.join(manifest::MANIFEST_FILE),
        "name: akit-catalog\nversion: 0.0.0\ndependencies:\n  apm:\n  - acme/kit-skills/grill-me#main\n",
    )
    .unwrap();

    let catalog = Catalog::with_root(&catalog_root);
    let items = ops::list_catalog(&catalog).unwrap();

    // Sorted skills-first, then by id: deploy-helper, grill-me, then the agent.
    let ids: Vec<&str> = items.iter().map(|i| i.id.as_str()).collect();
    assert_eq!(ids, ["deploy-helper", "grill-me", "reviewer"]);

    let deploy = &items[0];
    assert_eq!(deploy.item_type, ItemType::Skill);
    assert_eq!(deploy.description, "Ship apps safely");
    assert_eq!(deploy.source, None, "hand-authored skill is local");

    let grill = &items[1];
    assert_eq!(grill.item_type, ItemType::Skill);
    assert_eq!(
        grill.source.as_deref(),
        Some("acme/kit-skills/grill-me#main"),
        "pulled skill carries its remote provenance"
    );

    let reviewer = &items[2];
    assert_eq!(reviewer.item_type, ItemType::Agent);
    assert_eq!(reviewer.source, None, "hand-authored agent is local");
}

#[test]
fn list_catalog_is_empty_for_a_missing_catalog() {
    let tmp = tempfile::tempdir().unwrap();
    let catalog = Catalog::with_root(tmp.path().join("does-not-exist"));
    assert!(ops::list_catalog(&catalog).unwrap().is_empty());
}
