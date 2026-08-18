//! The global install index (issue #40): recording project paths, `akit where`,
//! and `akit update --propagate`.
//!
//! Index recording and `where` are exercised through the real binary (with
//! `AKIT_STATE_DIR` pointed at the fixture, never the developer's `~/.akit`);
//! propagation is driven through the library against an explicit index file, so
//! a mix of copy / symlink / drifted / missing / unreadable projects can be built
//! deterministically without a fake remote.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use akit::catalog::Catalog;
use akit::harness::HarnessId;
use akit::index::{self, InstallIndex, PropagateStatus};
use akit::install::{self, HarnessContext, InstallOptions};
use akit::lockfile::{ItemType, Mode};
use akit::project::Project;

// ── fixtures ─────────────────────────────────────────────────────────────────

fn make_skill(catalog: &Path, name: &str, body: &str) {
    let dir = catalog.join("skills").join(name);
    fs::create_dir_all(&dir).unwrap();
    fs::write(
        dir.join("SKILL.md"),
        format!("---\nname: {name}\ndescription: t\n---\n{body}\n"),
    )
    .unwrap();
}

fn make_project(base: &Path, name: &str) -> PathBuf {
    let root = base.join(name);
    fs::create_dir_all(&root).unwrap();
    root
}

/// A project that is a real git repo, so the managed exclude block is written
/// (and can be asserted to stay free of index state).
fn make_git_project(base: &Path, name: &str) -> PathBuf {
    let root = make_project(base, name);
    let ok = Command::new("git")
        .args(["init", "-q"])
        .current_dir(&root)
        .output()
        .expect("git available")
        .status
        .success();
    assert!(ok, "git init failed");
    root
}

/// Run the akit binary with the catalog + state dir pointed at the fixture.
fn akit(args: &[&str], project: Option<&Path>, catalog: &Path, state: &Path) -> (String, bool) {
    let mut command = Command::new(env!("CARGO_BIN_EXE_akit"));
    if let Some(project) = project {
        command.args(["--project", project.to_str().unwrap()]);
    }
    let out = command
        .args(args)
        .env("KIT_CATALOG_DIR", catalog)
        .env("AKIT_STATE_DIR", state)
        .env_remove("AKIT_HARNESSES")
        .stdin(std::process::Stdio::null())
        .output()
        .expect("akit binary runs");
    (
        String::from_utf8_lossy(&out.stdout).into_owned(),
        out.status.success(),
    )
}

fn install_skill(root: &Path, catalog: &Catalog, id: &str, harnesses: &[HarnessId], symlink: bool) {
    let project = Project::at(root);
    let ctx = HarnessContext::new(harnesses.to_vec()).unwrap();
    install::install_opts(
        &project,
        catalog,
        ItemType::Skill,
        id,
        &ctx,
        InstallOptions {
            force_symlink: symlink,
        },
    )
    .unwrap();
}

/// Write an index file listing `roots` verbatim (no canonicalization), as the
/// CLI would have recorded them.
fn write_index(path: &Path, roots: &[&Path]) {
    let mut doc = InstallIndex::default();
    for root in roots {
        doc.upsert(&root.to_string_lossy(), Some(1));
    }
    doc.save(path).unwrap();
}

fn canonical(path: &Path) -> String {
    fs::canonicalize(path)
        .unwrap()
        .to_string_lossy()
        .into_owned()
}

// ── index recording + pruning ────────────────────────────────────────────────

#[test]
fn install_records_every_project_in_the_global_index() {
    let tmp = tempfile::tempdir().unwrap();
    let catalog = tmp.path().join("catalog");
    let state = tmp.path().join("state");
    make_skill(&catalog, "demo", "body");
    let a = make_git_project(tmp.path(), "a");
    let b = make_git_project(tmp.path(), "b");

    for project in [&a, &b] {
        let (out, ok) = akit(
            &["install", "-H", "claude", "demo"],
            Some(project),
            &catalog,
            &state,
        );
        assert!(ok, "install failed: {out}");
    }
    // Re-installing must refresh the existing entry, not duplicate it.
    akit(
        &["install", "-H", "copilot", "demo"],
        Some(&a),
        &catalog,
        &state,
    );

    let index_file = state.join("installs.json");
    let doc = InstallIndex::load(&index_file);
    let paths: Vec<&str> = doc.projects.iter().map(|e| e.path.as_str()).collect();
    assert_eq!(doc.version, 1);
    assert_eq!(paths, vec![canonical(&a), canonical(&b)], "{doc:?}");
    assert!(doc.projects.iter().all(|e| e.last_install.is_some()));

    // The index is host state: nothing about it lands in a project.
    assert!(!a.join(".akit/installs.json").exists());
    assert!(!a.join("installs.json").exists());
    let excludes = fs::read_to_string(a.join(".git/info/exclude")).unwrap();
    assert!(excludes.contains(".akit/kit.lock.json"), "{excludes}");
    assert!(!excludes.contains("installs.json"), "{excludes}");
}

