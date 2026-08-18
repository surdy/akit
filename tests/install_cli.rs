//! End-to-end CLI tests for the harness-aware surface (issue #34):
//! `install` / `uninstall` / `installed` / `reset`, exercised through the real
//! `akit` binary with an explicit catalog + git project.

use std::fs;
use std::path::Path;
use std::process::Command;

fn git(args: &[&str], cwd: &Path) {
    let ok = Command::new("git")
        .args(args)
        .current_dir(cwd)
        .output()
        .expect("git available")
        .status
        .success();
    assert!(ok, "git {args:?} failed");
}

/// Run the akit binary against `project` with `KIT_CATALOG_DIR` pointed at
/// `catalog`, returning (stdout, success).
fn akit(project: &Path, catalog: &Path, args: &[&str]) -> (String, bool) {
    let out = Command::new(env!("CARGO_BIN_EXE_akit"))
        .args(["--project", project.to_str().unwrap()])
        .args(args)
        .env("KIT_CATALOG_DIR", catalog)
        // Keep the global install index (#40) inside the fixture, never the
        // developer's real `~/.akit/state`.
        .env("AKIT_STATE_DIR", catalog.with_file_name("akit-state"))
        // Never inherit a developer's ambient default harnesses.
        .env_remove("AKIT_HARNESSES")
        // Non-interactive: prompts must not hang the test.
        .stdin(std::process::Stdio::null())
        .output()
        .expect("akit binary runs");
    (
        String::from_utf8_lossy(&out.stdout).into_owned(),
        out.status.success(),
    )
}

fn make_skill(catalog: &Path, name: &str) {
    let dir = catalog.join("skills").join(name);
    fs::create_dir_all(&dir).unwrap();
    fs::write(
        dir.join("SKILL.md"),
        format!("---\nname: {name}\ndescription: t\n---\nbody\n"),
    )
    .unwrap();
}

fn make_agent_pkg(catalog: &Path, id: &str, variants: &[(&str, &str)]) {
    let dir = catalog.join("agents").join(id);
    fs::create_dir_all(&dir).unwrap();
    let mut yml = format!("id: {id}\ndescription: t\nvariants:\n");
    for (harness, file) in variants {
        yml.push_str(&format!("  {harness}: {file}\n"));
        fs::write(dir.join(file), "---\nname: a\n---\nprompt\n").unwrap();
    }
    fs::write(dir.join("agent.yml"), yml).unwrap();
}

fn setup() -> (tempfile::TempDir, std::path::PathBuf, std::path::PathBuf) {
    let tmp = tempfile::tempdir().unwrap();
    let catalog = tmp.path().join("catalog");
    make_skill(&catalog, "demo");
    let project = tmp.path().join("project");
    fs::create_dir_all(&project).unwrap();
    git(&["init", "-q"], &project);
    (tmp, catalog, project)
}

#[test]
fn install_shares_a_path_across_compatible_harnesses() {
    let (_tmp, catalog, project) = setup();
    let (out, ok) = akit(
        &project,
        &catalog,
        &[
            "install",
            "--harness",
            "copilot",
            "--harness",
            "codex",
            "demo",
        ],
    );
    assert!(ok, "install failed: {out}");
    // A single shared destination covers both harnesses.
    assert!(project.join(".agents/skills/demo/SKILL.md").exists());
    assert!(!project.join(".claude/skills/demo").exists());

    let (listed, ok) = akit(&project, &catalog, &["installed"]);
    assert!(ok);
    assert!(listed.contains("demo"), "installed missing demo:\n{listed}");
    assert!(listed.contains("copilot, codex"), "harnesses:\n{listed}");
}

