//! The lock screen's state.
//!
//! While locked, the host shows no windows, gives no client keyboard or
//! pointer focus, and routes every key to the lock screen's field. Unlocking
//! is a PAM conversation, the same one a login holds ([`Login::unlock`]), so
//! whatever the system's stack asks for is asked here too: the password, a
//! security key's PIN and touch, a new password once the old one expired.
//! The host runs it in a `derisk auth` process and, when the system offers a
//! fingerprint reader, a second conversation for the reader beside it, the
//! way GDM does: the person can touch the reader or type, and whichever
//! conversation PAM accepts first unlocks.
//!
//! This module only tracks the two conversations and decides when the screen
//! may unlock, so it can be tested without PAM or a display.

use crate::greetd::{Login, Notice, Phase, Request};

/// Lock screen state.
#[derive(Debug)]
pub struct LockScreen {
    locked: bool,
    /// The conversation the field answers.
    pub login: Login,
    /// The fingerprint reader's conversation, while the system has one.
    pub fingerprint: Option<Login>,
    /// The reader asked for a finger since the screen locked, so it is there
    /// to ask again once it times out.
    reader_listened: bool,
}

impl Default for LockScreen {
    fn default() -> Self {
        Self {
            locked: false,
            login: Login::unlock(String::new()),
            fingerprint: None,
            reader_listened: false,
        }
    }
}

/// The first requests of a lock: the field's conversation's, and the
/// reader's when there is one.
pub type Start = (Request, Option<Request>);

impl LockScreen {
    /// Whether the screen is locked.
    pub fn is_locked(&self) -> bool {
        self.locked
    }

    /// Locks the screen for `user`, with a fingerprint conversation beside
    /// the field's when `fingerprint`. Returns the first request of each, to
    /// start them; `None` when it was locked already, so a lock request from
    /// logind that follows derisk's own does not clear what is being typed.
    pub fn lock(&mut self, user: &str, fingerprint: bool) -> Option<Start> {
        if self.locked {
            return None;
        }
        self.locked = true;
        self.reader_listened = false;
        self.login = Login::unlock(user.to_owned());
        self.fingerprint = fingerprint.then(|| Login::unlock(user.to_owned()));
        let first = self.login.submit()?;
        let reader = self.fingerprint.as_mut().and_then(Login::submit);
        Some((first, reader))
    }

    /// Unlocks if either conversation was accepted. Returns `true` the one
    /// time it unlocks; the host then ends both conversations.
    pub fn poll(&mut self) -> bool {
        if !self.locked {
            return false;
        }
        if self.fingerprint_notice().is_some() {
            self.reader_listened = true;
        }
        let accepted = |login: &Login| *login.phase() == Phase::Started;
        if accepted(&self.login) || self.fingerprint.as_ref().is_some_and(accepted) {
            self.locked = false;
            self.fingerprint = None;
            self.login = Login::unlock(String::new());
            return true;
        }
        false
    }

    /// What the reader is saying while it waits for a finger, such as
    /// "Place your finger on the fingerprint reader".
    pub fn fingerprint_notice(&self) -> Option<&Notice> {
        let reader = self.fingerprint.as_ref()?;
        let notice = reader.notice()?;
        (*reader.phase() == Phase::Waiting && !notice.error).then_some(notice)
    }

    /// Asks the reader again after its conversation stopped (pam_fprintd
    /// gives up after a while with no finger), when the person does
    /// something on the lock screen. Only a reader that asked for a finger
    /// before is woken, so a machine whose reader is gone does not start a
    /// conversation on every key.
    pub fn wake_fingerprint(&mut self) -> Option<Request> {
        if !self.locked || !self.reader_listened {
            return None;
        }
        let reader = self.fingerprint.as_mut()?;
        (*reader.phase() == Phase::User)
            .then(|| reader.submit())
            .flatten()
    }

