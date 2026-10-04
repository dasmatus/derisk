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

    const TEST_PASSWORD: &str = "secret";
    const WRONG_PASSWORD: &str = "wrong";
    const FIRST_PASSWORD: &str = "first";
    const SECOND_PASSWORD: &str = "second";
    const PARTIAL_PASSWORD: &str = "sec";

    fn locked(password: &str) -> LockScreen {
        let mut lock = LockScreen::default();
        lock.lock();
        lock.password = password.to_owned();
        lock
    }

    #[test]
    fn starts_unlocked_and_cannot_submit() {
        let mut lock = LockScreen {
            password: TEST_PASSWORD.into(),
            ..Default::default()
        };
        assert!(!lock.is_locked());
        assert_eq!(lock.submit(), None);
    }

    #[test]
    fn correct_password_unlocks() {
        let mut lock = locked(TEST_PASSWORD);
        assert_eq!(lock.submit().as_deref(), Some(TEST_PASSWORD));
        assert!(lock.is_checking());
        assert!(lock.password.is_empty());
        lock.finish(true);
        assert!(!lock.is_locked());
        assert!(!lock.is_checking());
    }

    #[test]
    fn wrong_password_stays_locked() {
        let mut lock = locked(WRONG_PASSWORD);
        lock.submit();
        lock.finish(false);
        assert!(lock.is_locked());
        assert_eq!(lock.failures(), 1);
        assert_eq!(lock.message(), "Wrong password, try again");
    }

    #[test]
    fn empty_password_is_not_checked() {
        let mut lock = locked("");
        assert_eq!(lock.submit(), None);
        assert!(!lock.is_checking());
    }

    #[test]
    fn one_check_at_a_time() {
        let mut lock = locked(FIRST_PASSWORD);
        lock.submit();
        lock.password = SECOND_PASSWORD.into();
        assert_eq!(lock.submit(), None);
    }

    #[test]
    fn a_result_without_a_check_does_not_unlock() {
        let mut lock = locked(TEST_PASSWORD);
        lock.finish(true);
        assert!(lock.is_locked());
    }

    #[test]
    fn locking_again_keeps_what_was_typed() {
        let mut lock = locked(PARTIAL_PASSWORD);
        lock.lock();
        assert_eq!(lock.password, PARTIAL_PASSWORD);
    }

    #[test]
    fn relocking_resets_failures() {
        let mut lock = locked(WRONG_PASSWORD);
        lock.submit();
        lock.finish(false);
        lock.password = TEST_PASSWORD.into();
        lock.submit();
        lock.finish(true);
        lock.lock();
        assert_eq!(lock.failures(), 0);
    }
}
