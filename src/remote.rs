//! Git-backed remote source resolution for `owner/repo/path[#ref]` specs.
//!
//! This is the cache-fetch implementation behind the future APM-backed source path. It keeps
//! remote sources in a local git checkout cache, then lets the existing materialization pipeline
//! symlink/copy from that cache into a project.

use anyhow::{Context, Result, bail};
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::Command;

/// Environment variable that overrides the source checkout cache root.
pub const ENV_CACHE_DIR: &str = "KIT_CACHE_DIR";

/// Environment variable that overrides the remote git URL base used by the CLI.
pub const ENV_REMOTE_BASE_URL: &str = "KIT_REMOTE_BASE_URL";

/// Default remote git URL base.
pub const DEFAULT_BASE_URL: &str = "https://github.com";

/// A remote source of the form `owner/repo/path[#ref]`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourceSpec {
    pub owner: String,
    pub repo: String,
    pub path: String,
    pub ref_: Option<String>,
}

impl SourceSpec {
    /// Parse `owner/repo/path[#ref]`.
    ///
    /// Returns `None` for bare local names and malformed remote specs.
    pub fn parse(s: &str) -> Option<Self> {
        let (source, ref_) = match s.split_once('#') {
            Some((source, ref_)) if !ref_.is_empty() && !ref_.contains('#') => {
                (source, Some(ref_.to_string()))
            }
            Some(_) => return None,
            None => (s, None),
        };
        Self::from_source_and_ref(source, ref_)
    }

    /// Construct from the lockfile's `source` and `ref` fields.
    pub fn from_source_and_ref(source: &str, ref_: Option<String>) -> Option<Self> {
        if source.contains('#') {
            return None;
        }
        let parts = source.split('/').collect::<Vec<_>>();
        if parts.len() < 3 || parts.iter().any(|part| !valid_path_segment(part)) {
            return None;
        }
        Some(Self {
            owner: parts[0].to_string(),
            repo: parts[1].to_string(),
            path: parts[2..].join("/"),
            ref_,
        })
    }

    /// Lockfile source string, excluding `#ref`.
    pub fn source(&self) -> String {
        format!("{}/{}/{}", self.owner, self.repo, self.path)
    }

    /// Last path segment, used as the installed item leaf.
    pub fn leaf(&self) -> &str {
        self.path
            .rsplit('/')
            .next()
            .expect("SourceSpec path always has at least one segment")
    }
}

/// Return the source cache root.
///
/// Precedence: `$KIT_CACHE_DIR`, `$XDG_CACHE_HOME/akit`, then `~/.cache/akit`.
pub fn cache_root() -> PathBuf {
    if let Some(path) = nonempty_env(ENV_CACHE_DIR) {
        return PathBuf::from(path);
    }
    if let Some(path) = nonempty_env("XDG_CACHE_HOME") {
        return PathBuf::from(path).join("akit");
    }
    dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".cache")
        .join("akit")
}

/// Fetch (or reuse) the remote repository and return the requested item path inside the cache.
pub fn fetch(spec: &SourceSpec, base_url: &str) -> Result<PathBuf> {
    fetch_with_cache_root(spec, base_url, &cache_root())
}

/// Fetch using an explicit cache root. This is useful for hermetic callers and tests.
pub fn fetch_with_cache_root(
    spec: &SourceSpec,
    base_url: &str,
    cache_root: &Path,
) -> Result<PathBuf> {
    let checkout = checkout_dir(cache_root, spec);
    let item = resolved_item_path(&checkout, spec);

    if is_git_checkout(&checkout) {
        checkout_ref_if_available(&checkout, spec);
        if item.exists() {
            return Ok(item);
        }
        fetch_ref(&checkout, spec).with_context(|| {
            format!(
                "updating cached source {} in {}",
                spec.source(),
                checkout.display()
            )
        })?;
    } else if checkout.exists() {
        bail!(
            "cache path {} exists but is not a git checkout",
            checkout.display()
        );
    } else {
        clone_checkout(spec, base_url, &checkout)?;
    }

    let item = resolved_item_path(&checkout, spec);
    if !item.exists() {
        bail!(
            "remote source '{}' not found at {}",
            spec.source(),
            item.display()
        );
    }
    Ok(item)
}

/// Return the cached item path without fetching.
pub fn cached_item_path(spec: &SourceSpec) -> PathBuf {
    resolved_item_path(&checkout_dir(&cache_root(), spec), spec)
}

fn valid_path_segment(segment: &str) -> bool {
    !segment.is_empty() && segment != "." && segment != ".." && !segment.contains('\\')
}

fn nonempty_env(name: &str) -> Option<OsString> {
    std::env::var_os(name).filter(|value| !value.is_empty())
}

