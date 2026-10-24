//! What each command does, independent of how results are shown.

use crate::config::Config;
use crate::manifest::{self, Item, Manifest};
use crate::paths::Paths;
use crate::quarantine::{self, MoveOutcome, RestoreOutcome};
use crate::rules::{Action, AgeSource, Protected};
use crate::safety::is_open;
use crate::scan::{self, Candidate, FilesRule, ScanCtx};
use anyhow::{Context, Result};
use jiff::{SignedDuration, Timestamp};
use std::collections::{BTreeSet, HashSet};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};

/// How long a command rule (`uv cache prune`, ...) may run before it is killed.
const COMMAND_TIMEOUT: Duration = Duration::from_secs(5 * 60);

/// What one files rule would clean.
pub struct RulePlan {
    pub name: String,
    pub root: PathBuf,
    pub candidates: Vec<Candidate>,
    /// Remove directories a move leaves empty. Off for the trash, whose
    /// `files/` and `info/` directories must stay.
    pub prune: bool,
    /// Why this rule could not be scanned, if it could not.
    pub error: Option<String>,
}

impl RulePlan {
    pub fn bytes(&self) -> u64 {
        self.candidates.iter().map(|c| c.size).sum()
    }
}

/// Resolve symlinks so paths compare equal to what `/proc/*/fd` reports.
fn canonical(path: &Path) -> PathBuf {
    path.canonicalize().unwrap_or_else(|_| path.to_path_buf())
}

fn canonical_all(list: &[Protected]) -> Vec<Protected> {
    list.iter()
        .map(|p| Protected {
            path: canonical(&p.path),
            reason: p.reason,
        })
        .collect()
}

/// What a long scan or run is doing, for a progress line.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Progress<'a> {
    Scanning { rule: &'a str },
    Checking { done: usize, total: usize },
    Moving { done: usize, total: usize },
}

/// Scan every enabled files rule. A rule that cannot be scanned is reported
/// in its plan and skipped; it never stops the other rules.
pub fn plan(cfg: &Config, now: Timestamp, open: &HashSet<PathBuf>) -> Vec<RulePlan> {
    plan_reporting(cfg, now, open, &mut |_| {})
}

/// [`plan`], reporting each rule as it starts.
pub fn plan_reporting(
    cfg: &Config,
    now: Timestamp,
    open: &HashSet<PathBuf>,
    progress: &mut dyn FnMut(Progress),
) -> Vec<RulePlan> {
    let protected = canonical_all(&cfg.protected);
    let always_protected = canonical_all(&cfg.always_protected);
    let ctx = ScanCtx {
        now,
        protected: &protected,
        always_protected: &always_protected,
        open,
    };
    let mut out = Vec::new();
    for rule in cfg.rules.iter().filter(|r| r.enabled) {
        let Action::Files {
            root,
            age_source,
            unit,
            age_days,
            exclude,
            allow_protected,
        } = &rule.action
        else {
            continue;
        };
        progress(Progress::Scanning { rule: &rule.name });
        let root = canonical(root);
        let files_rule = FilesRule {
            root: &root,
            age_source: *age_source,
            unit: *unit,
            age_days: *age_days,
            exclude,
            allow_protected: *allow_protected,
        };
        let (candidates, error) = match scan::scan(&files_rule, &ctx) {
            Ok(c) => (c, None),
            Err(e) => (Vec::new(), Some(format!("{e:#}"))),
        };
        out.push(RulePlan {
            name: rule.name.clone(),
            root,
            candidates,
            prune: *age_source != AgeSource::Trashinfo,
            error,
        });
    }
    out
}

/// Command rules that `run` would execute (installed and enabled).
pub fn planned_commands(cfg: &Config) -> Vec<(String, String)> {
    cfg.rules
        .iter()
        .filter(|r| r.enabled)
        .filter_map(|r| match &r.action {
            Action::Command { program, args } if on_path(program) => {
                Some((r.name.clone(), format!("{program} {}", args.join(" "))))
            }
            _ => None,
        })
        .collect()
}