#[test]
fn install_reshapes_to_exactly_the_new_harness_set() {
    let (_tmp, catalog, project) = setup();
    akit(&project, &catalog, &["install", "-H", "copilot", "demo"]);
    assert!(project.join(".agents/skills/demo").exists());

    // Re-install for claude only: the old .agents copy must be removed.
    let (out, ok) = akit(&project, &catalog, &["install", "-H", "claude", "demo"]);
    assert!(ok, "reshape failed: {out}");
    assert!(project.join(".claude/skills/demo/SKILL.md").exists());
    assert!(
        !project.join(".agents/skills/demo").exists(),
        "stale materialization left behind after reshape"
    );
}

#[test]
fn partial_uninstall_keeps_remaining_harnesses() {
    let (_tmp, catalog, project) = setup();
    akit(
        &project,
        &catalog,
        &["install", "-H", "copilot", "-H", "claude", "demo"],
    );
    assert!(project.join(".claude/skills/demo").exists());

    let (out, ok) = akit(
        &project,
        &catalog,
        &["--json", "uninstall", "-H", "claude", "demo"],
    );
    assert!(ok, "uninstall failed: {out}");
    assert!(out.contains("\"not_installed\":false"), "json:\n{out}");
    // Claude dropped, copilot's shared path retained.
    assert!(!project.join(".claude/skills/demo").exists());
    assert!(project.join(".agents/skills/demo").exists());
}

#[test]
fn env_var_supplies_default_harnesses() {
    let (_tmp, catalog, project) = setup();
    let out = Command::new(env!("CARGO_BIN_EXE_akit"))
        .args(["--project", project.to_str().unwrap(), "install", "demo"])
        .env("KIT_CATALOG_DIR", &catalog)
        .env("AKIT_STATE_DIR", catalog.with_file_name("akit-state"))
        .env("AKIT_HARNESSES", "copilot codex")
        .stdin(std::process::Stdio::null())
        .output()
        .unwrap();
    assert!(out.status.success());
    assert!(project.join(".agents/skills/demo").exists());
}

#[test]
fn config_file_supplies_default_harnesses() {
    let (_tmp, catalog, project) = setup();
    fs::create_dir_all(project.join(".akit")).unwrap();
    fs::write(
        project.join(".akit/config.json"),
        r#"{"harnesses":["gemini"]}"#,
    )
    .unwrap();
    let (out, ok) = akit(&project, &catalog, &["install", "demo"]);
    assert!(ok, "install via config failed: {out}");
    assert!(project.join(".agents/skills/demo").exists());
}

#[test]
fn install_without_any_harness_source_errors_non_interactively() {
    let (_tmp, catalog, project) = setup();
    let (_out, ok) = akit(&project, &catalog, &["install", "demo"]);
    assert!(!ok, "expected non-interactive install to fail");
}

#[test]
fn reset_removes_all_owned_files() {
    let (_tmp, catalog, project) = setup();
    akit(&project, &catalog, &["install", "-H", "copilot", "demo"]);
    assert!(project.join(".agents/skills/demo").exists());

    let (out, ok) = akit(&project, &catalog, &["reset", "--yes"]);
    assert!(ok, "reset failed: {out}");
    assert!(!project.join(".agents/skills/demo").exists());
    let (listed, _) = akit(&project, &catalog, &["installed"]);
    assert!(
        listed.contains("No harness-aware installs"),
        "installed after reset:\n{listed}"
    );
}

#[test]
fn installed_reports_ok_health_and_health_report_json() {
    let (_tmp, catalog, project) = setup();
    akit(
        &project,
        &catalog,
        &["install", "-H", "copilot", "-H", "codex", "demo"],
    );

    let (listed, ok) = akit(&project, &catalog, &["installed"]);
    assert!(ok, "installed failed:\n{listed}");
    assert!(listed.contains("HEALTH"), "no health column:\n{listed}");
    assert!(listed.contains("copilot, codex"), "harnesses:\n{listed}");
    assert!(
        listed.contains("Health: ok"),
        "expected ok health:\n{listed}"
    );

    let (json, ok) = akit(&project, &catalog, &["--json", "installed"]);
    assert!(ok);
    let v: serde_json::Value = serde_json::from_str(&json).expect("HealthReport json");
    assert_eq!(v["healthy"], true, "{json}");
    assert_eq!(v["items"][0]["id"], "demo", "{json}");
    assert_eq!(v["items"][0]["degraded"], false, "{json}");
}

