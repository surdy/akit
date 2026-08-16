use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

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

fn make_bundle(catalog_root: &Path, name: &str, manifest: &str) {
    let dir = catalog_root.join("bundles");
    fs::create_dir_all(&dir).unwrap();
    fs::write(dir.join(format!("{name}.yml")), manifest).unwrap();
}

/// A skill restricted to specific harnesses via `skill.yml` (harness-aware).
fn make_skill_only(catalog_root: &Path, name: &str, harnesses: &[&str]) {
    make_skill(catalog_root, name);
    let list = harnesses
        .iter()
        .map(|h| format!("  - {h}\n"))
        .collect::<String>();
    fs::write(
        catalog_root.join("skills").join(name).join("skill.yml"),
        format!("harnesses:\n{list}"),
    )
    .unwrap();
}

/// Run the `akit` binary with the catalog + harness env pointed at a fixture.
fn akit_install(
    proj: &Path,
    catalog_root: &Path,
    harnesses: &str,
    args: &[&str],
) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_akit"))
        .args(["--project", proj.to_str().unwrap()])
        .args(args)
        .env("KIT_CATALOG_DIR", catalog_root)
        .env("AKIT_HARNESSES", harnesses)
        .output()
        .expect("akit binary should run")
}

#[test]
fn cli_status_labels_and_groups_bundle_items() {
    // Harness-aware `status` reads `.akit`: install two bundles + a standalone
    // via the CLI, then assert the bundle-grouped table and completeness lines.
    let tmp = tempfile::tempdir().unwrap();
    let base = tmp.path();
    let catalog_root = base.join("catalog");
    make_skill(&catalog_root, "alpha-skill");
    make_skill(&catalog_root, "zeta-skill");
    make_skill(&catalog_root, "standalone");
    make_bundle(&catalog_root, "zeta", "skills: [zeta-skill]\n");
    make_bundle(&catalog_root, "alpha", "skills: [alpha-skill]\n");
    let (proj, _project) = init_project(base);

    for args in [
        ["install", "--bundle", "zeta"].as_slice(),
        ["install", "standalone"].as_slice(),
        ["install", "--bundle", "alpha"].as_slice(),
    ] {
        assert!(
            akit_install(&proj, &catalog_root, "claude", args)
                .status
                .success(),
            "install {args:?} failed"
        );
    }

    let output = Command::new(env!("CARGO_BIN_EXE_akit"))
        .args(["--project", proj.to_str().unwrap(), "status"])
        .env("KIT_CATALOG_DIR", &catalog_root)
        .output()
        .expect("akit binary should run");

    assert!(
        output.status.success(),
        "akit failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    let lines = stdout.lines().collect::<Vec<_>>();
    // Header, then bundle-tagged rows (alpha before zeta), then the standalone `-`.
    assert!(
        lines[0].contains("BUNDLE") && lines[0].contains("HEALTH"),
        "{stdout}"
    );
    assert!(
        lines[1].starts_with("alpha") && lines[1].contains("alpha-skill"),
        "{stdout}"
    );
    assert!(
        lines[2].starts_with("zeta") && lines[2].contains("zeta-skill"),
        "{stdout}"
    );
    assert!(
        lines[3].starts_with('-') && lines[3].contains("standalone"),
        "{stdout}"
    );
    // Both single-member bundles are complete.
    assert!(
        stdout.contains("Bundle 'alpha': complete (1/1)"),
        "{stdout}"
    );
    assert!(stdout.contains("Bundle 'zeta': complete (1/1)"), "{stdout}");
}

// ── harness-aware `install --bundle` (issue #45) ─────────────────────────────

#[test]
fn cli_install_bundle_installs_all_members_tagged() {
    let tmp = tempfile::tempdir().unwrap();
    let base = tmp.path();
    let catalog_root = base.join("catalog");
    make_skill(&catalog_root, "deploy");
    make_skill(&catalog_root, "lint");
    make_bundle(&catalog_root, "web", "skills: [deploy, lint]\n");
    let (proj, _project) = init_project(base);

    let out = akit_install(
        &proj,
        &catalog_root,
        "claude",
        &["install", "--bundle", "web"],
    );
    assert!(
        out.status.success(),
        "install failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("Installed bundle 'web'"), "{stdout}");

    assert!(proj.join(".claude/skills/deploy/SKILL.md").is_file());
    assert!(proj.join(".claude/skills/lint/SKILL.md").is_file());
    // Both installs carry the bundle tag in the harness-aware lockfile.
    let lock = fs::read_to_string(proj.join(".akit/kit.lock.json")).unwrap();
    assert_eq!(lock.matches("\"bundle\": \"web\"").count(), 2, "{lock}");
}

#[test]
fn cli_install_bundle_dry_run_changes_nothing() {
    let tmp = tempfile::tempdir().unwrap();
    let base = tmp.path();
    let catalog_root = base.join("catalog");
    make_skill(&catalog_root, "deploy");
    make_bundle(&catalog_root, "web", "skills: [deploy]\n");
    let (proj, _project) = init_project(base);

    let out = akit_install(
        &proj,
        &catalog_root,
        "claude",
        &["install", "--bundle", "web", "--dry-run"],
    );
    assert!(out.status.success());
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("Plan: bundle 'web'"), "{stdout}");
    assert!(stdout.contains("dry run"), "{stdout}");
    // Nothing materialized, no lockfile written.
    assert!(!proj.join(".claude/skills/deploy").exists());
    assert!(!proj.join(".akit/kit.lock.json").exists());
}

#[test]
fn cli_install_bundle_partial_needs_yes_non_interactive() {
    let tmp = tempfile::tempdir().unwrap();
    let base = tmp.path();
    let catalog_root = base.join("catalog");
    make_skill_only(&catalog_root, "clauded", &["claude"]);
    make_skill(&catalog_root, "portable");
    make_bundle(&catalog_root, "mix", "skills: [clauded, portable]\n");
    let (proj, _project) = init_project(base);

    // Non-interactive partial install (codex can't take `clauded`) must refuse
    // without --yes rather than hang or silently do a partial install.
    let refused = akit_install(
        &proj,
        &catalog_root,
        "claude,codex",
        &["install", "--bundle", "mix"],
    );
    assert!(!refused.status.success());
    assert!(
        String::from_utf8_lossy(&refused.stderr).contains("--yes"),
        "stderr: {}",
        String::from_utf8_lossy(&refused.stderr)
    );
    assert!(!proj.join(".akit/kit.lock.json").exists());

    // With --yes it installs the servable parts and skips the rest.
    let ok = akit_install(
        &proj,
        &catalog_root,
        "claude,codex",
        &["install", "--bundle", "mix", "--yes"],
    );
    assert!(
        ok.status.success(),
        "install --yes failed: {}",
        String::from_utf8_lossy(&ok.stderr)
    );
    assert!(proj.join(".claude/skills/clauded/SKILL.md").is_file());
    assert!(proj.join(".claude/skills/portable/SKILL.md").is_file());
    // `portable` reaches codex via the shared neutral path; `clauded` does not.
    assert!(proj.join(".agents/skills/portable/SKILL.md").is_file());
    assert!(!proj.join(".agents/skills/clauded").exists());
}

#[test]
fn cli_install_rejects_id_and_bundle_together() {
    let tmp = tempfile::tempdir().unwrap();
    let base = tmp.path();
    let catalog_root = base.join("catalog");
    make_skill(&catalog_root, "deploy");
    make_bundle(&catalog_root, "web", "skills: [deploy]\n");
    let (proj, _project) = init_project(base);

    let out = akit_install(
        &proj,
        &catalog_root,
        "claude",
        &["install", "--bundle", "web", "deploy"],
    );
    assert!(!out.status.success());
    assert!(
        String::from_utf8_lossy(&out.stderr).contains("either <id> or --bundle"),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

#[test]
fn cli_uninstall_bundle_removes_tagged_members_only() {
    let tmp = tempfile::tempdir().unwrap();
    let base = tmp.path();
    let catalog_root = base.join("catalog");
    make_skill(&catalog_root, "deploy");
    make_skill(&catalog_root, "lint");
    make_skill(&catalog_root, "solo");
    make_bundle(&catalog_root, "web", "skills: [deploy, lint]\n");
    let (proj, _project) = init_project(base);

    assert!(
        akit_install(
            &proj,
            &catalog_root,
            "claude",
            &["install", "--bundle", "web"]
        )
        .status
        .success()
    );
    assert!(
        akit_install(&proj, &catalog_root, "claude", &["install", "solo"])
            .status
            .success()
    );

    let out = akit_install(
        &proj,
        &catalog_root,
        "claude",
        &["uninstall", "--bundle", "web"],
    );
    assert!(
        out.status.success(),
        "uninstall failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("Uninstalled bundle 'web'"), "{stdout}");

    assert!(!proj.join(".claude/skills/deploy").exists());
    assert!(!proj.join(".claude/skills/lint").exists());
    // The untagged standalone install survives.
    assert!(proj.join(".claude/skills/solo/SKILL.md").is_file());
    let lock = fs::read_to_string(proj.join(".akit/kit.lock.json")).unwrap();
    assert!(lock.contains("\"id\": \"solo\""), "{lock}");
    assert!(!lock.contains("\"bundle\": \"web\""), "{lock}");
}

#[test]
fn cli_uninstall_rejects_id_and_bundle_together() {
    let tmp = tempfile::tempdir().unwrap();
    let base = tmp.path();
    let catalog_root = base.join("catalog");
    make_skill(&catalog_root, "deploy");
    make_bundle(&catalog_root, "web", "skills: [deploy]\n");
    let (proj, _project) = init_project(base);

    let out = akit_install(
        &proj,
        &catalog_root,
        "claude",
        &["uninstall", "--bundle", "web", "deploy"],
    );
    assert!(!out.status.success());
    assert!(
        String::from_utf8_lossy(&out.stderr).contains("either <id> or --bundle"),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}