#[derive(Debug, Default)]
pub struct ApplyReport {
    pub manifest: Option<Manifest>,
    pub cross_device: Vec<PathBuf>,
    /// Gone, changed or opened since the scan; left where they are.
    pub skipped: usize,
    pub failed: Vec<(PathBuf, String)>,
}

/// Move every planned candidate into quarantine and record it.
///
/// `open` should be read fresh just before calling: anything opened or
/// touched since the scan is left alone.
pub fn apply(
    paths: &Paths,
    cfg: &Config,
    plans: &[RulePlan],
    open: &HashSet<PathBuf>,
    now: Timestamp,
) -> Result<ApplyReport> {
    apply_reporting(paths, cfg, plans, open, now, &mut |_| {})
}

/// [`apply`], reporting how many items have been checked and moved.
pub fn apply_reporting(
    paths: &Paths,
    cfg: &Config,
    plans: &[RulePlan],
    open: &HashSet<PathBuf>,
    now: Timestamp,
    progress: &mut dyn FnMut(Progress),
) -> Result<ApplyReport> {
    let runs_dir = paths.runs_dir();
    let id = manifest::new_id(now, &runs_dir);
    let grace = SignedDuration::from_hours(i64::from(cfg.grace_days) * 24);
    let mut report = ApplyReport::default();
    let mut planned = Vec::new();
    let mut run_dirs = BTreeSet::new();
    let total: usize = plans.iter().map(|p| p.candidates.len()).sum();
    let mut checked = 0;
    for p in plans.iter().filter(|p| !p.candidates.is_empty()) {
        let run_dir = quarantine::quarantine_root(&p.root, &paths.default_quarantine())?.join(&id);
        for c in &p.candidates {
            progress(Progress::Checking {
                done: checked,
                total,
            });
            checked += 1;
            if is_open(&c.path, open) || !scan::still_old(&c.path, c.cutoff) {
                report.skipped += 1;
                continue;
            }
            run_dirs.insert(run_dir.clone());
            for path in std::iter::once(&c.path).chain(&c.companions) {
                planned.push(Item {
                    rule: p.name.clone(),
                    original: path.clone(),
                    quarantined: quarantine::destination(&run_dir, &p.name, &p.root, path)?,
                    size: if path == &c.path { c.size } else { 0 },
                });
            }
        }
    }
    if planned.is_empty() {
        return Ok(report);
    }

    // Record the plan durably before touching anything, so a crash stays restorable.
    let mut m = Manifest {
        id,
        created: now,
        expires: now.checked_add(grace)?,
        complete: false,
        items: planned,
        run_dirs: run_dirs.into_iter().collect(),
    };
    m.save(&runs_dir)?;

    let mut moved = Vec::new();
    let total = m.items.len();
    for (done, item) in m.items.iter().enumerate() {
        progress(Progress::Moving { done, total });
        match quarantine::move_noreplace(&item.original, &item.quarantined) {
            Ok(MoveOutcome::Moved) => {
                let plan = plans.iter().find(|p| p.name == item.rule);
                if let Some(p) = plan.filter(|p| p.prune) {
                    quarantine::prune_empty_parents(&item.original, &p.root);
                }
                moved.push(item.clone());
            }
            Ok(MoveOutcome::CrossDevice) => report.cross_device.push(item.original.clone()),
            Ok(MoveOutcome::Gone) => report.skipped += 1,
            // One stuck file must not stop the rest of the run.
            Err(e) => report
                .failed
                .push((item.original.clone(), format!("{e:#}"))),
        }
    }
    m.items = moved;
    m.complete = true;
    if m.items.is_empty() {
        purge(paths, &m)?;
        return Ok(report);
    }
    m.save(&runs_dir)?;
    report.manifest = Some(m);
    Ok(report)
}

