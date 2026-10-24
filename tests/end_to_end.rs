//! Full scan → quarantine → restore/purge cycles on a throwaway home directory.

mod common;

use common::{DAY, Home, age, later};

use jiff::{SignedDuration, Timestamp};
use std::collections::HashSet;
use std::fs::{self, File};
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::time::{Duration, SystemTime};
use sweepr::app;
use sweepr::manifest;

fn found(plans: &[app::RulePlan]) -> Vec<PathBuf> {
    plans
        .iter()
        .flat_map(|p| p.candidates.iter().map(|c| c.path.clone()))
        .collect()
}

#[test]
fn trash_goes_by_deletion_date_not_file_age() {
    let h = Home::new();
    let (old_file_trashed_yesterday, _) = h.trash("photo.jpg", 700, 1);
    let (trashed_long_ago, info) = h.trash("notes.txt", 1, 40);
    let cfg = h.config("");
    let plans = found(&h.plan(&cfg));
    assert!(plans.contains(&trashed_long_ago));
    assert!(!plans.contains(&old_file_trashed_yesterday));

    let m = h.run(&cfg).manifest.unwrap();
    assert!(
        !trashed_long_ago.exists() && !info.exists(),
        "item and .trashinfo move together"
    );
    assert!(
        h.paths.data.join("Trash/files").is_dir(),
        "trash folders stay"
    );
    assert!(old_file_trashed_yesterday.exists());
    assert_eq!(m.items.len(), 2);
}

#[test]
fn odd_trashinfo_names_never_select_the_whole_trash() {
    let h = Home::new();
    let info_dir = h.paths.data.join("Trash/info");
    fs::create_dir_all(&info_dir).unwrap();
    h.file(&h.paths.data.join("Trash/files/keep"), 400);
    for name in [".trashinfo", "..trashinfo"] {
        fs::write(
            info_dir.join(name),
            "[Trash Info]\nDeletionDate=2000-01-01T00:00:00\n",
        )
        .unwrap();
    }
    let plans = found(&h.plan(&h.config("")));
    assert!(!plans.contains(&h.paths.data.join("Trash/files")));
    assert!(!plans.contains(&h.paths.data.join("Trash/files/.")));
}

#[test]
fn caches_go_by_age_and_protected_shader_cache_is_never_touched() {
    let h = Home::new();
    let old = h.file(&h.paths.cache.join("pip/http/a"), 40);
    let fresh = h.file(&h.paths.cache.join("pip/http/b"), 2);
    let shader = h.file(&h.paths.cache.join("nvidia/GLCache/x.bin"), 400);
    // Even a user rule over all of ~/.cache must not reach the shader cache.
    let cfg = h.config("[[custom]]\nname = \"all-cache\"\npath = \"~/.cache\"");
    let plans = found(&h.plan(&cfg));
    assert!(plans.contains(&old));
    assert!(!plans.contains(&fresh));
    assert!(!plans.contains(&shader));
}

#[test]
fn progress_names_each_rule_and_counts_every_move() {
    let h = Home::new();
    h.file(&h.paths.cache.join("pip/http/a"), 40);
    h.file(&h.paths.cache.join("pip/http/b"), 40);
    let cfg = h.config("");
    let mut rules = Vec::new();
    let plans = app::plan_reporting(&cfg, later(), &HashSet::new(), &mut |p| {
        if let app::Progress::Scanning { rule } = p {
            rules.push(rule.to_owned());
        }
    });
    assert!(rules.contains(&"pip".to_owned()) && rules.contains(&"trash".to_owned()));
    let mut moves = Vec::new();
    let report = app::apply_reporting(&h.paths, &cfg, &plans, &HashSet::new(), later(), &mut |p| {
        if let app::Progress::Moving { done, total } = p {
            moves.push((done, total));
        }
    })
    .unwrap();
    assert_eq!(report.manifest.unwrap().items.len(), 2);
    assert_eq!(moves, [(0, 2), (1, 2)]);
}

#[test]
fn cachedir_tag_stays_with_its_folder() {
    let h = Home::new();
    let tag = h.file(&h.paths.home.join(".npm/_cacache/CACHEDIR.TAG"), 400);
    let old = h.file(&h.paths.home.join(".npm/_cacache/content-v2/a"), 400);
    let plans = found(&h.plan(&h.config("")));
    assert!(plans.contains(&old));
    assert!(!plans.contains(&tag));
}

