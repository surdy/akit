use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use akit::catalog::Catalog;
use akit::lockfile::{ItemType, Lockfile, Mode};
use akit::ops::{self, HealthStatus};
use akit::project::Project;

fn git(args: &[&str], cwd: &Path) -> std::process::Output {
    Command::new("git")
        .args(args)
        .current_dir(cwd)
        .output()
        .expect("git should be available")
}

fn init_project(base: &Path) -> (PathBuf, Project) {
    let proj = base.join("project");
    fs::create_dir_all(&proj).unwrap();
    assert!(git(&["init", "-q"], &proj).status.success());
    let project = Project::locate(Some(proj.clone())).unwrap();
    (proj, project)
}

fn make_skill(catalog_root: &Path, name: &str) {
    let dir = catalog_root.join("skills").join(name);
    fs::create_dir_all(&dir).unwrap();
    fs::write(
        dir.join("SKILL.md"),
        format!("---\nname: {name}\ndescription: a test skill\n---\nbody\n"),
    )
    .unwrap();
}

fn make_agent(catalog_root: &Path, name: &str) {
    let dir = catalog_root.join("agents");
    fs::create_dir_all(&dir).unwrap();
    fs::write(
        dir.join(format!("{name}.agent.md")),
        format!("---\nname: {name}\ndescription: a test agent\n---\nbody\n"),
    )
    .unwrap();
}

fn make_bundle(catalog_root: &Path, name: &str, manifest: &str) {
    let dir = catalog_root.join("bundles");
    fs::create_dir_all(&dir).unwrap();
    fs::write(dir.join(format!("{name}.yml")), manifest).unwrap();
}

fn type_key(item_type: ItemType) -> &'static str {
    match item_type {
        ItemType::Skill => "skill",
        ItemType::Agent => "agent",
    }
}

fn ids(items: &[(&str, &str)]) -> BTreeSet<(String, String)> {
    items
        .iter()
        .map(|(item_type, id)| ((*item_type).to_string(), (*id).to_string()))
        .collect()
}

#[test]
fn add_bundle_installs_mixed_items_and_tags_lockfile() {
    let tmp = tempfile::tempdir().unwrap();
    let base = tmp.path();
    let catalog_root = base.join("catalog");
    make_skill(&catalog_root, "deploy-to-vercel");
    make_skill(&catalog_root, "lint-fix");
    make_agent(&catalog_root, "code-reviewer");
    make_bundle(
        &catalog_root,
        "demo",
        "skills: [deploy-to-vercel, lint-fix]\nagents: [code-reviewer]\n",
    );
    let catalog = Catalog::with_root(&catalog_root);
    let (proj, project) = init_project(base);

    let report = ops::add_bundle(&project, &catalog, "demo", Mode::Symlink).unwrap();

    assert_eq!(report.bundle, "demo");
    assert_eq!(report.items.len(), 3);
    assert!(
        report
            .items
            .iter()
            .all(|item| item.bundle.as_deref() == Some("demo"))
    );

    assert!(fs::symlink_metadata(proj.join(".github/skills/deploy-to-vercel")).is_ok());
    assert!(fs::symlink_metadata(proj.join(".github/skills/lint-fix")).is_ok());
    assert!(fs::symlink_metadata(proj.join(".github/agents/code-reviewer.agent.md")).is_ok());

    let lockfile = Lockfile::load(&project.lockfile_path()).unwrap();
    assert_eq!(lockfile.items.len(), 3);
    let actual = lockfile
        .items
        .iter()
        .map(|item| (type_key(item.item_type).to_string(), item.id.clone()))
        .collect::<BTreeSet<_>>();
    assert_eq!(
        actual,
        ids(&[
            ("skill", "deploy-to-vercel"),
            ("skill", "lint-fix"),
            ("agent", "code-reviewer"),
        ])
    );
    assert!(
        lockfile
            .items
            .iter()
            .all(|item| item.bundle.as_deref() == Some("demo"))
    );

    let listed = ops::list_items(&project).unwrap();
    assert_eq!(listed.len(), 3);
    assert!(
        listed
            .iter()
            .all(|item| item.bundle.as_deref() == Some("demo") && item.status == HealthStatus::Ok)
    );
}

