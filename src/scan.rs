//! Finding what a rule would clean. Scanning never changes anything on disk.
//!
//! A file's age is the newer of its mtime and ctime. mtime alone lies: `cp -p`,
//! `curl -R` and archive extraction all give brand-new files old mtimes, and a
//! file sweepr just restored keeps its old mtime. ctime cannot be backdated,
//! and every rename (including a restore) refreshes it.

use crate::rules::{AgeSource, Protected, Unit};
use crate::safety::is_open;
use anyhow::{Context, Result};
use globset::{Glob, GlobSet, GlobSetBuilder};
use jiff::Timestamp;
use jiff::civil::DateTime;
use jiff::tz::TimeZone;
use std::collections::HashSet;
use std::fs;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};

/// Directory name sweepr uses for quarantine inside a rule root.
pub const QUARANTINE_DIR: &str = ".sweepr-quarantine";
/// Marks a folder as a cache (bford.info/cachedir); it belongs to the folder, not the cache.
const CACHEDIR_TAG: &str = "CACHEDIR.TAG";
const SECS_PER_DAY: i64 = 86_400;

/// One thing to move: a path plus any files that must move with it
/// (a trashed item and its `.trashinfo`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Candidate {
    pub path: PathBuf,
    pub companions: Vec<PathBuf>,
    pub size: u64,
    /// Anything touched after this (Unix seconds) is too new to move.
    pub cutoff: i64,
}

/// Everything a scan needs besides the rule itself.
pub struct ScanCtx<'a> {
    pub now: Timestamp,
    pub protected: &'a [Protected],
    /// Protected even from rules with `allow_protected` (sweepr's own state).
    pub always_protected: &'a [Protected],
    pub open: &'a HashSet<PathBuf>,
}

pub struct FilesRule<'a> {
    pub root: &'a Path,
    pub age_source: AgeSource,
    pub unit: Unit,
    pub age_days: u32,
    pub exclude: &'a [String],
    pub allow_protected: bool,
}

pub fn scan(rule: &FilesRule, ctx: &ScanCtx) -> Result<Vec<Candidate>> {
    let Ok(root_meta) = fs::symlink_metadata(rule.root) else {
        return Ok(Vec::new()); // nothing installed there; not an error
    };
    if !root_meta.is_dir() {
        return Ok(Vec::new());
    }
    let mut protected = ctx.always_protected.to_vec();
    if !rule.allow_protected {
        protected.extend_from_slice(ctx.protected);
    }
    let walker = Walker {
        root: rule.root,
        dev: root_meta.dev(),
        exclude: build_globs(rule.exclude)?,
        protected,
        open: ctx.open,
        cutoff: ctx.now.as_second() - i64::from(rule.age_days) * SECS_PER_DAY,
    };
    let mut out = Vec::new();
    match (rule.age_source, rule.unit) {
        (AgeSource::Trashinfo, _) => scan_trash(rule.root, &walker, &mut out)?,
        (AgeSource::Mtime, Unit::File) => walker.old_files(rule.root, &mut out)?,
        (AgeSource::Mtime, Unit::Entry) => walker.old_entries(&mut out)?,
    }
    Ok(out)
}

/// A pattern without `/` matches at any depth, like `.gitignore`.
fn build_globs(patterns: &[String]) -> Result<GlobSet> {
    let mut b = GlobSetBuilder::new();
    for raw in patterns {
        let p = raw.trim_end_matches('/');
        let glob = |s: &str| Glob::new(s).with_context(|| format!("bad exclude pattern `{raw}`"));
        b.add(glob(p)?);
        if !p.contains('/') {
            b.add(glob(&format!("**/{p}"))?);
        }
    }
    Ok(b.build()?)
}

/// Newer of mtime and ctime.
fn touched(meta: &fs::Metadata) -> i64 {
    meta.mtime().max(meta.ctime())
}

/// True if `path` (or anything inside it) has not been touched since `cutoff`.
/// Called again just before moving, in case something changed since the scan.
pub fn still_old(path: &Path, cutoff: i64) -> bool {
    let Ok(meta) = fs::symlink_metadata(path) else {
        return false;
    };
    if touched(&meta) >= cutoff {
        return false;
    }
    if !meta.is_dir() {
        return true;
    }
    fs::read_dir(path).is_ok_and(|entries| {
        entries
            .into_iter()
            .all(|e| e.is_ok_and(|e| still_old(&e.path(), cutoff)))
    })
}

struct Walker<'a> {
    root: &'a Path,
    dev: u64,
    exclude: GlobSet,
    protected: Vec<Protected>,
    open: &'a HashSet<PathBuf>,
    cutoff: i64,
}