#[test]
fn installed_reports_degraded_when_a_materialization_is_deleted() {
    let (_tmp, catalog, project) = setup();
    akit(&project, &catalog, &["install", "-H", "copilot", "demo"]);
    // The single shared destination for copilot is `.agents/skills/demo`.
    let mat = project.join(".agents/skills/demo");
    assert!(mat.exists());
    fs::remove_dir_all(&mat).unwrap();

    let (listed, ok) = akit(&project, &catalog, &["installed"]);
    assert!(ok, "installed failed:\n{listed}");
    assert!(listed.contains("degraded"), "expected degraded:\n{listed}");
    assert!(
        listed.contains("uncovered: copilot"),
        "uncovered harness:\n{listed}"
    );

    let (json, ok) = akit(&project, &catalog, &["--json", "installed"]);
    assert!(ok);
    let v: serde_json::Value = serde_json::from_str(&json).expect("HealthReport json");
    assert_eq!(v["healthy"], false, "{json}");
    assert_eq!(v["items"][0]["degraded"], true, "{json}");
    assert_eq!(v["items"][0]["source_present"], true, "{json}");
}

#[test]
fn install_dry_run_previews_without_applying() {
    let (_tmp, catalog, project) = setup();
    let (out, ok) = akit(
        &project,
        &catalog,
        &["install", "-H", "copilot", "--dry-run", "demo"],
    );
    assert!(ok, "dry-run failed:\n{out}");
    assert!(out.contains("Plan:"), "no plan header:\n{out}");
    assert!(
        out.contains(".agents/skills/demo"),
        "no planned path:\n{out}"
    );
    assert!(out.contains("dry run"), "no dry-run notice:\n{out}");
    // Nothing was materialized and nothing recorded.
    assert!(!project.join(".agents/skills/demo").exists());
    let (listed, _) = akit(&project, &catalog, &["installed"]);
    assert!(listed.contains("No harness-aware installs"), "{listed}");
}

#[test]
fn install_dry_run_json_reports_create_remove_and_replaces() {
    let (_tmp, catalog, project) = setup();
    akit(&project, &catalog, &["install", "-H", "copilot", "demo"]);
    // A dry-run reshape to claude: drops the .agents copy, creates the .claude one.
    let (json, ok) = akit(
        &project,
        &catalog,
        &["--json", "install", "-H", "claude", "--dry-run", "demo"],
    );
    assert!(ok);
    let v: serde_json::Value = serde_json::from_str(&json).expect("InstallPreview json");
    assert_eq!(v["replaces"], true, "{json}");
    assert_eq!(v["create"][0]["path"], ".claude/skills/demo", "{json}");
    assert_eq!(v["remove"][0], ".agents/skills/demo", "{json}");
    // The real install is untouched by the dry run.
    assert!(project.join(".agents/skills/demo").exists());
}

#[test]
fn install_prints_per_harness_reload_guidance_for_skills() {
    let (_tmp, catalog, project) = setup();
    let (out, ok) = akit(&project, &catalog, &["install", "-H", "copilot", "demo"]);
    assert!(ok, "{out}");
    assert!(out.contains("reload:"), "no reload block:\n{out}");
    // Skills carry per-harness reload data now (#46): Copilot's is a reload
    // *command*, not the old harness-agnostic "start a new session" hint.
    assert!(
        out.contains("copilot skill:"),
        "no copilot skill hint:\n{out}"
    );
    assert!(
        out.contains("reload command"),
        "not command guidance:\n{out}"
    );
}

