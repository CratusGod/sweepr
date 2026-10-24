//! Human-readable output: tables for the terminal and the per-run details file.

use crate::app::RulePlan;
use crate::config::Config;
use crate::disk::human;
use crate::manifest::Manifest;
use crate::rules::{Action, AgeSource, Unit};
use std::fmt::Write as _;

pub fn scan_table(plans: &[RulePlan], verbose: bool) -> String {
    let mut out = String::new();
    let _ = writeln!(out, "{:<16} {:>8} {:>10}", "RULE", "ITEMS", "SIZE");
    for p in plans {
        let _ = writeln!(
            out,
            "{:<16} {:>8} {:>10}",
            p.name,
            p.candidates.len(),
            human(p.bytes())
        );
        if let Some(err) = &p.error {
            let _ = writeln!(out, "    skipped: {err}");
        }
        if verbose {
            for c in &p.candidates {
                let _ = writeln!(out, "    {}  ({})", c.path.display(), human(c.size));
            }
        }
    }
    let items: usize = plans.iter().map(|p| p.candidates.len()).sum();
    let bytes: u64 = plans.iter().map(RulePlan::bytes).sum();
    let _ = writeln!(out, "{:<16} {:>8} {:>10}", "TOTAL", items, human(bytes));
    out
}

pub fn rules_table(cfg: &Config) -> String {
    let mut out = String::from("Rules:\n");
    for r in &cfg.rules {
        let state = if r.enabled { "on " } else { "off" };
        let how = match &r.action {
            Action::Files {
                root,
                age_source,
                unit,
                age_days,
                ..
            } => {
                let age = match age_source {
                    AgeSource::Mtime => "modified",
                    AgeSource::Trashinfo => "trashed",
                };
                let unit = match unit {
                    Unit::File => "files",
                    Unit::Entry => "entries",
                };
                format!("{unit} {age} > {age_days}d in {}", root.display())
            }
            Action::Command { program, args } => format!("runs `{program} {}`", args.join(" ")),
        };
        let _ = writeln!(out, "  [{state}] {:<15} {how}", r.name);
    }
    out.push_str("\nNever touched (add a custom rule with allow_protected = true to override):\n");
    for p in &cfg.protected {
        let _ = writeln!(out, "  {}  - {}", p.path.display(), p.reason);
    }
    out
}

pub fn run_details(m: &Manifest) -> String {
    let mut out = format!(
        "sweepr run {}\nMoved {} items ({}) into quarantine.\nDeleted for good after {}.\n\
         Undo with: sweepr restore {}\n\n",
        m.id,
        m.items.len(),
        human(m.total_size()),
        m.expires.strftime("%Y-%m-%d %H:%M UTC"),
        m.id
    );
    for item in &m.items {
        let _ = writeln!(
            out,
            "[{}] {}  ({})",
            item.rule,
            item.original.display(),
            human(item.size)
        );
    }
    out
}

pub fn quarantine_line(m: &Manifest) -> String {
    format!(
        "  {}  {:>10}  {:>5} items  deleted after {}",
        m.id,
        human(m.total_size()),
        m.items.len(),
        m.expires.strftime("%Y-%m-%d")
    )
}