#[test]
fn reading_the_index_tolerates_and_prunes_stale_entries() {
    let tmp = tempfile::tempdir().unwrap();
    let catalog = Catalog::with_root(tmp.path().join("catalog"));
    make_skill(&catalog.root, "demo", "body");

    let live = make_project(tmp.path(), "live");
    install_skill(&live, &catalog, "demo", &[HarnessId::Claude], false);
    // Exists, but akit is no longer installed there (no `.akit` lockfile).
    let no_lock = make_project(tmp.path(), "no-lock");
    // Never existed / deleted since it was recorded.
    let gone = tmp.path().join("gone");

    let index_file = tmp.path().join("state").join("installs.json");
    write_index(&index_file, &[&live, &no_lock, &gone]);

    // Stale entries are skipped, never an error…
    let known = index::known_projects_at(&index_file);
    assert_eq!(known, vec![live.clone()]);
    // …and pruned from the file in place, so the next read is cheap.
    let doc = InstallIndex::load(&index_file);
    assert_eq!(doc.projects.len(), 1);
    assert_eq!(doc.projects[0].path, live.to_string_lossy());

    // A corrupt index is a rebuildable cache, not a failure.
    fs::write(&index_file, "{ not json").unwrap();
    assert!(index::known_projects_at(&index_file).is_empty());
}

// ── where ────────────────────────────────────────────────────────────────────

#[test]
fn where_lists_known_projects_holding_the_item_with_health() {
    let tmp = tempfile::tempdir().unwrap();
    let catalog = tmp.path().join("catalog");
    let state = tmp.path().join("state");
    make_skill(&catalog, "demo", "body");
    make_skill(&catalog, "other", "body");

    let a = make_project(tmp.path(), "a");
    let b = make_project(tmp.path(), "b");
    let c = make_project(tmp.path(), "c");
    akit(
        &["install", "-H", "claude", "demo"],
        Some(&a),
        &catalog,
        &state,
    );
    akit(
        &["install", "-H", "copilot", "-H", "codex", "demo"],
        Some(&b),
        &catalog,
        &state,
    );
    // Known to the index, but holds a different item — must not appear.
    akit(
        &["install", "-H", "claude", "other"],
        Some(&c),
        &catalog,
        &state,
    );

    // Index-driven: no --project, so this is the "from any cwd" path.
    let (out, ok) = akit(&["--json", "where", "demo"], None, &catalog, &state);
    assert!(ok, "where failed: {out}");
    let json: serde_json::Value = serde_json::from_str(&out).unwrap();
    assert_eq!(json["id"], "demo");
    assert_eq!(json["type"], "skill");
    let projects = json["projects"].as_array().unwrap();
    assert_eq!(projects.len(), 2, "{out}");
    assert_eq!(projects[0]["project"], canonical(&a));
    assert_eq!(projects[1]["project"], canonical(&b));
    assert_eq!(
        projects[0]["health"]["harnesses"],
        serde_json::json!(["claude"])
    );
    assert_eq!(
        projects[0]["health"]["materializations"][0]["path"],
        ".claude/skills/demo"
    );
    assert_eq!(
        projects[0]["health"]["materializations"][0]["drift"],
        "clean"
    );
    assert_eq!(
        projects[1]["health"]["materializations"][0]["path"],
        ".agents/skills/demo"
    );
    assert!(json["skipped"].as_array().unwrap().is_empty());

    // Drift in one project surfaces per-materialization, not as a failure.
    fs::write(a.join(".claude/skills/demo/SKILL.md"), "edited").unwrap();
    let (out, ok) = akit(&["where", "demo"], None, &catalog, &state);
    assert!(ok, "where failed: {out}");
    assert!(out.contains("installed in 2 project(s)"), "{out}");
    assert!(out.contains(&canonical(&a)), "{out}");
    assert!(out.contains("modified"), "{out}");
    assert!(!out.contains(&canonical(&c)), "{out}");

    // An item nobody installed is an empty (successful) report.
    let (out, ok) = akit(&["where", "other-missing"], None, &catalog, &state);
    assert!(ok, "{out}");
    assert!(out.contains("not installed in any known project"), "{out}");
}

