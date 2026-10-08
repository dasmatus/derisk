//! What the session does when nobody is using it: dim the screen, lock it
//! and suspend, after the minutes Settings › Power sets, and lock before
//! the machine sleeps whatever put it to sleep.
//!
//! Locking before sleep has to finish before the machine sleeps, or it
//! wakes showing the desktop until the lock screen's first frame. swayidle
//! holds logind's sleep delay inhibitor while its `before-sleep` command
//! runs (`-w`), and that command waits for the session to answer on
//! swayidle's standard input, which it inherits, that the lock screen is
//! up: [`SLEEP_DONE`], within [`SLEEP_WAIT_S`] seconds.
//!
//! The compositor knows when the seat was last used and which apps hold
//! the screen awake (a playing video, through `zwp_idle_inhibit_v1`), and
//! tells idle clients through `ext_idle_notifier_v1`. The session runs one
//! such client, swayidle, and has it print what happened instead of acting:
//! the session dims, locks and suspends itself, so the lock is its own lock
//! screen and a suspend goes through the same path as the palette's. While
//! an app inhibits idling the compositor reports no idle time at all, so
//! nothing here needs to know about inhibitors. swayidle also keeps
//! logind's idle hint, which logind's own `IdleAction=` reads.

use derisk_settings::Power;

/// What swayidle reported.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Event {
    /// Idle for the dimming time: dim the screen.
    Dim,
    /// Used again after dimming: undim it.
    Undim,
    /// Idle for the locking time.
    Lock,
    /// The machine is about to sleep: lock, then answer [`SLEEP_DONE`].
    Sleep,
    /// Idle for the suspending time.
    Suspend,
}

impl Event {
    /// The event a line of swayidle's output names, if any.
    pub fn parse(line: &str) -> Option<Self> {
        Some(match line.trim() {
            "dim" => Self::Dim,
            "undim" => Self::Undim,
            "lock" => Self::Lock,
            "sleep" => Self::Sleep,
            "suspend" => Self::Suspend,
            _ => return None,
        })
    }
}

/// What the session writes to swayidle's standard input once it locked
/// for [`Event::Sleep`].
pub const SLEEP_DONE: &[u8] = b"locked\n";

/// How long sleep waits for the lock, in seconds: a session too busy to
/// lock in that time should not keep the machine awake.
pub const SLEEP_WAIT_S: u32 = 5;

/// swayidle's command line for `power`: a timeout per setting that is not
/// zero ("never"), each echoing its [`Event`], a lock before sleep, and the
/// idle hint from the first timeout on.
pub fn argv(power: &Power) -> Vec<String> {
    let mut argv = vec!["swayidle".to_owned(), "-w".to_owned()];
    let mut timeout = |minutes: u16, event: &str, resume: Option<&str>| {
        if minutes == 0 {
            return;
        }
        argv.extend([
            "timeout".to_owned(),
            (u32::from(minutes) * 60).to_string(),
            format!("echo {event}"),
        ]);
        if let Some(resume) = resume {
            argv.extend(["resume".to_owned(), format!("echo {resume}")]);
        }
    };
    timeout(power.dim_after_min, "dim", Some("undim"));
    timeout(power.lock_after_min, "lock", None);
    timeout(power.suspend_after_min, "suspend", None);
    argv.extend([
        "before-sleep".to_owned(),
        format!("echo sleep; timeout {SLEEP_WAIT_S} head -n 1 >/dev/null"),
    ]);
    let first = [
        power.dim_after_min,
        power.lock_after_min,
        power.suspend_after_min,
    ]
    .into_iter()
    .filter(|m| *m > 0)
    .min();
    if let Some(minutes) = first {
        argv.extend(["idlehint".to_owned(), (u32::from(minutes) * 60).to_string()]);
    }
    argv
}

#[cfg(test)]
mod tests {
    use super::*;

    fn power(dim: u16, lock: u16, suspend: u16) -> Power {
        Power {
            dim_after_min: dim,
            lock_after_min: lock,
            suspend_after_min: suspend,
            ..derisk_settings::Settings::default().power
        }
    }

    #[test]
    fn every_setting_becomes_a_timeout() {
        assert_eq!(
            argv(&power(5, 10, 30)).join(" "),
            "swayidle -w timeout 300 echo dim resume echo undim timeout 600 echo lock \
             timeout 1800 echo suspend before-sleep echo sleep; timeout 5 head -n 1 >/dev/null idlehint 300"
        );
    }

    #[test]
    fn never_leaves_the_timeout_out() {
        assert_eq!(
            argv(&power(0, 15, 0)).join(" "),
            "swayidle -w timeout 900 echo lock before-sleep echo sleep; timeout 5 head -n 1 >/dev/null idlehint 900"
        );
        assert_eq!(
            argv(&power(0, 0, 0)).join(" "),
            "swayidle -w before-sleep echo sleep; timeout 5 head -n 1 >/dev/null"
        );
    }

    #[test]
    fn lines_parse() {
        assert_eq!(Event::parse("dim\n"), Some(Event::Dim));
        assert_eq!(Event::parse("undim"), Some(Event::Undim));
        assert_eq!(Event::parse("lock"), Some(Event::Lock));
        assert_eq!(Event::parse("sleep"), Some(Event::Sleep));
        assert_eq!(Event::parse("suspend"), Some(Event::Suspend));
        assert_eq!(Event::parse("swayidle: something"), None);
    }
}
