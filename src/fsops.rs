//! Filesystem materialization helpers: symlink (default) and copy (fallback).

use crate::lockfile::Mode;
use anyhow::{Context, Result, bail};
use std::path::Path;

/// Result of a materialization attempt.
#[derive(Debug, PartialEq, Eq)]
pub enum MaterializeOutcome {
    /// The link/copy was created during this call.
    Created,
    /// An equivalent link/copy was already present (idempotent no-op).
    AlreadyPresent,
}

impl MaterializeOutcome {
    pub fn created(&self) -> bool {
        matches!(self, MaterializeOutcome::Created)
    }
}

/// Materialize `src` at `dst` using the given mode, creating parent directories as needed.
pub fn materialize(mode: Mode, src: &Path, dst: &Path) -> Result<MaterializeOutcome> {
    match mode {
        Mode::Symlink => symlink(src, dst),
        Mode::Copy => copy(src, dst),
    }
}

fn symlink(src: &Path, dst: &Path) -> Result<MaterializeOutcome> {
    if let Some(parent) = dst.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("creating {}", parent.display()))?;
    }

    if let Ok(meta) = std::fs::symlink_metadata(dst) {
        if meta.file_type().is_symlink() {
            // Idempotent if the existing symlink resolves to the same source.
            if let (Ok(a), Ok(b)) = (dst.canonicalize(), src.canonicalize())
                && a == b
            {
                return Ok(MaterializeOutcome::AlreadyPresent);
            }
            let cur = std::fs::read_link(dst).unwrap_or_default();
            bail!(
                "{} already exists as a symlink to a different target ({}); refusing to overwrite",
                dst.display(),
                cur.display()
            );
        }
        bail!(
            "{} already exists and is not a ckit-managed symlink; refusing to overwrite",
            dst.display()
        );
    }

    create_symlink(src, dst)
        .with_context(|| format!("symlinking {} -> {}", dst.display(), src.display()))?;
    Ok(MaterializeOutcome::Created)
}

#[cfg(unix)]
fn create_symlink(src: &Path, dst: &Path) -> std::io::Result<()> {
    std::os::unix::fs::symlink(src, dst)
}

#[cfg(windows)]
fn create_symlink(src: &Path, dst: &Path) -> std::io::Result<()> {
    if src.is_dir() {
        std::os::windows::fs::symlink_dir(src, dst)
    } else {
        std::os::windows::fs::symlink_file(src, dst)
    }
}

/// Copy `src` to `dst`. Basic implementation to freeze the contract; drift handling and the
/// Windows auto-fallback are refined in issue #5.
fn copy(src: &Path, dst: &Path) -> Result<MaterializeOutcome> {
    if dst.exists() {
        return Ok(MaterializeOutcome::AlreadyPresent);
    }
    if let Some(parent) = dst.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("creating {}", parent.display()))?;
    }
    if src.is_dir() {
        copy_dir_all(src, dst)?;
    } else {
        std::fs::copy(src, dst)
            .with_context(|| format!("copying {} -> {}", src.display(), dst.display()))?;
    }
    Ok(MaterializeOutcome::Created)
}

fn copy_dir_all(src: &Path, dst: &Path) -> Result<()> {
    std::fs::create_dir_all(dst).with_context(|| format!("creating {}", dst.display()))?;
    for entry in std::fs::read_dir(src).with_context(|| format!("reading {}", src.display()))? {
        let entry = entry?;
        let from = entry.path();
        let to = dst.join(entry.file_name());
        if entry.file_type()?.is_dir() {
            copy_dir_all(&from, &to)?;
        } else {
            std::fs::copy(&from, &to)
                .with_context(|| format!("copying {} -> {}", from.display(), to.display()))?;
        }
    }
    Ok(())
}

/// Remove a materialized target (symlink or copied dir/file). Returns `true` if something
/// was removed. Idempotent: a missing target is a no-op.
pub fn remove(dst: &Path) -> Result<bool> {
    let meta = match std::fs::symlink_metadata(dst) {
        Ok(m) => m,
        Err(_) => return Ok(false),
    };
    if meta.file_type().is_symlink() || meta.is_file() {
        std::fs::remove_file(dst).with_context(|| format!("removing {}", dst.display()))?;
    } else {
        std::fs::remove_dir_all(dst).with_context(|| format!("removing {}", dst.display()))?;
    }
    Ok(true)
}