// ── cross-project divergence (issue #41) ─────────────────────────────────────

/// The same catalog id can be perfectly `clean` in two projects and still hold
/// different bytes in each — that is the conflict class `drift` cannot see.
/// Copies are grouped by content; symlink installs resolve to the catalog and so
/// are never part of a divergence.
#[test]
fn divergence_groups_copy_installs_by_content_and_ignores_symlinks() {
    let tmp = tempfile::tempdir().unwrap();
    let catalog = Catalog::with_root(tmp.path().join("catalog"));
    make_skill(&catalog.root, "demo", "v1");

    let a = make_project(tmp.path(), "a");
    install_skill(&a, &catalog, "demo", &[HarnessId::Copilot], false);
    let b = make_project(tmp.path(), "b");
    install_skill(&b, &catalog, "demo", &[HarnessId::Copilot], false);
    let link = make_project(tmp.path(), "link");
    install_skill(&link, &catalog, "demo", &[HarnessId::Claude], true);

    let index_file = tmp.path().join("state").join("installs.json");
    write_index(&index_file, &[&a, &b, &link]);

    // Every copy holds the same catalog content: one variant, no divergence.
    let report = index::locate_at(&index_file, &catalog, ItemType::Skill, "demo").unwrap();
    assert_eq!(report.projects.len(), 3, "{report:?}");
    assert!(!report.diverged, "{report:?}");
    assert_eq!(report.variants.len(), 1);
    assert!(index::divergences_at(&index_file).is_empty());

    // One project's copy is hand-edited. Both projects still read `clean`
    // against their own recorded hashes, but they no longer agree.
    let edited = b.join(".agents/skills/demo/SKILL.md");
    fs::write(&edited, "hand-edited").unwrap();

    let report = index::locate_at(&index_file, &catalog, ItemType::Skill, "demo").unwrap();
    assert!(report.diverged, "{report:?}");
    assert_eq!(report.variants.len(), 2);
    let paths: Vec<&str> = report
        .variants
        .iter()
        .flat_map(|v| v.paths.iter().map(String::as_str))
        .collect();
    let holds = |root: &Path| paths.iter().any(|p| p.starts_with(root.to_str().unwrap()));
    assert!(holds(&a), "{paths:?}");
    assert!(holds(&b), "{paths:?}");
    // A symlink tracks the catalog live and cannot diverge, so it is not listed.
    assert!(!holds(&link), "{paths:?}");
    // Each group names exactly the paths that share its content.
    assert!(report.variants.iter().all(|v| v.paths.len() == 1));

    // The index-wide sweep finds the same id without being told which to check.
    let diverged = index::divergences_at(&index_file);
    assert_eq!(diverged.len(), 1, "{diverged:?}");
    assert_eq!(diverged[0].id, "demo");
    assert_eq!(diverged[0].item_type, ItemType::Skill);
    assert_eq!(diverged[0].variants.len(), 2);

    // Detection is read-only: the user's edit survives untouched.
    assert_eq!(fs::read_to_string(&edited).unwrap(), "hand-edited");

    // Bringing the outlier back in line collapses the groups again.
    fs::write(&edited, "---\nname: demo\ndescription: t\n---\nv1\n").unwrap();
    assert!(index::divergences_at(&index_file).is_empty());
}