fn checkout_dir(cache_root: &Path, spec: &SourceSpec) -> PathBuf {
    cache_root.join("sources").join(&spec.owner).join(format!(
        "{}@{}",
        spec.repo,
        cache_ref(spec.ref_.as_deref())
    ))
}

fn cache_ref(ref_: Option<&str>) -> String {
    let raw = ref_.unwrap_or("default");
    raw.chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || matches!(ch, '.' | '_' | '-') {
                ch
            } else {
                '_'
            }
        })
        .collect()
}

fn item_path(checkout: &Path, spec: &SourceSpec) -> PathBuf {
    spec.path
        .split('/')
        .fold(checkout.to_path_buf(), |path, segment| path.join(segment))
}

fn resolved_item_path(checkout: &Path, spec: &SourceSpec) -> PathBuf {
    let direct = item_path(checkout, spec);
    if direct.exists() || spec.path.contains('/') {
        return direct;
    }

    let skill_dir = checkout.join("skills").join(&spec.path);
    if skill_dir.exists() {
        return skill_dir;
    }

    let agent_leaf = spec
        .path
        .strip_suffix(".agent.md")
        .map_or_else(|| format!("{}.agent.md", spec.path), str::to_string);
    let agent_file = checkout.join("agents").join(agent_leaf);
    if agent_file.exists() {
        return agent_file;
    }

    direct
}

fn is_git_checkout(path: &Path) -> bool {
    path.join(".git").is_dir()
}

fn clone_checkout(spec: &SourceSpec, base_url: &str, checkout: &Path) -> Result<()> {
    if let Some(parent) = checkout.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("creating {}", parent.display()))?;
    }
    let url = clone_url(base_url, spec);
    if let Some(ref_) = spec.ref_.as_deref() {
        let branch_clone = run_git_status(
            &[
                "clone".into(),
                "--depth".into(),
                "1".into(),
                "--branch".into(),
                ref_.into(),
                url.clone().into(),
                checkout.as_os_str().to_os_string(),
            ],
            None,
        );
        if branch_clone.is_ok() {
            return Ok(());
        }
        cleanup_failed_checkout(checkout)?;
        run_git(
            &[
                "clone".into(),
                "--depth".into(),
                "1".into(),
                url.into(),
                checkout.as_os_str().to_os_string(),
            ],
            None,
        )?;
        fetch_ref(checkout, spec)
    } else {
        run_git(
            &[
                "clone".into(),
                "--depth".into(),
                "1".into(),
                url.into(),
                checkout.as_os_str().to_os_string(),
            ],
            None,
        )
    }
}

fn cleanup_failed_checkout(checkout: &Path) -> Result<()> {
    match std::fs::remove_dir_all(checkout) {
        Ok(()) => Ok(()),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(err) => Err(err).with_context(|| format!("removing {}", checkout.display())),
    }
}

fn fetch_ref(checkout: &Path, spec: &SourceSpec) -> Result<()> {
    let Some(ref_) = spec.ref_.as_deref() else {
        return Ok(());
    };
    run_git(
        &[
            "fetch".into(),
            "--depth".into(),
            "1".into(),
            "origin".into(),
            ref_.into(),
        ],
        Some(checkout),
    )?;
    run_git(
        &[
            "checkout".into(),
            "-q".into(),
            "--detach".into(),
            "FETCH_HEAD".into(),
        ],
        Some(checkout),
    )
}

fn checkout_ref_if_available(checkout: &Path, spec: &SourceSpec) {
    if let Some(ref_) = spec.ref_.as_deref() {
        let _ = run_git_status(
            &["checkout".into(), "-q".into(), ref_.into()],
            Some(checkout),
        );
    }
}

fn clone_url(base_url: &str, spec: &SourceSpec) -> String {
    format!(
        "{}/{}/{}",
        base_url.trim_end_matches('/'),
        spec.owner,
        spec.repo
    )
}

fn run_git(args: &[OsString], cwd: Option<&Path>) -> Result<()> {
    let output = run_git_status(args, cwd)?;
    if output.status.success() {
        return Ok(());
    }
    bail!(
        "git {} failed with status {}\nstdout:\n{}\nstderr:\n{}",
        args.iter()
            .map(|arg| arg.to_string_lossy())
            .collect::<Vec<_>>()
            .join(" "),
        output.status,
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    )
}

fn run_git_status(args: &[OsString], cwd: Option<&Path>) -> Result<std::process::Output> {
    let mut command = Command::new("git");
    command.args(args);
    if let Some(cwd) = cwd {
        command.current_dir(cwd);
    }
    command
        .output()
        .with_context(|| format!("running git {}", display_args(args)))
}

fn display_args(args: &[OsString]) -> String {
    args.iter()
        .map(|arg| arg.to_string_lossy())
        .collect::<Vec<_>>()
        .join(" ")
}
