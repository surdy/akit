//! Filesystem materialization helpers: symlink (default) and copy (fallback).

use crate::lockfile::Mode;
use anyhow::{Context, Result, bail};
use std::collections::BTreeSet;
use std::io;
use std::path::{Path, PathBuf};

/// Result of a materialization attempt.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MaterializeOutcome {
    /// The link/copy was created during this call.
    Created,
    /// An equivalent link/copy was already present (idempotent no-op).
    AlreadyPresent,
}

/// Result of materialization with the effective mode that was actually used.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MaterializeReport {
    pub outcome: MaterializeOutcome,
    pub mode: Mode,
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

/// Materialize `src` at `dst`, falling back from symlink to copy if symlinking fails.
pub fn materialize_with_fallback(
    requested_mode: Mode,
    src: &Path,
    dst: &Path,
) -> Result<MaterializeReport> {
    match requested_mode {
        Mode::Copy => Ok(MaterializeReport {
            outcome: copy(src, dst)?,
            mode: Mode::Copy,
        }),
        Mode::Symlink => match symlink(src, dst) {
            Ok(outcome) => Ok(MaterializeReport {
                outcome,
                mode: Mode::Symlink,
            }),
            Err(err) => {
                eprintln!(
                    "warning: symlink failed for {} -> {}; falling back to copy: {err:#}",
                    dst.display(),
                    src.display()
                );
                Ok(MaterializeReport {
                    outcome: copy(src, dst).with_context(|| {
                        format!("copy fallback after symlink failure for {}", dst.display())
                    })?,
                    mode: Mode::Copy,
                })
            }
        },
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
            "{} already exists and is not a akit-managed symlink; refusing to overwrite",
            dst.display()
        );
    }

    create_symlink(src, dst)
        .with_context(|| format!("symlinking {} -> {}", dst.display(), src.display()))?;
    Ok(MaterializeOutcome::Created)
}

fn create_symlink(src: &Path, dst: &Path) -> std::io::Result<()> {
    if std::env::var_os("CKIT_TEST_FORCE_SYMLINK_FAILURE").is_some() {
        return Err(io::Error::other("forced symlink failure"));
    }
    create_symlink_platform(src, dst)
}

#[cfg(unix)]
fn create_symlink_platform(src: &Path, dst: &Path) -> std::io::Result<()> {
    std::os::unix::fs::symlink(src, dst)
}

#[cfg(windows)]
fn create_symlink_platform(src: &Path, dst: &Path) -> std::io::Result<()> {
    if src.is_dir() {
        std::os::windows::fs::symlink_dir(src, dst)
    } else {
        std::os::windows::fs::symlink_file(src, dst)
    }
}

/// Copy `src` to `dst`.
fn copy(src: &Path, dst: &Path) -> Result<MaterializeOutcome> {
    match std::fs::symlink_metadata(dst) {
        Ok(_) => {
            if drifted(src, dst)? {
                bail!(
                    "{} already exists and differs from {}; refusing to overwrite",
                    dst.display(),
                    src.display()
                );
            }
            return Ok(MaterializeOutcome::AlreadyPresent);
        }
        Err(e) if e.kind() == io::ErrorKind::NotFound => {}
        Err(e) => return Err(e).with_context(|| format!("reading {}", dst.display())),
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

/// Return `true` when the copied materialization differs from the collection source.
pub fn drifted(src: &Path, dst: &Path) -> Result<bool> {
    let src_meta = std::fs::metadata(src).with_context(|| format!("reading {}", src.display()))?;
    let dst_meta = std::fs::metadata(dst).with_context(|| format!("reading {}", dst.display()))?;

    if src_meta.is_file() && dst_meta.is_file() {
        return files_differ(src, dst);
    }
    if src_meta.is_dir() && dst_meta.is_dir() {
        return dirs_differ(src, dst);
    }
    Ok(true)
}

fn files_differ(src: &Path, dst: &Path) -> Result<bool> {
    let src_bytes = std::fs::read(src).with_context(|| format!("reading {}", src.display()))?;
    let dst_bytes = std::fs::read(dst).with_context(|| format!("reading {}", dst.display()))?;
    Ok(src_bytes != dst_bytes)
}

fn dirs_differ(src: &Path, dst: &Path) -> Result<bool> {
    let src_entries = entry_names(src)?;
    let dst_entries = entry_names(dst)?;
    if src_entries != dst_entries {
        return Ok(true);
    }
    for entry in src_entries {
        if drifted(&src.join(&entry), &dst.join(&entry))? {
            return Ok(true);
        }
    }
    Ok(false)
}

fn entry_names(dir: &Path) -> Result<BTreeSet<PathBuf>> {
    let mut entries = BTreeSet::new();
    for entry in std::fs::read_dir(dir).with_context(|| format!("reading {}", dir.display()))? {
        let entry = entry?;
        entries.insert(PathBuf::from(entry.file_name()));
    }
    Ok(entries)
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
