//! The filesystem transport seam (issue #31).
//!
//! akit materializes and reconciles files through a small [`FsTransport`] trait
//! rather than calling `std::fs` directly. This lets the same materialization
//! engine run against the **local** filesystem here and against a **remote**
//! project over SFTP in an embedding host (madari, #62) without duplicating the
//! copy/hash/cleanup logic.
//!
//! The trait is deliberately minimal and copy-oriented: remote transports set
//! [`FsTransport::supports_symlink`] to `false`, which forces the engine to copy
//! (matching the approved remote = copy-only policy).

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

/// The kind of a filesystem entry, as reported without following symlinks.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileKind {
    File,
    Dir,
    Symlink,
}

/// The filesystem operations akit needs to materialize, hash, and clean up
/// managed files. Paths are absolute (already joined against the project root).
pub trait FsTransport {
    /// Whether a path exists (following symlinks for the existence check).
    fn exists(&self, path: &Path) -> Result<bool>;

    /// The entry kind at `path` without following symlinks, or `None` if absent.
    fn symlink_kind(&self, path: &Path) -> Result<Option<FileKind>>;

    /// Read a file's bytes.
    fn read(&self, path: &Path) -> Result<Vec<u8>>;

    /// Sorted entry names (not paths) directly under `dir`.
    fn read_dir(&self, dir: &Path) -> Result<Vec<String>>;

    /// Create `path` and all missing parents.
    fn create_dir_all(&self, path: &Path) -> Result<()>;

    /// Write `bytes` to `path`, creating parents as needed.
    fn write(&self, path: &Path, bytes: &[u8]) -> Result<()>;

    /// Remove a single file or symlink. Idempotent (missing = `Ok`).
    fn remove_file(&self, path: &Path) -> Result<()>;

    /// Remove a directory and its contents. Idempotent (missing = `Ok`).
    fn remove_dir_all(&self, path: &Path) -> Result<()>;

    /// Whether this directory has no entries. `false` if it does not exist.
    fn dir_is_empty(&self, path: &Path) -> Result<bool>;

    /// Create a symlink at `link` pointing to `target`. Only called when
    /// [`FsTransport::supports_symlink`] is `true`.
    fn symlink(&self, target: &Path, link: &Path) -> Result<()>;

    /// Atomically move `from` onto `to` on the same filesystem, replacing any
    /// existing `to`. Callers stage content at a sibling temp `from` and rename
    /// it into place so a destination is never observed half-written; the temp
    /// therefore always shares `to`'s parent directory (and filesystem). Remote
    /// transports (SFTP) must provide a genuine atomic rename.
    fn rename(&self, from: &Path, to: &Path) -> Result<()>;

    /// Whether symlink materialization is available on this transport. Remote
    /// transports return `false`, forcing copies.
    fn supports_symlink(&self) -> bool;
}

/// The local-filesystem transport backed by `std::fs`.
#[derive(Debug, Default, Clone, Copy)]
pub struct LocalFs;

impl FsTransport for LocalFs {
    fn exists(&self, path: &Path) -> Result<bool> {
        Ok(path.exists())
    }

    fn symlink_kind(&self, path: &Path) -> Result<Option<FileKind>> {
        match std::fs::symlink_metadata(path) {
            Ok(meta) => {
                let ft = meta.file_type();
                let kind = if ft.is_symlink() {
                    FileKind::Symlink
                } else if ft.is_dir() {
                    FileKind::Dir
                } else {
                    FileKind::File
                };
                Ok(Some(kind))
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(e).with_context(|| format!("stat {}", path.display())),
        }
    }

    fn read(&self, path: &Path) -> Result<Vec<u8>> {
        std::fs::read(path).with_context(|| format!("reading {}", path.display()))
    }

    fn read_dir(&self, dir: &Path) -> Result<Vec<String>> {
        let mut names = Vec::new();
        for entry in std::fs::read_dir(dir).with_context(|| format!("reading {}", dir.display()))? {
            let entry = entry.with_context(|| format!("reading entry in {}", dir.display()))?;
            names.push(entry.file_name().to_string_lossy().into_owned());
        }
        names.sort();
        Ok(names)
    }

    fn create_dir_all(&self, path: &Path) -> Result<()> {
        std::fs::create_dir_all(path).with_context(|| format!("creating {}", path.display()))
    }

    fn write(&self, path: &Path, bytes: &[u8]) -> Result<()> {
        if let Some(parent) = path.parent() {
            self.create_dir_all(parent)?;
        }
        std::fs::write(path, bytes).with_context(|| format!("writing {}", path.display()))
    }

    fn remove_file(&self, path: &Path) -> Result<()> {
        match std::fs::remove_file(path) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(e).with_context(|| format!("removing {}", path.display())),
        }
    }

    fn remove_dir_all(&self, path: &Path) -> Result<()> {
        match std::fs::remove_dir_all(path) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(e).with_context(|| format!("removing {}", path.display())),
        }
    }

    fn dir_is_empty(&self, path: &Path) -> Result<bool> {
        match std::fs::read_dir(path) {
            Ok(mut entries) => Ok(entries.next().is_none()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(false),
            Err(e) => Err(e).with_context(|| format!("reading {}", path.display())),
        }
    }

    fn symlink(&self, target: &Path, link: &Path) -> Result<()> {
        if let Some(parent) = link.parent() {
            self.create_dir_all(parent)?;
        }
        symlink_platform(target, link)
            .with_context(|| format!("symlinking {} -> {}", link.display(), target.display()))
    }

    fn rename(&self, from: &Path, to: &Path) -> Result<()> {
        if let Some(parent) = to.parent() {
            self.create_dir_all(parent)?;
        }
        std::fs::rename(from, to)
            .with_context(|| format!("renaming {} -> {}", from.display(), to.display()))
    }

    fn supports_symlink(&self) -> bool {
        true
    }
}

#[cfg(unix)]
fn symlink_platform(target: &Path, link: &Path) -> std::io::Result<()> {
    std::os::unix::fs::symlink(target, link)
}

#[cfg(windows)]
fn symlink_platform(target: &Path, link: &Path) -> std::io::Result<()> {
    if target.is_dir() {
        std::os::windows::fs::symlink_dir(target, link)
    } else {
        std::os::windows::fs::symlink_file(target, link)
    }
}

/// Recursively copy `src` (file or directory) to `dst` over `fs`. Parents of
/// `dst` are created. Existing `dst` content is assumed already cleared by the
/// caller.
pub fn copy_tree(fs: &dyn FsTransport, src: &Path, dst: &Path) -> Result<()> {
    match fs.symlink_kind(src)? {
        Some(FileKind::Dir) => {
            fs.create_dir_all(dst)?;
            for name in fs.read_dir(src)? {
                copy_tree(fs, &src.join(&name), &dst.join(&name))?;
            }
            Ok(())
        }
        Some(FileKind::File | FileKind::Symlink) => {
            let bytes = fs.read(src)?;
            fs.write(dst, &bytes)
        }
        None => anyhow::bail!("source {} does not exist", src.display()),
    }
}

/// Collect the project-relative-sorted absolute paths under `root`, used by the
/// hasher and tests. Directories are descended; symlinks/files are leaves.
pub(crate) fn walk_files(fs: &dyn FsTransport, root: &Path) -> Result<Vec<PathBuf>> {
    let mut out = Vec::new();
    match fs.symlink_kind(root)? {
        Some(FileKind::Dir) => {
            for name in fs.read_dir(root)? {
                out.extend(walk_files(fs, &root.join(&name))?);
            }
        }
        Some(_) => out.push(root.to_path_buf()),
        None => {}
    }
    Ok(out)
}
