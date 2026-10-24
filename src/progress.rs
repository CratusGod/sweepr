//! A one-line progress meter on stderr. Silent when stderr is not a terminal
//! (systemd timers, pipes), so logs never fill with redraws.

use crate::app::Progress;
use std::io::{IsTerminal, Write};
use std::time::{Duration, Instant};

/// Redraw at most this often; thousands of tiny files move in well under a second.
const REDRAW_EVERY: Duration = Duration::from_millis(100);

pub struct Meter<W: Write> {
    out: Option<W>,
    last_draw: Option<Instant>,
}

impl Meter<std::io::Stderr> {
    pub fn stderr() -> Self {
        let err = std::io::stderr();
        let on = err.is_terminal();
        Self::new(on.then_some(err))
    }
}

impl<W: Write> Meter<W> {
    /// `None` makes a meter that draws nothing.
    pub fn new(out: Option<W>) -> Self {
        Self {
            out,
            last_draw: None,
        }
    }

    pub fn update(&mut self, p: Progress) {
        let Some(out) = self.out.as_mut() else {
            return;
        };
        // A new rule always shows; counters are throttled.
        let throttled = !matches!(p, Progress::Scanning { .. })
            && self.last_draw.is_some_and(|t| t.elapsed() < REDRAW_EVERY);
        if throttled {
            return;
        }
        self.last_draw = Some(Instant::now());
        let _ = write!(out, "\r\x1b[2K{}", line(p));
        let _ = out.flush();
    }

    /// Erase the meter so normal output starts on a clean line.
    pub fn finish(&mut self) {
        if let Some(out) = self.out.as_mut().filter(|_| self.last_draw.is_some()) {
            let _ = write!(out, "\r\x1b[2K");
            let _ = out.flush();
        }
        self.last_draw = None;
    }
}

fn line(p: Progress) -> String {
    match p {
        Progress::Scanning { rule } => format!("Scanning {rule}..."),
        Progress::Checking { done, total } => format!("Checking {done}/{total}..."),
        Progress::Moving { done, total } => format!("Moving {done}/{total}..."),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn draws_each_rule_throttles_counters_and_clears() {
        let mut m = Meter::new(Some(Vec::new()));
        m.update(Progress::Scanning { rule: "trash" });
        m.update(Progress::Moving { done: 1, total: 9 }); // within 100 ms: skipped
        m.update(Progress::Scanning { rule: "npm" });
        m.finish();
        let out = String::from_utf8(m.out.take().unwrap()).unwrap();
        assert!(out.contains("Scanning trash..."));
        assert!(out.contains("Scanning npm..."));
        assert!(!out.contains("Moving"));
        assert!(out.ends_with("\r\x1b[2K"));
    }

    #[test]
    fn a_meter_without_a_terminal_writes_nothing() {
        let mut m: Meter<Vec<u8>> = Meter::new(None);
        m.update(Progress::Scanning { rule: "trash" });
        m.finish();
        assert!(m.out.is_none());
    }
}
