use std::fs;
use std::path::Path;

use akit::catalog::Catalog;
use akit::harness::HarnessId;
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

/// Write a harness-aware agent package `agents/<id>/` with an `agent.yml` plus a
/// variant file per listed harness.
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
fn list_catalog_surfaces_agent_packages_with_harnesses() {
    let tmp = tempfile::tempdir().unwrap();
    let catalog_root = tmp.path().join("catalog");
    // A legacy flat agent and a harness-aware package coexist.
    make_agent(&catalog_root, "legacy", "---\nname: Legacy\n---\nprompt\n");
    make_agent_pkg(
        &catalog_root,
        "reviewer",
        "Reviews PRs",
        &[("copilot", "c.md"), ("claude", "cl.md")],
    );

    let catalog = Catalog::with_root(&catalog_root);
    let items = ops::list_catalog(&catalog).unwrap();

    let pkg = items.iter().find(|i| i.id == "reviewer").unwrap();
    assert_eq!(pkg.item_type, ItemType::Agent);
    assert_eq!(pkg.description, "Reviews PRs");
    assert_eq!(pkg.harnesses, vec![HarnessId::Copilot, HarnessId::Claude]);
    assert!(!pkg.disabled);

    // The legacy flat agent is still listed, with no per-harness contract.
    let flat = items.iter().find(|i| i.id == "legacy").unwrap();
    assert!(flat.harnesses.is_empty());
    assert!(!flat.disabled);
}

#[test]
fn list_catalog_prefers_package_over_flat_for_same_id_and_flags_invalid() {
    let tmp = tempfile::tempdir().unwrap();
    let catalog_root = tmp.path().join("catalog");
    // Same id in both shapes: the package must win.
    make_agent(&catalog_root, "dup", "---\nname: Flat Dup\n---\nprompt\n");
    make_agent_pkg(&catalog_root, "dup", "Package Dup", &[("codex", "x.toml")]);
    // An invalid package (no variants) stays visible-but-disabled.
    let broken = catalog_root.join("agents").join("broken");
    fs::create_dir_all(&broken).unwrap();
    fs::write(
        broken.join("agent.yml"),
        "name: Broken\ndescription: Broken\nvariants: {}\n",
    )
    .unwrap();

    let catalog = Catalog::with_root(&catalog_root);
    let items = ops::list_catalog(&catalog).unwrap();

    let dups: Vec<_> = items.iter().filter(|i| i.id == "dup").collect();
    assert_eq!(dups.len(), 1, "duplicate id should collapse to one row");
    assert_eq!(dups[0].description, "Package Dup", "package should win");
    assert_eq!(dups[0].harnesses, vec![HarnessId::Codex]);

    let broken_item = items.iter().find(|i| i.id == "broken").unwrap();
    assert!(broken_item.disabled, "invalid package should be disabled");
    assert!(broken_item.harnesses.is_empty());
}

#[test]
fn description_less_package_is_listed_but_disabled() {
    let tmp = tempfile::tempdir().unwrap();
    let catalog_root = tmp.path().join("catalog");
    // A package that omits `description:` is invalid — but it must still be
    // visible in `ls`, carrying its load error, rather than vanishing.
    let dir = catalog_root.join("agents").join("mute");
    fs::create_dir_all(&dir).unwrap();
    fs::write(dir.join("claude.md"), "---\nname: a\n---\nprompt\n").unwrap();
    fs::write(
        dir.join("agent.yml"),
        "name: Mute\nvariants:\n  claude: claude.md\n",
    )
    .unwrap();

    let catalog = Catalog::with_root(&catalog_root);
    let items = ops::list_catalog(&catalog).unwrap();

    let item = items
        .iter()
        .find(|i| i.id == "mute")
        .expect("invalid package must still be listed");
    assert!(item.disabled, "description-less package should be disabled");
    assert!(item.harnesses.is_empty());
    assert!(
        item.description.contains("no description"),
        "row should explain the defect: {}",
        item.description
    );

    // ... and it must not resolve for installation either.
    let err = catalog.resolve_agent_package("mute").unwrap_err();
    assert!(err.to_string().contains("no description"), "{err}");
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

#[test]
fn cli_ls_lists_the_catalog() {
    let tmp = tempfile::tempdir().unwrap();
    let catalog_root = tmp.path().join("catalog");
    make_skill(
        &catalog_root,
        "grill-me",
        "---\nname: Grill Me\ndescription: Stress-test a plan\n---\nbody\n",
    );

    let output = std::process::Command::new(env!("CARGO_BIN_EXE_akit"))
        .env("KIT_CATALOG_DIR", &catalog_root)
        .args(["--json", "ls"])
        .output()
        .expect("akit binary should run");

    assert!(
        output.status.success(),
        "akit failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("\"id\":\"grill-me\""), "{stdout}");
}

#[test]
fn cli_drop_removes_a_local_catalog_item() {
    // A hand-authored skill that was never pulled (no manifest entry) can still be dropped.
    let tmp = tempfile::tempdir().unwrap();
    let catalog_root = tmp.path().join("catalog");
    make_skill(
        &catalog_root,
        "local-skill",
        "---\nname: Local\ndescription: hand-authored\n---\nbody\n",
    );

    let output = std::process::Command::new(env!("CARGO_BIN_EXE_akit"))
        .env("KIT_CATALOG_DIR", &catalog_root)
        .args(["--json", "drop", "local-skill"])
        .output()
        .expect("akit binary should run");

    assert!(
        output.status.success(),
        "drop failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let json: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(json["item_removed"], true);
    assert_eq!(
        json["manifest_pruned"], false,
        "local item has no manifest entry"
    );
    assert!(json.get("source").is_none(), "local item has no source");
    assert!(
        !catalog_root.join("skills/local-skill").exists(),
        "local skill should be deleted"
    );
}
