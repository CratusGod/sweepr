//! Where sweepr reads config and keeps state, following the XDG base directory spec.

use anyhow::{Context, Result, bail};
use std::env;
use std::path::{Path, PathBuf};

#[derive(Clone, Debug)]
pub struct Paths {
    pub home: PathBuf,
    pub config: PathBuf,
    pub cache: PathBuf,
    pub data: PathBuf,
    pub state: PathBuf,
}

impl Paths {
    /// Resolve from the environment, failing loudly if HOME is unusable.
    pub fn from_env() -> Result<Self> {
        let home = env::var_os("HOME")
            .map(PathBuf::from)
            .context("HOME is not set")?;
        if !home.is_absolute() {
            bail!("HOME must be an absolute path, got {}", home.display());
        }
        let xdg = |var: &str, fallback: &str| -> PathBuf {
            env::var_os(var)
                .map(PathBuf::from)
                .filter(|p| p.is_absolute())
                .unwrap_or_else(|| home.join(fallback))
        };
        Ok(Self {
            config: xdg("XDG_CONFIG_HOME", ".config"),
            cache: xdg("XDG_CACHE_HOME", ".cache"),
            data: xdg("XDG_DATA_HOME", ".local/share"),
            state: xdg("XDG_STATE_HOME", ".local/state"),
            home,
        })
    }

    /// Standard layout under an arbitrary home; used by tests.
    pub fn under(home: &Path) -> Self {
        Self {
            config: home.join(".config"),
            cache: home.join(".cache"),
            data: home.join(".local/share"),
            state: home.join(".local/state"),
            home: home.to_path_buf(),
        }
    }

    pub fn config_file(&self) -> PathBuf {
        self.config.join("sweepr/config.toml")
    }

    pub fn state_dir(&self) -> PathBuf {
        self.state.join("sweepr")
    }

    pub fn runs_dir(&self) -> PathBuf {
        self.state_dir().join("runs")
    }

    pub fn default_quarantine(&self) -> PathBuf {
        self.data.join("sweepr/quarantine")
    }

    /// Expand a leading `~/` to the home directory. Other paths must be absolute.
    pub fn expand(&self, raw: &str) -> Result<PathBuf> {
        let path = match raw.strip_prefix("~/") {
            Some(rest) => self.home.join(rest),
            None if raw == "~" => self.home.clone(),
            None => PathBuf::from(raw),
        };
        if !path.is_absolute() {
            bail!("rule path must be absolute or start with ~/: {raw}");
        }
        Ok(path)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn expands_tilde_and_rejects_relative() {
        let p = Paths::under(Path::new("/home/u"));
        assert_eq!(p.expand("~/x/y").unwrap(), PathBuf::from("/home/u/x/y"));
        assert_eq!(p.expand("/abs").unwrap(), PathBuf::from("/abs"));
        assert!(p.expand("relative/path").is_err());
    }
}
