//! The real `sweepr` binary in a throwaway home. Nothing here is old enough
//! to move (the binary uses the real clock); `commands.rs` covers moving.

mod common;

use common::NO_COMMANDS;
use std::fs;
use std::process::{Command, Output};

struct Sandbox {
    home: tempfile::TempDir,
}

impl Sandbox {
    fn new() -> Self {
        let home = tempfile::tempdir().unwrap();
        let config = home.path().join(".config/sweepr/config.toml");
        fs::create_dir_all(config.parent().unwrap()).unwrap();
        fs::write(&config, NO_COMMANDS).unwrap();
        Self { home }
    }

    fn sweepr(&self, args: &[&str]) -> Output {
        Command::new(env!("CARGO_BIN_EXE_sweepr"))
            .args(args)
            .env("HOME", self.home.path())
            .env_remove("XDG_CONFIG_HOME")
            .env_remove("XDG_CACHE_HOME")
            .env_remove("XDG_DATA_HOME")
            .env_remove("XDG_STATE_HOME")
            // Never reach the developer's real desktop.
            .env("DBUS_SESSION_BUS_ADDRESS", "unix:path=/nonexistent")
            .output()
            .unwrap()
    }

    /// Run, require success, return stdout.
    fn ok(&self, args: &[&str]) -> String {
        let out = self.sweepr(args);
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert!(out.status.success(), "sweepr {args:?} failed: {stderr}");
        String::from_utf8(out.stdout).unwrap()
    }
}

#[test]
fn scan_changes_nothing_and_lists_every_rule() {
    let s = Sandbox::new();
    let out = s.ok(&["scan", "-v"]);
    assert!(out.contains("trash") && out.contains("TOTAL"));
    assert!(out.contains("Nothing was changed"));
    assert!(!s.home.path().join(".local/state/sweepr/runs").exists());
}

#[test]
fn run_with_nothing_old_says_so() {
    let s = Sandbox::new();
    assert!(s.ok(&["run"]).contains("Nothing old enough"));
    assert!(s.ok(&["run", "--scheduled"]).is_empty());
}

#[test]
fn enable_disable_and_status() {
    let s = Sandbox::new();
    assert!(s.ok(&["status"]).contains("preview only"));
    s.ok(&["enable"]);
    assert!(s.ok(&["status"]).contains("Scheduled runs: on"));
    s.ok(&["disable"]);
    assert!(s.ok(&["status"]).contains("Quarantine:     empty"));
}

#[test]
fn rules_lists_protected_folders_with_reasons() {
    let out = Sandbox::new().ok(&["rules"]);
    assert!(out.contains("[on ] trash") && out.contains("[off] uv"));
    assert!(out.contains("nvidia"));
}

#[test]
fn tick_and_purge_on_an_empty_quarantine() {
    let s = Sandbox::new();
    s.ok(&["tick"]);
    assert!(s.ok(&["purge", "--expired"]).contains("Purged 0"));
}

#[test]
fn mistakes_fail_loudly() {
    let s = Sandbox::new();
    let out = s.sweepr(&["restore"]);
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("say what to restore"));

    let config = s.home.path().join(".config/sweepr/config.toml");
    fs::write(&config, "age_dayz = 3\n").unwrap();
    let out = s.sweepr(&["scan"]);
    assert!(
        !out.status.success(),
        "a typo in the config must not be ignored"
    );
}

#[test]
fn generate_writes_man_pages_and_completions() {
    let s = Sandbox::new();
    let dir = s.home.path().join("man");
    s.ok(&["generate", "man", "--dir", dir.to_str().unwrap()]);
    assert!(dir.join("sweepr.1").is_file());
    assert!(s.ok(&["generate", "zsh"]).contains("#compdef sweepr"));
}
