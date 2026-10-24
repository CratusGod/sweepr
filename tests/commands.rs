//! The CLI commands end to end, with a later clock and a stand-in for the
//! desktop notification (which records what was shown and "clicks" a button).

mod common;

use common::{Home, NO_COMMANDS, later};
use jiff::{SignedDuration, Timestamp};
use std::cell::RefCell;
use std::fs;
use std::path::PathBuf;
use sweepr::commands::{self, Notify};
use sweepr::config::Config;
use sweepr::manifest;
use sweepr::notify::{Choice, Message};
use sweepr::{app, paths::Paths};

/// Records each notification and answers with `click`.
struct Desktop {
    shown: RefCell<Vec<(String, bool)>>,
    click: Option<&'static str>,
}

impl Desktop {
    fn clicking(click: Option<&'static str>) -> Self {
        Self {
            shown: RefCell::new(Vec::new()),
            click,
        }
    }

    fn show(&self, msg: &Message) -> Choice {
        self.shown
            .borrow_mut()
            .push((msg.summary.to_owned(), msg.critical));
        self.click
            .map_or(Choice::Dismissed, |a| Choice::Action(a.into()))
    }

    fn summaries(&self) -> Vec<String> {
        self.shown.borrow().iter().map(|(s, _)| s.clone()).collect()
    }
}

fn never(_: &Message) -> Choice {
    panic!("a terminal run must not wait on a notification")
}

fn setup(extra: &str) -> (Home, Config, PathBuf) {
    let h = Home::new();
    let old = h.file(&h.paths.cache.join("pip/http/a"), 40);
    let cfg = h.config(&format!("{extra}{NO_COMMANDS}"));
    (h, cfg, old)
}

fn runs(paths: &Paths) -> usize {
    manifest::load_all(&paths.runs_dir()).unwrap().runs.len()
}

#[test]
fn terminal_run_quarantines_and_restore_last_undoes_it() {
    let (h, cfg, old) = setup("");
    commands::run(&h.paths, &cfg, later(), false, &never).unwrap();
    assert!(!old.exists());
    let m = &manifest::load_all(&h.paths.runs_dir()).unwrap().runs[0];
    assert!(m.details_path_in(&h.paths.runs_dir()).is_file());
    commands::status(&h.paths, &cfg).unwrap();

    commands::restore(&h.paths, None, true, None).unwrap();
    assert!(old.is_file());
    assert_eq!(runs(&h.paths), 0);
}

#[test]
fn scheduled_runs_only_preview_until_enabled() {
    let (h, cfg, old) = setup("");
    let desktop = Desktop::clicking(None);
    commands::run(&h.paths, &cfg, later(), true, &|m| desktop.show(m)).unwrap();
    assert!(old.exists());
    assert_eq!(desktop.summaries(), ["sweepr preview"]);
    assert_eq!(runs(&h.paths), 0);
}

#[test]
fn restore_button_puts_the_run_back() {
    let (h, cfg, old) = setup("");
    app::set_enabled(&h.paths, true).unwrap();
    let desktop = Desktop::clicking(Some("restore"));
    commands::run(&h.paths, &cfg, later(), true, &|m| desktop.show(m)).unwrap();
    assert_eq!(desktop.summaries(), ["sweepr cleaned up"]);
    assert!(old.is_file());
    assert_eq!(runs(&h.paths), 0);
}

#[test]
fn delete_now_button_purges_the_run() {
    let (h, cfg, old) = setup("");
    app::set_enabled(&h.paths, true).unwrap();
    let desktop = Desktop::clicking(Some("purge"));
    commands::run(&h.paths, &cfg, later(), true, &|m| desktop.show(m)).unwrap();
    assert!(!old.exists());
    assert_eq!(runs(&h.paths), 0);
    assert!(!h.paths.default_quarantine().join("x").exists());
}

#[test]
fn nothing_old_means_no_notification() {
    let h = Home::new();
    let cfg = h.config(NO_COMMANDS);
    app::set_enabled(&h.paths, true).unwrap();
    commands::run(&h.paths, &cfg, later(), true, &never).unwrap();
    assert_eq!(runs(&h.paths), 0);
}

#[test]
fn disk_warning_offers_to_empty_quarantine_once_a_day() {
    let (h, cfg, _) = setup("disk_warn_percent = 1\n");
    commands::run(&h.paths, &cfg, later(), false, &never).unwrap();
    let desktop = Desktop::clicking(Some("purge"));
    let notify: Notify = &|m| desktop.show(m);
    // Real clock: the run made at `later()` has not expired yet.
    commands::tick(&h.paths, &cfg, Timestamp::now(), notify).unwrap();
    assert_eq!(*desktop.shown.borrow(), [("Disk almost full".into(), true)]);
    assert_eq!(runs(&h.paths), 0);

    commands::tick(&h.paths, &cfg, Timestamp::now(), notify).unwrap();
    assert_eq!(desktop.shown.borrow().len(), 1, "warned twice in a day");
}

#[test]
fn tick_purges_runs_whose_grace_period_ended() {
    let (h, cfg, old) = setup("disk_warn_percent = 100\n");
    commands::run(&h.paths, &cfg, later(), false, &never).unwrap();
    let after_grace = later() + SignedDuration::from_hours(8 * 24);
    commands::tick(&h.paths, &cfg, after_grace, &never).unwrap();
    assert_eq!(runs(&h.paths), 0);
    assert!(!old.exists());
}

#[test]
fn restore_by_path_and_purge_by_id() {
    let h = Home::new();
    let a = h.file(&h.paths.cache.join("pip/http/a"), 40);
    let b = h.file(&h.paths.cache.join("pip/http/b"), 40);
    let cfg = h.config(NO_COMMANDS);
    commands::run(&h.paths, &cfg, later(), false, &never).unwrap();

    commands::restore(&h.paths, None, false, Some(a.clone())).unwrap();
    assert!(a.is_file() && !b.exists());

    let id = manifest::load_all(&h.paths.runs_dir()).unwrap().runs[0]
        .id
        .clone();
    commands::purge(&h.paths, later(), Some(id.clone()), false, false).unwrap();
    assert_eq!(runs(&h.paths), 0);
    let err = commands::restore(&h.paths, Some(id), false, None).unwrap_err();
    assert!(format!("{err:#}").contains("no run"));
}

#[test]
fn restore_and_purge_need_a_target() {
    let h = Home::new();
    let none = commands::restore(&h.paths, None, false, None).unwrap_err();
    assert!(none.to_string().contains("say what to restore"));
    let empty = commands::restore(&h.paths, None, true, None).unwrap_err();
    assert!(format!("{empty:#}").contains("quarantine is empty"));
    let none = commands::purge(&h.paths, later(), None, false, false).unwrap_err();
    assert!(none.to_string().contains("say what to purge"));
    commands::purge(&h.paths, later(), None, false, true).unwrap();
    commands::purge(&h.paths, later(), None, true, false).unwrap();
}

#[test]
fn status_reports_a_damaged_run_record_without_failing() {
    let (h, cfg, _) = setup("");
    commands::run(&h.paths, &cfg, later(), false, &never).unwrap();
    fs::write(h.paths.runs_dir().join("broken.json"), "{").unwrap();
    commands::status(&h.paths, &cfg).unwrap();
}
