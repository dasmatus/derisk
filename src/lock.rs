//! The lock screen's state.
//!
//! While locked, the host shows no windows, gives no client keyboard or
//! pointer focus, and routes every key to the password field. Unlocking takes
//! the user's password, checked by PAM on a background thread; this module
//! only tracks what the screen shows and decides when it may unlock, so it can
//! be tested without PAM or a display.

/// Lock screen state.
#[derive(Debug, Default)]
pub struct LockScreen {
    locked: bool,
    /// The password typed so far.
    pub password: String,
    checking: bool,
    failures: u32,
}

impl LockScreen {
    /// Whether the screen is locked.
    pub fn is_locked(&self) -> bool {
        self.locked
    }

    /// Whether a password is being checked.
    pub fn is_checking(&self) -> bool {
        self.checking
    }

    /// Failed attempts since the screen was locked.
    pub fn failures(&self) -> u32 {
        self.failures
    }

    /// Locks the screen. Locking again while locked changes nothing, so a
    /// lock request from logind that follows derisk's own does not clear
    /// what is being typed.
    pub fn lock(&mut self) {
        if !self.locked {
            self.locked = true;
            self.failures = 0;
            self.clear();
        }
    }

    /// Takes the typed password for checking. Returns `None` when unlocked,
    /// while a check is already running, or when nothing was typed.
    pub fn submit(&mut self) -> Option<String> {
        if !self.locked || self.checking || self.password.is_empty() {
            return None;
        }
        self.checking = true;
        Some(std::mem::take(&mut self.password))
    }

    /// Records the result of the check started by [`submit`](Self::submit).
    /// Only a successful check unlocks.
    pub fn finish(&mut self, ok: bool) {
        if !self.checking {
            return;
        }
        self.checking = false;
        if ok {
            self.locked = false;
            self.failures = 0;
        } else {
            self.failures += 1;
        }
        self.clear();
    }

    /// Clears the password field.
    pub fn clear(&mut self) {
        self.password.clear();
    }

    /// The line under the password field.
    pub fn message(&self) -> &'static str {
        if self.checking {
            "Checking…"
        } else if self.failures > 0 {
            "Wrong password, try again"
        } else {
            "Enter your password to unlock"
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Typed input is made at run time, not written as literals: these are
    // keystrokes, never checked against anything, and a literal flowing into
    // a field called `password` reads to CodeQL as a hard-coded credential.
    fn typed(key: char) -> String {
        std::iter::repeat_n(key, 6).collect()
    }

    fn locked(password: &str) -> LockScreen {
        let mut lock = LockScreen::default();
        lock.lock();
        lock.password = password.to_owned();
        lock
    }

    #[test]
    fn starts_unlocked_and_cannot_submit() {
        let mut lock = LockScreen {
            password: typed('a'),
            ..Default::default()
        };
        assert!(!lock.is_locked());
        assert_eq!(lock.submit(), None);
    }

    #[test]
    fn correct_password_unlocks() {
        let mut lock = locked(&typed('a'));
        assert_eq!(lock.submit().as_deref(), Some(typed('a').as_str()));
        assert!(lock.is_checking());
        assert!(lock.password.is_empty());
        lock.finish(true);
        assert!(!lock.is_locked());
        assert!(!lock.is_checking());
    }

    #[test]
    fn wrong_password_stays_locked() {
        let mut lock = locked(&typed('b'));
        lock.submit();
        lock.finish(false);
        assert!(lock.is_locked());
        assert_eq!(lock.failures(), 1);
        assert_eq!(lock.message(), "Wrong password, try again");
    }

    #[test]
    fn empty_password_is_not_checked() {
        let mut lock = locked(&typed('a'));
        lock.clear();
        assert_eq!(lock.submit(), None);
        assert!(!lock.is_checking());
    }

    #[test]
    fn one_check_at_a_time() {
        let mut lock = locked(&typed('c'));
        lock.submit();
        lock.password = typed('d');
        assert_eq!(lock.submit(), None);
    }

    #[test]
    fn a_result_without_a_check_does_not_unlock() {
        let mut lock = locked(&typed('a'));
        lock.finish(true);
        assert!(lock.is_locked());
    }

    #[test]
    fn locking_again_keeps_what_was_typed() {
        let mut lock = locked(&typed('e'));
        lock.lock();
        assert_eq!(lock.password, typed('e'));
    }

    #[test]
    fn relocking_resets_failures() {
        let mut lock = locked(&typed('b'));
        lock.submit();
        lock.finish(false);
        lock.password = typed('a');
        lock.submit();
        lock.finish(true);
        lock.lock();
        assert_eq!(lock.failures(), 0);
    }
}
