use std::fs;
use std::path::Path;
use std::process::Command;

use akit::collection::Collection;
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

fn init_project(base: &Path) -> (std::path::PathBuf, Project) {
    let proj = base.join("project");
    fs::create_dir_all(&proj).unwrap();
    assert!(git(&["init", "-q"], &proj).status.success());
    let project = Project::locate(Some(proj.clone())).unwrap();
    (proj, project)
}

fn make_skill(collection_root: &Path, name: &str) {
    let dir = collection_root.join("skills").join(name);
    fs::create_dir_all(&dir).unwrap();
    fs::write(
        dir.join("SKILL.md"),
        format!("---\nname: {name}\ndescription: a test skill\n---\nbody\n"),
    )
    .unwrap();
}

fn make_agent(collection_root: &Path, name: &str) {
    let dir = collection_root.join("agents");
    fs::create_dir_all(&dir).unwrap();
    fs::write(
        dir.join(format!("{name}.agent.md")),
        format!("---\nname: {name}\ndescription: a test agent\n---\nbody\n"),
    )
    .unwrap();
}

#[test]
fn rm_closes_loop_for_skill_and_is_idempotent() {
    let tmp = tempfile::tempdir().unwrap();
    let base = tmp.path();
    let collection_root = base.join("collection");
    make_skill(&collection_root, "demo");
    let collection = Collection::with_root(&collection_root);
    let (proj, project) = init_project(base);

    ops::add_skill(&project, &collection, "demo").unwrap();
    let items = ops::list_items(&project).unwrap();
    assert_eq!(items.len(), 1);
    assert_eq!(items[0].item_type, ItemType::Skill);
    assert_eq!(items[0].mode, Mode::Symlink);
    assert_eq!(items[0].target, ".github/skills/demo");
    assert_eq!(items[0].status, HealthStatus::Ok);

    let report = ops::remove_skill(&project, "demo").unwrap();
    assert!(report.target_removed);
    assert!(report.exclude_removed);
    assert!(report.lock_removed);
    assert!(!report.not_installed);

    assert!(ops::list_items(&project).unwrap().is_empty());

    let exclude = fs::read_to_string(proj.join(".git/info/exclude")).unwrap();
    assert!(
        !exclude.lines().any(|l| l == "/.github/skills/demo"),
        "exclude still has skill line:\n{exclude}"
    );
    let lf = Lockfile::load(&project.lockfile_path()).unwrap();
    assert!(lf.items.is_empty(), "lockfile entry was not removed");

    let status = git(&["status", "--porcelain"], &proj);
    assert!(
        status.stdout.is_empty(),
        "git status not clean after rm: {}",
        String::from_utf8_lossy(&status.stdout)
    );

    let second = ops::remove_skill(&project, "demo").unwrap();
    assert!(second.not_installed);
    assert!(!second.target_removed);
    assert!(!second.exclude_removed);
    assert!(!second.lock_removed);

    let status2 = git(&["status", "--porcelain"], &proj);
    assert!(
        status2.stdout.is_empty(),
        "git status not clean after no-op rm: {}",
        String::from_utf8_lossy(&status2.stdout)
    );
}

#[test]
fn agent_add_appears_as_symlink_and_lists_type_agent() {
    let tmp = tempfile::tempdir().unwrap();
    let base = tmp.path();
    let collection_root = base.join("collection");
    make_agent(&collection_root, "helper");
    let collection = Collection::with_root(&collection_root);
    let (proj, project) = init_project(base);

    let report = ops::add_item(
        &project,
        &collection,
        ItemType::Agent,
        "helper",
        Mode::Symlink,
        None,
    )
    .unwrap();
    assert_eq!(report.item_type, ItemType::Agent);
    assert_eq!(report.target, ".github/agents/helper.agent.md");
    assert!(report.link_created);

    let link = proj.join(".github/agents/helper.agent.md");
    let meta = fs::symlink_metadata(&link).unwrap();
    assert!(meta.file_type().is_symlink(), "agent should be a symlink");
    assert_eq!(
        link.canonicalize().unwrap(),
        collection_root
            .join("agents/helper.agent.md")
            .canonicalize()
            .unwrap()
    );

    let items = ops::list_items(&project).unwrap();
    assert_eq!(items.len(), 1);
    assert_eq!(items[0].item_type, ItemType::Agent);
    assert_eq!(items[0].mode, Mode::Symlink);
    assert_eq!(items[0].target, ".github/agents/helper.agent.md");
    assert_eq!(items[0].status, HealthStatus::Ok);
}

#[test]
fn ls_reports_orphaned_and_missing_targets() {
    let tmp = tempfile::tempdir().unwrap();
    let base = tmp.path();
    let collection_root = base.join("collection");
    make_skill(&collection_root, "demo");
    let collection = Collection::with_root(&collection_root);
    let (proj, project) = init_project(base);

    ops::add_skill(&project, &collection, "demo").unwrap();
    assert_eq!(
        ops::list_items(&project).unwrap()[0].status,
        HealthStatus::Ok
    );

    fs::remove_dir_all(collection_root.join("skills/demo")).unwrap();
    assert_eq!(
        ops::list_items(&project).unwrap()[0].status,
        HealthStatus::Orphaned
    );

    fs::remove_file(proj.join(".github/skills/demo")).unwrap();
    assert_eq!(
        ops::list_items(&project).unwrap()[0].status,
        HealthStatus::Missing
    );
}

#[test]
fn cli_status_alias_outputs_json_without_collection() {
    let tmp = tempfile::tempdir().unwrap();
    let base = tmp.path();
    let (proj, _project) = init_project(base);

    let output = Command::new(env!("CARGO_BIN_EXE_akit"))
        .args(["--project", proj.to_str().unwrap(), "--json", "status"])
        .output()
        .expect("akit binary should run");

    assert!(
        output.status.success(),
        "akit failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(String::from_utf8_lossy(&output.stdout), "[]\n");
}
