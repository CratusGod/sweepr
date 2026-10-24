//! Disk usage, computed the way `df` reports it.

use anyhow::{Context, Result};
use std::path::Path;

/// Used space as a whole percent, rounded up like `df` does.
pub fn usage_percent(path: &Path) -> Result<u8> {
    let s = rustix::fs::statvfs(path).with_context(|| format!("statvfs {}", path.display()))?;
    Ok(percent(s.f_blocks, s.f_bfree, s.f_bavail))
}

/// `df`'s formula: used / (used + available to unprivileged users).
pub fn percent(blocks: u64, free: u64, avail: u64) -> u8 {
    let used = blocks.saturating_sub(free);
    let usable = used + avail;
    if usable == 0 {
        return 0;
    }
    let pct = (used * 100).div_ceil(usable);
    u8::try_from(pct.min(100)).unwrap_or(100)
}

pub fn human(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit < UNITS.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{bytes} B")
    } else {
        format!("{value:.1} {}", UNITS[unit])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn matches_df_on_this_machines_numbers() {
        // 777.5 GiB fs, 89% reported by df on 2026-10-23
        assert_eq!(percent(1000, 110, 110), 89);
        assert_eq!(percent(1000, 100, 50), 95);
        assert_eq!(percent(0, 0, 0), 0);
        assert_eq!(percent(100, 0, 0), 100);
    }

    #[test]
    fn human_sizes() {
        assert_eq!(human(512), "512 B");
        assert_eq!(human(1536), "1.5 KB");
        assert_eq!(human(3 * 1024 * 1024 * 1024), "3.0 GB");
    }

    #[test]
    fn reads_a_real_filesystem() {
        let pct = usage_percent(Path::new("/")).unwrap();
        assert!(pct <= 100);
    }
}