#[test]
fn remove_bundle_uses_lockfile_tags_and_leaves_unrelated_items() {
    let tmp = tempfile::tempdir().unwrap();
    let base = tmp.path();
    let catalog_root = base.join("catalog");
    for skill in ["deploy-to-vercel", "lint-fix", "standalone", "other-skill"] {
        make_skill(&catalog_root, skill);
    }
    make_agent(&catalog_root, "code-reviewer");
    make_agent(&catalog_root, "other-agent");
    make_bundle(
        &catalog_root,
        "demo",
        "skills: [deploy-to-vercel, lint-fix]\nagents: [code-reviewer]\n",
    );
    make_bundle(
        &catalog_root,
        "other",
        "skills: [other-skill]\nagents: [other-agent]\n",
    );
    let catalog = Catalog::with_root(&catalog_root);
    let (proj, project) = init_project(base);

    ops::add_bundle(&project, &catalog, "demo", Mode::Symlink).unwrap();
    ops::add_item(
        &project,
        &catalog,
        ItemType::Skill,
        "standalone",
        Mode::Symlink,
        None,
    )
    .unwrap();
    ops::add_bundle(&project, &catalog, "other", Mode::Symlink).unwrap();

    make_bundle(&catalog_root, "demo", "skills: [standalone]\n");
    let report = ops::remove_bundle(&project, "demo").unwrap();

    assert_eq!(report.items.len(), 3);
    let removed = report
        .items
        .iter()
        .map(|item| (type_key(item.item_type).to_string(), item.id.clone()))
        .collect::<BTreeSet<_>>();
    assert_eq!(
        removed,
        ids(&[
            ("skill", "deploy-to-vercel"),
            ("skill", "lint-fix"),
            ("agent", "code-reviewer"),
        ])
    );

    assert!(!proj.join(".github/skills/deploy-to-vercel").exists());
    assert!(!proj.join(".github/skills/lint-fix").exists());
    assert!(!proj.join(".github/agents/code-reviewer.agent.md").exists());
    assert!(proj.join(".github/skills/standalone").exists());
    assert!(proj.join(".github/skills/other-skill").exists());
    assert!(proj.join(".github/agents/other-agent.agent.md").exists());

    let lockfile = Lockfile::load(&project.lockfile_path()).unwrap();
    assert_eq!(lockfile.items.len(), 3);
    assert!(lockfile.items.iter().any(|item| {
        item.item_type == ItemType::Skill && item.id == "standalone" && item.bundle.is_none()
    }));
    assert!(lockfile.items.iter().any(|item| {
        item.item_type == ItemType::Skill
            && item.id == "other-skill"
            && item.bundle.as_deref() == Some("other")
    }));
    assert!(lockfile.items.iter().any(|item| {
        item.item_type == ItemType::Agent
            && item.id == "other-agent"
            && item.bundle.as_deref() == Some("other")
    }));
}

#[test]
fn bundle_manifest_missing_keys_are_empty() {
    let tmp = tempfile::tempdir().unwrap();
    let base = tmp.path();
    let catalog_root = base.join("catalog");
    make_skill(&catalog_root, "only-skill");
    make_bundle(&catalog_root, "skills-only", "skills: [only-skill]\n");
    let catalog = Catalog::with_root(&catalog_root);
    let (proj, project) = init_project(base);

    let report = ops::add_bundle(&project, &catalog, "skills-only", Mode::Symlink).unwrap();

    assert_eq!(report.items.len(), 1);
    assert!(proj.join(".github/skills/only-skill").exists());
    let lockfile = Lockfile::load(&project.lockfile_path()).unwrap();
    assert_eq!(lockfile.items.len(), 1);
    assert_eq!(lockfile.items[0].bundle.as_deref(), Some("skills-only"));
}

#[test]
fn missing_bundle_item_fails_whole_bundle_with_clear_error() {
    let tmp = tempfile::tempdir().unwrap();
    let base = tmp.path();
    let catalog_root = base.join("catalog");
    make_skill(&catalog_root, "present");
    make_bundle(&catalog_root, "broken", "skills: [present, missing]\n");
    let catalog = Catalog::with_root(&catalog_root);
    let (proj, project) = init_project(base);

    let err = ops::add_bundle(&project, &catalog, "broken", Mode::Symlink).unwrap_err();

    let message = format!("{err:#}");
    assert!(
        message.contains("bundle 'broken' references skill 'missing'"),
        "{message}"
    );
    assert!(message.contains("skill 'missing' not found"), "{message}");
    assert!(
        !proj.join(".github/skills/present").exists(),
        "bundle add should fail before materializing any item"
    );
    assert!(
        Lockfile::load(&project.lockfile_path())
            .unwrap()
            .items
            .is_empty()
    );
}