impl Walker<'_> {
    /// Paths sweepr must never select or descend into.
    fn skipped(&self, path: &Path) -> bool {
        path.file_name().is_some_and(|n| n == QUARANTINE_DIR)
            || self.protected.iter().any(|p| p.covers(path))
            || path
                .strip_prefix(self.root)
                .is_ok_and(|rel| self.exclude.is_match(rel))
    }

    fn candidate(&self, path: PathBuf, companions: Vec<PathBuf>, size: u64) -> Candidate {
        Candidate {
            path,
            companions,
            size,
            cutoff: self.cutoff,
        }
    }

    fn old_files(&self, dir: &Path, out: &mut Vec<Candidate>) -> Result<()> {
        let Some(entries) = read_dir_or_warn(dir, dir == self.root)? else {
            return Ok(());
        };
        for entry in entries {
            let path = entry.path();
            if self.skipped(&path) {
                continue;
            }
            let Ok(meta) = fs::symlink_metadata(&path) else {
                continue;
            };
            if meta.is_dir() {
                if meta.dev() == self.dev {
                    self.old_files(&path, out)?;
                }
            } else if touched(&meta) < self.cutoff
                && !self.open.contains(&path)
                && entry.file_name() != CACHEDIR_TAG
            {
                out.push(self.candidate(path, Vec::new(), meta.len()));
            }
        }
        Ok(())
    }

    fn old_entries(&self, out: &mut Vec<Candidate>) -> Result<()> {
        let Some(entries) = read_dir_or_warn(self.root, true)? else {
            return Ok(());
        };
        for entry in entries {
            let path = entry.path();
            if self.skipped(&path) || is_open(&path, self.open) {
                continue;
            }
            if let Some((newest, size)) = self.newest_and_size(&path)
                && newest < self.cutoff
            {
                out.push(self.candidate(path, Vec::new(), size));
            }
        }
        Ok(())
    }

    /// Newest touch time and total size of a file or tree. `None` if the tree
    /// holds anything sweepr must not or cannot move (protected, excluded,
    /// unreadable, on another filesystem).
    fn newest_and_size(&self, path: &Path) -> Option<(i64, u64)> {
        let meta = fs::symlink_metadata(path).ok()?;
        if !meta.is_dir() {
            return Some((touched(&meta), meta.len()));
        }
        if meta.dev() != self.dev {
            return None;
        }
        let (mut newest, mut size) = (touched(&meta), 0u64);
        for entry in fs::read_dir(path).ok()? {
            let child = entry.ok()?.path();
            if self.skipped(&child) {
                return None;
            }
            let (n, s) = self.newest_and_size(&child)?;
            newest = newest.max(n);
            size += s;
        }
        Some((newest, size))
    }
}

/// Sorted entries of `dir`. An unreadable rule root is an error; an
/// unreadable subdirectory (often root-owned build leftovers) is skipped.
fn read_dir_or_warn(dir: &Path, is_root: bool) -> Result<Option<Vec<fs::DirEntry>>> {
    match fs::read_dir(dir) {
        Ok(rd) => {
            let mut entries: Vec<_> = rd.flatten().collect();
            entries.sort_by_key(|e| e.file_name());
            Ok(Some(entries))
        }
        Err(e) if is_root => Err(e).with_context(|| format!("reading {}", dir.display())),
        Err(e) => {
            eprintln!("sweepr: skipping {}: {e}", dir.display());
            Ok(None)
        }
    }
}

/// FreeDesktop trash: `info/NAME.trashinfo` holds the deletion date of `files/NAME`.
fn scan_trash(root: &Path, w: &Walker, out: &mut Vec<Candidate>) -> Result<()> {
    let info_dir = root.join("info");
    if !info_dir.is_dir() {
        return Ok(());
    }
    let Some(entries) = read_dir_or_warn(&info_dir, true)? else {
        return Ok(());
    };
    for entry in entries {
        let info = entry.path();
        let Some(name) = info
            .file_name()
            .and_then(|n| n.to_str())
            .and_then(|n| n.strip_suffix(".trashinfo"))
            .filter(|n| !matches!(*n, "" | "." | ".."))
        else {
            continue;
        };
        let Some(trashed_at) = deletion_date(&info) else {
            eprintln!(
                "sweepr: skipping {}: no readable DeletionDate",
                info.display()
            );
            continue;
        };
        // A restored item keeps its old DeletionDate; the restore refreshed
        // the info file's ctime, so count that too.
        let info_ctime = fs::symlink_metadata(&info).map_or(i64::MAX, |m| m.ctime());
        if trashed_at.max(info_ctime) >= w.cutoff {
            continue;
        }
        let item = root.join("files").join(name);
        if fs::symlink_metadata(&item).is_err() {
            // Orphaned info file: the item is already gone, so move just the info.
            out.push(w.candidate(info, Vec::new(), 0));
            continue;
        }
        if w.skipped(&item) || is_open(&item, w.open) {
            continue;
        }
        // A trashed tree holding anything unmovable stays whole in the trash.
        if let Some((_, size)) = w.newest_and_size(&item) {
            // Trash items are aged by trash date, not by their own times.
            out.push(Candidate {
                cutoff: i64::MAX,
                ..w.candidate(item, vec![info], size)
            });
        }
    }
    Ok(())
}

/// Parse `DeletionDate=YYYY-MM-DDThh:mm:ss` (local time) into Unix seconds.
fn deletion_date(info: &Path) -> Option<i64> {
    let text = fs::read_to_string(info).ok()?;
    let raw = text
        .lines()
        .find_map(|l| l.trim().strip_prefix("DeletionDate="))?
        .trim();
    let civil: DateTime = raw.parse().ok()?;
    let zoned = civil.to_zoned(TimeZone::system()).ok()?;
    Some(zoned.timestamp().as_second())
}
