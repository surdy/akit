//! End-to-end test for issue #1: `add` one skill, verify symlink, git-exclude, lockfile,
//! a clean `git status`, and idempotency on re-run.

use std::fs;
use std::path::Path;
use std::process::Command;

use akit::catalog::Catalog;
use akit::lockfile::{ItemType, Lockfile, Mode};
use akit::ops;
use akit::project::Project;

fn git(args: &[&str], cwd: &Path) -> std::process::Output {
    Command::new("git")
        .args(args)
        .current_dir(cwd)
        .output()
        .expect("git should be available")
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

#[test]
fn add_skill_end_to_end_and_idempotent() {
    let tmp = tempfile::tempdir().unwrap();
    let base = tmp.path();

    // Catalog with one skill.
    let catalog_root = base.join("catalog");
    make_skill(&catalog_root, "demo");
    let catalog = Catalog::with_root(&catalog_root);

    // A real git project so `.git/info/exclude` exists.
    let proj = base.join("project");
    fs::create_dir_all(&proj).unwrap();
    assert!(git(&["init", "-q"], &proj).status.success());
    let project = Project::locate(Some(proj.clone())).unwrap();

    // --- add ---
    let report = ops::add_skill(&project, &catalog, "demo").unwrap();
    assert!(report.link_created);
    assert!(report.lock_added);
    assert!(report.exclude_added);
    assert!(!report.not_a_git_repo);

    // Symlink exists and resolves to the catalog source.
    let link = proj.join(".github/skills/demo");
    let meta = fs::symlink_metadata(&link).unwrap();
    assert!(meta.file_type().is_symlink(), "target should be a symlink");
    assert_eq!(
        link.canonicalize().unwrap(),
        catalog_root.join("skills/demo").canonicalize().unwrap()
    );

    // git/info/exclude has both lines.
    let exclude = fs::read_to_string(proj.join(".git/info/exclude")).unwrap();
    assert!(
        exclude.lines().any(|l| l == "/.github/skills/demo"),
        "exclude missing skill line:\n{exclude}"
    );
    assert!(
        exclude.lines().any(|l| l == "/.copilot/kit.lock.json"),
        "exclude missing lockfile line:\n{exclude}"
    );

    // Lockfile records the item.
    let lf = Lockfile::load(&project.lockfile_path()).unwrap();
    assert_eq!(lf.version, 1);
    assert_eq!(lf.items.len(), 1);
    assert_eq!(lf.items[0].id, "demo");
    assert_eq!(lf.items[0].item_type, ItemType::Skill);
    assert_eq!(lf.items[0].mode, Mode::Symlink);
    assert_eq!(lf.items[0].source, "local");
    assert_eq!(lf.items[0].target, ".github/skills/demo");

    // git status is clean (everything pulled is excluded).
    let status = git(&["status", "--porcelain"], &proj);
    assert!(
        status.stdout.is_empty(),
        "git status not clean: {}",
        String::from_utf8_lossy(&status.stdout)
    );

    // --- idempotent re-run ---
    let report2 = ops::add_skill(&project, &catalog, "demo").unwrap();
    assert!(!report2.link_created, "second add should not re-create link");
    assert!(!report2.lock_added, "second add should not duplicate lock entry");
    assert!(!report2.exclude_added, "second add should not duplicate exclude");

    let lf2 = Lockfile::load(&project.lockfile_path()).unwrap();
    assert_eq!(lf2.items.len(), 1, "no duplicate lockfile entries");

    let exclude2 = fs::read_to_string(proj.join(".git/info/exclude")).unwrap();
    assert_eq!(
        exclude2
            .lines()
            .filter(|l| *l == "/.github/skills/demo")
            .count(),
        1,
        "no duplicate exclude lines"
    );

    let status2 = git(&["status", "--porcelain"], &proj);
    assert!(status2.stdout.is_empty(), "git status not clean after re-run");
}

#[test]
fn unknown_skill_errors() {
    let tmp = tempfile::tempdir().unwrap();
    let base = tmp.path();
    let catalog = Catalog::with_root(base.join("catalog"));
    let proj = base.join("project");
    fs::create_dir_all(&proj).unwrap();
    git(&["init", "-q"], &proj);
    let project = Project::locate(Some(proj)).unwrap();

    let err = ops::add_skill(&project, &catalog, "missing").unwrap_err();
    assert!(err.to_string().contains("not found"), "got: {err}");
}