/// `where --json` gains `variants`/`diverged`, and `doctor` gains `foreign`
/// always plus `divergences` only under `--all`.
#[test]
fn doctor_and_where_json_gain_the_new_conflict_states() {
    let tmp = tempfile::tempdir().unwrap();
    let catalog = tmp.path().join("catalog");
    let state = tmp.path().join("state");
    make_skill(&catalog, "demo", "v1");

    let a = make_git_project(tmp.path(), "a");
    let b = make_git_project(tmp.path(), "b");
    for project in [&a, &b] {
        let (out, ok) = akit(
            &["install", "-H", "copilot", "demo"],
            Some(project),
            &catalog,
            &state,
        );
        assert!(ok, "install failed: {out}");
    }

    // Baseline: `foreign` is present and empty, `divergences` absent without --all.
    let (out, ok) = akit(&["--json", "doctor"], Some(&a), &catalog, &state);
    assert!(ok, "doctor failed: {out}");
    let json: serde_json::Value = serde_json::from_str(&out).unwrap();
    assert_eq!(json["healthy"], true, "{out}");
    assert!(json["foreign"].as_array().unwrap().is_empty(), "{out}");
    assert!(json.get("divergences").is_none(), "{out}");

    // A hand-written skill in a managed target path is reported, not touched…
    let foreign = a.join(".github/skills/handmade");
    fs::create_dir_all(&foreign).unwrap();
    fs::write(foreign.join("SKILL.md"), "mine").unwrap();
    let (out, ok) = akit(&["--json", "doctor"], Some(&a), &catalog, &state);
    assert!(ok, "{out}");
    let json: serde_json::Value = serde_json::from_str(&out).unwrap();
    let entries = json["foreign"].as_array().unwrap();
    assert_eq!(entries.len(), 1, "{out}");
    assert_eq!(entries[0]["path"], ".github/skills/handmade");
    assert_eq!(entries[0]["type"], "skill");
    assert_eq!(entries[0]["harnesses"], serde_json::json!(["copilot"]));
    // …and it is not akit drift, so the existing verdict is unchanged.
    assert_eq!(json["healthy"], true, "{out}");
    assert_eq!(
        fs::read_to_string(foreign.join("SKILL.md")).unwrap(),
        "mine"
    );
    // Nor is a foreign occupant ever claimed: an install that targets one is
    // refused and leaves the bytes alone (the #32 guard, seen from the CLI).
    make_skill(&catalog, "other", "v1");
    let occupied = a.join(".agents/skills/other");
    fs::create_dir_all(&occupied).unwrap();
    fs::write(occupied.join("SKILL.md"), "not akit's").unwrap();
    let (out, ok) = akit(
        &["install", "-H", "copilot", "other"],
        Some(&a),
        &catalog,
        &state,
    );
    assert!(!ok, "install over a foreign file must fail: {out}");
    assert_eq!(
        fs::read_to_string(occupied.join("SKILL.md")).unwrap(),
        "not akit's"
    );

    // Diverge the two projects, then ask the index-wide question.
    fs::write(b.join(".agents/skills/demo/SKILL.md"), "hand-edited").unwrap();
    let (out, ok) = akit(&["--json", "doctor", "--all"], Some(&a), &catalog, &state);
    assert!(ok, "doctor --all failed: {out}");
    let json: serde_json::Value = serde_json::from_str(&out).unwrap();
    let diverged = json["divergences"].as_array().unwrap();
    assert_eq!(diverged.len(), 1, "{out}");
    assert_eq!(diverged[0]["id"], "demo");
    assert_eq!(diverged[0]["type"], "skill");
    assert_eq!(diverged[0]["variants"].as_array().unwrap().len(), 2);

    // `where` answers the same question for one id, from any cwd.
    let (out, ok) = akit(&["--json", "where", "demo"], None, &catalog, &state);
    assert!(ok, "{out}");
    let json: serde_json::Value = serde_json::from_str(&out).unwrap();
    assert_eq!(json["diverged"], true, "{out}");
    let variants = json["variants"].as_array().unwrap();
    assert_eq!(variants.len(), 2, "{out}");
    assert!(
        variants
            .iter()
            .all(|v| !v["hash"].as_str().unwrap().is_empty())
    );

    let (out, ok) = akit(&["where", "demo"], None, &catalog, &state);
    assert!(ok, "{out}");
    assert!(out.contains("Diverged: 2 distinct contents"), "{out}");
    assert!(out.contains(&canonical(&b)), "{out}");
}

// ── propagate ────────────────────────────────────────────────────────────────

