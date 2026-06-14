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
        .env_remove("KIT_COLLECTION_DIR")
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
    collection: &Path,
    cache: &Path,
    base_url: &str,
) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_akit"))
        .args(["--json"])
        .args(args)
        .env(remote::ENV_CACHE_DIR, cache)
        .env("KIT_COLLECTION_DIR", collection)
        .env(remote::ENV_REMOTE_BASE_URL, base_url)
        .output()
        .expect("akit binary should run")
}

#[test]
fn pull_remote_into_collection_via_local_bare_repo() {
    let tmp = test_tempdir();
    let base = tmp.path();
    let git_base = make_local_bare_remote(base);
    let cache = base.join("cache");
    let collection = base.join("collection");
    let base_url = format!("file://{}", git_base.display());

    // First pull copies the remote skill into the collection.
    let output = run_akit_pull(
        &["pull", "acme/kit-skills/deploy-to-vercel#main"],
        &collection,
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

    let skill_dir = collection.join("skills/deploy-to-vercel");
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
        &collection,
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
        &collection,
        &cache,
        &base_url,
    );
    assert!(output.status.success());
    let json: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(json["id"], "vercel-deploy");
    assert_eq!(json["created"], true);
    assert!(
        collection
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
    let collection = base.join("collection");
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
        let output = run_akit_pull(&args, &collection, &cache, &base_url);
        assert!(
            output.status.success(),
            "akit pull failed\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }

    // The manifest records both items (shorthand for the default id, object form for `--as`).
    let manifest = fs::read_to_string(collection.join("apm.yml")).unwrap();
    assert!(
        manifest.contains("acme/kit-skills/deploy-to-vercel#main"),
        "{manifest}"
    );
    assert!(manifest.contains("alias: vercel"), "{manifest}");

    // Simulate a fresh machine: wipe the materialized items but keep the manifest.
    fs::remove_dir_all(collection.join("skills")).unwrap();
    assert!(!collection.join("skills/deploy-to-vercel").exists());

    // Restore re-fetches everything in the manifest.
    let output = run_akit_pull(&["restore"], &collection, &cache, &base_url);
    assert!(
        output.status.success(),
        "akit restore failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let json: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(json["summary"]["pulled"], 2);
    assert_eq!(json["summary"]["errors"], 0);
    assert!(collection.join("skills/deploy-to-vercel/SKILL.md").is_file());
    assert!(collection.join("skills/vercel/SKILL.md").is_file());

    // Restore is idempotent: a second run reports everything already present.
    let output = run_akit_pull(&["restore"], &collection, &cache, &base_url);
    assert!(output.status.success());
    let json: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(json["summary"]["already_present"], 2);
    assert_eq!(json["summary"]["pulled"], 0);
}
