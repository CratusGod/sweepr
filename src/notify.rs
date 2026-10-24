//! Desktop notifications over D-Bus. Failing to notify never fails a run:
//! the same text always goes to stdout, which systemd keeps in the journal.

use notify_rust::{Notification, Timeout, Urgency};

/// How long a notification waits for a click before sweepr exits.
const WAIT_MS: u32 = 10 * 60 * 1000;

#[derive(Debug, PartialEq, Eq)]
pub enum Choice {
    Action(String),
    Dismissed,
}

pub struct Message<'a> {
    pub summary: &'a str,
    pub body: &'a str,
    pub critical: bool,
    /// `(id, label)` pairs shown as buttons.
    pub actions: &'a [(&'a str, &'a str)],
}

/// Show a notification; if it has buttons, wait for one to be clicked.
pub fn show(msg: &Message) -> Choice {
    println!("{}: {}", msg.summary, msg.body);
    let mut n = Notification::new();
    n.appname("sweepr")
        .summary(msg.summary)
        .body(msg.body)
        .icon("user-trash-full")
        .urgency(if msg.critical {
            Urgency::Critical
        } else {
            Urgency::Normal
        })
        .timeout(if msg.actions.is_empty() {
            Timeout::Default
        } else {
            Timeout::Milliseconds(WAIT_MS)
        });
    for (id, label) in msg.actions {
        n.action(id, label);
    }
    let handle = match n.show() {
        Ok(h) => h,
        Err(e) => {
            eprintln!("sweepr: could not show a notification: {e}");
            return Choice::Dismissed;
        }
    };
    if msg.actions.is_empty() {
        return Choice::Dismissed;
    }
    let mut choice = Choice::Dismissed;
    handle.wait_for_action(|action| {
        if action != "__closed" {
            choice = Choice::Action(action.to_string());
        }
    });
    choice
}
