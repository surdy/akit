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
fn install_prints_reload_guidance_for_skills() {
    let (_tmp, catalog, project) = setup();
    let (out, ok) = akit(&project, &catalog, &["install", "-H", "copilot", "demo"]);
    assert!(ok, "{out}");
    assert!(out.contains("reload:"), "no reload block:\n{out}");
    assert!(out.contains("skills:"), "no skill reload hint:\n{out}");
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