#[test]
fn backdated_mtime_alone_does_not_make_a_file_old() {
    // `cp -p`, `curl -R` and restores keep old mtimes on brand-new files.
    let h = Home::new();
    let f = h.file(&h.paths.cache.join("pip/fresh-copy"), 0);
    File::open(&f)
        .unwrap()
        .set_modified(SystemTime::now() - Duration::from_secs(400 * DAY))
        .unwrap();
    let plans = app::plan(&h.config(""), Timestamp::now(), &HashSet::new());
    assert!(!found(&plans).contains(&f));
}

#[test]
fn allow_protected_is_an_explicit_opt_in_but_never_reaches_sweepr_state() {
    let h = Home::new();
    let shader = h.file(&h.paths.cache.join("nvidia/GLCache/x.bin"), 400);
    let cfg =
        h.config("[[custom]]\nname = \"gl\"\npath = \"~/.cache/nvidia\"\nallow_protected = true");
    assert!(found(&h.plan(&cfg)).contains(&shader));

    let own = h.file(&h.paths.data.join("sweepr/quarantine/old/x"), 400);
    let cfg =
        h.config("[[custom]]\nname = \"share\"\npath = \"~/.local/share\"\nallow_protected = true");
    assert!(!found(&h.plan(&cfg)).iter().any(|p| own.starts_with(p)));
}

#[test]
fn open_files_symlinks_and_nested_excludes_are_respected() {
    let h = Home::new();
    let held = h.file(&h.paths.cache.join("pip/held"), 40);
    let kept = h.file(&h.paths.cache.join("pip/deep/er/keep.me"), 40);
    let outside = h.file(&h.paths.home.join("precious/doc"), 400);
    let link = h.paths.cache.join("pip/link");
    std::os::unix::fs::symlink(&outside, &link).unwrap();
    let cfg = h.config("[rules.pip]\nexclude = [\"*.me\"]");

    let open: HashSet<PathBuf> = [held.clone()].into();
    let plans = app::plan(&cfg, later(), &open);
    assert!(!found(&plans).contains(&held), "open files are skipped");
    assert!(
        !found(&plans).contains(&kept),
        "a pattern without / matches at any depth"
    );

    app::apply(&h.paths, &cfg, &plans, &open, later()).unwrap();
    assert!(outside.exists(), "a symlink target is never moved");
    assert!(held.exists());
}

#[test]
fn a_file_opened_after_the_scan_is_left_alone() {
    let h = Home::new();
    let f = h.file(&h.paths.cache.join("pip/late"), 40);
    let cfg = h.config("");
    let plans = h.plan(&cfg);
    let opened_since: HashSet<PathBuf> = [f.clone()].into();
    let report = app::apply(&h.paths, &cfg, &plans, &opened_since, later()).unwrap();
    assert!(f.exists());
    assert_eq!(report.skipped, 1);
    assert!(report.manifest.is_none());
}

#[test]
fn unreadable_subfolder_does_not_stop_the_scan() {
    let h = Home::new();
    let locked = h.paths.cache.join("pip/root-owned");
    h.file(&locked.join("x"), 40);
    fs::set_permissions(&locked, fs::Permissions::from_mode(0o000)).unwrap();
    let old = h.file(&h.paths.cache.join("pip/a"), 40);
    let trash_old = h.trash("t", 1, 40).0;
    let plans = h.plan(&h.config(""));
    fs::set_permissions(&locked, fs::Permissions::from_mode(0o755)).unwrap();
    let all = found(&plans);
    assert!(all.contains(&old));
    assert!(all.contains(&trash_old), "other rules still run");
    assert!(plans.iter().all(|p| p.error.is_none()));
}

#[test]
fn entries_move_whole_only_when_everything_inside_is_old() {
    let h = Home::new();
    let yay = h.paths.cache.join("yay");
    h.file(&yay.join("old-pkg/PKGBUILD"), 60);
    h.file(&yay.join("old-pkg/src/main.c"), 60);
    h.file(&yay.join("busy-pkg/PKGBUILD"), 60);
    h.file(&yay.join("busy-pkg/src/new.c"), 1);
    for d in ["old-pkg/src", "old-pkg", "busy-pkg/src", "busy-pkg"] {
        age(&yay.join(d), 60);
    }
    let plans = found(&h.plan(&h.config("")));
    assert!(plans.contains(&yay.join("old-pkg")));
    assert!(!plans.iter().any(|p| p.starts_with(yay.join("busy-pkg"))));
}