#[test]
fn skill_reload_guidance_differs_per_harness() {
    // Claude watches its skill directory; Copilot needs `/skills reload`. The
    // two must not print the same line for one install that serves both.
    let (_tmp, catalog, project) = setup();
    let (out, ok) = akit(
        &project,
        &catalog,
        &["install", "-H", "copilot", "-H", "claude", "demo"],
    );
    assert!(ok, "{out}");
    assert!(out.contains("copilot skill:"), "{out}");
    assert!(
        out.contains("claude skill: picked up automatically"),
        "claude should be live:\n{out}"
    );
}

#[test]
fn install_prints_per_harness_reload_for_agents() {
    let (_tmp, catalog, project) = setup();
    make_agent_pkg(
        &catalog,
        "rev",
        &[("copilot", "rev.md"), ("claude", "rev.md")],
    );
    let (out, ok) = akit(
        &project,
        &catalog,
        &["install", "--agent", "-H", "copilot", "-H", "claude", "rev"],
    );
    assert!(ok, "{out}");
    assert!(out.contains("copilot agent:"), "no copilot reload:\n{out}");
    assert!(out.contains("claude agent:"), "no claude reload:\n{out}");
}

#[test]
fn repair_restores_a_deleted_materialization() {
    let (_tmp, catalog, project) = setup();
    akit(&project, &catalog, &["install", "-H", "copilot", "demo"]);
    let mat = project.join(".agents/skills/demo");
    fs::remove_dir_all(&mat).unwrap();

    let (out, ok) = akit(&project, &catalog, &["repair"]);
    assert!(ok, "repair failed:\n{out}");
    assert!(out.contains("Restored"), "no restore line:\n{out}");
    assert!(mat.join("SKILL.md").exists(), "file not restored");
    // Health is clean again.
    let (listed, _) = akit(&project, &catalog, &["installed"]);
    assert!(listed.contains("Health: ok"), "not healthy:\n{listed}");
}

#[test]
fn repair_leaves_locally_modified_files_untouched() {
    let (_tmp, catalog, project) = setup();
    akit(&project, &catalog, &["install", "-H", "copilot", "demo"]);
    let skill = project.join(".agents/skills/demo/SKILL.md");
    fs::write(&skill, "edited by user").unwrap();

    let (json, ok) = akit(&project, &catalog, &["--json", "repair"]);
    assert!(ok, "repair failed:\n{json}");
    let v: serde_json::Value = serde_json::from_str(&json).expect("RepairReport json");
    assert_eq!(v["skipped_modified"][0], ".agents/skills/demo", "{json}");
    assert!(v["restored_paths"].as_array().unwrap().is_empty(), "{json}");
    assert_eq!(fs::read_to_string(&skill).unwrap(), "edited by user");
}

#[test]
fn detach_keeps_files_and_drops_ownership() {
    let (_tmp, catalog, project) = setup();
    akit(&project, &catalog, &["install", "-H", "copilot", "demo"]);
    let mat = project.join(".agents/skills/demo");

    let (out, ok) = akit(&project, &catalog, &["detach", "demo"]);
    assert!(ok, "detach failed:\n{out}");
    assert!(out.contains("Detached"), "no detach line:\n{out}");
    // Bytes preserved, ownership gone.
    assert!(mat.join("SKILL.md").exists(), "files should be kept");
    let (listed, _) = akit(&project, &catalog, &["installed"]);
    assert!(
        listed.contains("No harness-aware installs"),
        "still owned:\n{listed}"
    );
}

#[test]
fn forget_reports_when_no_record_exists() {
    let (_tmp, catalog, project) = setup();
    let (json, ok) = akit(&project, &catalog, &["--json", "forget", "ghost"]);
    assert!(ok, "forget failed:\n{json}");
    let v: serde_json::Value = serde_json::from_str(&json).expect("DetachReport json");
    assert_eq!(v["not_installed"], true, "{json}");
}

