//! The materialization engine (issue #31).
//!
//! Executes a [`crate::plan::Plan`] over an [`crate::transport::FsTransport`]:
//! it copies (or, when verified and supported, symlinks) each planned skill
//! directory / agent file, records a stable content hash for drift detection,
//! reconciles user edits, and cleans up managed files without ever touching
//! content akit does not own.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::lockfile::Mode;
use crate::ownership::MaterializationRecord;
use crate::plan::PlannedMaterialization;
use crate::transport::{FileKind, FsTransport, copy_tree, walk_files};

/// A planned materialization paired with its resolved absolute source (a skill
/// directory or an agent variant file). The caller (ops/CLI, #34) supplies the
/// source; the engine stays decoupled from catalog layout.
#[derive(Debug, Clone)]
pub struct MaterializeItem<'a> {
    /// Absolute source path (skill dir, or agent variant file).
    pub source: PathBuf,
    /// The plan entry describing the destination, mode, and coverage.
    pub planned: &'a PlannedMaterialization,
}

/// The drift status of a materialized file relative to what akit recorded.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Drift {
    /// On-disk content matches the recorded hash (or a symlink is intact).
    Clean,
    /// The materialization is gone.
    Missing,
    /// A copy was edited after materialization (on-disk hash != recorded hash).
    Modified,
}

/// Hash raw bytes to a lowercase hex sha256 string.
pub fn hash_bytes(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    hex(&hasher.finalize())
}

/// A stable content hash of the file or directory tree at `path`.
///
/// For a directory, entry names and their contents are folded in sorted order so
/// the hash is deterministic across machines and runs (unlike `DefaultHasher`).
pub fn content_hash(fs: &dyn FsTransport, path: &Path) -> Result<String> {
    let mut hasher = Sha256::new();
    match fs.symlink_kind(path)? {
        Some(FileKind::Dir) => {
            let root = path;
            for file in walk_files(fs, root)? {
                let rel = file
                    .strip_prefix(root)
                    .unwrap_or(&file)
                    .to_string_lossy()
                    .replace('\\', "/");
                hasher.update(rel.as_bytes());
                hasher.update([0u8]);
                let bytes = fs.read(&file)?;
                hasher.update((bytes.len() as u64).to_le_bytes());
                hasher.update(&bytes);
            }
        }
        Some(FileKind::File | FileKind::Symlink) => {
            let bytes = fs.read(path)?;
            hasher.update(&bytes);
        }
        None => anyhow::bail!("cannot hash missing path {}", path.display()),
    }
    Ok(hex(&hasher.finalize()))
}

fn hex(bytes: &[u8]) -> String {
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        s.push_str(&format!("{b:02x}"));
    }
    s
}

/// Materialize one planned entry, returning its ownership record.
///
/// Content is first staged at a sibling temp path, hash-validated, then
/// **atomically renamed** onto the destination so the destination is never
/// observed half-written (see [`stage_one`]/[`commit_one`]). For groups, prefer
/// [`materialize_all`], which stages every entry before committing any.
///
/// The effective mode downgrades to [`Mode::Copy`] whenever the transport does
/// not support symlinks (remote) or the plan requested a copy. Symlinked
/// materializations record no hash (they reflect the source directly); copies
/// record the source content hash for later drift detection.
///
/// `project_root` is used to store the record path project-relative.
pub fn materialize_one(
    fs: &dyn FsTransport,
    project_root: &Path,
    item: &MaterializeItem<'_>,
) -> Result<MaterializationRecord> {
    let staged = stage_one(fs, project_root, item)?;
    let record = staged.record.clone();
    commit_one(fs, &staged).with_context(|| format!("committing {}", record.path))?;
    Ok(record)
}

