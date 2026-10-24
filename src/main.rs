mod cli;

use anyhow::{Context, Result, bail};
use clap::Parser;
use cli::{Cli, Cmd};
use jiff::Timestamp;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use sweepr::app;
use sweepr::config::Config;
use sweepr::disk::{self, human};
use sweepr::manifest::{self, Manifest};
use sweepr::notify::{self, Choice, Message};
use sweepr::paths::Paths;
use sweepr::progress::Meter;
use sweepr::report;
use sweepr::safety::{self, RunLock};

fn main() -> ExitCode {
    match real_main() {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("sweepr: {e:#}");
            ExitCode::FAILURE
        }
    }
}

fn real_main() -> Result<()> {
    let cli = Cli::parse();
    // Packaging runs this under fakeroot, before any user config exists.
    if let Cmd::Generate { what, dir } = cli.cmd {
        return match dir {
            Some(dir) => cli::generate_man(&dir),
            None => cli::generate_completion(what, &mut std::io::stdout().lock()),
        };
    }
    safety::ensure_not_root()?;
    let paths = Paths::from_env()?;
    let cfg = Config::load(&paths)?;
    let now = Timestamp::now();
    match cli.cmd {
        Cmd::Scan { verbose } => {
            let plans = plan_with_meter(&cfg, now);
            print!("{}", report::scan_table(&plans, verbose));
            for (name, cmd) in app::planned_commands(&cfg) {
                println!("{name:<16} also runs `{cmd}` (deletes directly, not undoable)");
            }
            println!("\nNothing was changed. `sweepr run` moves these into quarantine.");
            Ok(())
        }
        Cmd::Run { scheduled } => run(&paths, &cfg, now, scheduled),
        Cmd::Restore { run, last, path } => restore(&paths, run, last, path),
        Cmd::Purge { run, expired, all } => purge(&paths, now, run, expired, all),
        Cmd::Status => status(&paths, &cfg),
        Cmd::Rules => {
            print!("{}", report::rules_table(&cfg));
            Ok(())
        }
        Cmd::Tick => tick(&paths, &cfg, now),
        Cmd::Enable => {
            app::set_enabled(&paths, true)?;
            println!("Scheduled runs will now move old files into quarantine.");
            Ok(())
        }
        Cmd::Disable => {
            app::set_enabled(&paths, false)?;
            println!("Scheduled runs are preview-only again.");
            Ok(())
        }
        Cmd::Generate { .. } => unreachable!("handled before loading config"),
    }
}

/// Scan every rule, showing which one is being scanned on a terminal.
fn plan_with_meter(cfg: &Config, now: Timestamp) -> Vec<app::RulePlan> {
    let mut meter = Meter::stderr();
    let plans = app::plan_reporting(cfg, now, &safety::open_files(), &mut |p| meter.update(p));
    meter.finish();
    plans
}

