use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use akit::lockfile::{ItemType, Lockfile, Mode};
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
fn remote_cli_fetch_add_and_rm_via_local_bare_repo() {
    let tmp = test_tempdir();
    let base = tmp.path();
    let git_base = make_local_bare_remote(base);
    let (proj, project) = init_project(base);
    let cache = base.join("cache");
    let base_url = format!("file://{}", git_base.display());

    let output = run_akit(
        &["add", "acme/kit-skills/deploy-to-vercel#main"],
        &proj,
        &cache,
        Some(&base_url),
    );
    assert!(
        output.status.success(),
        "akit add failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    let json: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(json["id"], "deploy-to-vercel");
    assert_eq!(json["type"], "skill");
    assert_eq!(json["target"], ".github/skills/deploy-to-vercel");
    assert_eq!(json["source"], "acme/kit-skills/deploy-to-vercel");
    assert_eq!(json["ref"], "main");

    let target = proj.join(".github/skills/deploy-to-vercel");
    assert!(
        fs::symlink_metadata(&target).is_ok(),
        "remote skill should be materialized"
    );
    assert!(target.join("SKILL.md").is_file());

    let lockfile = Lockfile::load(&project.lockfile_path()).unwrap();
    assert_eq!(lockfile.items.len(), 1);
    let item = &lockfile.items[0];
    assert_eq!(item.id, "deploy-to-vercel");
    assert_eq!(item.item_type, ItemType::Skill);
    assert_eq!(item.mode, Mode::Symlink);
    assert_eq!(item.source, "acme/kit-skills/deploy-to-vercel");
    assert_eq!(item.git_ref.as_deref(), Some("main"));
    assert_eq!(item.target, ".github/skills/deploy-to-vercel");

    let exclude = fs::read_to_string(proj.join(".git/info/exclude")).unwrap();
    assert!(
        exclude
            .lines()
            .any(|line| line == "/.github/skills/deploy-to-vercel")
    );
    assert!(
        exclude
            .lines()
            .any(|line| line == "/.copilot/kit.lock.json")
    );

    let status = git(&["status", "--porcelain"], &proj);
    assert!(
        status.stdout.is_empty(),
        "git status not clean after add: {}",
        String::from_utf8_lossy(&status.stdout)
    );

    let output = run_akit(&["rm", "deploy-to-vercel"], &proj, &cache, Some(&base_url));
    assert!(
        output.status.success(),
        "akit rm failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(fs::symlink_metadata(&target).is_err());

    let lockfile = Lockfile::load(&project.lockfile_path()).unwrap();
    assert!(lockfile.items.is_empty());
    let exclude = fs::read_to_string(proj.join(".git/info/exclude")).unwrap();
    assert!(
        !exclude
            .lines()
            .any(|line| line == "/.github/skills/deploy-to-vercel")
    );

    let status = git(&["status", "--porcelain"], &proj);
    assert!(
        status.stdout.is_empty(),
        "git status not clean after rm: {}",
        String::from_utf8_lossy(&status.stdout)
    );
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
    assert!(
        catalog
            .join("skills/vercel-deploy/SKILL.md")
            .is_file()
    );
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
    assert!(!fs::read_to_string(&skill_md).unwrap().contains("updated body"));

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
    assert!(fs::read_to_string(&skill_md).unwrap().contains("updated body"));

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
    assert!(body.contains("body") && !body.contains("updated body"), "{body}");
    let manifest = fs::read_to_string(catalog.join("akit.yml")).unwrap();
    assert!(manifest.contains(&format!("commit: {c1}")), "{manifest}");

    // `restore --latest` moves to the head of the ref (C2) and rewrites the recorded commit.
    let output = run_akit_pull(&["restore", "--latest", "--force"], &catalog, &cache, &base_url);
    assert!(
        output.status.success(),
        "akit restore --latest failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(fs::read_to_string(&skill_md).unwrap().contains("updated body"));
    let manifest = fs::read_to_string(catalog.join("akit.yml")).unwrap();
    assert!(manifest.contains(&format!("commit: {c2}")), "{manifest}");
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
