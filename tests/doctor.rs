use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use akit::catalog::Catalog;
use akit::doctor;
use akit::lockfile::{ItemType, Mode};
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

fn remove_exclude_line(project_root: &Path, line: &str) {
    let exclude = project_root.join(".git/info/exclude");
    let existing = fs::read_to_string(&exclude).unwrap();
    let mut content = existing
        .lines()
        .filter(|existing_line| *existing_line != line)
        .collect::<Vec<_>>()
        .join("\n");
    if !content.is_empty() {
        content.push('\n');
    }
    fs::write(exclude, content).unwrap();
}

fn append_exclude_line(project_root: &Path, line: &str) {
    let exclude = project_root.join(".git/info/exclude");
    let mut existing = fs::read_to_string(&exclude).unwrap();
    if !existing.ends_with('\n') {
        existing.push('\n');
    }
    existing.push_str(line);
    existing.push('\n');
    fs::write(exclude, existing).unwrap();
}

#[test]
fn sync_restores_deleted_symlink_and_doctor_reports_ok() {
    let tmp = tempfile::tempdir().unwrap();
    let base = tmp.path();
    let catalog_root = base.join("catalog");
    make_skill(&catalog_root, "demo");
    let catalog = Catalog::with_root(&catalog_root);
    let (proj, project) = init_project(base);

    ops::add_skill(&project, &catalog, "demo").unwrap();
    let target = proj.join(".github/skills/demo");
    fs::remove_file(&target).unwrap();

    let before = doctor::diagnose(&project, &catalog).unwrap();
    assert_eq!(before.items[0].status, HealthStatus::Missing);

    let sync = doctor::sync(&project, &catalog).unwrap();
    assert_eq!(sync.summary.restored, 1);
    assert!(sync.items[0].restored);
    assert!(
        fs::symlink_metadata(&target)
            .unwrap()
            .file_type()
            .is_symlink()
    );

    let after = doctor::diagnose(&project, &catalog).unwrap();
    assert!(after.summary.healthy, "{after:#?}");
    assert_eq!(after.items[0].status, HealthStatus::Ok);
    let listed = ops::list_items_with_catalog(&project, &catalog).unwrap();
    assert_eq!(listed[0].status, HealthStatus::Ok);
}

#[test]
fn doctor_reports_orphaned_source_and_sync_skips_it() {
    let tmp = tempfile::tempdir().unwrap();
    let base = tmp.path();
    let catalog_root = base.join("catalog");
    make_skill(&catalog_root, "demo");
    let catalog = Catalog::with_root(&catalog_root);
    let (proj, project) = init_project(base);

    ops::add_skill(&project, &catalog, "demo").unwrap();
    fs::remove_dir_all(catalog_root.join("skills/demo")).unwrap();

    let report = doctor::diagnose(&project, &catalog).unwrap();
    assert_eq!(report.items[0].status, HealthStatus::Orphaned);
    assert!(!report.items[0].source_present);

    let sync = doctor::sync(&project, &catalog).unwrap();
    assert_eq!(sync.summary.skipped_orphan, 1);
    assert!(sync.items[0].skipped_orphan);
    assert!(fs::symlink_metadata(proj.join(".github/skills/demo")).is_ok());
}

#[test]
fn sync_restores_missing_exclude_line_and_is_idempotent() {
    let tmp = tempfile::tempdir().unwrap();
    let base = tmp.path();
    let catalog_root = base.join("catalog");
    make_skill(&catalog_root, "demo");
    let catalog = Catalog::with_root(&catalog_root);
    let (proj, project) = init_project(base);

    ops::add_skill(&project, &catalog, "demo").unwrap();
    remove_exclude_line(&proj, "/.github/skills/demo");

    let before = doctor::diagnose(&project, &catalog).unwrap();
    assert!(!before.items[0].exclude_present);
    assert!(
        before
            .exclude
            .missing
            .iter()
            .any(|line| line == "/.github/skills/demo")
    );

    let sync = doctor::sync(&project, &catalog).unwrap();
    assert_eq!(sync.summary.exclude_added, 1);
    assert!(sync.items[0].exclude_added);
    assert!(
        sync.exclude
            .target_lines_added
            .iter()
            .any(|line| line == "/.github/skills/demo")
    );

    let exclude = fs::read_to_string(proj.join(".git/info/exclude")).unwrap();
    assert!(exclude.lines().any(|line| line == "/.github/skills/demo"));

    let second = doctor::sync(&project, &catalog).unwrap();
    assert_eq!(second.summary.exclude_added, 0);
    assert!(second.summary.healthy);
}

#[test]
fn doctor_flags_stale_exclude_line_and_sync_does_not_delete_it() {
    let tmp = tempfile::tempdir().unwrap();
    let base = tmp.path();
    let catalog_root = base.join("catalog");
    make_skill(&catalog_root, "demo");
    let catalog = Catalog::with_root(&catalog_root);
    let (proj, project) = init_project(base);

    ops::add_skill(&project, &catalog, "demo").unwrap();
    append_exclude_line(&proj, "/.github/skills/old");

    let report = doctor::diagnose(&project, &catalog).unwrap();
    assert!(
        report
            .exclude
            .stale
            .iter()
            .any(|line| line == "/.github/skills/old")
    );

    let sync = doctor::sync(&project, &catalog).unwrap();
    assert!(
        sync.exclude
            .stale
            .iter()
            .any(|line| line == "/.github/skills/old")
    );
    let exclude = fs::read_to_string(proj.join(".git/info/exclude")).unwrap();
    assert!(exclude.lines().any(|line| line == "/.github/skills/old"));
}

#[test]
fn doctor_reports_copy_mode_drift() {
    let tmp = tempfile::tempdir().unwrap();
    let base = tmp.path();
    let catalog_root = base.join("catalog");
    make_skill(&catalog_root, "demo");
    let catalog = Catalog::with_root(&catalog_root);
    let (proj, project) = init_project(base);

    ops::add_item(
        &project,
        &catalog,
        ItemType::Skill,
        "demo",
        Mode::Copy,
        None,
    )
    .unwrap();
    fs::write(proj.join(".github/skills/demo/SKILL.md"), "changed\n").unwrap();

    let report = doctor::diagnose(&project, &catalog).unwrap();
    assert_eq!(report.items[0].status, HealthStatus::Drifted);
    assert_eq!(report.summary.drifted, 1);

    let sync = doctor::sync(&project, &catalog).unwrap();
    assert_eq!(sync.summary.drifted, 1);
    assert!(sync.items[0].drifted);
    assert_eq!(
        fs::read_to_string(proj.join(".github/skills/demo/SKILL.md")).unwrap(),
        "changed\n"
    );
}