/// Transactionally materialize a whole plan: **stage** every entry to a sibling
/// temp path (validating hashes) before **committing** any via atomic rename.
///
/// If staging any entry fails, previously staged temps are cleaned up and no
/// destination is touched — the caller's prior destinations and lockfile stay
/// intact. Commit renames are per-destination atomic; a failure mid-commit is
/// surfaced with the offending path so the caller can report recovery state.
pub fn materialize_all(
    fs: &dyn FsTransport,
    project_root: &Path,
    items: &[MaterializeItem<'_>],
) -> Result<Vec<MaterializationRecord>> {
    // Phase 1 — stage all to temps; on any failure, roll back staged temps.
    let mut staged = Vec::with_capacity(items.len());
    for item in items {
        match stage_one(fs, project_root, item) {
            Ok(s) => staged.push(s),
            Err(e) => {
                for done in &staged {
                    let _ = clear_destination(fs, &done.temp_abs);
                }
                return Err(e);
            }
        }
    }

    // Phase 2 — commit each staged temp via atomic rename.
    let mut records = Vec::with_capacity(staged.len());
    for s in &staged {
        commit_one(fs, s).with_context(|| format!("committing {}", s.record.path))?;
        records.push(s.record.clone());
    }
    Ok(records)
}

/// A materialization staged at `temp_abs`, ready to be atomically renamed onto
/// `final_abs`. Holds the ownership record that describes the committed result.
struct Staged {
    temp_abs: PathBuf,
    final_abs: PathBuf,
    record: MaterializationRecord,
}

/// The sibling temp path used to stage `final_abs` (same parent directory, hence
/// same filesystem, so the commit rename is atomic).
fn temp_sibling(final_abs: &Path) -> PathBuf {
    let name = final_abs
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| "item".to_string());
    final_abs.with_file_name(format!(".{name}.akit-stage"))
}

/// Stage one item to its sibling temp path without mutating the destination.
/// Copies are hash-validated against their source so a truncated or corrupt
/// (e.g. remote) write is caught before it can be committed.
fn stage_one(
    fs: &dyn FsTransport,
    project_root: &Path,
    item: &MaterializeItem<'_>,
) -> Result<Staged> {
    let final_abs = project_root.join(&item.planned.path);
    let temp_abs = temp_sibling(&final_abs);
    // Remove any leftover temp from a prior aborted run before staging.
    clear_destination(fs, &temp_abs)?;

    let use_symlink = item.planned.mode == Mode::Symlink && fs.supports_symlink();
    let (mode, hash) = if use_symlink {
        fs.symlink(&item.source, &temp_abs)?;
        (Mode::Symlink, None)
    } else {
        copy_tree(fs, &item.source, &temp_abs)
            .with_context(|| format!("staging {}", item.planned.path))?;
        let staged_hash = content_hash(fs, &temp_abs)?;
        let source_hash = content_hash(fs, &item.source)?;
        if staged_hash != source_hash {
            let _ = clear_destination(fs, &temp_abs);
            anyhow::bail!(
                "staged copy of {} does not match its source (hash mismatch)",
                item.planned.path
            );
        }
        (Mode::Copy, Some(staged_hash))
    };

    let mut covers = item.planned.covers.clone();
    covers.sort();
    Ok(Staged {
        temp_abs,
        final_abs,
        record: MaterializationRecord {
            path: item.planned.path.clone(),
            mode,
            covers,
            hash,
        },
    })
}

/// Commit a staged materialization: clear the destination, then atomically move
/// the temp into place.
fn commit_one(fs: &dyn FsTransport, staged: &Staged) -> Result<()> {
    clear_destination(fs, &staged.final_abs)?;
    fs.rename(&staged.temp_abs, &staged.final_abs)
}

/// Determine whether a recorded materialization has drifted on disk.
pub fn check_drift(
    fs: &dyn FsTransport,
    project_root: &Path,
    record: &MaterializationRecord,
) -> Result<Drift> {
    let abs = project_root.join(&record.path);
    if fs.symlink_kind(&abs)?.is_none() {
        return Ok(Drift::Missing);
    }
    match (&record.mode, &record.hash) {
        // Symlinks reflect the source; presence is sufficient.
        (Mode::Symlink, _) => Ok(Drift::Clean),
        (Mode::Copy, Some(recorded)) => {
            let current = content_hash(fs, &abs)?;
            if &current == recorded {
                Ok(Drift::Clean)
            } else {
                Ok(Drift::Modified)
            }
        }
        // A copy without a recorded hash cannot be verified; treat as clean.
        (Mode::Copy, None) => Ok(Drift::Clean),
    }
}

/// Remove a materialization and prune now-empty managed ancestor directories,
/// never removing directories that still hold other content.
///
/// Returns `true` if the destination existed and was removed.
pub fn remove_materialization(
    fs: &dyn FsTransport,
    project_root: &Path,
    rel_path: &str,
) -> Result<bool> {
    let abs = project_root.join(rel_path);
    let removed = match fs.symlink_kind(&abs)? {
        Some(FileKind::Dir) => {
            fs.remove_dir_all(&abs)?;
            true
        }
        Some(FileKind::File | FileKind::Symlink) => {
            fs.remove_file(&abs)?;
            true
        }
        None => false,
    };
    prune_empty_ancestors(fs, project_root, rel_path)?;
    Ok(removed)
}