#[test]
fn adopt_reclaims_ownership_after_lost_lockfile() {
    let (_tmp, catalog, project) = setup();
    akit(&project, &catalog, &["install", "-H", "copilot", "demo"]);
    // Simulate a lost lockfile while the materialized files remain intact.
    fs::remove_file(project.join(".akit/kit.lock.json")).unwrap();
    let (listed, _) = akit(&project, &catalog, &["installed"]);
    assert!(listed.contains("No harness-aware installs"), "{listed}");

    let (out, ok) = akit(&project, &catalog, &["adopt", "-H", "copilot", "demo"]);
    assert!(ok, "adopt failed:\n{out}");
    assert!(out.contains("Adopted"), "no adopt line:\n{out}");
    let (listed, _) = akit(&project, &catalog, &["installed"]);
    assert!(listed.contains("demo"), "not re-adopted:\n{listed}");
    assert!(listed.contains("Health: ok"), "not healthy:\n{listed}");
}

#[test]
fn reset_preview_lists_owned_paths_before_refusing_non_interactively() {
    let (_tmp, catalog, project) = setup();
    akit(&project, &catalog, &["install", "-H", "copilot", "demo"]);
    // Non-interactive reset (no --yes): previews the owned paths, then refuses.
    let (out, ok) = akit(&project, &catalog, &["reset"]);
    assert!(!ok, "reset should refuse non-interactively:\n{out}");
    assert!(out.contains("Reset would remove"), "no preview:\n{out}");
    assert!(
        out.contains(".agents/skills/demo"),
        "path not listed:\n{out}"
    );
    // And it did not delete anything.
    assert!(project.join(".agents/skills/demo").exists());
}

// ── `install --symlink` (issue #45) ──────────────────────────────────────────

#[test]
fn install_symlink_symlinks_a_follower_only_install() {
    let (_tmp, catalog, project) = setup();
    let (out, ok) = akit(
        &project,
        &catalog,
        &["install", "--symlink", "-H", "claude", "demo"],
    );
    assert!(ok, "install --symlink failed: {out}");
    let dest = project.join(".claude/skills/demo");
    let meta = std::fs::symlink_metadata(&dest).unwrap();
    assert!(meta.is_symlink(), "expected a symlink at {dest:?}");
    assert_eq!(
        std::fs::read_link(&dest).unwrap(),
        catalog.join("skills/demo")
    );
}

#[test]
fn install_symlink_copies_and_notes_when_a_coverer_is_not_a_follower() {
    let (_tmp, catalog, project) = setup();
    // Copilot+Claude collapse onto the shared `.claude/skills` path; Copilot is
    // not a confirmed symlink-follower, so the path is copied and a note is shown.
    let (out, ok) = akit(
        &project,
        &catalog,
        &[
            "install",
            "--symlink",
            "-H",
            "copilot",
            "-H",
            "claude",
            "demo",
        ],
    );
    assert!(ok, "install failed: {out}");
    let dest = project.join(".claude/skills/demo");
    assert!(!std::fs::symlink_metadata(&dest).unwrap().is_symlink());
    assert!(
        out.contains("note:") && out.contains("copilot"),
        "expected a symlink-downgrade note mentioning copilot:\n{out}"
    );
}

#[test]
fn install_symlink_dry_run_shows_symlink_mode() {
    let (_tmp, catalog, project) = setup();
    let (out, ok) = akit(
        &project,
        &catalog,
        &["install", "--symlink", "--dry-run", "-H", "claude", "demo"],
    );
    assert!(ok, "dry-run failed: {out}");
    assert!(
        out.contains("[symlink]"),
        "expected [symlink] in plan:\n{out}"
    );
    // Nothing materialized.
    assert!(!project.join(".claude/skills/demo").exists());
}

// ── `doctor` repointed onto `.akit` (issue #45) ──────────────────────────────