#[test]
fn cli_add_and_rm_bundle_apply_copy_mode() {
    let tmp = tempfile::tempdir().unwrap();
    let base = tmp.path();
    let catalog_root = base.join("catalog");
    make_skill(&catalog_root, "deploy-to-vercel");
    make_agent(&catalog_root, "code-reviewer");
    make_bundle(
        &catalog_root,
        "demo",
        "skills: [deploy-to-vercel]\nagents: [code-reviewer]\n",
    );
    let (proj, project) = init_project(base);

    let add_output = Command::new(env!("CARGO_BIN_EXE_akit"))
        .args([
            "--project",
            proj.to_str().unwrap(),
            "--json",
            "add",
            "--copy",
            "--bundle",
            "demo",
        ])
        .env("KIT_CATALOG_DIR", &catalog_root)
        .output()
        .expect("akit binary should run");

    assert!(
        add_output.status.success(),
        "akit failed: {}",
        String::from_utf8_lossy(&add_output.stderr)
    );
    let add_json: serde_json::Value = serde_json::from_slice(&add_output.stdout).unwrap();
    assert_eq!(add_json["bundle"], "demo");
    assert_eq!(add_json["items"].as_array().unwrap().len(), 2);
    assert!(
        add_json["items"]
            .as_array()
            .unwrap()
            .iter()
            .all(|item| { item["bundle"] == "demo" && item["mode"] == "copy" })
    );
    assert!(
        !fs::symlink_metadata(proj.join(".github/skills/deploy-to-vercel"))
            .unwrap()
            .file_type()
            .is_symlink()
    );
    assert!(
        !fs::symlink_metadata(proj.join(".github/agents/code-reviewer.agent.md"))
            .unwrap()
            .file_type()
            .is_symlink()
    );

    let rm_output = Command::new(env!("CARGO_BIN_EXE_akit"))
        .args([
            "--project",
            proj.to_str().unwrap(),
            "--json",
            "rm",
            "--bundle",
            "demo",
        ])
        .output()
        .expect("akit binary should run");

    assert!(
        rm_output.status.success(),
        "akit failed: {}",
        String::from_utf8_lossy(&rm_output.stderr)
    );
    let rm_json: serde_json::Value = serde_json::from_slice(&rm_output.stdout).unwrap();
    assert_eq!(rm_json["bundle"], "demo");
    assert_eq!(rm_json["items"].as_array().unwrap().len(), 2);
    assert!(
        Lockfile::load(&project.lockfile_path())
            .unwrap()
            .items
            .is_empty()
    );
    assert!(!proj.join(".github/skills/deploy-to-vercel").exists());
    assert!(!proj.join(".github/agents/code-reviewer.agent.md").exists());
}

#[test]
fn cli_ls_labels_and_groups_bundle_items() {
    let tmp = tempfile::tempdir().unwrap();
    let base = tmp.path();
    let catalog_root = base.join("catalog");
    make_skill(&catalog_root, "alpha-skill");
    make_skill(&catalog_root, "zeta-skill");
    make_skill(&catalog_root, "standalone");
    make_bundle(&catalog_root, "zeta", "skills: [zeta-skill]\n");
    make_bundle(&catalog_root, "alpha", "skills: [alpha-skill]\n");
    let catalog = Catalog::with_root(&catalog_root);
    let (proj, project) = init_project(base);

    ops::add_bundle(&project, &catalog, "zeta", Mode::Symlink).unwrap();
    ops::add_item(
        &project,
        &catalog,
        ItemType::Skill,
        "standalone",
        Mode::Symlink,
        None,
    )
    .unwrap();
    ops::add_bundle(&project, &catalog, "alpha", Mode::Symlink).unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_akit"))
        .args(["--project", proj.to_str().unwrap(), "ls"])
        .output()
        .expect("akit binary should run");

    assert!(
        output.status.success(),
        "akit failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    let lines = stdout.lines().collect::<Vec<_>>();
    assert!(lines[0].contains("BUNDLE"), "{stdout}");
    assert!(lines[1].starts_with("alpha"), "{stdout}");
    assert!(lines[1].contains("alpha-skill"), "{stdout}");
    assert!(lines[2].starts_with("zeta"), "{stdout}");
    assert!(lines[2].contains("zeta-skill"), "{stdout}");
    assert!(lines[3].starts_with('-'), "{stdout}");
    assert!(lines[3].contains("standalone"), "{stdout}");
}
