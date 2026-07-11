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