#[test]
fn doctor_reports_ok_then_degraded_over_akit() {
    let (_tmp, catalog, project) = setup();
    assert!(akit(&project, &catalog, &["install", "-H", "claude", "demo"]).1);

    let (out, ok) = akit(&project, &catalog, &["doctor"]);
    assert!(ok, "doctor failed: {out}");
    assert!(out.contains("Doctor: ok"), "{out}");

    // Delete the materialization → doctor must flag it degraded (non-ok verdict).
    std::fs::remove_dir_all(project.join(".claude/skills/demo")).unwrap();
    let (out, ok) = akit(&project, &catalog, &["doctor"]);
    assert!(ok, "doctor failed: {out}");
    assert!(
        out.contains("degraded"),
        "expected degraded verdict:\n{out}"
    );
    assert!(!out.contains("Doctor: ok"), "{out}");
}

#[test]
fn doctor_json_is_a_diagnosis_over_akit() {
    let (_tmp, catalog, project) = setup();
    akit(&project, &catalog, &["install", "-H", "claude", "demo"]);
    let (out, ok) = akit(&project, &catalog, &["--json", "doctor"]);
    assert!(ok, "doctor --json failed: {out}");
    let v: serde_json::Value = serde_json::from_str(&out).unwrap();
    assert!(v["items"].is_array());
    assert!(v["bundles"].is_array());
    assert_eq!(v["healthy"], true);
    assert!(v["missing_excludes"].is_array() && v["stale_excludes"].is_array());
}

// ── `sync` repointed onto `.akit` (== `repair`) (issue #45) ──────────────────

#[test]
fn sync_restores_a_missing_materialization_over_akit() {
    let (_tmp, catalog, project) = setup();
    assert!(akit(&project, &catalog, &["install", "-H", "claude", "demo"]).1);
    let dest = project.join(".claude/skills/demo");
    assert!(dest.exists());

    // Delete the owned materialization, then `sync` must restore it from the catalog.
    std::fs::remove_dir_all(&dest).unwrap();
    let (out, ok) = akit(&project, &catalog, &["sync"]);
    assert!(ok, "sync failed: {out}");
    assert!(out.contains("Restored"), "{out}");
    assert!(dest.join("SKILL.md").is_file());

    // Idempotent: a second sync finds nothing to do.
    let (out, ok) = akit(&project, &catalog, &["sync"]);
    assert!(ok, "sync failed: {out}");
    assert!(out.contains("Nothing to repair"), "{out}");
}

// ── `uninstall --dry-run` + drift gate (issue #47) ───────────────────────────

fn make_bundle(catalog: &Path, name: &str, manifest: &str) {
    let dir = catalog.join("bundles");
    fs::create_dir_all(&dir).unwrap();
    fs::write(dir.join(format!("{name}.yml")), manifest).unwrap();
}

/// Locally edit an installed copy so its content no longer matches the hash akit
/// recorded — i.e. make it drifted.
fn modify(path: &Path) {
    fs::write(path, "---\nname: demo\n---\nhand-edited\n").unwrap();
}

#[test]
fn uninstall_dry_run_previews_without_removing() {
    let (_tmp, catalog, project) = setup();
    assert!(akit(&project, &catalog, &["install", "-H", "copilot", "demo"]).1);

    let (out, ok) = akit(&project, &catalog, &["uninstall", "--dry-run", "demo"]);
    assert!(ok, "dry-run failed:\n{out}");
    assert!(out.contains("full removal"), "no plan header:\n{out}");
    assert!(out.contains("remove:"), "no remove section:\n{out}");
    assert!(
        out.contains(".agents/skills/demo"),
        "path not listed:\n{out}"
    );
    assert!(out.contains("dry run"), "no dry-run notice:\n{out}");

    // Nothing was deleted and the install is still recorded.
    assert!(project.join(".agents/skills/demo/SKILL.md").exists());
    let (listed, _) = akit(&project, &catalog, &["installed"]);
    assert!(
        listed.contains("demo"),
        "install dropped by dry run:\n{listed}"
    );
}

