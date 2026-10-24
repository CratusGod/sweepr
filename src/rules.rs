//! Built-in cleaning rules and protected locations.
//!
//! Each rule says *how* age is measured, because "older than 30 days" means
//! different things in different places: a trashed file's own mtime says
//! nothing about when it was trashed, and a shader cache that is still in
//! use never gets its mtime refreshed.

use crate::paths::Paths;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

pub const DEFAULT_AGE_DAYS: u32 = 30;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AgeSource {
    /// Last modification time of the file (or newest file inside an entry).
    Mtime,
    /// `DeletionDate` from the FreeDesktop `.trashinfo` file.
    Trashinfo,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Unit {
    /// Move individual files that are old enough.
    File,
    /// Move whole top-level entries whose newest file is old enough.
    /// Used where half a directory is worse than none (e.g. git clones).
    Entry,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Action {
    Files {
        root: PathBuf,
        age_source: AgeSource,
        unit: Unit,
        age_days: u32,
        exclude: Vec<String>,
        allow_protected: bool,
    },
    /// A tool's own pruner. Runs only if the program is installed.
    Command { program: String, args: Vec<String> },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Rule {
    pub name: String,
    pub description: String,
    pub enabled: bool,
    pub action: Action,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Protected {
    pub path: PathBuf,
    pub reason: &'static str,
}

impl Protected {
    pub fn covers(&self, path: &Path) -> bool {
        path.starts_with(&self.path)
    }
}

fn files(name: &str, desc: &str, root: PathBuf, source: AgeSource, unit: Unit) -> Rule {
    Rule {
        name: name.into(),
        description: desc.into(),
        enabled: true,
        action: Action::Files {
            root,
            age_source: source,
            unit,
            age_days: DEFAULT_AGE_DAYS,
            exclude: Vec::new(),
            allow_protected: false,
        },
    }
}

fn command(name: &str, desc: &str, program: &str, args: &[&str]) -> Rule {
    Rule {
        name: name.into(),
        description: desc.into(),
        enabled: true,
        action: Action::Command {
            program: program.into(),
            args: args.iter().map(|a| a.to_string()).collect(),
        },
    }
}

pub fn builtin_rules(p: &Paths) -> Vec<Rule> {
    use AgeSource::{Mtime, Trashinfo};
    use Unit::{Entry, File};
    let mut downloads = files(
        "downloads",
        "Downloads folder (off by default)",
        p.home.join("Downloads"),
        Mtime,
        Entry,
    );
    downloads.enabled = false;

    vec![
        files(
            "trash",
            "Trash, aged by when each item was trashed",
            p.data.join("Trash"),
            Trashinfo,
            Entry,
        ),
        files(
            "thumbnails",
            "File manager thumbnails",
            p.cache.join("thumbnails"),
            Mtime,
            File,
        ),
        files(
            "pip",
            "pip download cache",
            p.cache.join("pip"),
            Mtime,
            File,
        ),
        files(
            "go-build",
            "Go build cache",
            p.cache.join("go-build"),
            Mtime,
            File,
        ),
        files(
            "electron",
            "Downloaded Electron builds",
            p.cache.join("electron"),
            Mtime,
            Entry,
        ),
        files(
            "node-gyp",
            "node-gyp header downloads",
            p.cache.join("node-gyp"),
            Mtime,
            Entry,
        ),
        files(
            "npm",
            "npm package cache",
            p.home.join(".npm/_cacache"),
            Mtime,
            File,
        ),
        files(
            "yarn",
            "Yarn package cache",
            p.cache.join("yarn"),
            Mtime,
            File,
        ),
        files(
            "yay",
            "yay AUR build directories",
            p.cache.join("yay"),
            Mtime,
            Entry,
        ),
        files(
            "paru",
            "paru AUR build directories",
            p.cache.join("paru/clone"),
            Mtime,
            Entry,
        ),
        files(
            "cargo-registry",
            "Downloaded .crate archives",
            p.home.join(".cargo/registry/cache"),
            Mtime,
            File,
        ),
        downloads,
        command(
            "uv",
            "uv cache, via `uv cache prune`",
            "uv",
            &["cache", "prune"],
        ),
        command(
            "pnpm",
            "pnpm store, via `pnpm store prune`",
            "pnpm",
            &["store", "prune"],
        ),
    ]
}

const SHADER: &str = "shader cache: the GPU driver sizes it itself; deleting by age makes games recompile shaders and stutter";
const BROWSER: &str = "browser profile or cache: cleaning it can log you out or lose history";

const OWN: &str = "sweepr's own records and quarantine";

/// Never reachable, not even with `allow_protected`: sweeping sweepr's own
/// quarantine would strand files where no run record can find them.
pub fn own_state(p: &Paths) -> Vec<Protected> {
    vec![
        Protected {
            path: p.state_dir(),
            reason: OWN,
        },
        Protected {
            path: p.data.join("sweepr"),
            reason: OWN,
        },
        Protected {
            path: p.config.join("sweepr"),
            reason: OWN,
        },
    ]
}

pub fn builtin_protected(p: &Paths) -> Vec<Protected> {
    let entry = |path: PathBuf, reason| Protected { path, reason };
    vec![
        entry(p.cache.join("nvidia"), SHADER),
        entry(p.cache.join("mesa_shader_cache"), SHADER),
        entry(p.cache.join("mesa_shader_cache_db"), SHADER),
        entry(p.cache.join("radv_builtin_shaders"), SHADER),
        entry(
            p.data.join("Steam"),
            "Steam library and its per-game shader caches",
        ),
        entry(p.home.join(".steam"), "Steam runtime and library links"),
        entry(p.home.join(".mozilla"), BROWSER),
        entry(p.cache.join("mozilla"), BROWSER),
        entry(p.home.join(".zen"), BROWSER),
        entry(p.cache.join("zen"), BROWSER),
        entry(p.config.join("chromium"), BROWSER),
        entry(p.cache.join("chromium"), BROWSER),
        entry(p.config.join("google-chrome"), BROWSER),
        entry(p.cache.join("google-chrome"), BROWSER),
        entry(p.config.join("BraveSoftware"), BROWSER),
        entry(p.cache.join("BraveSoftware"), BROWSER),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builtin_names_are_unique() {
        let p = Paths::under(Path::new("/home/u"));
        let rules = builtin_rules(&p);
        let mut names: Vec<_> = rules.iter().map(|r| r.name.as_str()).collect();
        names.sort_unstable();
        names.dedup();
        assert_eq!(names.len(), rules.len());
    }

    #[test]
    fn trash_uses_trashinfo_and_downloads_is_off() {
        let p = Paths::under(Path::new("/home/u"));
        let rules = builtin_rules(&p);
        let trash = rules.iter().find(|r| r.name == "trash").unwrap();
        assert!(matches!(
            trash.action,
            Action::Files {
                age_source: AgeSource::Trashinfo,
                ..
            }
        ));
        let dl = rules.iter().find(|r| r.name == "downloads").unwrap();
        assert!(!dl.enabled);
    }

    #[test]
    fn nvidia_cache_is_protected_and_no_builtin_rule_targets_it() {
        let p = Paths::under(Path::new("/home/u"));
        let protected = builtin_protected(&p);
        let nvidia = p.cache.join("nvidia/GLCache/x.bin");
        assert!(protected.iter().any(|pr| pr.covers(&nvidia)));
        for rule in builtin_rules(&p) {
            if let Action::Files { root, .. } = &rule.action {
                assert!(
                    !protected.iter().any(|pr| pr.covers(root)),
                    "{} targets a protected path",
                    rule.name
                );
            }
        }
    }
}
