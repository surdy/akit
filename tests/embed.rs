//! Embedding test for issue #10: drive akit purely through its public library API,
//! the way a GUI host (pterm) does — no `main`, no CLI. Exercises the whole surface
//! a host needs (resolve → add → list → search → doctor → sync) and proves the
//! returned reports serialize to JSON for the host to re-emit to its frontend.

use std::fs;
use std::path::Path;
use std::process::Command;

use akit::catalog::Catalog;
use akit::doctor;
use akit::lockfile::{ItemType, Mode};
use akit::ops::{self, HealthStatus};
use akit::project::Project;
use akit::search;

fn git(args: &[&str], cwd: &Path) {
    Command::new("git")
        .args(args)
        .current_dir(cwd)
        .output()
        .expect("git should be available");
}

fn make_skill(root: &Path, name: &str, desc: &str) {
    let dir = root.join("skills").join(name);
    fs::create_dir_all(&dir).unwrap();
    fs::write(
        dir.join("SKILL.md"),
        format!("---\nname: {name}\ndescription: {desc}\ncategory: ops\n---\nbody\n"),
    )
    .unwrap();
}

fn make_agent(root: &Path, name: &str, desc: &str) {
    let dir = root.join("agents");
    fs::create_dir_all(&dir).unwrap();
    fs::write(
        dir.join(format!("{name}.agent.md")),
        format!("---\nname: {name}\ndescription: {desc}\n---\nbody\n"),
    )
    .unwrap();
}

/// A downstream host can perform the full lifecycle through the public API alone.
#[test]
fn library_consumer_full_lifecycle() {
    let tmp = tempfile::tempdir().unwrap();
    let base = tmp.path();

    // Host owns where the catalog lives (env or explicit root).
    let catalog_root = base.join("catalog");
    make_skill(&catalog_root, "deploy-helper", "Ship apps safely");
    make_agent(&catalog_root, "reviewer", "Reviews diffs");
    let catalog = Catalog::with_root(&catalog_root);

    // Host resolves the project explicitly (e.g. the active workspace dir).
    let proj = base.join("project");
    fs::create_dir_all(&proj).unwrap();
    git(&["init", "-q"], &proj);
    let project = Project::locate(Some(proj.clone())).unwrap();

    // add — one skill, one agent.
    let skill_add = ops::add_skill(&project, &catalog, "deploy-helper").unwrap();
    assert!(skill_add.link_created);
    let agent_add = ops::add_item(
        &project,
        &catalog,
        ItemType::Agent,
        "reviewer",
        Mode::Symlink,
        None,
    )
    .unwrap();
    assert!(agent_add.link_created);

    // list — with health, and serializable to JSON for the host to re-emit.
    let items = ops::list_items_with_catalog(&project, &catalog).unwrap();
    assert_eq!(items.len(), 2);
    assert!(items.iter().all(|i| i.status == HealthStatus::Ok));
    let json = serde_json::to_string(&items).unwrap();
    assert!(json.contains("\"type\":\"skill\""), "json: {json}");
    assert!(json.contains("\"type\":\"agent\""), "json: {json}");
    assert!(json.contains("\"status\":\"ok\""), "json: {json}");

    // search — fuzzy over catalog frontmatter.
    let hits = search::search(&catalog, "deploy").unwrap();
    assert!(
        hits.iter().any(|h| h.name == "deploy-helper"),
        "hits: {hits:?}"
    );

    // doctor — read-only, healthy.
    let report = doctor::diagnose(&project, &catalog).unwrap();
    assert!(report.summary.healthy, "doctor: {report:?}");

    // Break a target, then sync repairs it.
    fs::remove_file(proj.join(".github/agents/reviewer.agent.md")).unwrap();
    let before = ops::list_items_with_catalog(&project, &catalog).unwrap();
    assert!(
        before
            .iter()
            .any(|i| i.id == "reviewer" && i.status == HealthStatus::Missing),
        "expected reviewer missing: {before:?}"
    );

    let sync = doctor::sync(&project, &catalog).unwrap();
    assert!(sync.summary.healthy, "sync should restore: {sync:?}");
    let after = ops::list_items_with_catalog(&project, &catalog).unwrap();
    assert!(
        after.iter().all(|i| i.status == HealthStatus::Ok),
        "all ok after sync: {after:?}"
    );

    // Reports themselves serialize (host re-emits them over the IPC boundary).
    assert!(serde_json::to_string(&skill_add).is_ok());
    assert!(serde_json::to_string(&report).is_ok());
    assert!(serde_json::to_string(&sync).is_ok());
}
