//! Command-line definition, plus the man page and shell completions built from it.

use anyhow::{Context, Result};
use clap::{Command, CommandFactory, Parser, Subcommand, ValueEnum};
use clap_complete::Shell;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

/// Moves old caches and trash into a quarantine you can undo, then deletes
/// them for good after a grace period.
#[derive(Parser)]
#[command(name = "sweepr", version, about)]
pub struct Cli {
    #[command(subcommand)]
    pub cmd: Cmd,
}

#[derive(Subcommand)]
pub enum Cmd {
    /// Show what would be cleaned. Changes nothing.
    Scan {
        /// List every item, not just totals per rule
        #[arg(short, long)]
        verbose: bool,
    },
    /// Move old files into quarantine now
    Run {
        /// Used by the systemd timer: only a preview until `sweepr enable`
        #[arg(long)]
        scheduled: bool,
    },
    /// Put quarantined files back
    Restore {
        /// Run id (see `sweepr status`)
        run: Option<String>,
        /// Restore the most recent run
        #[arg(long, conflicts_with = "run")]
        last: bool,
        /// Restore only this path (or everything under it), from any run
        #[arg(long)]
        path: Option<PathBuf>,
    },
    /// Delete quarantined files for good
    Purge {
        /// Run id (see `sweepr status`)
        run: Option<String>,
        /// Only runs whose grace period has ended
        #[arg(long, conflicts_with_all = ["run", "all"])]
        expired: bool,
        /// Every run in quarantine
        #[arg(long, conflicts_with = "run")]
        all: bool,
    },
    /// Show quarantine contents, disk usage and whether scheduled runs are on
    Status,
    /// List rules and protected locations
    Rules,
    /// Hourly housekeeping: purge expired runs, warn when the disk is nearly full
    Tick,
    /// Let scheduled runs move files (they only preview until this is run)
    Enable,
    /// Make scheduled runs preview-only again
    Disable,
    /// Write the man pages or print a shell completion script (used when packaging)
    #[command(hide = true)]
    Generate {
        what: Generated,
        /// Folder for the man pages (one per command)
        #[arg(long, required_if_eq("what", "man"))]
        dir: Option<PathBuf>,
    },
}

#[derive(Clone, Copy, ValueEnum)]
pub enum Generated {
    Man,
    Bash,
    Zsh,
    Fish,
}

/// Sections clap cannot derive from the command definition.
const MAN_EXTRA: &str = r#".SH FILES
.TP
\fI~/.config/sweepr/config.toml\fR
Rule overrides and custom folders. Example: \fI/usr/share/doc/sweepr/config.example.toml\fR
.TP
\fI~/.local/share/sweepr/quarantine/\fR
Quarantined files, one folder per run. A folder on another filesystem uses
\fI<folder>/.sweepr-quarantine/\fR instead.
.TP
\fI~/.local/state/sweepr/runs/\fR
One record per run (\fI.json\fR, plus a readable \fI.txt\fR).
.SH TIMERS
\fBsweepr.timer\fR runs \fBsweepr run --scheduled\fR monthly.
\fBsweepr-tick.timer\fR runs \fBsweepr tick\fR hourly.
Logs: \fBjournalctl --user -u sweepr -u sweepr-tick\fR
.SH SEE ALSO
\fBsystemctl\fR(1), \fBsystemd.timer\fR(5)
"#;

/// Write `sweepr.1` and one `sweepr-<command>.1` per command into `dir`.
pub fn generate_man(dir: &Path) -> Result<()> {
    fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
    clap_mangen::generate_to(Cli::command(), dir)?;
    let main_page = dir.join("sweepr.1");
    fs::OpenOptions::new()
        .append(true)
        .open(&main_page)
        .and_then(|mut f| f.write_all(MAN_EXTRA.as_bytes()))
        .with_context(|| format!("writing {}", main_page.display()))
}

/// The command tree without hidden commands. clap_complete ignores `hide`
/// on subcommands, so they are dropped here instead.
fn public_command() -> Command {
    let full = Cli::command();
    let visible: Vec<Command> = full
        .get_subcommands()
        .filter(|c| !c.is_hide_set())
        .cloned()
        .collect();
    let mut cmd = Command::new("sweepr")
        .version(env!("CARGO_PKG_VERSION"))
        .subcommands(visible);
    if let Some(about) = full.get_about() {
        cmd = cmd.about(about.clone());
    }
    cmd
}

/// Print a completion script for `shell` to `out`.
pub fn generate_completion(what: Generated, out: &mut dyn Write) -> Result<()> {
    let mut cmd = public_command();
    let shell = match what {
        Generated::Man => anyhow::bail!("man pages are written with --dir"),
        Generated::Bash => Shell::Bash,
        Generated::Zsh => Shell::Zsh,
        Generated::Fish => Shell::Fish,
    };
    clap_complete::generate(shell, &mut cmd, "sweepr", out);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn render(what: Generated) -> String {
        let mut buf = Vec::new();
        generate_completion(what, &mut buf).unwrap();
        String::from_utf8(buf).unwrap()
    }

    #[test]
    fn cli_definition_is_consistent() {
        Cli::command().debug_assert();
    }

    #[test]
    fn man_pages_exist_for_every_linked_command_but_not_generate() {
        let dir = tempfile::tempdir().unwrap();
        generate_man(dir.path()).unwrap();
        let main = fs::read_to_string(dir.path().join("sweepr.1")).unwrap();
        assert!(main.contains(".SH FILES"));
        for name in [
            "scan", "run", "restore", "purge", "status", "rules", "enable",
        ] {
            assert!(
                main.contains(&format!("sweepr\\-{name}(1)")),
                "{name} not linked"
            );
            assert!(
                dir.path().join(format!("sweepr-{name}.1")).is_file(),
                "{name}"
            );
        }
        assert!(!dir.path().join("sweepr-generate.1").exists());
    }

    #[test]
    fn completions_cover_the_commands_but_not_generate() {
        for what in [Generated::Bash, Generated::Zsh, Generated::Fish] {
            let script = render(what);
            assert!(script.contains("restore"));
            assert!(!script.contains("generate"));
        }
    }
}