/// Run each enabled command rule whose program is installed.
/// These delete directly; they are not covered by restore.
pub fn run_commands(cfg: &Config) -> Vec<(String, Result<()>)> {
    cfg.rules
        .iter()
        .filter(|r| r.enabled)
        .filter_map(|r| match &r.action {
            Action::Command { program, args } if on_path(program) => {
                Some((r.name.clone(), run_with_timeout(program, args)))
            }
            _ => None,
        })
        .collect()
}

fn run_with_timeout(program: &str, args: &[String]) -> Result<()> {
    let mut child = Command::new(program)
        .args(args)
        .spawn()
        .with_context(|| format!("starting {program}"))?;
    let started = Instant::now();
    loop {
        if let Some(status) = child.try_wait()? {
            return if status.success() {
                Ok(())
            } else {
                anyhow::bail!("{program} exited with {status}")
            };
        }
        if started.elapsed() > COMMAND_TIMEOUT {
            let _ = child.kill();
            let _ = child.wait();
            anyhow::bail!(
                "{program} took over {} s and was stopped",
                COMMAND_TIMEOUT.as_secs()
            );
        }
        std::thread::sleep(Duration::from_millis(200));
    }
}

fn on_path(program: &str) -> bool {
    std::env::var_os("PATH")
        .is_some_and(|path| std::env::split_paths(&path).any(|dir| dir.join(program).is_file()))
}

#[derive(Debug, Default, PartialEq, Eq)]
pub struct RestoreReport {
    pub restored: usize,
    pub occupied: Vec<PathBuf>,
    pub missing: usize,
}

/// Restore every item of a run, or only those at or under `only`.
pub fn restore(paths: &Paths, m: &Manifest, only: Option<&Path>) -> Result<RestoreReport> {
    let mut report = RestoreReport::default();
    let mut remaining = Vec::new();
    for item in &m.items {
        if only.is_some_and(|o| !item.original.starts_with(o)) {
            remaining.push(item.clone());
            continue;
        }
        match quarantine::restore(&item.original, &item.quarantined)? {
            RestoreOutcome::Restored => report.restored += 1,
            RestoreOutcome::NotQuarantined => report.missing += 1,
            RestoreOutcome::Occupied => {
                report.occupied.push(item.original.clone());
                remaining.push(item.clone());
            }
        }
    }
    if remaining.is_empty() {
        purge(paths, m)?;
    } else {
        Manifest {
            items: remaining,
            ..m.clone()
        }
        .save(&paths.runs_dir())?;
    }
    Ok(report)
}

/// Delete a run's quarantined files for good and forget the run.
pub fn purge(paths: &Paths, m: &Manifest) -> Result<()> {
    for dir in &m.run_dirs {
        quarantine::purge_dir(dir)?;
        // Drop an in-root `.sweepr-quarantine` once its last run is gone.
        if let Some(parent) = dir.parent().filter(|p| p.ends_with(scan::QUARANTINE_DIR)) {
            let _ = std::fs::remove_dir(parent);
        }
    }
    m.remove(&paths.runs_dir())
}

/// Purge every run whose grace period has ended. Returns what was purged.
pub fn purge_expired(paths: &Paths, now: Timestamp) -> Result<Vec<Manifest>> {
    let expired: Vec<_> = manifest::load_all(&paths.runs_dir())?
        .warn()
        .into_iter()
        .filter(|m| m.expires <= now)
        .collect();
    for m in &expired {
        purge(paths, m)?;
    }
    Ok(expired)
}

pub fn is_enabled(paths: &Paths) -> bool {
    paths.state_dir().join("enabled").exists()
}

pub fn set_enabled(paths: &Paths, on: bool) -> Result<()> {
    let marker = paths.state_dir().join("enabled");
    if on {
        std::fs::create_dir_all(paths.state_dir())?;
        std::fs::write(&marker, b"")?;
    } else if marker.exists() {
        std::fs::remove_file(&marker)?;
    }
    Ok(())
}
