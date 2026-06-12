use std::fs;
use std::path::Path;
use std::process::Command;

use ckit::collection::Collection;
use ckit::lockfile::{ItemType, Lockfile, Mode};
use ckit::ops::{self, HealthStatus};
use ckit::project::Project;

fn git(args: &[&str], cwd: &Path) -> std::process::Output {
    Command::new("git")
        .args(args)
        .current_dir(cwd)
        .output()
        .expect("git should be available")
}

fn init_project(base: &Path) -> (std::path::PathBuf, Project) {
    let proj = base.join("project");
    fs::create_dir_all(&proj).unwrap();
    assert!(git(&["init", "-q"], &proj).status.success());
    let project = Project::locate(Some(proj.clone())).unwrap();
    (proj, project)
}

fn make_skill(collection_root: &Path, name: &str) {
    let dir = collection_root.join("skills").join(name);
    fs::create_dir_all(dir.join("notes")).unwrap();
    fs::write(
        dir.join("SKILL.md"),
        format!("---\nname: {name}\ndescription: a test skill\n---\nbody\n"),
    )
    .unwrap();
    fs::write(dir.join("notes").join("extra.md"), "extra\n").unwrap();
}

#[test]
fn add_copy_creates_real_files_and_records_copy() {
    let tmp = tempfile::tempdir().unwrap();
    let base = tmp.path();
    let collection_root = base.join("collection");
    make_skill(&collection_root, "demo");
    let collection = Collection::with_root(&collection_root);
    let (proj, project) = init_project(base);

    let report = ops::add_item(&project, &collection, ItemType::Skill, "demo", Mode::Copy).unwrap();

    assert!(report.link_created);
    assert_eq!(report.mode, Mode::Copy);

    let target = proj.join(".github/skills/demo");
    let meta = fs::symlink_metadata(&target).unwrap();
    assert!(!meta.file_type().is_symlink(), "target should be a copy");
    assert!(target.join("SKILL.md").is_file());
    assert!(target.join("notes/extra.md").is_file());

    let lf = Lockfile::load(&project.lockfile_path()).unwrap();
    assert_eq!(lf.items.len(), 1);
    assert_eq!(lf.items[0].mode, Mode::Copy);
}

#[test]
fn rm_copy_mode_removes_target_exclude_and_lockfile_entry() {
    let tmp = tempfile::tempdir().unwrap();
    let base = tmp.path();
    let collection_root = base.join("collection");
    make_skill(&collection_root, "demo");
    let collection = Collection::with_root(&collection_root);
    let (proj, project) = init_project(base);

    ops::add_item(&project, &collection, ItemType::Skill, "demo", Mode::Copy).unwrap();
    let target = proj.join(".github/skills/demo");
    assert!(target.is_dir());

    let report = ops::remove_item(&project, ItemType::Skill, "demo").unwrap();

    assert!(report.target_removed);
    assert!(report.exclude_removed);
    assert!(report.lock_removed);
    assert!(!target.exists(), "copy-mode directory should be removed");

    let exclude = fs::read_to_string(proj.join(".git/info/exclude")).unwrap();
    assert!(
        !exclude.lines().any(|l| l == "/.github/skills/demo"),
        "exclude still has skill line:\n{exclude}"
    );

    let lf = Lockfile::load(&project.lockfile_path()).unwrap();
    assert!(lf.items.is_empty(), "lockfile entry was not removed");
}

#[test]
fn symlink_failure_falls_back_to_copy_with_warning() {
    let tmp = tempfile::tempdir().unwrap();
    let base = tmp.path();
    let collection_root = base.join("collection");
    make_skill(&collection_root, "demo");
    let (proj, project) = init_project(base);

    let output = Command::new(env!("CARGO_BIN_EXE_ckit"))
        .args(["--project", proj.to_str().unwrap(), "--json", "add", "demo"])
        .env("KIT_COLLECTION_DIR", &collection_root)
        .env("CKIT_TEST_FORCE_SYMLINK_FAILURE", "1")
        .output()
        .expect("ckit binary should run");

    assert!(
        output.status.success(),
        "ckit failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("warning: symlink failed"), "{stderr}");
    assert!(stderr.contains("falling back to copy"), "{stderr}");

    let json: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(json["mode"], "copy");

    let target = proj.join(".github/skills/demo");
    assert!(
        !fs::symlink_metadata(&target)
            .unwrap()
            .file_type()
            .is_symlink()
    );

    let lf = Lockfile::load(&project.lockfile_path()).unwrap();
    assert_eq!(lf.items.len(), 1);
    assert_eq!(lf.items[0].mode, Mode::Copy);
}

#[test]
fn list_reports_copy_drift_after_materialized_file_changes() {
    let tmp = tempfile::tempdir().unwrap();
    let base = tmp.path();
    let collection_root = base.join("collection");
    make_skill(&collection_root, "demo");
    let collection = Collection::with_root(&collection_root);
    let (proj, project) = init_project(base);

    ops::add_item(&project, &collection, ItemType::Skill, "demo", Mode::Copy).unwrap();

    let items = ops::list_items_with_collection(&project, &collection).unwrap();
    assert_eq!(items.len(), 1);
    assert_eq!(items[0].status, HealthStatus::Ok);

    fs::write(proj.join(".github/skills/demo/SKILL.md"), "changed\n").unwrap();

    let items = ops::list_items_with_collection(&project, &collection).unwrap();
    assert_eq!(items.len(), 1);
    assert_eq!(items[0].status, HealthStatus::Drifted);
}