/// Build one project per propagation outcome, refresh the catalog, and assert
/// each is handled per policy: clean copies re-materialize, drifted copies are
/// conflicts, symlinks are already live, missing files are repair's job, and an
/// unreadable project is skipped rather than fatal.
#[test]
fn propagate_updates_clean_copies_and_skips_the_rest() {
    let tmp = tempfile::tempdir().unwrap();
    let catalog = Catalog::with_root(tmp.path().join("catalog"));
    make_skill(&catalog.root, "demo", "v1");
    make_skill(&catalog.root, "unrelated", "v1");

    let copy = make_project(tmp.path(), "copy");
    install_skill(&copy, &catalog, "demo", &[HarnessId::Copilot], false);
    let symlink = make_project(tmp.path(), "symlink");
    install_skill(&symlink, &catalog, "demo", &[HarnessId::Claude], true);
    let drifted = make_project(tmp.path(), "drifted");
    install_skill(&drifted, &catalog, "demo", &[HarnessId::Copilot], false);
    fs::write(drifted.join(".agents/skills/demo/SKILL.md"), "hand-edited").unwrap();
    let missing = make_project(tmp.path(), "missing");
    install_skill(&missing, &catalog, "demo", &[HarnessId::Copilot], false);
    fs::remove_dir_all(missing.join(".agents/skills/demo")).unwrap();
    // Holds a different item: known, walked, but nothing to report.
    let unrelated = make_project(tmp.path(), "unrelated-item");
    install_skill(
        &unrelated,
        &catalog,
        "unrelated",
        &[HarnessId::Claude],
        false,
    );
    // A lockfile this akit cannot read at all.
    let broken = make_project(tmp.path(), "broken");
    fs::create_dir_all(broken.join(".akit")).unwrap();
    fs::write(
        broken.join(".akit/kit.lock.json"),
        r#"{"version":1,"items":[]}"#,
    )
    .unwrap();

    // The symlink install must already be pointing at the catalog.
    assert!(
        fs::symlink_metadata(symlink.join(".claude/skills/demo"))
            .unwrap()
            .file_type()
            .is_symlink()
    );

    let index_file = tmp.path().join("state").join("installs.json");
    write_index(
        &index_file,
        &[&copy, &symlink, &drifted, &missing, &unrelated, &broken],
    );

    // The catalog moves on (what `update` would have just fetched).
    make_skill(&catalog.root, "demo", "v2");

    let targets = vec![(ItemType::Skill, "demo".to_string())];
    let report = index::propagate_at(&index_file, &catalog, &targets).unwrap();

    assert_eq!(report.summary.projects, 6, "{report:?}");
    assert_eq!(report.summary.updated, 1);
    assert_eq!(report.summary.symlink, 1);
    assert_eq!(report.summary.drifted, 1);
    assert_eq!(report.summary.missing, 1);
    assert_eq!(report.summary.errors, 1);

    let status_of = |root: &Path| -> PropagateStatus {
        let entry = report
            .projects
            .iter()
            .find(|p| p.project == root.to_string_lossy())
            .unwrap_or_else(|| panic!("no propagation entry for {}", root.display()));
        entry.items[0].materializations[0].status
    };
    assert_eq!(status_of(&copy), PropagateStatus::Updated);
    assert_eq!(status_of(&symlink), PropagateStatus::Symlink);
    assert_eq!(status_of(&drifted), PropagateStatus::Drifted);
    assert_eq!(status_of(&missing), PropagateStatus::Missing);

    // Only the clean copy was rewritten; the drifted one keeps the user's bytes.
    assert!(
        fs::read_to_string(copy.join(".agents/skills/demo/SKILL.md"))
            .unwrap()
            .contains("v2")
    );
    assert_eq!(
        fs::read_to_string(drifted.join(".agents/skills/demo/SKILL.md")).unwrap(),
        "hand-edited"
    );
    // The symlink needed no work: it resolves to the refreshed catalog already.
    assert!(
        fs::read_to_string(symlink.join(".claude/skills/demo/SKILL.md"))
            .unwrap()
            .contains("v2")
    );
    // A project that only holds other items reports nothing at all.
    assert!(
        !report
            .projects
            .iter()
            .any(|p| p.project == unrelated.to_string_lossy())
    );
    // The unreadable project is reported and skipped, never fatal.
    let broken_entry = report
        .projects
        .iter()
        .find(|p| p.project == broken.to_string_lossy())
        .expect("broken project reported");
    assert!(broken_entry.items.is_empty());
    assert!(
        broken_entry
            .error
            .as_deref()
            .unwrap()
            .contains("schema version 1")
    );

    // The re-materialized copy's recorded hash was updated, so it reads clean…
    let health = akit::reconcile::health(&Project::at(&copy), &catalog).unwrap();
    assert_eq!(
        health.items[0].materializations[0].drift,
        akit::materialize::Drift::Clean
    );
    assert_eq!(health.items[0].materializations[0].mode, Mode::Copy);
    // …and a second run has nothing left to do.
    let again = index::propagate_at(&index_file, &catalog, &targets).unwrap();
    assert_eq!(again.summary.updated, 0);
    assert_eq!(again.summary.up_to_date, 1);
}