#[test]
fn restore_puts_everything_back_byte_identical() {
    let h = Home::new();
    let a = h.file(&h.paths.cache.join("pip/deep/a"), 40);
    let b = h.file(&h.paths.cache.join("go-build/b"), 40);
    let before: Vec<_> = [&a, &b].iter().map(|p| fs::read(p).unwrap()).collect();
    let m = h.run(&h.config("")).manifest.unwrap();
    assert!(!a.exists() && !b.exists());
    assert!(
        !h.paths.cache.join("pip/deep").exists(),
        "emptied dirs are pruned"
    );

    let r = app::restore(&h.paths, &m, None).unwrap();
    assert_eq!(r.restored, 2);
    let after: Vec<_> = [&a, &b].iter().map(|p| fs::read(p).unwrap()).collect();
    assert_eq!(before, after);
    assert!(
        manifest::load_all(&h.paths.runs_dir())
            .unwrap()
            .runs
            .is_empty()
    );
}

#[test]
fn restore_by_path_only_touches_that_path() {
    let h = Home::new();
    let a = h.file(&h.paths.cache.join("pip/a"), 40);
    let b = h.file(&h.paths.cache.join("pip/b"), 40);
    let m = h.run(&h.config("")).manifest.unwrap();
    app::restore(&h.paths, &m, Some(&a)).unwrap();
    assert!(a.exists() && !b.exists());
    let left = manifest::load_all(&h.paths.runs_dir()).unwrap().runs;
    assert_eq!(left.len(), 1);
    assert_eq!(left[0].items.len(), 1);
}

#[test]
fn a_damaged_record_does_not_block_restoring_another_run() {
    let h = Home::new();
    let a = h.file(&h.paths.cache.join("pip/a"), 40);
    let m = h.run(&h.config("")).manifest.unwrap();
    fs::write(h.paths.runs_dir().join("20000101T000000Z.json"), "").unwrap();
    let loaded = manifest::load_all(&h.paths.runs_dir()).unwrap();
    assert_eq!(loaded.broken.len(), 1);
    let again = manifest::load_one(&h.paths.runs_dir(), &m.id)
        .unwrap()
        .unwrap();
    app::restore(&h.paths, &again, None).unwrap();
    assert!(a.exists());
}

#[test]
fn expired_runs_are_purged_and_fresh_ones_kept() {
    let h = Home::new();
    let a = h.file(&h.paths.cache.join("pip/a"), 40);
    let m = h.run(&h.config("")).manifest.unwrap();
    let quarantined = m.items[0].quarantined.clone();

    assert!(app::purge_expired(&h.paths, later()).unwrap().is_empty());
    assert!(quarantined.exists());

    let after_grace = later() + SignedDuration::from_hours(8 * 24);
    assert_eq!(app::purge_expired(&h.paths, after_grace).unwrap().len(), 1);
    assert!(!quarantined.exists() && !a.exists());
    assert!(
        manifest::load_all(&h.paths.runs_dir())
            .unwrap()
            .runs
            .is_empty()
    );
}

#[test]
fn nothing_old_means_no_run_record() {
    let h = Home::new();
    h.file(&h.paths.cache.join("pip/a"), 1);
    assert!(h.run(&h.config("")).manifest.is_none());
    assert!(
        manifest::load_all(&h.paths.runs_dir())
            .unwrap()
            .runs
            .is_empty()
    );
}

#[test]
fn enable_marker_round_trip() {
    let h = Home::new();
    assert!(!app::is_enabled(&h.paths));
    app::set_enabled(&h.paths, true).unwrap();
    assert!(app::is_enabled(&h.paths));
    app::set_enabled(&h.paths, false).unwrap();
    assert!(!app::is_enabled(&h.paths));
}

#[test]
fn shipped_example_config_is_valid() {
    let h = Home::new();
    let cfg = h.config(include_str!("../config.example.toml"));
    assert!(cfg.rules.iter().any(|r| r.name == "screenshots"));
}
