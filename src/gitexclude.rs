//! Idempotent, line-scoped editing of git's `info/exclude` so personal pulls never touch
//! the tracked `.gitignore` and are never committed.

use anyhow::{Context, Result};
use std::path::Path;

/// Ensure `line` is present in the exclude file. Returns `true` if it was added.
pub fn add_line(exclude_path: &Path, line: &str) -> Result<bool> {
    let existing = read_existing(exclude_path)?;
    if existing.lines().any(|l| l.trim() == line) {
        return Ok(false);
    }
    if let Some(parent) = exclude_path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("creating {}", parent.display()))?;
    }
    let mut content = existing;
    if !content.is_empty() && !content.ends_with('\n') {
        content.push('\n');
    }
    content.push_str(line);
    content.push('\n');
    std::fs::write(exclude_path, content)
        .with_context(|| format!("writing {}", exclude_path.display()))?;
    Ok(true)
}

/// Remove `line` from the exclude file if present. Returns `true` if it was removed.
pub fn remove_line(exclude_path: &Path, line: &str) -> Result<bool> {
    if !exclude_path.exists() {
        return Ok(false);
    }
    let existing = std::fs::read_to_string(exclude_path)
        .with_context(|| format!("reading {}", exclude_path.display()))?;
    let mut removed = false;
    let kept: Vec<&str> = existing
        .lines()
        .filter(|l| {
            let keep = l.trim() != line;
            if !keep {
                removed = true;
            }
            keep
        })
        .collect();
    if removed {
        let mut content = kept.join("\n");
        if !content.is_empty() {
            content.push('\n');
        }
        std::fs::write(exclude_path, content)
            .with_context(|| format!("writing {}", exclude_path.display()))?;
    }
    Ok(removed)
}

fn read_existing(exclude_path: &Path) -> Result<String> {
    if exclude_path.exists() {
        std::fs::read_to_string(exclude_path)
            .with_context(|| format!("reading {}", exclude_path.display()))
    } else {
        Ok(String::new())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn add_is_idempotent_and_remove_works() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("exclude");
        assert!(add_line(&path, "/.github/skills/demo").unwrap());
        assert!(!add_line(&path, "/.github/skills/demo").unwrap());
        let content = std::fs::read_to_string(&path).unwrap();
        assert_eq!(
            content.lines().filter(|l| *l == "/.github/skills/demo").count(),
            1
        );
        assert!(remove_line(&path, "/.github/skills/demo").unwrap());
        assert!(!remove_line(&path, "/.github/skills/demo").unwrap());
    }
}
