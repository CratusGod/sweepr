mod cli;

use anyhow::Result;
use clap::Parser;
use cli::{Cli, Cmd};
use jiff::Timestamp;
use std::process::ExitCode;
use sweepr::config::Config;
use sweepr::paths::Paths;
use sweepr::{app, commands, notify, report, safety};

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
            let plans = commands::plan_with_meter(&cfg, now);
            print!("{}", report::scan_table(&plans, verbose));
            for (name, cmd) in app::planned_commands(&cfg) {
                println!("{name:<16} also runs `{cmd}` (deletes directly, not undoable)");
            }
            println!("\nNothing was changed. `sweepr run` moves these into quarantine.");
            Ok(())
        }
        Cmd::Run { scheduled } => commands::run(&paths, &cfg, now, scheduled, &notify::show),
        Cmd::Restore { run, last, path } => commands::restore(&paths, run, last, path),
        Cmd::Purge { run, expired, all } => commands::purge(&paths, now, run, expired, all),
        Cmd::Status => commands::status(&paths, &cfg),
        Cmd::Rules => {
            print!("{}", report::rules_table(&cfg));
            Ok(())
        }
        Cmd::Tick => commands::tick(&paths, &cfg, now, &notify::show),
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