#[test]
fn propagate_reports_an_item_whose_catalog_source_is_gone() {
    let tmp = tempfile::tempdir().unwrap();
    let catalog = Catalog::with_root(tmp.path().join("catalog"));
    make_skill(&catalog.root, "demo", "v1");
    let project = make_project(tmp.path(), "p");
    install_skill(&project, &catalog, "demo", &[HarnessId::Copilot], false);

    let index_file = tmp.path().join("state").join("installs.json");
    write_index(&index_file, &[&project]);
    fs::remove_dir_all(catalog.root.join("skills/demo")).unwrap();

    let report = index::propagate_at(
        &index_file,
        &catalog,
        &[(ItemType::Skill, "demo".to_string())],
    )
    .unwrap();
    assert_eq!(report.summary.errors, 1);
    let item = &report.projects[0].items[0];
    assert!(item.materializations.is_empty());
    assert!(item.error.is_some(), "{item:?}");
    // The materialized copy is left exactly as it was.
    assert!(project.join(".agents/skills/demo/SKILL.md").is_file());
}

/// Agents are always copies of a per-harness variant file, so they are the other
/// half of the propagation contract: `where --agent` finds them and a refreshed
/// variant reaches the projects that copied it.
#[test]
fn where_and_propagate_cover_agent_packages() {
    let tmp = tempfile::tempdir().unwrap();
    let catalog_root = tmp.path().join("catalog");
    let state = tmp.path().join("state");
    let catalog = Catalog::with_root(&catalog_root);
    let pkg = catalog_root.join("agents/reviewer");
    fs::create_dir_all(&pkg).unwrap();
    fs::write(
        pkg.join("agent.yml"),
        "id: reviewer\ndescription: t\nvariants:\n  copilot: copilot.agent.md\n",
    )
    .unwrap();
    fs::write(pkg.join("copilot.agent.md"), "---\nname: r\n---\nv1\n").unwrap();

    let project = make_project(tmp.path(), "p");
    let (out, ok) = akit(
        &["install", "--agent", "-H", "copilot", "reviewer"],
        Some(&project),
        &catalog_root,
        &state,
    );
    assert!(ok, "install failed: {out}");
    let installed = project.join(".github/agents/reviewer.agent.md");
    assert!(installed.is_file());

    let (out, ok) = akit(
        &["where", "--agent", "reviewer"],
        None,
        &catalog_root,
        &state,
    );
    assert!(ok, "{out}");
    assert!(out.contains("Agent 'reviewer'"), "{out}");
    assert!(out.contains(".github/agents/reviewer.agent.md"), "{out}");
    // The skill namespace is separate: the same id as a skill finds nothing.
    let (out, ok) = akit(&["where", "reviewer"], None, &catalog_root, &state);
    assert!(ok, "{out}");
    assert!(out.contains("not installed in any known project"), "{out}");

    fs::write(pkg.join("copilot.agent.md"), "---\nname: r\n---\nv2\n").unwrap();
    let report = index::propagate_at(
        &state.join("installs.json"),
        &catalog,
        &[(ItemType::Agent, "reviewer".to_string())],
    )
    .unwrap();
    assert_eq!(report.summary.updated, 1, "{report:?}");
    assert!(fs::read_to_string(&installed).unwrap().contains("v2"));
}

#[test]
fn propagate_with_no_targets_does_nothing() {
    let tmp = tempfile::tempdir().unwrap();
    let catalog = Catalog::with_root(tmp.path().join("catalog"));
    let index_file = tmp.path().join("state").join("installs.json");
    let report = index::propagate_at(&index_file, &catalog, &[]).unwrap();
    assert_eq!(report.summary.projects, 0);
    assert!(report.projects.is_empty());
}

// ── CLI wiring ───────────────────────────────────────────────────────────────

#[test]
fn update_json_stays_unchanged_without_propagate() {
    let tmp = tempfile::tempdir().unwrap();
    let catalog = tmp.path().join("catalog");
    let state = tmp.path().join("state");
    make_skill(&catalog, "demo", "body");

    let (out, ok) = akit(&["--json", "update"], None, &catalog, &state);
    assert!(ok, "update failed: {out}");
    // Additive by construction: the propagation object appears only with the flag.
    assert!(!out.contains("propagation"), "{out}");

    let (out, ok) = akit(
        &["update", "--check", "--propagate"],
        None,
        &catalog,
        &state,
    );
    assert!(!ok, "--check with --propagate must be refused: {out}");
}
