//! Shared fixtures: a throwaway home directory and a clock far in the future.
//!
//! sweepr ages files by the newer of mtime and ctime, and ctime cannot be set
//! from userspace. So tests scan from a clock far in the future (`later()`)
//! and give each fixture an mtime relative to that clock.
#![allow(dead_code)] // each test binary uses a different subset

use jiff::{SignedDuration, Timestamp};
use std::collections::HashSet;
use std::fs::{self, File};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};
use sweepr::app;
use sweepr::config::Config;
use sweepr::paths::Paths;

/// Command rules (`uv cache prune`, ...) act on the real user's caches,
/// whatever home a test uses, so tests that run them switch them off.
pub const NO_COMMANDS: &str = "[rules.uv]\nenabled = false\n[rules.pnpm]\nenabled = false\n";

pub const DAY: u64 = 86_400;
/// The clock the tests scan from: 1000 days from now.
pub const AHEAD_DAYS: u64 = 1000;

pub fn later() -> Timestamp {
    Timestamp::now() + SignedDuration::from_hours(AHEAD_DAYS as i64 * 24)
}

pub struct Home {
    pub dir: tempfile::TempDir,
    pub paths: Paths,
}

impl Home {
    pub fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let paths = Paths::under(&dir.path().canonicalize().unwrap());
        Self { dir, paths }
    }

    pub fn config(&self, toml: &str) -> Config {
        Config::from_toml(&self.paths, toml).unwrap()
    }

    /// A file that looks `days_old` days old to a scan run at `later()`.
    pub fn file(&self, path: &Path, days_old: u64) -> PathBuf {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, path.to_string_lossy().as_bytes()).unwrap();
        age(path, days_old);
        path.to_path_buf()
    }

    pub fn trash(
        &self,
        name: &str,
        file_days_old: u64,
        trashed_days_ago: u64,
    ) -> (PathBuf, PathBuf) {
        let root = self.paths.data.join("Trash");
        let item = self.file(&root.join("files").join(name), file_days_old);
        let when = later() - SignedDuration::from_hours(trashed_days_ago as i64 * 24);
        let local = when.to_zoned(jiff::tz::TimeZone::system());
        let info = root.join("info").join(format!("{name}.trashinfo"));
        fs::create_dir_all(info.parent().unwrap()).unwrap();
        let body = format!(
            "[Trash Info]\nPath=/home/u/{name}\nDeletionDate={}\n",
            local.strftime("%Y-%m-%dT%H:%M:%S")
        );
        fs::write(&info, body).unwrap();
        (item, info)
    }

    pub fn plan(&self, cfg: &Config) -> Vec<app::RulePlan> {
        app::plan(cfg, later(), &HashSet::new())
    }

    pub fn run(&self, cfg: &Config) -> app::ApplyReport {
        app::apply(&self.paths, cfg, &self.plan(cfg), &HashSet::new(), later()).unwrap()
    }
}

/// Set mtime to `days_old` days before `later()`.
pub fn age(path: &Path, days_old: u64) {
    let t = SystemTime::now() + Duration::from_secs((AHEAD_DAYS - days_old) * DAY);
    File::open(path).unwrap().set_modified(t).unwrap();
}