    /// The line under the field and whether it reports a failure: the
    /// field's own notice, else the reader's, else what to do.
    pub fn message(&self) -> (&str, bool) {
        match (self.login.notice(), self.fingerprint_notice()) {
            (Some(notice), _) if notice.error => (&notice.text, true),
            (_, Some(reader)) if self.login.editable() => (&reader.text, false),
            (Some(notice), _) => (&notice.text, false),
            (None, _) => (self.login.status(), false),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::greetd::{AuthMessageType, Response};

    // Typed input is made at run time, not written as literals: these are
    // keystrokes, never checked against anything, and a literal flowing into
    // a field called `password` reads to CodeQL as a hard-coded credential.
    fn typed(key: char) -> String {
        std::iter::repeat_n(key, 6).collect()
    }

    fn message(kind: AuthMessageType, text: &str) -> Response {
        Response::AuthMessage {
            auth_message_type: kind,
            auth_message: text.into(),
        }
    }

    fn locked(fingerprint: bool) -> LockScreen {
        let mut lock = LockScreen::default();
        let (first, reader) = lock.lock("alice", fingerprint).unwrap();
        assert_eq!(
            first,
            Request::CreateSession {
                username: "alice".into()
            }
        );
        assert_eq!(reader.is_some(), fingerprint);
        lock.login
            .respond(message(AuthMessageType::Secret, "Password: "));
        lock
    }

    #[test]
    fn starts_unlocked() {
        let mut lock = LockScreen::default();
        assert!(!lock.is_locked());
        assert!(!lock.poll());
        assert_eq!(lock.wake_fingerprint(), None);
    }

    #[test]
    fn the_right_password_unlocks() {
        let mut lock = locked(false);
        assert_eq!(lock.message(), ("Enter your password to unlock", false));
        lock.login.answer = typed('a');
        assert!(lock.login.submit().is_some());
        assert!(!lock.poll());
        assert_eq!(lock.login.respond(Response::Success), None);
        assert!(lock.poll());
        assert!(!lock.is_locked());
        assert!(lock.login.answer.is_empty());
    }

    #[test]
    fn a_wrong_password_stays_locked_and_asks_again() {
        let mut lock = locked(false);
        lock.login.answer = typed('b');
        lock.login.submit();
        let refused = Response::Error {
            error_type: "auth_error".into(),
            description: "pam_authenticate: AUTH_ERR".into(),
        };
        assert_eq!(lock.login.respond(refused), Some(Request::CancelSession));
        assert!(!lock.poll());
        assert!(lock.is_locked());
        assert_eq!(lock.message(), ("Wrong password, try again", true));
    }

    #[test]
    fn locking_again_keeps_what_was_typed() {
        let mut lock = locked(false);
        lock.login.answer = typed('e');
        assert_eq!(lock.lock("alice", false), None);
        assert_eq!(lock.login.answer, typed('e'));
    }

    #[test]
    fn a_finger_unlocks_while_the_field_waits() {
        let mut lock = locked(true);
        let reader = lock.fingerprint.as_mut().unwrap();
        assert_eq!(
            reader.respond(message(
                AuthMessageType::Info,
                "Place your finger on the fingerprint reader"
            )),
            Some(Request::PostAuthMessageResponse { response: None })
        );
        assert!(!lock.poll());
        assert_eq!(
            lock.message(),
            ("Place your finger on the fingerprint reader", false)
        );
        lock.login.answer = typed('a');
        lock.fingerprint
            .as_mut()
            .unwrap()
            .respond(Response::Success);
        assert!(lock.poll());
        assert!(!lock.is_locked());
        assert!(lock.fingerprint.is_none());
    }

    #[test]
    fn a_reader_that_timed_out_is_woken_by_a_key() {
        let mut lock = locked(true);
        let reader = lock.fingerprint.as_mut().unwrap();
        reader.respond(message(AuthMessageType::Info, "Place your finger"));
        lock.poll();
        let reader = lock.fingerprint.as_mut().unwrap();
        let timeout = Response::Error {
            error_type: "error".into(),
            description: "pam_authenticate: AUTHINFO_UNAVAIL".into(),
        };
        assert_eq!(reader.respond(timeout), Some(Request::CancelSession));
        assert_eq!(reader.respond(Response::Success), None);
        // The reader's failure is not the field's: nothing red appears.
        assert_eq!(lock.message(), ("Enter your password to unlock", false));
        assert_eq!(
            lock.wake_fingerprint(),
            Some(Request::CreateSession {
                username: "alice".into()
            })
        );
        // Already asking: a second key does not start another round.
        assert_eq!(lock.wake_fingerprint(), None);
    }

    #[test]
    fn a_missing_reader_is_left_alone() {
        let mut lock = locked(true);
        let reader = lock.fingerprint.as_mut().unwrap();
        let none = Response::Error {
            error_type: "error".into(),
            description: "no devices available".into(),
        };
        reader.respond(none);
        reader.respond(Response::Success);
        lock.poll();
        assert_eq!(lock.wake_fingerprint(), None);
    }
}
