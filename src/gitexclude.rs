//! Idempotent, line-scoped editing of git's `info/exclude` so personal pulls never touch
//! the tracked `.gitignore` and are never committed.
//!
//! Two layering styles coexist:
//!   - [`add_line`] / [`remove_line`] edit individual anonymous lines (the legacy
//!     flat-model path).
//!   - [`set_managed_lines`] / [`managed_lines`] maintain a dedicated
//!     `# >>> akit-managed >>>` … `# <<< akit-managed <<<` block whose contents
//!     akit fully owns. The harness-aware model (v0.10+) drives excludes through
//!     this block so it can safely identify — and prune — exactly its own lines
//!     (stale-exclude cleanup) without ever touching user-authored entries.

use anyhow::{Context, Result};
use std::path::Path;

use crate::transport::FsTransport;

/// Opening sentinel of the akit-managed exclude block.
pub const MANAGED_BEGIN: &str = "# >>> akit-managed (do not edit) >>>";
/// Closing sentinel of the akit-managed exclude block.
pub const MANAGED_END: &str = "# <<< akit-managed <<<";

/// Ensure `line` is present in the exclude file. Returns `true` if it was added.
pub fn add_line(fs: &dyn FsTransport, exclude_path: &Path, line: &str) -> Result<bool> {
    let existing = read_existing(fs, exclude_path)?;
    if existing.lines().any(|l| l.trim() == line) {
        return Ok(false);
    }
    let mut content = existing;
    if !content.is_empty() && !content.ends_with('\n') {
        content.push('\n');
    }
    content.push_str(line);
    content.push('\n');
    fs.write(exclude_path, content.as_bytes())
        .with_context(|| format!("writing {}", exclude_path.display()))?;
    Ok(true)
}

/// Remove `line` from the exclude file if present. Returns `true` if it was removed.
pub fn remove_line(fs: &dyn FsTransport, exclude_path: &Path, line: &str) -> Result<bool> {
    if !fs.exists(exclude_path)? {
        return Ok(false);
    }
    let existing = read_existing(fs, exclude_path)?;
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
        fs.write(exclude_path, content.as_bytes())
            .with_context(|| format!("writing {}", exclude_path.display()))?;
    }
    Ok(removed)
}

fn read_existing(fs: &dyn FsTransport, exclude_path: &Path) -> Result<String> {
    if fs.exists(exclude_path)? {
        let bytes = fs
            .read(exclude_path)
            .with_context(|| format!("reading {}", exclude_path.display()))?;
        String::from_utf8(bytes)
            .with_context(|| format!("{} is not valid UTF-8", exclude_path.display()))
    } else {
        Ok(String::new())
    }
}

/// Read the current akit-managed block lines (excluding the sentinels). Returns
/// an empty vec when no block is present.
pub fn managed_lines(fs: &dyn FsTransport, exclude_path: &Path) -> Result<Vec<String>> {
    let existing = read_existing(fs, exclude_path)?;
    let mut out = Vec::new();
    let mut inside = false;
    for line in existing.lines() {
        let trimmed = line.trim();
        if trimmed == MANAGED_BEGIN {
            inside = true;
            continue;
        }
        if trimmed == MANAGED_END {
            inside = false;
            continue;
        }
        if inside && !trimmed.is_empty() {
            out.push(trimmed.to_string());
        }
    }
    Ok(out)
}

