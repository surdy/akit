use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use akit::project::Project;
use akit::remote::{self, SourceSpec};

fn test_tempdir() -> tempfile::TempDir {
    let root = std::env::current_dir()
        .unwrap()
        .join("target")
        .join("akit-test-tmp");
    fs::create_dir_all(&root).unwrap();
    tempfile::Builder::new()
        .prefix("remote-")
        .tempdir_in(root)
        .unwrap()
}

fn git(args: &[&str], cwd: &Path) -> std::process::Output {
    Command::new("git")
        .args(args)
        .current_dir(cwd)
        .output()
        .expect("git should be available")
}

fn assert_git(args: &[&str], cwd: &Path) {
    let output = git(args, cwd);
    assert!(
        output.status.success(),
        "git {args:?} failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

fn init_project(base: &Path) -> (PathBuf, Project) {
    let proj = base.join("project");
    fs::create_dir_all(&proj).unwrap();
    assert_git(&["init", "-q"], &proj);
    let project = Project::locate(Some(proj.clone())).unwrap();
    (proj, project)
}

fn make_skill(repo_root: &Path, path: &str, name: &str) {
    let dir = path
        .split('/')
        .fold(repo_root.to_path_buf(), |path, segment| path.join(segment));
    fs::create_dir_all(&dir).unwrap();
    fs::write(
        dir.join("SKILL.md"),
        format!("---\nname: {name}\ndescription: remote test skill\n---\nbody\n"),
    )
    .unwrap();
}

fn make_local_bare_remote(base: &Path) -> PathBuf {
    let work = base.join("remote-work");
    fs::create_dir_all(&work).unwrap();
    assert_git(&["init", "-q", "--initial-branch", "main"], &work);
    make_skill(&work, "skills/deploy-to-vercel", "deploy-to-vercel");
    assert_git(&["add", "."], &work);
    assert_git(
        &[
            "-c",
            "user.email=223556219+Copilot@users.noreply.github.com",
            "-c",
            "user.name=surdy",
            "commit",
            "-q",
            "-m",
            "initial",
        ],
        &work,
    );

    let git_base = base.join("git-base");
    let bare = git_base.join("acme").join("kit-skills");
    fs::create_dir_all(bare.parent().unwrap()).unwrap();
    assert_git(
        &[
            "clone",
            "-q",
            "--bare",
            work.to_str().unwrap(),
            bare.to_str().unwrap(),
        ],
        base,
    );
    // Mirror github.com, which permits fetching any reachable commit by SHA so
    // SHA-pinned sources can be pulled.
    assert_git(
        &[
            "--git-dir",
            bare.to_str().unwrap(),
            "config",
            "uploadpack.allowReachableSHA1InWant",
            "true",
        ],
        base,
    );
    git_base
}

/// Build a local bare remote whose repo holds a harness-aware agent **package**
/// at `agents/reviewer/` (an `agent.yml` plus one native variant per harness).
/// Returns the git base dir; the package is reachable as `acme/kit-agents/agents/reviewer`.
fn make_local_bare_agent_pkg_remote(base: &Path) -> PathBuf {
    let work = base.join("agent-remote-work");
    let pkg = work.join("agents").join("reviewer");
    fs::create_dir_all(&pkg).unwrap();
    fs::write(
        pkg.join("agent.yml"),
        "id: reviewer\nname: Code Reviewer\ndescription: Reviews PRs\n\
         variants:\n  copilot: copilot.agent.md\n  claude: claude.md\n",
    )
    .unwrap();
    fs::write(pkg.join("copilot.agent.md"), "---\nname: r\n---\nprompt\n").unwrap();
    fs::write(pkg.join("claude.md"), "---\nname: r\n---\nprompt\n").unwrap();

    assert_git(&["init", "-q", "--initial-branch", "main"], &work);
    assert_git(&["add", "."], &work);
    assert_git(
        &[
            "-c",
            "user.email=ci@example.com",
            "-c",
            "user.name=ci",
            "commit",
            "-q",
            "-m",
            "initial",
        ],
        &work,
    );

    let git_base = base.join("git-base");
    let bare = git_base.join("acme").join("kit-agents");
    fs::create_dir_all(bare.parent().unwrap()).unwrap();
    assert_git(
        &[
            "clone",
            "-q",
            "--bare",
            work.to_str().unwrap(),
            bare.to_str().unwrap(),
        ],
        base,
    );
    // Allow SHA fetches so a commit-pinned `restore` can fetch the recorded commit.
    assert_git(
        &[
            "--git-dir",
            bare.to_str().unwrap(),
            "config",
            "uploadpack.allowReachableSHA1InWant",
            "true",
        ],
        base,
    );
    git_base
}

/// Build a local bare remote whose repo holds a **legacy flat** agent at
/// `agents/reviewer.agent.md` — the shape removed in v0.32.0. Reachable as
/// `acme/kit-agents/agents/reviewer.agent.md`.
fn make_local_bare_flat_agent_remote(base: &Path) -> PathBuf {
    let work = base.join("flat-agent-remote-work");
    let agents = work.join("agents");
    fs::create_dir_all(&agents).unwrap();
    fs::write(
        agents.join("reviewer.agent.md"),
        "---\nname: Reviewer\ndescription: Reviews PRs\n---\nprompt\n",
    )
    .unwrap();

    assert_git(&["init", "-q", "--initial-branch", "main"], &work);
    assert_git(&["add", "."], &work);
    assert_git(
        &[
            "-c",
            "user.email=ci@example.com",
            "-c",
            "user.name=ci",
            "commit",
            "-q",
            "-m",
            "initial",
        ],
        &work,
    );

    let git_base = base.join("git-base");
    let bare = git_base.join("acme").join("kit-agents");
    fs::create_dir_all(bare.parent().unwrap()).unwrap();
    assert_git(
        &[
            "clone",
            "-q",
            "--bare",
            work.to_str().unwrap(),
            bare.to_str().unwrap(),
        ],
        base,
    );
    git_base
}

fn run_akit(
    args: &[&str],
    project: &Path,
    cache: &Path,
    base_url: Option<&str>,
) -> std::process::Output {
    let mut command = Command::new(env!("CARGO_BIN_EXE_akit"));
    command
        .args(["--project", project.to_str().unwrap(), "--json"])
        .args(args)
        .env(remote::ENV_CACHE_DIR, cache)
        // Keep the global install index (#40) inside the fixture.
        .env("AKIT_STATE_DIR", cache.with_file_name("akit-state"))
        .env_remove("KIT_CATALOG_DIR")
        .env_remove(remote::ENV_REMOTE_BASE_URL);
    if let Some(base_url) = base_url {
        command.env(remote::ENV_REMOTE_BASE_URL, base_url);
    }
    command.output().expect("akit binary should run")
}

#[test]
fn source_spec_parse_cases() {
    let spec = SourceSpec::parse("owner/repo/path/to/skill#main").unwrap();
    assert_eq!(spec.owner, "owner");
    assert_eq!(spec.repo, "repo");
    assert_eq!(spec.path, "path/to/skill");
    assert_eq!(spec.ref_.as_deref(), Some("main"));
    assert_eq!(spec.source(), "owner/repo/path/to/skill");
    assert_eq!(spec.leaf(), "skill");

    let spec = SourceSpec::parse("owner/repo/path").unwrap();
    assert_eq!(spec.owner, "owner");
    assert_eq!(spec.repo, "repo");
    assert_eq!(spec.path, "path");
    assert_eq!(spec.ref_, None);

    assert!(SourceSpec::parse("name").is_none());
    assert!(SourceSpec::parse("owner/repo").is_none());
    assert!(SourceSpec::parse("owner/repo/path#").is_none());
}

#[test]
#[ignore = "requires network access to github.com"]
fn live_vercel_skill_can_be_added() {
    let tmp = test_tempdir();
    let base = tmp.path();
    let (proj, _project) = init_project(base);
    let cache = base.join("cache");

    let output = run_akit(
        &["add", "vercel-labs/agent-skills/deploy-to-vercel#main"],
        &proj,
        &cache,
        None,
    );
    assert!(
        output.status.success(),
        "akit add failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        proj.join(".github/skills/deploy-to-vercel/SKILL.md")
            .is_file()
    );
}

fn run_akit_pull(
    args: &[&str],
    catalog: &Path,
    cache: &Path,
    base_url: &str,
) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_akit"))
        .args(["--json"])
        .args(args)
        .env(remote::ENV_CACHE_DIR, cache)
        .env("KIT_CATALOG_DIR", catalog)
        // Keep the global install index (#40) inside the fixture, and on the same
        // path `run_akit_install` records into.
        .env("AKIT_STATE_DIR", catalog.with_file_name("akit-state"))
        .env(remote::ENV_REMOTE_BASE_URL, base_url)
        .output()
        .expect("akit binary should run")
}

#[test]
fn pull_remote_into_catalog_via_local_bare_repo() {
    let tmp = test_tempdir();
    let base = tmp.path();
    let git_base = make_local_bare_remote(base);
    let cache = base.join("cache");
    let catalog = base.join("catalog");
    let base_url = format!("file://{}", git_base.display());

    // First pull copies the remote skill into the catalog.
    let output = run_akit_pull(
        &["pull", "acme/kit-skills/deploy-to-vercel#main"],
        &catalog,
        &cache,
        &base_url,
    );
    assert!(
        output.status.success(),
        "akit pull failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    let json: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(json["id"], "deploy-to-vercel");
    assert_eq!(json["type"], "skill");
    assert_eq!(json["source"], "acme/kit-skills/deploy-to-vercel");
    assert_eq!(json["ref"], "main");
    assert_eq!(json["created"], true);
    assert_eq!(json["overwritten"], false);

    let skill_dir = catalog.join("skills/deploy-to-vercel");
    assert!(skill_dir.join("SKILL.md").is_file());
    // It is a standalone copy, not a symlink into the cache.
    assert!(
        !fs::symlink_metadata(&skill_dir)
            .unwrap()
            .file_type()
            .is_symlink()
    );

    // Re-pulling an identical item is an idempotent no-op.
    let output = run_akit_pull(
        &["pull", "acme/kit-skills/deploy-to-vercel#main"],
        &catalog,
        &cache,
        &base_url,
    );
    assert!(output.status.success());
    let json: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(json["created"], false);
    assert_eq!(json["overwritten"], false);

    // A custom id stores a second copy under that name.
    let output = run_akit_pull(
        &[
            "pull",
            "--as",
            "vercel-deploy",
            "acme/kit-skills/deploy-to-vercel#main",
        ],
        &catalog,
        &cache,
        &base_url,
    );
    assert!(output.status.success());
    let json: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(json["id"], "vercel-deploy");
    assert_eq!(json["created"], true);
    assert!(catalog.join("skills/vercel-deploy/SKILL.md").is_file());
}

#[test]
fn pull_records_manifest_and_restore_rebootstraps() {
    let tmp = test_tempdir();
    let base = tmp.path();
    let git_base = make_local_bare_remote(base);
    let cache = base.join("cache");
    let catalog = base.join("catalog");
    let base_url = format!("file://{}", git_base.display());

    // Pull a default-id skill and a custom-id (`--as`) skill.
    for args in [
        vec!["pull", "acme/kit-skills/deploy-to-vercel#main"],
        vec![
            "pull",
            "--as",
            "vercel",
            "acme/kit-skills/deploy-to-vercel#main",
        ],
    ] {
        let output = run_akit_pull(&args, &catalog, &cache, &base_url);
        assert!(
            output.status.success(),
            "akit pull failed\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }

    // Recording the resolved commit forces the object form (a string shorthand can't carry both
    // ref and commit); the `--as` pull additionally carries the alias.
    let manifest = fs::read_to_string(catalog.join("akit.yml")).unwrap();
    assert!(
        manifest.contains("git: acme/kit-skills") && manifest.contains("ref: main"),
        "{manifest}"
    );
    assert!(manifest.contains("commit: "), "{manifest}");
    assert!(manifest.contains("alias: vercel"), "{manifest}");

    // Simulate a fresh machine: wipe the materialized items but keep the manifest.
    fs::remove_dir_all(catalog.join("skills")).unwrap();
    assert!(!catalog.join("skills/deploy-to-vercel").exists());

    // Restore re-fetches everything in the manifest.
    let output = run_akit_pull(&["restore"], &catalog, &cache, &base_url);
    assert!(
        output.status.success(),
        "akit restore failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let json: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(json["summary"]["pulled"], 2);
    assert_eq!(json["summary"]["errors"], 0);
    assert!(catalog.join("skills/deploy-to-vercel/SKILL.md").is_file());
    assert!(catalog.join("skills/vercel/SKILL.md").is_file());

    // Restore is idempotent: a second run reports everything already present.
    let output = run_akit_pull(&["restore"], &catalog, &cache, &base_url);
    assert!(output.status.success());
    let json: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(json["summary"]["already_present"], 2);
    assert_eq!(json["summary"]["pulled"], 0);
}

#[test]
fn drop_removes_catalog_item_and_prunes_manifest() {
    let tmp = test_tempdir();
    let base = tmp.path();
    let git_base = make_local_bare_remote(base);
    let cache = base.join("cache");
    let catalog = base.join("catalog");
    let base_url = format!("file://{}", git_base.display());

    // Pull a skill, then drop it.
    let output = run_akit_pull(
        &["pull", "acme/kit-skills/deploy-to-vercel#main"],
        &catalog,
        &cache,
        &base_url,
    );
    assert!(output.status.success());
    assert!(catalog.join("skills/deploy-to-vercel/SKILL.md").is_file());

    let output = run_akit_pull(&["drop", "deploy-to-vercel"], &catalog, &cache, &base_url);
    assert!(
        output.status.success(),
        "akit drop failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let json: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(json["id"], "deploy-to-vercel");
    assert_eq!(json["item_removed"], true);
    assert_eq!(json["manifest_pruned"], true);

    // Catalog item is gone and the manifest no longer lists it.
    assert!(!catalog.join("skills/deploy-to-vercel").exists());
    let manifest = fs::read_to_string(catalog.join("akit.yml")).unwrap();
    assert!(!manifest.contains("deploy-to-vercel"), "{manifest}");

    // Restore now has nothing to do for that item.
    let output = run_akit_pull(&["restore"], &catalog, &cache, &base_url);
    assert!(output.status.success());
    let json: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(json["summary"]["pulled"], 0);
    assert!(!catalog.join("skills/deploy-to-vercel").exists());

    // Dropping something that exists nowhere fails and touches nothing.
    let output = run_akit_pull(&["drop", "never-existed"], &catalog, &cache, &base_url);
    assert!(!output.status.success());
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("nothing to drop"),
        "stderr:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

/// Commit a new SKILL.md body upstream and push it to the bare remote's `main`.
fn push_remote_change(base: &Path, git_base: &Path, body: &str) {
    let work = base.join("remote-work");
    fs::write(
        work.join("skills/deploy-to-vercel/SKILL.md"),
        format!("---\nname: deploy-to-vercel\ndescription: remote test skill\n---\n{body}\n"),
    )
    .unwrap();
    assert_git(&["add", "."], &work);
    assert_git(
        &[
            "-c",
            "user.email=223556219+Copilot@users.noreply.github.com",
            "-c",
            "user.name=surdy",
            "commit",
            "-q",
            "-m",
            "upstream change",
        ],
        &work,
    );
    let bare = git_base.join("acme").join("kit-skills");
    assert_git(&["push", "-q", bare.to_str().unwrap(), "main"], &work);
}

#[test]
fn update_refreshes_outdated_catalog_items() {
    let tmp = test_tempdir();
    let base = tmp.path();
    let git_base = make_local_bare_remote(base);
    let cache = base.join("cache");
    let catalog = base.join("catalog");
    let base_url = format!("file://{}", git_base.display());
    let skill_md = catalog.join("skills/deploy-to-vercel/SKILL.md");

    // Pull a branch-tracking skill.
    let output = run_akit_pull(
        &["pull", "acme/kit-skills/deploy-to-vercel#main"],
        &catalog,
        &cache,
        &base_url,
    );
    assert!(output.status.success());
    assert!(fs::read_to_string(&skill_md).unwrap().contains("body"));

    // Nothing changed upstream yet: check reports up-to-date and writes nothing.
    let output = run_akit_pull(&["update", "--check"], &catalog, &cache, &base_url);
    assert!(output.status.success());
    let json: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(json["summary"]["outdated"], 0);
    assert_eq!(json["summary"]["up_to_date"], 1);

    // Move upstream forward.
    push_remote_change(base, &git_base, "updated body");

    // Check now flags the item as outdated without touching the catalog copy.
    let output = run_akit_pull(&["update", "--check"], &catalog, &cache, &base_url);
    assert!(output.status.success());
    let json: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(json["summary"]["outdated"], 1);
    assert_eq!(json["items"][0]["status"], "outdated");
    assert!(fs::read_to_string(&skill_md).unwrap().contains("body"));
    assert!(
        !fs::read_to_string(&skill_md)
            .unwrap()
            .contains("updated body")
    );

    // Applying the update rewrites the catalog copy to the latest commit.
    let output = run_akit_pull(&["update"], &catalog, &cache, &base_url);
    assert!(
        output.status.success(),
        "akit update failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let json: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(json["summary"]["updated"], 1);
    assert_eq!(json["items"][0]["status"], "updated");
    assert!(
        fs::read_to_string(&skill_md)
            .unwrap()
            .contains("updated body")
    );

    // A second run is a no-op now that the copy matches upstream.
    let output = run_akit_pull(&["update"], &catalog, &cache, &base_url);
    assert!(output.status.success());
    let json: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(json["summary"]["updated"], 0);
    assert_eq!(json["summary"]["up_to_date"], 1);

    // Targeting an unknown id is an error.
    let output = run_akit_pull(&["update", "never-existed"], &catalog, &cache, &base_url);
    assert!(!output.status.success());
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("nothing to update"),
        "stderr:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn update_skips_sha_pinned_items() {
    let tmp = test_tempdir();
    let base = tmp.path();
    let git_base = make_local_bare_remote(base);
    let cache = base.join("cache");
    let catalog = base.join("catalog");
    let base_url = format!("file://{}", git_base.display());

    // Resolve the initial commit SHA and pull pinned to it.
    let head = git(&["rev-parse", "HEAD"], &base.join("remote-work"));
    assert!(head.status.success());
    let sha = String::from_utf8_lossy(&head.stdout).trim().to_string();
    let source = format!("acme/kit-skills/deploy-to-vercel#{sha}");

    let output = run_akit_pull(
        &["pull", "--as", "pinned", &source],
        &catalog,
        &cache,
        &base_url,
    );
    assert!(
        output.status.success(),
        "akit pull failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    // Even after upstream moves, a SHA-pinned item is reported as pinned and never refetched.
    push_remote_change(base, &git_base, "moved on");
    let output = run_akit_pull(&["update", "--check"], &catalog, &cache, &base_url);
    assert!(output.status.success());
    let json: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(json["summary"]["pinned"], 1);
    assert_eq!(json["summary"]["outdated"], 0);
    assert_eq!(json["items"][0]["status"], "pinned");
}

/// The commit SHA currently at the tip of the upstream work tree.
fn remote_head(base: &Path) -> String {
    let out = git(&["rev-parse", "HEAD"], &base.join("remote-work"));
    assert!(out.status.success());
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

#[test]
fn pull_records_resolved_commit() {
    let tmp = test_tempdir();
    let base = tmp.path();
    let git_base = make_local_bare_remote(base);
    let cache = base.join("cache");
    let catalog = base.join("catalog");
    let base_url = format!("file://{}", git_base.display());

    let output = run_akit_pull(
        &["pull", "acme/kit-skills/deploy-to-vercel#main"],
        &catalog,
        &cache,
        &base_url,
    );
    assert!(output.status.success());

    // The manifest records the exact commit the ref resolved to.
    let head = remote_head(base);
    let manifest = fs::read_to_string(catalog.join("akit.yml")).unwrap();
    assert!(manifest.contains(&format!("commit: {head}")), "{manifest}");

    // ...and `pull --json` surfaces the same commit.
    let json: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(json["commit"], head);
}

#[test]
fn restore_pins_to_recorded_commit_until_latest() {
    let tmp = test_tempdir();
    let base = tmp.path();
    let git_base = make_local_bare_remote(base);
    let cache = base.join("cache");
    let catalog = base.join("catalog");
    let base_url = format!("file://{}", git_base.display());
    let skill_md = catalog.join("skills/deploy-to-vercel/SKILL.md");

    // Pull pins the catalog to commit C1 ("body").
    let output = run_akit_pull(
        &["pull", "acme/kit-skills/deploy-to-vercel#main"],
        &catalog,
        &cache,
        &base_url,
    );
    assert!(output.status.success());
    let c1 = remote_head(base);

    // Upstream advances to C2 ("updated body").
    push_remote_change(base, &git_base, "updated body");
    let c2 = remote_head(base);
    assert_ne!(c1, c2);

    // Simulate a fresh machine: keep only the manifest.
    fs::remove_dir_all(catalog.join("skills")).unwrap();

    // Default restore reproduces the *recorded* commit C1, not the upstream head.
    let output = run_akit_pull(&["restore"], &catalog, &cache, &base_url);
    assert!(
        output.status.success(),
        "akit restore failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let body = fs::read_to_string(&skill_md).unwrap();
    assert!(
        body.contains("body") && !body.contains("updated body"),
        "{body}"
    );
    let manifest = fs::read_to_string(catalog.join("akit.yml")).unwrap();
    assert!(manifest.contains(&format!("commit: {c1}")), "{manifest}");

    // `restore --latest` moves to the head of the ref (C2) and rewrites the recorded commit.
    let output = run_akit_pull(
        &["restore", "--latest", "--force"],
        &catalog,
        &cache,
        &base_url,
    );
    assert!(
        output.status.success(),
        "akit restore --latest failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        fs::read_to_string(&skill_md)
            .unwrap()
            .contains("updated body")
    );
    let manifest = fs::read_to_string(catalog.join("akit.yml")).unwrap();
    assert!(manifest.contains(&format!("commit: {c2}")), "{manifest}");
}

#[test]
fn log_lists_history_and_marks_current() {
    let tmp = test_tempdir();
    let base = tmp.path();
    let git_base = make_local_bare_remote(base);
    let cache = base.join("cache");
    let catalog = base.join("catalog");
    let base_url = format!("file://{}", git_base.display());

    // Pull at C1, advance upstream to C2, and update so the manifest records C2.
    let output = run_akit_pull(
        &["pull", "acme/kit-skills/deploy-to-vercel#main"],
        &catalog,
        &cache,
        &base_url,
    );
    assert!(output.status.success());
    let c1 = remote_head(base);

    push_remote_change(base, &git_base, "updated body");
    let c2 = remote_head(base);
    assert_ne!(c1, c2);

    let output = run_akit_pull(&["update"], &catalog, &cache, &base_url);
    assert!(output.status.success());

    // `log` lists the recorded ref's history newest-first and marks the installed commit (C2).
    let output = run_akit_pull(&["log", "deploy-to-vercel"], &catalog, &cache, &base_url);
    assert!(
        output.status.success(),
        "akit log failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let json: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    let rows = json.as_array().unwrap();
    assert_eq!(rows.len(), 2, "expected two commits, got {json}");
    assert_eq!(rows[0]["commit"], c2);
    assert_eq!(rows[0]["ref"], "main");
    assert_eq!(rows[0]["current"], true);
    assert_eq!(rows[1]["commit"], c1);
    assert_eq!(rows[1]["current"], false);

    // Logging an id that was never pulled is an error.
    let output = run_akit_pull(&["log", "never-pulled"], &catalog, &cache, &base_url);
    assert!(!output.status.success());
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("was pulled from a source"),
        "stderr:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn update_to_rolls_back_to_prior_commit() {
    let tmp = test_tempdir();
    let base = tmp.path();
    let git_base = make_local_bare_remote(base);
    let cache = base.join("cache");
    let catalog = base.join("catalog");
    let base_url = format!("file://{}", git_base.display());
    let skill_md = catalog.join("skills/deploy-to-vercel/SKILL.md");

    // Pull C1, advance to C2, update to C2.
    let output = run_akit_pull(
        &["pull", "acme/kit-skills/deploy-to-vercel#main"],
        &catalog,
        &cache,
        &base_url,
    );
    assert!(output.status.success());
    let c1 = remote_head(base);

    push_remote_change(base, &git_base, "updated body");
    let c2 = remote_head(base);

    let output = run_akit_pull(&["update"], &catalog, &cache, &base_url);
    assert!(output.status.success());
    assert!(
        fs::read_to_string(&skill_md)
            .unwrap()
            .contains("updated body")
    );

    // Roll back to C1: the catalog copy is re-materialized at the old commit.
    let output = run_akit_pull(
        &["update", "deploy-to-vercel", "--to", &c1],
        &catalog,
        &cache,
        &base_url,
    );
    assert!(
        output.status.success(),
        "akit update --to failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let json: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(json["items"][0]["status"], "updated");
    assert_eq!(json["items"][0]["previous_commit"], c2);
    assert_eq!(json["items"][0]["commit"], c1);

    let body = fs::read_to_string(&skill_md).unwrap();
    assert!(
        body.contains("body") && !body.contains("updated body"),
        "{body}"
    );

    // The manifest is now pinned to the full SHA, so `update --check` reports it as pinned.
    let manifest = fs::read_to_string(catalog.join("akit.yml")).unwrap();
    assert!(manifest.contains(&format!("commit: {c1}")), "{manifest}");
    assert!(manifest.contains(&format!("ref: {c1}")), "{manifest}");

    let output = run_akit_pull(&["update", "--check"], &catalog, &cache, &base_url);
    assert!(output.status.success());
    let json: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(json["summary"]["pinned"], 1);
    assert_eq!(json["items"][0]["status"], "pinned");
}

#[test]
fn update_to_rejects_unreachable_sha_without_mutating_manifest() {
    let tmp = test_tempdir();
    let base = tmp.path();
    let git_base = make_local_bare_remote(base);
    let cache = base.join("cache");
    let catalog = base.join("catalog");
    let base_url = format!("file://{}", git_base.display());

    let output = run_akit_pull(
        &["pull", "acme/kit-skills/deploy-to-vercel#main"],
        &catalog,
        &cache,
        &base_url,
    );
    assert!(output.status.success());
    let c1 = remote_head(base);
    let manifest_before = fs::read_to_string(catalog.join("akit.yml")).unwrap();

    // A syntactically-valid but unreachable SHA is rejected with actionable guidance.
    let bogus = "0123456789abcdef0123456789abcdef01234567";
    let output = run_akit_pull(
        &["update", "deploy-to-vercel", "--to", bogus],
        &catalog,
        &cache,
        &base_url,
    );
    assert!(!output.status.success());
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("not reachable"),
        "stderr:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );

    // The manifest is untouched: it still records the original commit C1.
    let manifest_after = fs::read_to_string(catalog.join("akit.yml")).unwrap();
    assert_eq!(manifest_before, manifest_after);
    assert!(
        manifest_after.contains(&format!("commit: {c1}")),
        "{manifest_after}"
    );
}

#[test]
fn update_advances_and_records_commit() {
    let tmp = test_tempdir();
    let base = tmp.path();
    let git_base = make_local_bare_remote(base);
    let cache = base.join("cache");
    let catalog = base.join("catalog");
    let base_url = format!("file://{}", git_base.display());

    let output = run_akit_pull(
        &["pull", "acme/kit-skills/deploy-to-vercel#main"],
        &catalog,
        &cache,
        &base_url,
    );
    assert!(output.status.success());
    let c1 = remote_head(base);

    push_remote_change(base, &git_base, "updated body");
    let c2 = remote_head(base);

    let output = run_akit_pull(&["update"], &catalog, &cache, &base_url);
    assert!(output.status.success());
    let json: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(json["items"][0]["status"], "updated");
    assert_eq!(json["items"][0]["previous_commit"], c1);
    assert_eq!(json["items"][0]["commit"], c2);

    // The manifest now records the advanced commit.
    let manifest = fs::read_to_string(catalog.join("akit.yml")).unwrap();
    assert!(manifest.contains(&format!("commit: {c2}")), "{manifest}");
}

/// End-to-end `update --propagate` (issue #40): a refreshed catalog item is
/// re-materialized into the known projects that copied it, while a project whose
/// copy was hand-edited is reported as a conflict and left alone.
#[test]
fn update_propagate_resyncs_copy_installs_in_known_projects() {
    let tmp = test_tempdir();
    let base = tmp.path();
    let git_base = make_local_bare_remote(base);
    let cache = base.join("cache");
    let catalog = base.join("catalog");
    let base_url = format!("file://{}", git_base.display());

    // Install the (remote-backed) skill into two projects, so both land in the
    // global install index.
    let clean = base.join("clean-project");
    let edited = base.join("edited-project");
    for project in [&clean, &edited] {
        fs::create_dir_all(project).unwrap();
        assert_git(&["init", "-q"], project);
        let output = run_akit_install(
            &["install", "acme/kit-skills/deploy-to-vercel#main"],
            project,
            &catalog,
            &cache,
            &base_url,
            "copilot",
        );
        assert!(
            output.status.success(),
            "install failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
    let installed = |project: &Path| project.join(".agents/skills/deploy-to-vercel/SKILL.md");
    fs::write(installed(&edited), "hand-edited").unwrap();

    push_remote_change(base, &git_base, "updated body");

    let output = run_akit_pull(&["update", "--propagate"], &catalog, &cache, &base_url);
    assert!(
        output.status.success(),
        "update --propagate failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let json: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    // The pre-existing update shape is untouched; propagation is additive.
    assert_eq!(json["items"][0]["status"], "updated");
    let summary = &json["propagation"]["summary"];
    assert_eq!(summary["projects"], 2, "{json}");
    assert_eq!(summary["updated"], 1);
    assert_eq!(summary["drifted"], 1);
    assert_eq!(summary["errors"], 0);

    // The clean copy now carries the refreshed content; the edited one does not.
    assert!(
        fs::read_to_string(installed(&clean))
            .unwrap()
            .contains("updated body")
    );
    assert_eq!(
        fs::read_to_string(installed(&edited)).unwrap(),
        "hand-edited"
    );

    // Re-running has nothing to update, and the human report names both outcomes.
    let output = Command::new(env!("CARGO_BIN_EXE_akit"))
        .args(["update", "--propagate"])
        .env("KIT_CATALOG_DIR", &catalog)
        .env("AKIT_STATE_DIR", catalog.with_file_name("akit-state"))
        .env(remote::ENV_CACHE_DIR, &cache)
        .env(remote::ENV_REMOTE_BASE_URL, &base_url)
        .output()
        .expect("akit binary should run");
    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("Propagate:"), "{stdout}");
    assert!(stdout.contains("up to date"), "{stdout}");
    assert!(
        stdout.contains("drifted") && stdout.contains("not overwritten"),
        "{stdout}"
    );
    assert!(
        stdout.contains("1 updated") || stdout.contains("0 updated"),
        "{stdout}"
    );
}

#[test]
fn pull_of_a_flat_remote_agent_is_rejected_with_a_migration_message() {
    let tmp = test_tempdir();
    let base = tmp.path();
    let git_base = make_local_bare_flat_agent_remote(base);
    let cache = base.join("cache");
    let catalog = base.join("catalog");
    let base_url = format!("file://{}", git_base.display());

    for spec in [
        "acme/kit-agents/agents/reviewer.agent.md#main",
        // The bare-path form resolves to the same flat file and must fail the same way.
        "acme/kit-agents/reviewer#main",
    ] {
        let output = run_akit_pull(&["pull", "--agent", spec], &catalog, &cache, &base_url);
        assert!(!output.status.success(), "pull of a flat agent must fail");
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(
            stderr.contains("no longer a supported catalog shape"),
            "expected a migration hint, got:\n{stderr}"
        );
        assert!(stderr.contains("agent.yml"), "{stderr}");
    }

    // Nothing was written to the catalog, and no manifest entry was recorded.
    assert!(!catalog.join("agents/reviewer.agent.md").exists());
    assert!(!catalog.join("akit.yml").exists());
}

#[test]
fn old_manifest_with_a_flat_agent_entry_degrades_gracefully() {
    let tmp = test_tempdir();
    let base = tmp.path();
    // A real (package) remote so the *other* entry still restores normally.
    let git_base = make_local_bare_agent_pkg_remote(base);
    let cache = base.join("cache");
    let catalog = base.join("catalog");
    let base_url = format!("file://{}", git_base.display());

    // A manifest as written before v0.32.0: a flat `.agent.md` string shorthand
    // alongside a modern agent package entry.
    fs::create_dir_all(&catalog).unwrap();
    fs::write(
        catalog.join("akit.yml"),
        "name: akit-catalog\nversion: 0.0.0\ndependencies:\n  apm:\n  \
         - acme/kit-agents/agents/legacy.agent.md#main\n  - git: acme/kit-agents\n    \
         path: agents/reviewer\n    type: agent\n    ref: main\n",
    )
    .unwrap();

    let output = run_akit_pull(&["restore"], &catalog, &cache, &base_url);
    let stdout = String::from_utf8_lossy(&output.stdout);
    let json: serde_json::Value = serde_json::from_str(&stdout).unwrap_or_else(|e| {
        panic!("restore should still emit a report ({e})\nstdout:\n{stdout}");
    });

    // The legacy entry is a per-item error with a migration hint; it does not panic
    // and does not abort the run.
    let items = json["items"].as_array().unwrap();
    let legacy = items
        .iter()
        .find(|i| i["id"] == "legacy")
        .expect("legacy entry must be reported, not dropped");
    assert_eq!(legacy["status"], "error");
    let err = legacy["error"].as_str().unwrap();
    assert!(err.contains("legacy flat"), "{err}");
    assert!(err.contains("agent.yml"), "{err}");

    // The package entry beside it restored normally.
    let reviewer = items.iter().find(|i| i["id"] == "reviewer").unwrap();
    assert_ne!(reviewer["status"], "error", "{stdout}");
    assert!(catalog.join("agents/reviewer/agent.yml").is_file());

    // The manifest is left untouched so the entry can be migrated and retried.
    let manifest = fs::read_to_string(catalog.join("akit.yml")).unwrap();
    assert!(manifest.contains("legacy.agent.md"), "{manifest}");

    // `update` degrades the same way ...
    let output = run_akit_pull(&["update"], &catalog, &cache, &base_url);
    let json: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    let legacy = json["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|i| i["id"] == "legacy")
        .unwrap();
    assert_eq!(legacy["status"], "error");
    assert!(legacy["error"].as_str().unwrap().contains("legacy flat"));

    // ... and `drop` is the escape hatch that forgets the stale entry.
    let output = run_akit_pull(&["drop", "--agent", "legacy"], &catalog, &cache, &base_url);
    assert!(
        output.status.success(),
        "drop of a stale flat entry should succeed\nstderr:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let manifest = fs::read_to_string(catalog.join("akit.yml")).unwrap();
    assert!(!manifest.contains("legacy.agent.md"), "{manifest}");
}

#[test]
fn pull_agent_package_into_catalog_and_drop() {
    let tmp = test_tempdir();
    let base = tmp.path();
    let git_base = make_local_bare_agent_pkg_remote(base);
    let cache = base.join("cache");
    let catalog = base.join("catalog");
    let base_url = format!("file://{}", git_base.display());

    // Pull the harness-aware agent package into the catalog.
    let output = run_akit_pull(
        &["pull", "--agent", "acme/kit-agents/agents/reviewer#main"],
        &catalog,
        &cache,
        &base_url,
    );
    assert!(
        output.status.success(),
        "akit pull --agent failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let json: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(json["id"], "reviewer");
    assert_eq!(json["type"], "agent");
    assert_eq!(json["created"], true);

    // The catalog now holds a package DIRECTORY (agent.yml + variants), not a flat file.
    let pkg = catalog.join("agents/reviewer");
    assert!(
        pkg.join("agent.yml").is_file(),
        "package descriptor missing"
    );
    assert!(pkg.join("copilot.agent.md").is_file());
    assert!(pkg.join("claude.md").is_file());
    assert!(!catalog.join("agents/reviewer.agent.md").exists());

    // `ls` surfaces it as a package with its supported harnesses.
    let ls = Command::new(env!("CARGO_BIN_EXE_akit"))
        .args(["--json", "ls"])
        .env("KIT_CATALOG_DIR", &catalog)
        .output()
        .unwrap();
    let items: serde_json::Value = serde_json::from_slice(&ls.stdout).unwrap();
    let reviewer = items
        .as_array()
        .unwrap()
        .iter()
        .find(|i| i["id"] == "reviewer")
        .expect("reviewer listed");
    assert_eq!(
        reviewer["harnesses"],
        serde_json::json!(["copilot", "claude"])
    );

    // `restore` rebootstraps the package from the manifest after the catalog copy
    // is lost (round-trips the whole directory, not a flat file).
    fs::remove_dir_all(&pkg).unwrap();
    let restore = run_akit_pull(&["restore"], &catalog, &cache, &base_url);
    assert!(
        restore.status.success(),
        "restore failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&restore.stdout),
        String::from_utf8_lossy(&restore.stderr)
    );
    assert!(
        pkg.join("agent.yml").is_file(),
        "restore should recreate the package directory"
    );
    assert!(pkg.join("copilot.agent.md").is_file());

    // `drop` removes the whole package directory and prunes the manifest.
    let drop = run_akit_pull(
        &["drop", "--agent", "reviewer"],
        &catalog,
        &cache,
        &base_url,
    );
    assert!(
        drop.status.success(),
        "{}",
        String::from_utf8_lossy(&drop.stderr)
    );
    let djson: serde_json::Value = serde_json::from_slice(&drop.stdout).unwrap();
    assert_eq!(djson["item_removed"], true);
    assert_eq!(djson["manifest_pruned"], true);
    assert!(!pkg.exists(), "package directory should be gone after drop");
}

// ── `install <remote>` — pull into catalog, then install (issue #45) ──────────

/// Run `akit install` with a writable temp catalog plus the remote cache/base
/// and harness env, so a remote `<id>` pulls into the catalog before installing.
fn run_akit_install(
    args: &[&str],
    project: &Path,
    catalog: &Path,
    cache: &Path,
    base_url: &str,
    harnesses: &str,
) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_akit"))
        .args(["--project", project.to_str().unwrap()])
        .args(args)
        .env("KIT_CATALOG_DIR", catalog)
        // Keep the global install index (#40) inside the fixture.
        .env("AKIT_STATE_DIR", catalog.with_file_name("akit-state"))
        .env(remote::ENV_CACHE_DIR, cache)
        .env(remote::ENV_REMOTE_BASE_URL, base_url)
        .env("AKIT_HARNESSES", harnesses)
        .output()
        .expect("akit binary should run")
}

#[test]
fn install_remote_skill_pulls_into_catalog_then_installs() {
    let tmp = test_tempdir();
    let base = tmp.path();
    let git_base = make_local_bare_remote(base);
    let (proj, _project) = init_project(base);
    let catalog = base.join("catalog");
    let cache = base.join("cache");
    let base_url = format!("file://{}", git_base.display());

    let out = run_akit_install(
        &["install", "acme/kit-skills/deploy-to-vercel#main"],
        &proj,
        &catalog,
        &cache,
        &base_url,
        "claude",
    );
    assert!(
        out.status.success(),
        "install <remote> failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("Pulled skill 'deploy-to-vercel'"),
        "{stdout}"
    );
    assert!(
        stdout.contains("Installed skill 'deploy-to-vercel' for claude"),
        "{stdout}"
    );

    // Landed in the catalog…
    assert!(catalog.join("skills/deploy-to-vercel/SKILL.md").is_file());
    // …and installed into the project for claude.
    assert!(
        proj.join(".claude/skills/deploy-to-vercel/SKILL.md")
            .is_file()
    );
}

#[test]
fn install_remote_agent_package_pulls_and_installs_native_files() {
    let tmp = test_tempdir();
    let base = tmp.path();
    let git_base = make_local_bare_agent_pkg_remote(base);
    let (proj, _project) = init_project(base);
    let catalog = base.join("catalog");
    let cache = base.join("cache");
    let base_url = format!("file://{}", git_base.display());

    let out = run_akit_install(
        &["install", "--agent", "acme/kit-agents/agents/reviewer#main"],
        &proj,
        &catalog,
        &cache,
        &base_url,
        "copilot,claude",
    );
    assert!(
        out.status.success(),
        "install --agent <remote> failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    // Package landed in the catalog and native files were installed per harness.
    assert!(catalog.join("agents/reviewer/agent.yml").is_file());
    assert!(proj.join(".github/agents/reviewer.agent.md").is_file());
    assert!(proj.join(".claude/agents/reviewer.md").is_file());
}

#[test]
fn install_remote_dry_run_is_refused() {
    let tmp = test_tempdir();
    let base = tmp.path();
    let git_base = make_local_bare_remote(base);
    let (proj, _project) = init_project(base);
    let catalog = base.join("catalog");
    let cache = base.join("cache");
    let base_url = format!("file://{}", git_base.display());

    let out = run_akit_install(
        &[
            "install",
            "--dry-run",
            "acme/kit-skills/deploy-to-vercel#main",
        ],
        &proj,
        &catalog,
        &cache,
        &base_url,
        "claude",
    );
    assert!(!out.status.success());
    assert!(
        String::from_utf8_lossy(&out.stderr).contains("can't preview a remote source"),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    // Nothing was pulled into the catalog.
    assert!(!catalog.join("skills/deploy-to-vercel").exists());
}

#[test]
fn install_malformed_remote_spec_is_rejected() {
    let tmp = test_tempdir();
    let base = tmp.path();
    let (proj, _project) = init_project(base);
    let catalog = base.join("catalog");
    let cache = base.join("cache");

    // Two segments: not a valid owner/repo/path spec, but contains '/', so it's
    // treated as a malformed remote rather than a (slash-free) catalog id.
    let out = run_akit_install(
        &["install", "owner/repo"],
        &proj,
        &catalog,
        &cache,
        "file:///unused",
        "claude",
    );
    assert!(!out.status.success());
    assert!(
        String::from_utf8_lossy(&out.stderr).contains("invalid remote source spec"),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}
