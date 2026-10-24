//! User configuration: `~/.config/sweepr/config.toml`.
//!
//! Unknown keys are rejected so a typo fails loudly instead of being ignored.

use crate::paths::Paths;
use crate::rules::{self, Action, AgeSource, Protected, Rule, Unit};
use anyhow::{Context, Result, bail};
use serde::Deserialize;
use std::collections::BTreeMap;
use std::fs;

pub const DEFAULT_GRACE_DAYS: u32 = 7;
pub const DEFAULT_DISK_WARN_PERCENT: u8 = 90;

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawConfig {
    age_days: Option<u32>,
    grace_days: Option<u32>,
    disk_warn_percent: Option<u8>,
    #[serde(default)]
    rules: BTreeMap<String, RuleOverride>,
    #[serde(default)]
    custom: Vec<CustomRule>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RuleOverride {
    enabled: Option<bool>,
    age_days: Option<u32>,
    exclude: Option<Vec<String>>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct CustomRule {
    name: String,
    path: String,
    #[serde(default = "default_source")]
    age_source: AgeSource,
    #[serde(default = "default_unit")]
    unit: Unit,
    age_days: Option<u32>,
    #[serde(default)]
    exclude: Vec<String>,
    #[serde(default = "default_true")]
    enabled: bool,
    #[serde(default)]
    allow_protected: bool,
}

fn default_source() -> AgeSource {
    AgeSource::Mtime
}
fn default_unit() -> Unit {
    Unit::File
}
fn default_true() -> bool {
    true
}

#[derive(Clone, Debug)]
pub struct Config {
    pub grace_days: u32,
    pub disk_warn_percent: u8,
    pub rules: Vec<Rule>,
    pub protected: Vec<Protected>,
    /// sweepr's own state and quarantine; no rule can reach these.
    pub always_protected: Vec<Protected>,
}

impl Config {
    /// Load the config file if present, otherwise use built-in defaults.
    pub fn load(paths: &Paths) -> Result<Self> {
        let file = paths.config_file();
        let text = match fs::read_to_string(&file) {
            Ok(t) => t,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
            Err(e) => return Err(e).with_context(|| format!("reading {}", file.display())),
        };
        Self::from_toml(paths, &text).with_context(|| format!("in {}", file.display()))
    }

    pub fn from_toml(paths: &Paths, text: &str) -> Result<Self> {
        let raw: RawConfig = toml::from_str(text).context("invalid config")?;
        let default_age = raw.age_days.unwrap_or(rules::DEFAULT_AGE_DAYS);
        validate_days("age_days", default_age)?;
        let grace_days = raw.grace_days.unwrap_or(DEFAULT_GRACE_DAYS);
        validate_days("grace_days", grace_days)?;
        let disk_warn_percent = raw.disk_warn_percent.unwrap_or(DEFAULT_DISK_WARN_PERCENT);
        if !(1..=100).contains(&disk_warn_percent) {
            bail!("disk_warn_percent must be 1-100, got {disk_warn_percent}");
        }

        let mut builtins = rules::builtin_rules(paths);
        for name in raw.rules.keys() {
            if !builtins.iter().any(|r| &r.name == name) {
                bail!("[rules.{name}] does not match a built-in rule (see `sweepr rules`)");
            }
        }
        for rule in &mut builtins {
            if let Action::Files { age_days, .. } = &mut rule.action {
                *age_days = default_age;
            }
            if let Some(o) = raw.rules.get(&rule.name) {
                apply_override(rule, o)?;
            }
        }

        let mut all = builtins;
        for c in raw.custom {
            validate_name(&c.name)?;
            if all.iter().any(|r| r.name == c.name) {
                bail!("custom rule name `{}` is already used", c.name);
            }
            let root = paths.expand(&c.path)?;
            if paths.home.starts_with(&root) {
                bail!(
                    "custom rule `{}` would sweep your whole home folder or above it",
                    c.name
                );
            }
            let age = c.age_days.unwrap_or(default_age);
            validate_days(&format!("custom rule `{}` age_days", c.name), age)?;
            all.push(Rule {
                description: format!("custom: {}", c.path),
                name: c.name,
                enabled: c.enabled,
                action: Action::Files {
                    root,
                    age_source: c.age_source,
                    unit: c.unit,
                    age_days: age,
                    exclude: c.exclude,
                    allow_protected: c.allow_protected,
                },
            });
        }

        Ok(Self {
            grace_days,
            disk_warn_percent,
            rules: all,
            protected: rules::builtin_protected(paths),
            always_protected: rules::own_state(paths),
        })
    }
}

fn apply_override(rule: &mut Rule, o: &RuleOverride) -> Result<()> {
    if let Some(enabled) = o.enabled {
        rule.enabled = enabled;
    }
    match &mut rule.action {
        Action::Files {
            age_days, exclude, ..
        } => {
            if let Some(days) = o.age_days {
                validate_days(&format!("[rules.{}] age_days", rule.name), days)?;
                *age_days = days;
            }
            if let Some(ex) = &o.exclude {
                exclude.clone_from(ex);
            }
        }
        Action::Command { .. } => {
            if o.age_days.is_some() || o.exclude.is_some() {
                bail!(
                    "[rules.{}] runs a command; only `enabled` applies",
                    rule.name
                );
            }
        }
    }
    Ok(())
}

/// Rule names become folder names inside quarantine, so keep them plain.
fn validate_name(name: &str) -> Result<()> {
    let ok = !name.is_empty()
        && name.len() <= 64
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_');
    if !ok {
        bail!("rule name `{name}` may only use letters, digits, - and _");
    }
    Ok(())
}

fn validate_days(what: &str, days: u32) -> Result<()> {
    if days == 0 {
        bail!("{what} must be at least 1");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::{Path, PathBuf};

    fn paths() -> Paths {
        Paths::under(Path::new("/home/u"))
    }

    fn files_age(cfg: &Config, name: &str) -> u32 {
        match &cfg.rules.iter().find(|r| r.name == name).unwrap().action {
            Action::Files { age_days, .. } => *age_days,
            Action::Command { .. } => panic!("not a files rule"),
        }
    }

    #[test]
    fn empty_config_gives_defaults() {
        let cfg = Config::from_toml(&paths(), "").unwrap();
        assert_eq!(cfg.grace_days, 7);
        assert_eq!(cfg.disk_warn_percent, 90);
        assert_eq!(files_age(&cfg, "pip"), 30);
    }

    #[test]
    fn overrides_and_custom_rules_apply() {
        let text = r#"
            age_days = 45
            [rules.downloads]
            enabled = true
            age_days = 90
            [[custom]]
            name = "logs"
            path = "~/logs"
            unit = "entry"
        "#;
        let cfg = Config::from_toml(&paths(), text).unwrap();
        assert_eq!(files_age(&cfg, "pip"), 45);
        assert_eq!(files_age(&cfg, "downloads"), 90);
        let dl = cfg.rules.iter().find(|r| r.name == "downloads").unwrap();
        assert!(dl.enabled);
        let logs = cfg.rules.iter().find(|r| r.name == "logs").unwrap();
        assert!(
            matches!(&logs.action, Action::Files { root, unit: Unit::Entry, age_days: 45, .. }
            if root == &PathBuf::from("/home/u/logs"))
        );
    }

    #[test]
    fn typos_and_bad_values_fail_loudly() {
        let p = paths();
        assert!(Config::from_toml(&p, "agedays = 3").is_err());
        assert!(Config::from_toml(&p, "[rules.pipp]\nenabled = false").is_err());
        assert!(Config::from_toml(&p, "age_days = 0").is_err());
        assert!(Config::from_toml(&p, "disk_warn_percent = 0").is_err());
        assert!(Config::from_toml(&p, "[rules.uv]\nage_days = 3").is_err());
        let dup = "[[custom]]\nname = \"pip\"\npath = \"~/x\"";
        assert!(Config::from_toml(&p, dup).is_err());
        let rel = "[[custom]]\nname = \"x\"\npath = \"x\"";
        assert!(Config::from_toml(&p, rel).is_err());
        assert!(Config::from_toml(&p, "grace_days = 0").is_err());
        let traversal = "[[custom]]\nname = \"../x\"\npath = \"~/x\"";
        assert!(Config::from_toml(&p, traversal).is_err());
        let whole_home = "[[custom]]\nname = \"all\"\npath = \"~\"";
        assert!(Config::from_toml(&p, whole_home).is_err());
        let root = "[[custom]]\nname = \"all\"\npath = \"/\"";
        assert!(Config::from_toml(&p, root).is_err());
    }
}