#[test]
fn uninstall_dry_run_json_reports_paths_and_drift() {
    let (_tmp, catalog, project) = setup();
    assert!(akit(&project, &catalog, &["install", "-H", "copilot", "demo"]).1);

    let (json, ok) = akit(
        &project,
        &catalog,
        &["--json", "uninstall", "--dry-run", "demo"],
    );
    assert!(ok, "dry-run failed:\n{json}");
    let v: serde_json::Value = serde_json::from_str(&json).expect("RemovePreview json");
    assert_eq!(v["id"], "demo", "{json}");
    assert_eq!(v["item_type"], "skill", "{json}");
    assert_eq!(v["reshape"], false, "{json}");
    assert_eq!(v["not_installed"], false, "{json}");
    assert_eq!(v["remove"][0]["path"], ".agents/skills/demo", "{json}");
    assert_eq!(v["remove"][0]["drift"], "clean", "{json}");
    assert!(project.join(".agents/skills/demo").exists());

    // A locally modified copy is reported as drifted by the same preview.
    modify(&project.join(".agents/skills/demo/SKILL.md"));
    let (json, ok) = akit(
        &project,
        &catalog,
        &["--json", "uninstall", "--dry-run", "demo"],
    );
    assert!(ok, "dry-run failed:\n{json}");
    let v: serde_json::Value = serde_json::from_str(&json).unwrap();
    assert_eq!(v["remove"][0]["drift"], "modified", "{json}");
}

#[test]
fn uninstall_dry_run_of_an_absent_item_reports_not_installed() {
    let (_tmp, catalog, project) = setup();
    let (out, ok) = akit(&project, &catalog, &["uninstall", "--dry-run", "demo"]);
    assert!(ok, "dry-run failed:\n{out}");
    assert!(out.contains("not installed"), "{out}");
}

#[test]
fn uninstall_scoped_dry_run_previews_the_reshape() {
    let (_tmp, catalog, project) = setup();
    assert!(
        akit(
            &project,
            &catalog,
            &["install", "-H", "copilot", "-H", "claude", "demo"],
        )
        .1
    );
    // Both harnesses share `.claude/skills`; dropping claude reshapes onto `.agents`.
    let (out, ok) = akit(
        &project,
        &catalog,
        &["uninstall", "-H", "claude", "--dry-run", "demo"],
    );
    assert!(ok, "dry-run failed:\n{out}");
    assert!(out.contains("would stay installed for copilot"), "{out}");
    assert!(out.contains("remove:"), "{out}");
    assert!(out.contains(".claude/skills/demo"), "{out}");
    assert!(out.contains("create (reshape):"), "{out}");
    assert!(out.contains(".agents/skills/demo"), "{out}");
    // Untouched: the original materialization is still there, the new one is not.
    assert!(project.join(".claude/skills/demo/SKILL.md").exists());
    assert!(!project.join(".agents/skills/demo").exists());
}

#[test]
fn uninstall_of_a_clean_copy_needs_no_confirmation() {
    let (_tmp, catalog, project) = setup();
    assert!(akit(&project, &catalog, &["install", "-H", "copilot", "demo"]).1);

    // No TTY, no --yes: a clean copy is removed silently, as before.
    let (out, ok) = akit(&project, &catalog, &["uninstall", "demo"]);
    assert!(ok, "uninstall failed:\n{out}");
    assert!(out.contains("Uninstalled"), "{out}");
    assert!(!project.join(".agents/skills/demo").exists());
}