fn run(paths: &Paths, cfg: &Config, now: Timestamp, scheduled: bool) -> Result<()> {
    let lock = RunLock::acquire(&paths.state_dir())?;
    let plans = plan_with_meter(cfg, now);
    for p in plans.iter().filter(|p| p.error.is_some()) {
        eprintln!(
            "sweepr: rule `{}` skipped: {}",
            p.name,
            p.error.as_deref().unwrap_or("")
        );
    }
    let bytes: u64 = plans.iter().map(app::RulePlan::bytes).sum();

    if scheduled && !app::is_enabled(paths) {
        if bytes > 0 {
            let body = format!(
                "Could free {}. Review with `sweepr scan -v`, then turn on with `sweepr enable`.",
                human(bytes)
            );
            let msg = Message {
                summary: "sweepr preview",
                body: &body,
                critical: false,
                actions: &[],
            };
            notify::show(&msg);
        }
        return Ok(());
    }

    // Re-read open files right before moving: the scan may have taken a while.
    let mut meter = Meter::stderr();
    let applied = app::apply_reporting(paths, cfg, &plans, &safety::open_files(), now, &mut |p| {
        meter.update(p)
    });
    meter.finish();
    let applied = applied?;
    for path in &applied.cross_device {
        eprintln!("sweepr: skipped {} (on another filesystem)", path.display());
    }
    for (path, err) in &applied.failed {
        eprintln!("sweepr: could not move {}: {err}", path.display());
    }
    if applied.skipped > 0 {
        println!(
            "Left {} item(s) alone: changed, opened or gone since the scan.",
            applied.skipped
        );
    }
    for (name, result) in app::run_commands(cfg) {
        if let Err(e) = result {
            eprintln!("sweepr: rule `{name}` failed: {e:#}");
        }
    }
    let Some(m) = applied.manifest else {
        println!("Nothing old enough to clean.");
        return Ok(());
    };
    let details = m.details_path_in(&paths.runs_dir());
    if let Err(e) = std::fs::write(&details, report::run_details(&m)) {
        eprintln!("sweepr: could not write {}: {e}", details.display());
    }
    let mut body = format!(
        "Moved {} ({} items) to quarantine. Deleted for good after {}.",
        human(m.total_size()),
        m.items.len(),
        m.expires.strftime("%b %d")
    );
    if !applied.failed.is_empty() {
        body.push_str(&format!(
            " {} item(s) could not be moved; see `journalctl --user -u sweepr`.",
            applied.failed.len()
        ));
    }
    if !scheduled {
        // Someone at a terminal: print the undo commands instead of making
        // them wait for a notification click.
        println!("{body}");
        println!(
            "Undo: sweepr restore --last    Details: {}",
            details.display()
        );
        return Ok(());
    }
    let actions = [
        ("restore", "Restore"),
        ("purge", "Delete now"),
        ("details", "Details"),
    ];
    let msg = Message {
        summary: "sweepr cleaned up",
        body: &body,
        critical: false,
        actions: &actions,
    };
    // Don't block other sweepr commands while waiting up to 10 minutes for a click.
    drop(lock);
    let choice = notify::show(&msg);
    on_click(paths, &m.id, &choice, &details)
}

/// Act on a notification button. The run is re-read from disk under the lock,
/// since the user may have restored or purged it from a terminal meanwhile.
fn on_click(paths: &Paths, id: &str, choice: &Choice, details: &Path) -> Result<()> {
    let Choice::Action(action) = choice else {
        return Ok(());
    };
    if action == "details" {
        open(details);
        return Ok(());
    }
    let _lock = RunLock::acquire_waiting(&paths.state_dir())?;
    let Some(m) = manifest::load_one(&paths.runs_dir(), id)? else {
        println!("Run {id} is no longer in quarantine.");
        return Ok(());
    };
    match action.as_str() {
        "restore" => print_restore(&app::restore(paths, &m, None)?),
        "purge" => app::purge(paths, &m)?,
        _ => {}
    }
    Ok(())
}

fn restore(paths: &Paths, run: Option<String>, last: bool, path: Option<PathBuf>) -> Result<()> {
    let _lock = RunLock::acquire(&paths.state_dir())?;
    let runs_dir = paths.runs_dir();
    let targets: Vec<Manifest> = match (&run, last, &path) {
        (Some(id), _, _) => vec![manifest::load_one(&runs_dir, id)?.with_context(|| no_run(id))?],
        (None, true, _) => {
            let all = manifest::load_all(&runs_dir)?.warn();
            vec![all.into_iter().last().context("quarantine is empty")?]
        }
        (None, false, Some(_)) => manifest::load_all(&runs_dir)?
            .warn()
            .into_iter()
            .rev()
            .collect(),
        (None, false, None) => bail!("say what to restore: a run id, --last, or --path"),
    };
    let only = path.as_deref().map(resolve).transpose()?;
    let mut total = app::RestoreReport::default();
    for m in &targets {
        let r = app::restore(paths, m, only.as_deref())?;
        total.restored += r.restored;
        total.missing += r.missing;
        total.occupied.extend(r.occupied);
    }
    print_restore(&total);
    Ok(())
}

fn purge(
    paths: &Paths,
    now: Timestamp,
    run: Option<String>,
    expired: bool,
    all: bool,
) -> Result<()> {
    let _lock = RunLock::acquire(&paths.state_dir())?;
    if expired {
        let gone = app::purge_expired(paths, now)?;
        println!("Purged {} expired run(s).", gone.len());
        return Ok(());
    }
    let runs_dir = paths.runs_dir();
    let targets: Vec<Manifest> = match (&run, all) {
        (Some(id), _) => vec![manifest::load_one(&runs_dir, id)?.with_context(|| no_run(id))?],
        (None, true) => manifest::load_all(&runs_dir)?.warn(),
        (None, false) => bail!("say what to purge: a run id, --expired, or --all"),
    };
    for m in &targets {
        app::purge(paths, m)?;
        println!("Deleted run {} ({}) for good.", m.id, human(m.total_size()));
    }
    Ok(())
}