/// Rewrite the akit-managed block to exactly `lines` (deduped, order preserved),
/// leaving all content outside the block untouched. An empty `lines` removes the
/// block entirely. Returns `true` if the file changed.
///
/// This is the single primitive the harness-aware model uses for excludes: the
/// caller computes the full desired owned set and calls this, so add, remove,
/// reshape, stale-prune, and repair are all the same operation.
pub fn set_managed_lines(
    fs: &dyn FsTransport,
    exclude_path: &Path,
    lines: &[String],
) -> Result<bool> {
    // Dedupe while preserving first-seen order.
    let mut desired: Vec<String> = Vec::new();
    for l in lines {
        let l = l.trim().to_string();
        if !l.is_empty() && !desired.contains(&l) {
            desired.push(l);
        }
    }

    let existing = read_existing(fs, exclude_path)?;

    // Split existing into lines outside the managed block, dropping the old block.
    let mut outside: Vec<String> = Vec::new();
    let mut inside = false;
    let mut had_block = false;
    for line in existing.lines() {
        let trimmed = line.trim();
        if trimmed == MANAGED_BEGIN {
            inside = true;
            had_block = true;
            continue;
        }
        if trimmed == MANAGED_END {
            inside = false;
            continue;
        }
        if !inside {
            outside.push(line.to_string());
        }
    }

    // Nothing to do: no desired lines and no existing block.
    if desired.is_empty() && !had_block {
        return Ok(false);
    }

    // Trim trailing blank lines from the outside content for a tidy join.
    while matches!(outside.last(), Some(l) if l.trim().is_empty()) {
        outside.pop();
    }

    let mut result: Vec<String> = outside;
    if !desired.is_empty() {
        if !result.is_empty() {
            result.push(String::new());
        }
        result.push(MANAGED_BEGIN.to_string());
        result.extend(desired);
        result.push(MANAGED_END.to_string());
    }

    let mut content = result.join("\n");
    if !content.is_empty() {
        content.push('\n');
    }

    if content == existing {
        return Ok(false);
    }
    fs.write(exclude_path, content.as_bytes())
        .with_context(|| format!("writing {}", exclude_path.display()))?;
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::transport::LocalFs;

    #[test]
    fn add_is_idempotent_and_remove_works() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("exclude");
        assert!(add_line(&LocalFs, &path, "/.github/skills/demo").unwrap());
        assert!(!add_line(&LocalFs, &path, "/.github/skills/demo").unwrap());
        let content = std::fs::read_to_string(&path).unwrap();
        assert_eq!(
            content
                .lines()
                .filter(|l| *l == "/.github/skills/demo")
                .count(),
            1
        );
        assert!(remove_line(&LocalFs, &path, "/.github/skills/demo").unwrap());
        assert!(!remove_line(&LocalFs, &path, "/.github/skills/demo").unwrap());
    }

    #[test]
    fn managed_block_round_trips_and_preserves_user_lines() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("exclude");
        std::fs::write(&path, "# user\n/build\n").unwrap();

        assert!(
            set_managed_lines(
                &LocalFs,
                &path,
                &["/.agents/skills/a".into(), "/.akit/kit.lock.json".into()]
            )
            .unwrap()
        );
        let content = std::fs::read_to_string(&path).unwrap();
        assert!(content.contains("# user"));
        assert!(content.contains("/build"));
        assert!(content.contains(MANAGED_BEGIN));
        assert!(content.contains("/.agents/skills/a"));
        assert!(content.contains(MANAGED_END));

        // Reading back the block yields exactly the managed lines.
        assert_eq!(
            managed_lines(&LocalFs, &path).unwrap(),
            vec![
                "/.agents/skills/a".to_string(),
                "/.akit/kit.lock.json".to_string()
            ]
        );

        // Idempotent when unchanged.
        assert!(
            !set_managed_lines(
                &LocalFs,
                &path,
                &["/.agents/skills/a".into(), "/.akit/kit.lock.json".into()]
            )
            .unwrap()
        );
    }

    #[test]
    fn managed_block_prunes_and_rewrites_without_touching_user_lines() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("exclude");
        set_managed_lines(
            &LocalFs,
            &path,
            &["/.agents/skills/a".into(), "/.claude/skills/b".into()],
        )
        .unwrap();
        std::fs::write(
            &path,
            format!("/mine\n{}", std::fs::read_to_string(&path).unwrap()),
        )
        .unwrap();

        // Shrink the managed set: the dropped line is pruned, user line stays.
        assert!(set_managed_lines(&LocalFs, &path, &["/.agents/skills/a".into()]).unwrap());
        let content = std::fs::read_to_string(&path).unwrap();
        assert!(content.contains("/mine"));
        assert!(content.contains("/.agents/skills/a"));
        assert!(!content.contains("/.claude/skills/b"));
    }

    #[test]
    fn empty_managed_set_removes_the_block_only() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("exclude");
        std::fs::write(&path, "/keep\n").unwrap();
        set_managed_lines(&LocalFs, &path, &["/.agents/skills/a".into()]).unwrap();
        assert!(set_managed_lines(&LocalFs, &path, &[]).unwrap());
        let content = std::fs::read_to_string(&path).unwrap();
        assert!(content.contains("/keep"));
        assert!(!content.contains(MANAGED_BEGIN));
        assert!(managed_lines(&LocalFs, &path).unwrap().is_empty());
        // No block + empty desired = no-op.
        assert!(!set_managed_lines(&LocalFs, &path, &[]).unwrap());
    }
}
