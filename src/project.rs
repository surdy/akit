//! The project: where pulled items are materialized, and where the lockfile + git exclude live.

use anyhow::{Context, Result};
use std::path::{Path, PathBuf};

/// A handle to the target project (a working directory, ideally a git repo).
pub struct Project {
    /// The project root (git top-level if found, else the starting directory).
    pub root: PathBuf,
    /// The resolved `.git` directory, if this is a git repo.
    pub git_dir: Option<PathBuf>,
}

impl Project {
    /// Locate the project. If `explicit` is given it is used as the starting point;
    /// otherwise the current working directory is used. From there we walk up to the
    /// nearest `.git` to determine the root; if none is found, the starting directory
    /// is the root and `git_dir` is `None`.
    pub fn locate(explicit: Option<PathBuf>) -> Result<Self> {
        let start = match explicit {
            Some(p) => p,
            None => std::env::current_dir().context("could not get current directory")?,
        };
        let start = start.canonicalize().unwrap_or(start);

        let mut cur: &Path = &start;
        loop {
            let dotgit = cur.join(".git");
            if dotgit.exists() {
                return Ok(Self {
                    root: cur.to_path_buf(),
                    git_dir: resolve_git_dir(&dotgit),
                });
            }
            match cur.parent() {
                Some(p) => cur = p,
                None => break,
            }
        }
        Ok(Self {
            root: start,
            git_dir: None,
        })
    }

    /// `<root>/.github/skills`
    pub fn github_skills_dir(&self) -> PathBuf {
        self.root.join(".github").join("skills")
    }

    /// `<root>/.github/agents`
    pub fn github_agents_dir(&self) -> PathBuf {
        self.root.join(".github").join("agents")
    }

    /// `<root>/.copilot/kit.lock.json`
    pub fn lockfile_path(&self) -> PathBuf {
        self.root.join(".copilot").join("kit.lock.json")
    }

    /// `<git_dir>/info/exclude`, if this is a git repo.
    pub fn git_info_exclude_path(&self) -> Option<PathBuf> {
        self.git_dir
            .as_ref()
            .map(|g| g.join("info").join("exclude"))
    }
}

/// Resolve the actual git directory from a `.git` entry, which may be a directory
/// (normal repo) or a file containing `gitdir: <path>` (worktrees / submodules).
fn resolve_git_dir(dotgit: &Path) -> Option<PathBuf> {
    if dotgit.is_dir() {
        return Some(dotgit.to_path_buf());
    }
    if dotgit.is_file()
        && let Ok(content) = std::fs::read_to_string(dotgit)
        && let Some(rest) = content.trim().strip_prefix("gitdir:")
    {
        let p = PathBuf::from(rest.trim());
        let abs = if p.is_absolute() {
            p
        } else {
            dotgit.parent().map(|d| d.join(&p)).unwrap_or(p)
        };
        return Some(abs);
    }
    None
}