/// Walk upward from `rel_path`, removing each empty ancestor directory until a
/// non-empty directory or the project root is reached. Because only *empty*
/// directories are removed, user content is never destroyed — this is the safe
/// approximation of "remove only directories akit created".
fn prune_empty_ancestors(fs: &dyn FsTransport, project_root: &Path, rel_path: &str) -> Result<()> {
    let mut current = Path::new(rel_path).parent().map(Path::to_path_buf);
    while let Some(rel) = current {
        if rel.as_os_str().is_empty() {
            break;
        }
        let abs = project_root.join(&rel);
        if fs.dir_is_empty(&abs)? {
            fs.remove_dir_all(&abs)?;
            current = rel.parent().map(Path::to_path_buf);
        } else {
            break;
        }
    }
    Ok(())
}

/// Remove any managed destination at `abs` (symlink, file, or directory) so a
/// re-materialization starts clean. A path that does not exist is a no-op.
fn clear_destination(fs: &dyn FsTransport, abs: &Path) -> Result<()> {
    match fs.symlink_kind(abs)? {
        Some(FileKind::Dir) => fs.remove_dir_all(abs),
        Some(FileKind::File | FileKind::Symlink) => fs.remove_file(abs),
        None => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::plan::{MatKind, PlannedMaterialization};
    use crate::transport::LocalFs;
    use tempfile::TempDir;

    fn planned(path: &str, mode: Mode, kind: MatKind) -> PlannedMaterialization {
        PlannedMaterialization {
            path: path.to_string(),
            mode,
            covers: vec![crate::harness::HarnessId::Copilot],
            kind,
            source_file: None,
        }
    }

    #[test]
    fn hash_is_stable_and_content_sensitive() {
        assert_eq!(hash_bytes(b"abc"), hash_bytes(b"abc"));
        assert_ne!(hash_bytes(b"abc"), hash_bytes(b"abd"));
        // Known sha256("abc").
        assert_eq!(
            hash_bytes(b"abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }

    #[test]
    fn directory_hash_changes_when_a_file_changes() {
        let tmp = TempDir::new().unwrap();
        let fs = LocalFs;
        let dir = tmp.path().join("skill");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("SKILL.md"), "hello").unwrap();
        let h1 = content_hash(&fs, &dir).unwrap();
        std::fs::write(dir.join("SKILL.md"), "changed").unwrap();
        let h2 = content_hash(&fs, &dir).unwrap();
        assert_ne!(h1, h2);
    }

    #[test]
    fn copy_materialization_records_source_hash_and_detects_edits() {
        let tmp = TempDir::new().unwrap();
        let fs = LocalFs;
        let root = tmp.path().join("project");
        let src = tmp.path().join("catalog/skills/deploy");
        std::fs::create_dir_all(&src).unwrap();
        std::fs::write(src.join("SKILL.md"), "body").unwrap();

        let p = planned(".agents/skills/deploy", Mode::Copy, MatKind::SkillDir);
        let item = MaterializeItem {
            source: src.clone(),
            planned: &p,
        };
        let record = materialize_one(&fs, &root, &item).unwrap();
        assert_eq!(record.mode, Mode::Copy);
        assert!(record.hash.is_some());
        assert!(root.join(".agents/skills/deploy/SKILL.md").is_file());
        assert_eq!(check_drift(&fs, &root, &record).unwrap(), Drift::Clean);

        // A user edit is detected as drift.
        std::fs::write(root.join(".agents/skills/deploy/SKILL.md"), "edited").unwrap();
        assert_eq!(check_drift(&fs, &root, &record).unwrap(), Drift::Modified);

        // Removal is detected as missing.
        std::fs::remove_dir_all(root.join(".agents/skills/deploy")).unwrap();
        assert_eq!(check_drift(&fs, &root, &record).unwrap(), Drift::Missing);
    }

    #[test]
    fn agent_file_materializes_and_reconciles() {
        let tmp = TempDir::new().unwrap();
        let fs = LocalFs;
        let root = tmp.path().join("project");
        let src = tmp.path().join("catalog/agents/reviewer/claude.md");
        std::fs::create_dir_all(src.parent().unwrap()).unwrap();
        std::fs::write(&src, "---\nname: reviewer\n---\nbody").unwrap();

        let p = planned(".claude/agents/reviewer.md", Mode::Copy, MatKind::AgentFile);
        let item = MaterializeItem {
            source: src,
            planned: &p,
        };
        let record = materialize_one(&fs, &root, &item).unwrap();
        assert!(root.join(".claude/agents/reviewer.md").is_file());
        assert_eq!(check_drift(&fs, &root, &record).unwrap(), Drift::Clean);
    }

    #[test]
    fn symlink_materialization_records_no_hash() {
        let tmp = TempDir::new().unwrap();
        let fs = LocalFs;
        let root = tmp.path().join("project");
        let src = tmp.path().join("catalog/skills/deploy");
        std::fs::create_dir_all(&src).unwrap();
        std::fs::write(src.join("SKILL.md"), "body").unwrap();

        let p = planned(".agents/skills/deploy", Mode::Symlink, MatKind::SkillDir);
        let item = MaterializeItem {
            source: src,
            planned: &p,
        };
        let record = materialize_one(&fs, &root, &item).unwrap();
        assert_eq!(record.mode, Mode::Symlink);
        assert_eq!(record.hash, None);
        assert_eq!(check_drift(&fs, &root, &record).unwrap(), Drift::Clean);
    }

    #[test]
    fn remote_transport_downgrades_symlink_to_copy() {
        // A transport that reports no symlink support must force a copy even when
        // the plan asked for a symlink.
        struct NoSymlink(LocalFs);
        impl FsTransport for NoSymlink {
            fn exists(&self, p: &Path) -> Result<bool> {
                self.0.exists(p)
            }
            fn symlink_kind(&self, p: &Path) -> Result<Option<FileKind>> {
                self.0.symlink_kind(p)
            }
            fn read(&self, p: &Path) -> Result<Vec<u8>> {
                self.0.read(p)
            }
            fn read_dir(&self, p: &Path) -> Result<Vec<String>> {
                self.0.read_dir(p)
            }
            fn create_dir_all(&self, p: &Path) -> Result<()> {
                self.0.create_dir_all(p)
            }
            fn write(&self, p: &Path, b: &[u8]) -> Result<()> {
                self.0.write(p, b)
            }
            fn remove_file(&self, p: &Path) -> Result<()> {
                self.0.remove_file(p)
            }
            fn remove_dir_all(&self, p: &Path) -> Result<()> {
                self.0.remove_dir_all(p)
            }
            fn dir_is_empty(&self, p: &Path) -> Result<bool> {
                self.0.dir_is_empty(p)
            }
            fn symlink(&self, _t: &Path, _l: &Path) -> Result<()> {
                anyhow::bail!("remote transport cannot symlink")
            }
            fn rename(&self, from: &Path, to: &Path) -> Result<()> {
                self.0.rename(from, to)
            }
            fn supports_symlink(&self) -> bool {
                false
            }
        }

        let tmp = TempDir::new().unwrap();
        let fs = NoSymlink(LocalFs);
        let root = tmp.path().join("project");
        let src = tmp.path().join("catalog/skills/deploy");
        std::fs::create_dir_all(&src).unwrap();
        std::fs::write(src.join("SKILL.md"), "body").unwrap();

        let p = planned(".agents/skills/deploy", Mode::Symlink, MatKind::SkillDir);
        let item = MaterializeItem {
            source: src,
            planned: &p,
        };
        let record = materialize_one(&fs, &root, &item).unwrap();
        assert_eq!(record.mode, Mode::Copy);
        assert!(record.hash.is_some());
        assert!(root.join(".agents/skills/deploy/SKILL.md").is_file());
    }

    #[test]
    fn remove_prunes_empty_managed_dirs_but_keeps_populated_ones() {
        let tmp = TempDir::new().unwrap();
        let fs = LocalFs;
        let root = tmp.path().join("project");
        // Two skills under .agents/skills.
        for name in ["deploy", "lint"] {
            let src = tmp.path().join("catalog/skills").join(name);
            std::fs::create_dir_all(&src).unwrap();
            std::fs::write(src.join("SKILL.md"), "body").unwrap();
            let path = format!(".agents/skills/{name}");
            let p = planned(&path, Mode::Copy, MatKind::SkillDir);
            materialize_one(
                &fs,
                &root,
                &MaterializeItem {
                    source: src,
                    planned: &p,
                },
            )
            .unwrap();
        }

        // Removing one leaves the shared parent (still holds the other skill).
        assert!(remove_materialization(&fs, &root, ".agents/skills/deploy").unwrap());
        assert!(root.join(".agents/skills").is_dir());
        assert!(root.join(".agents/skills/lint").is_dir());

        // Removing the last one prunes the now-empty `.agents/skills` and `.agents`.
        assert!(remove_materialization(&fs, &root, ".agents/skills/lint").unwrap());
        assert!(!root.join(".agents/skills").exists());
        assert!(!root.join(".agents").exists());
    }

    #[test]
    fn remove_missing_is_noop() {
        let tmp = TempDir::new().unwrap();
        let fs = LocalFs;
        let root = tmp.path().join("project");
        assert!(!remove_materialization(&fs, &root, ".agents/skills/gone").unwrap());
    }

    #[test]
    fn rematerialize_replaces_prior_content() {
        let tmp = TempDir::new().unwrap();
        let fs = LocalFs;
        let root = tmp.path().join("project");
        let src = tmp.path().join("catalog/skills/deploy");
        std::fs::create_dir_all(&src).unwrap();
        std::fs::write(src.join("SKILL.md"), "v1").unwrap();
        std::fs::write(src.join("extra.txt"), "keep").unwrap();

        let p = planned(".agents/skills/deploy", Mode::Copy, MatKind::SkillDir);
        let item = MaterializeItem {
            source: src.clone(),
            planned: &p,
        };
        materialize_one(&fs, &root, &item).unwrap();

        // Source loses a file; re-materialize must not leave the stale file behind.
        std::fs::remove_file(src.join("extra.txt")).unwrap();
        std::fs::write(src.join("SKILL.md"), "v2").unwrap();
        materialize_one(&fs, &root, &item).unwrap();

        assert!(!root.join(".agents/skills/deploy/extra.txt").exists());
        let body = std::fs::read_to_string(root.join(".agents/skills/deploy/SKILL.md")).unwrap();
        assert_eq!(body, "v2");
    }

    #[test]
    fn materialize_all_commits_and_leaves_no_stage_temps() {
        let tmp = TempDir::new().unwrap();
        let fs = LocalFs;
        let root = tmp.path().join("project");
        for id in ["a", "b"] {
            let src = tmp.path().join(format!("catalog/skills/{id}"));
            std::fs::create_dir_all(&src).unwrap();
            std::fs::write(src.join("SKILL.md"), id).unwrap();
        }
        let pa = planned(".agents/skills/a", Mode::Copy, MatKind::SkillDir);
        let pb = planned(".agents/skills/b", Mode::Copy, MatKind::SkillDir);
        let items = vec![
            MaterializeItem {
                source: tmp.path().join("catalog/skills/a"),
                planned: &pa,
            },
            MaterializeItem {
                source: tmp.path().join("catalog/skills/b"),
                planned: &pb,
            },
        ];

        let records = materialize_all(&fs, &root, &items).unwrap();
        assert_eq!(records.len(), 2);
        assert!(root.join(".agents/skills/a/SKILL.md").exists());
        assert!(root.join(".agents/skills/b/SKILL.md").exists());
        // No staging siblings survive a successful commit.
        assert!(!root.join(".agents/skills/.a.akit-stage").exists());
        assert!(!root.join(".agents/skills/.b.akit-stage").exists());
    }

    #[test]
    fn materialize_all_rolls_back_when_an_entry_fails_to_stage() {
        let tmp = TempDir::new().unwrap();
        let fs = LocalFs;
        let root = tmp.path().join("project");
        let good = tmp.path().join("catalog/skills/a");
        std::fs::create_dir_all(&good).unwrap();
        std::fs::write(good.join("SKILL.md"), "a").unwrap();
        // Second source does not exist, so staging it fails.
        let missing = tmp.path().join("catalog/skills/missing");

        let pa = planned(".agents/skills/a", Mode::Copy, MatKind::SkillDir);
        let pm = planned(".agents/skills/missing", Mode::Copy, MatKind::SkillDir);
        let items = vec![
            MaterializeItem {
                source: good,
                planned: &pa,
            },
            MaterializeItem {
                source: missing,
                planned: &pm,
            },
        ];

        assert!(materialize_all(&fs, &root, &items).is_err());
        // Nothing committed and no staging temps left behind (previous state intact).
        assert!(!root.join(".agents/skills/a").exists());
        assert!(!root.join(".agents/skills/missing").exists());
        assert!(!root.join(".agents/skills/.a.akit-stage").exists());
    }
}