fn status(paths: &Paths, cfg: &Config) -> Result<()> {
    let mode = if app::is_enabled(paths) {
        "on"
    } else {
        "preview only (`sweepr enable`)"
    };
    println!("Scheduled runs: {mode}");
    println!("Grace period:   {} days", cfg.grace_days);
    match disk::usage_percent(&paths.home) {
        Ok(pct) => println!(
            "Home disk:      {pct}% used (warns at {}%)",
            cfg.disk_warn_percent
        ),
        Err(e) => println!("Home disk:      unknown ({e:#})"),
    }
    let loaded = manifest::load_all(&paths.runs_dir())?;
    for msg in &loaded.broken {
        println!("Warning:        {msg}");
    }
    if loaded.runs.is_empty() {
        println!("Quarantine:     empty");
    } else {
        println!("Quarantine:");
        loaded
            .runs
            .iter()
            .for_each(|m| println!("{}", report::quarantine_line(m)));
    }
    Ok(())
}

fn tick(paths: &Paths, cfg: &Config, now: Timestamp) -> Result<()> {
    let lock = RunLock::acquire(&paths.state_dir())?;
    for m in app::purge_expired(paths, now)? {
        println!("Purged expired run {} ({}).", m.id, human(m.total_size()));
    }
    let pct = disk::usage_percent(&paths.home)?;
    if pct < cfg.disk_warn_percent || warned_recently(paths, now) {
        return Ok(());
    }
    std::fs::write(paths.state_dir().join("disk-warned"), b"")?;
    let held: u64 = manifest::load_all(&paths.runs_dir())?
        .warn()
        .iter()
        .map(Manifest::total_size)
        .sum();
    let body = if held > 0 {
        format!(
            "Home is {pct}% full. Quarantine holds {}. Run `sweepr scan` to see more.",
            human(held)
        )
    } else {
        format!("Home is {pct}% full. Run `sweepr scan` to see what can go.")
    };
    let actions: &[(&str, &str)] = if held > 0 {
        &[("purge", "Empty quarantine")]
    } else {
        &[]
    };
    drop(lock);
    let msg = Message {
        summary: "Disk almost full",
        body: &body,
        critical: true,
        actions,
    };
    if notify::show(&msg) == Choice::Action("purge".into()) {
        let _lock = RunLock::acquire_waiting(&paths.state_dir())?;
        // Re-read: runs may have been restored or purged while the notification was up.
        for m in manifest::load_all(&paths.runs_dir())?.warn() {
            app::purge(paths, &m)?;
        }
    }
    Ok(())
}

/// At most one disk warning a day.
fn warned_recently(paths: &Paths, now: Timestamp) -> bool {
    const DAY_SECS: i64 = 86_400;
    std::fs::metadata(paths.state_dir().join("disk-warned"))
        .and_then(|m| m.modified())
        .ok()
        .and_then(|t| Timestamp::try_from(t).ok())
        .is_some_and(|t| now.as_second() - t.as_second() < DAY_SECS)
}

fn no_run(id: &str) -> String {
    format!("no run `{id}` in quarantine (see `sweepr status`)")
}

/// Absolute path with symlinks resolved, matching how runs record paths.
/// The path itself usually no longer exists, so resolve its parent.
fn resolve(p: &Path) -> Result<PathBuf> {
    let abs = std::path::absolute(p).with_context(|| format!("resolving {}", p.display()))?;
    if let Ok(real) = abs.canonicalize() {
        return Ok(real);
    }
    match (
        abs.parent().and_then(|d| d.canonicalize().ok()),
        abs.file_name(),
    ) {
        (Some(dir), Some(name)) => Ok(dir.join(name)),
        _ => Ok(abs),
    }
}

fn print_restore(r: &app::RestoreReport) {
    println!("Restored {} item(s).", r.restored);
    if r.missing > 0 {
        println!(
            "  {} item(s) were not in quarantine (already restored or purged)",
            r.missing
        );
    }
    for p in &r.occupied {
        println!("  kept in quarantine, something new is at {}", p.display());
    }
}

fn open(path: &Path) {
    if let Err(e) = std::process::Command::new("xdg-open").arg(path).spawn() {
        eprintln!("sweepr: could not open {}: {e}", path.display());
    }
}
