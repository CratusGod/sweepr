//! Moving files aside and back. sweepr only ever renames; it never copies,
//! so a move is instant and cannot run out of space halfway.

use crate::scan::QUARANTINE_DIR;
use anyhow::{Context, Result};
use rustix::fs::{CWD, RenameFlags, renameat_with};
use rustix::io::Errno;
use std::fs;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};

#[derive(Debug, PartialEq, Eq)]
pub enum MoveOutcome {
    Moved,
    /// Source and destination are on different filesystems; nothing was moved.
    CrossDevice,
    /// The source vanished between scan and move.
    Gone,
}

#[derive(Debug, PartialEq, Eq)]
pub enum RestoreOutcome {
    Restored,
    /// Not in quarantine (already restored, or the run crashed before moving it).
    NotQuarantined,
    /// Something new now sits at the original path; left alone.
    Occupied,
}

/// Where a rule's items go. The shared quarantine is used when it is on the
/// same filesystem as the rule root; otherwise (separate partition, btrfs
/// subvolume) a quarantine inside the rule root, so the move stays a rename.
pub fn quarantine_root(rule_root: &Path, shared: &Path) -> Result<PathBuf> {
    fs::create_dir_all(shared).with_context(|| format!("creating {}", shared.display()))?;
    let dev = |p: &Path| fs::metadata(p).map(|m| m.dev());
    let root_dev = dev(rule_root).with_context(|| format!("reading {}", rule_root.display()))?;
    if root_dev == dev(shared)? {
        Ok(shared.to_path_buf())
    } else {
        Ok(rule_root.join(QUARANTINE_DIR))
    }
}

/// Destination for `path` inside a run: `<run_dir>/<rule>/<path relative to root>`.
pub fn destination(run_dir: &Path, rule: &str, rule_root: &Path, path: &Path) -> Result<PathBuf> {
    let rel = path
        .strip_prefix(rule_root)
        .with_context(|| format!("{} is outside {}", path.display(), rule_root.display()))?;
    Ok(run_dir.join(rule).join(rel))
}

/// Rename without ever replacing an existing file.
pub fn move_noreplace(src: &Path, dst: &Path) -> Result<MoveOutcome> {
    if let Some(parent) = dst.parent() {
        fs::create_dir_all(parent).with_context(|| format!("creating {}", parent.display()))?;
    }
    match renameat_with(CWD, src, CWD, dst, RenameFlags::NOREPLACE) {
        Ok(()) => Ok(MoveOutcome::Moved),
        Err(Errno::XDEV) => Ok(MoveOutcome::CrossDevice),
        Err(Errno::NOENT) if fs::symlink_metadata(src).is_err() => Ok(MoveOutcome::Gone),
        Err(e) => Err(std::io::Error::from(e))
            .with_context(|| format!("moving {} to {}", src.display(), dst.display())),
    }
}

/// Remove directories left empty by a move, walking up to (not including) `stop_at`.
pub fn prune_empty_parents(moved: &Path, stop_at: &Path) {
    let mut dir = moved.parent();
    while let Some(d) = dir {
        if d == stop_at || !d.starts_with(stop_at) || fs::remove_dir(d).is_err() {
            break; // remove_dir fails on non-empty dirs, which is the stop signal
        }
        dir = d.parent();
    }
}

pub fn restore(original: &Path, quarantined: &Path) -> Result<RestoreOutcome> {
    if fs::symlink_metadata(quarantined).is_err() {
        return Ok(RestoreOutcome::NotQuarantined);
    }
    if fs::symlink_metadata(original).is_ok() {
        return Ok(RestoreOutcome::Occupied);
    }
    match move_noreplace(quarantined, original)? {
        MoveOutcome::Moved => Ok(RestoreOutcome::Restored),
        MoveOutcome::Gone => Ok(RestoreOutcome::NotQuarantined),
        MoveOutcome::CrossDevice => anyhow::bail!(
            "cannot restore {}: quarantine is now on another filesystem",
            original.display()
        ),
    }
}

/// Delete a run's quarantine directory for good.
pub fn purge_dir(run_dir: &Path) -> Result<()> {
    match fs::remove_dir_all(run_dir) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e).with_context(|| format!("deleting {}", run_dir.display())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn move_then_restore_round_trip() {
        let t = tempfile::tempdir().unwrap();
        let root = t.path().join("cache");
        let src = root.join("a/b/file");
        fs::create_dir_all(src.parent().unwrap()).unwrap();
        fs::write(&src, b"data").unwrap();
        let q = quarantine_root(&root, &t.path().join("q")).unwrap();
        let dst = destination(&q.join("run1"), "pip", &root, &src).unwrap();

        assert_eq!(move_noreplace(&src, &dst).unwrap(), MoveOutcome::Moved);
        prune_empty_parents(&src, &root);
        assert!(!root.join("a").exists(), "emptied dirs are pruned");
        assert!(root.exists(), "the rule root itself stays");

        assert_eq!(restore(&src, &dst).unwrap(), RestoreOutcome::Restored);
        assert_eq!(fs::read(&src).unwrap(), b"data");
        assert_eq!(restore(&src, &dst).unwrap(), RestoreOutcome::NotQuarantined);
    }

    #[test]
    fn never_overwrites() {
        let t = tempfile::tempdir().unwrap();
        let (a, b) = (t.path().join("a"), t.path().join("b"));
        fs::write(&a, b"old").unwrap();
        fs::write(&b, b"new").unwrap();
        assert!(move_noreplace(&a, &b).is_err());
        assert_eq!(fs::read(&b).unwrap(), b"new");
        assert_eq!(restore(&b, &a).unwrap(), RestoreOutcome::Occupied);
        assert_eq!(fs::read(&a).unwrap(), b"old");
    }

    #[test]
    fn vanished_source_is_reported_not_fatal() {
        let t = tempfile::tempdir().unwrap();
        let out = move_noreplace(&t.path().join("nope"), &t.path().join("q/nope")).unwrap();
        assert_eq!(out, MoveOutcome::Gone);
    }

    #[test]
    fn purge_is_idempotent() {
        let t = tempfile::tempdir().unwrap();
        let run = t.path().join("run");
        fs::create_dir_all(run.join("x")).unwrap();
        purge_dir(&run).unwrap();
        purge_dir(&run).unwrap();
        assert!(!run.exists());
    }
}
