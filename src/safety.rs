//! Guards that keep sweepr from touching what it must not.

use anyhow::{Context, Result, bail};
use rustix::fs::{FlockOperation, flock};
use std::collections::HashSet;
use std::fs::{self, File, OpenOptions};
use std::path::{Path, PathBuf};

/// sweepr cleans one user's home. As root it could reach every user's files.
pub fn ensure_not_root() -> Result<()> {
    if rustix::process::geteuid().is_root() {
        bail!("sweepr refuses to run as root; run it as your normal user");
    }
    Ok(())
}

/// Every path currently held open by a process this user can inspect.
///
/// Moving a file a program has open would not break that program right away,
/// but whatever it writes next would land in quarantine and be lost.
pub fn open_files() -> HashSet<PathBuf> {
    let Ok(procs) = fs::read_dir("/proc") else {
        return HashSet::new();
    };
    procs
        .flatten()
        .filter(|e| {
            e.file_name()
                .to_string_lossy()
                .bytes()
                .all(|b| b.is_ascii_digit())
        })
        .filter_map(|e| fs::read_dir(e.path().join("fd")).ok())
        .flat_map(|fds| fds.flatten())
        .filter_map(|fd| fs::read_link(fd.path()).ok())
        .filter(|target| target.is_absolute())
        .collect()
}

/// True if any open file is `path` itself or lives inside it.
pub fn is_open(path: &Path, open: &HashSet<PathBuf>) -> bool {
    open.contains(path) || open.iter().any(|o| o.starts_with(path))
}

/// An exclusive lock held for the whole run, so two sweepr runs never race.
pub struct RunLock {
    _file: File,
}

impl RunLock {
    /// Take the lock or fail at once if another sweepr holds it.
    pub fn acquire(state_dir: &Path) -> Result<Self> {
        Self::take(state_dir, FlockOperation::NonBlockingLockExclusive)
    }

    /// Wait for the lock; used after a notification click, when a brief
    /// hourly tick may be holding it.
    pub fn acquire_waiting(state_dir: &Path) -> Result<Self> {
        Self::take(state_dir, FlockOperation::LockExclusive)
    }

    fn take(state_dir: &Path, op: FlockOperation) -> Result<Self> {
        fs::create_dir_all(state_dir)
            .with_context(|| format!("creating {}", state_dir.display()))?;
        let path = state_dir.join("lock");
        let file = OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .open(&path)
            .with_context(|| format!("opening {}", path.display()))?;
        flock(&file, op).map_err(|_| anyhow::anyhow!("another sweepr run is in progress"))?;
        Ok(Self { _file: file })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sees_a_file_this_process_holds_open() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("held.txt");
        let _handle = File::create(&path).unwrap();
        let open = open_files();
        assert!(is_open(&path, &open));
        assert!(
            is_open(dir.path(), &open),
            "a dir with an open file inside counts as open"
        );
        assert!(!is_open(&dir.path().join("other"), &open));
    }

    #[test]
    fn second_lock_fails_while_first_is_held() {
        let dir = tempfile::tempdir().unwrap();
        let first = RunLock::acquire(dir.path()).unwrap();
        assert!(RunLock::acquire(dir.path()).is_err());
        drop(first);
        assert!(RunLock::acquire(dir.path()).is_ok());
    }
}