#[test]
fn uninstall_refuses_to_delete_a_drifted_copy_without_yes() {
    let (_tmp, catalog, project) = setup();
    assert!(akit(&project, &catalog, &["install", "-H", "copilot", "demo"]).1);
    let file = project.join(".agents/skills/demo/SKILL.md");
    modify(&file);

    let (out, ok) = akit(&project, &catalog, &["uninstall", "demo"]);
    assert!(!ok, "uninstall should refuse non-interactively:\n{out}");
    assert!(out.contains("locally modified"), "no drift preview:\n{out}");
    // Nothing was deleted and the edit survives.
    assert_eq!(
        fs::read_to_string(&file).unwrap(),
        "---\nname: demo\n---\nhand-edited\n"
    );
    let (listed, _) = akit(&project, &catalog, &["installed"]);
    assert!(listed.contains("demo"), "install dropped:\n{listed}");
}

#[test]
fn uninstall_json_refuses_a_drifted_copy_without_yes() {
    let (_tmp, catalog, project) = setup();
    assert!(akit(&project, &catalog, &["install", "-H", "copilot", "demo"]).1);
    modify(&project.join(".agents/skills/demo/SKILL.md"));

    // `--json` can't prompt, so it refuses rather than deleting silently.
    let (out, ok) = akit(&project, &catalog, &["--json", "uninstall", "demo"]);
    assert!(!ok, "json uninstall should refuse:\n{out}");
    assert!(out.is_empty(), "stdout polluted on refusal:\n{out}");
    assert!(project.join(".agents/skills/demo").exists());
}

#[test]
fn uninstall_yes_deletes_a_drifted_copy() {
    let (_tmp, catalog, project) = setup();
    assert!(akit(&project, &catalog, &["install", "-H", "copilot", "demo"]).1);
    modify(&project.join(".agents/skills/demo/SKILL.md"));

    let (out, ok) = akit(&project, &catalog, &["uninstall", "--yes", "demo"]);
    assert!(ok, "uninstall --yes failed:\n{out}");
    assert!(!project.join(".agents/skills/demo").exists());
    let (listed, _) = akit(&project, &catalog, &["installed"]);
    assert!(listed.contains("No harness-aware installs"), "{listed}");
}

#[test]
fn uninstall_bundle_dry_run_and_drift_gate() {
    let (_tmp, catalog, project) = setup();
    make_skill(&catalog, "lint");
    make_bundle(&catalog, "web", "skills: [demo, lint]\n");
    assert!(
        akit(
            &project,
            &catalog,
            &["install", "-H", "copilot", "--bundle", "web"],
        )
        .1
    );

    // Aggregate dry run lists every member's paths and changes nothing.
    let (out, ok) = akit(
        &project,
        &catalog,
        &["uninstall", "--bundle", "web", "--dry-run"],
    );
    assert!(ok, "bundle dry-run failed:\n{out}");
    assert!(out.contains("uninstall bundle 'web' (2 item(s))"), "{out}");
    assert!(out.contains(".agents/skills/demo"), "{out}");
    assert!(out.contains(".agents/skills/lint"), "{out}");
    assert!(out.contains("dry run"), "{out}");
    assert!(project.join(".agents/skills/demo").exists());
    assert!(project.join(".agents/skills/lint").exists());

    // One drifted member gates the whole bundle uninstall.
    modify(&project.join(".agents/skills/lint/SKILL.md"));
    let (out, ok) = akit(&project, &catalog, &["uninstall", "--bundle", "web"]);
    assert!(!ok, "bundle uninstall should refuse:\n{out}");
    assert!(out.contains("locally modified"), "no drift preview:\n{out}");
    // Aggregate confirmation: neither member was removed.
    assert!(project.join(".agents/skills/demo").exists());
    assert!(project.join(".agents/skills/lint").exists());

    // `--yes` waives it for the whole bundle.
    let (out, ok) = akit(
        &project,
        &catalog,
        &["uninstall", "--bundle", "web", "--yes"],
    );
    assert!(ok, "bundle uninstall failed:\n{out}");
    assert!(!project.join(".agents/skills/demo").exists());
    assert!(!project.join(".agents/skills/lint").exists());
}
