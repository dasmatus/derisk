//! What derisk's polkit authentication dialog shows, and the helper's
//! protocol it is fed from.
//!
//! polkit asks the session's agent to authenticate someone before it allows
//! an action: run0, an app installing for every user, enrolling a
//! fingerprint. The agent never checks a password itself. It connects to
//! `polkit-agent-helper-1` (socket-activated as root), names the user and
//! the request's cookie, and relays what PAM's `polkit-1` stack asks; the
//! helper answers polkitd itself. So the dialog asks whatever that stack
//! asks, in order: the fingerprint reader first when the person enrolled a
//! finger, then a password or a security key's PIN, a verification code
//! where the stack has one, and a new password when the old one expired.
//!
//! This is the dialog's state, kept free of D-Bus, sockets and drawing so it
//! is tested on its own; `derisk session` holds the conversation and draws
//! it.

use crate::greetd::{Ask, Notice};

/// One line from the helper, unescaped.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum HelperLine {
    /// PAM asks for something: hidden as typed unless `echo`.
    Prompt {
        /// PAM's prompt, such as `Password: `.
        text: String,
        /// Whether what is typed may be shown.
        echo: bool,
    },
    /// Something went wrong, in PAM's words.
    Error(String),
    /// Something to tell the person, such as the reader listening.
    Info(String),
    /// polkitd has been told the person authenticated.
    Success,
    /// PAM refused.
    Failure,
}

impl HelperLine {
    /// Parses one line the helper wrote, the newline already removed. The
    /// helper escapes each message as `g_strescape` does; anything it never
    /// writes is `None`.
    pub fn parse(line: &str) -> Option<Self> {
        let line = compress(line);
        let message = |prefix: &str| line.strip_prefix(prefix).map(str::to_owned);
        if let Some(text) = message("PAM_PROMPT_ECHO_OFF ") {
            Some(Self::Prompt { text, echo: false })
        } else if let Some(text) = message("PAM_PROMPT_ECHO_ON ") {
            Some(Self::Prompt { text, echo: true })
        } else if let Some(text) = message("PAM_ERROR_MSG ") {
            Some(Self::Error(text))
        } else if let Some(text) = message("PAM_TEXT_INFO ") {
            Some(Self::Info(text))
        } else if line.starts_with("SUCCESS") {
            Some(Self::Success)
        } else if line.starts_with("FAILURE") {
            Some(Self::Failure)
        } else {
            None
        }
    }
}

/// Undoes `g_strescape`: `\n`, `\t` and the other C escapes, octal bytes,
/// and a backslash before anything else as that character, as
/// `g_strcompress` reads them. Octal bytes are UTF-8 put back together.
pub fn compress(text: &str) -> String {
    let mut out = Vec::with_capacity(text.len());
    let mut bytes = text.bytes().peekable();
    while let Some(b) = bytes.next() {
        if b != b'\\' {
            out.push(b);
            continue;
        }
        let Some(next) = bytes.next() else {
            break;
        };
        out.push(match next {
            b'0'..=b'7' => {
                let mut value = u32::from(next - b'0');
                for _ in 0..2 {
                    match bytes.peek() {
                        Some(d @ b'0'..=b'7') => {
                            value = value * 8 + u32::from(d - b'0');
                            bytes.next();
                        }
                        _ => break,
                    }
                }
                value as u8
            }
            b'b' => 8,
            b'f' => 12,
            b'n' => b'\n',
            b'r' => b'\r',
            b't' => b'\t',
            b'v' => 11,
            other => other,
        });
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// Someone polkit accepts as authenticating the request.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Identity {
    /// Their user ID.
    pub uid: u32,
    /// Their user name, which is what the helper is told.
    pub name: String,
}

/// Picks who is asked first: the person at the desktop when polkit accepts
/// them (an action that only needs them to prove it is them, or an
/// administrator acting as themselves), else the first administrator.
pub fn pick(identities: &[Identity], user: &str) -> usize {
    identities
        .iter()
        .position(|identity| identity.name == user)
        .unwrap_or(0)
}

/// What the dialog shows for one request.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct AuthDialog {
    /// What the app wants to do, in its own words (polkit's message).
    pub message: String,
    /// polkit's action ID, shown under the details.
    pub action: String,
    /// Who may authenticate, and which of them is.
    pub identities: Vec<Identity>,
    /// Index into `identities`.
    pub chosen: usize,
    /// What is typed into the field.
    pub answer: String,
    prompt: Option<(String, bool)>,
    notice: Option<Notice>,
    /// Whether an answer went out and the helper has not said more.
    waiting: bool,
}

impl AuthDialog {
    /// A dialog for `message`, asking `identities[chosen]`.
    pub fn new(message: &str, action: &str, identities: Vec<Identity>, chosen: usize) -> Self {
        Self {
            message: message.to_owned(),
            action: action.to_owned(),
            chosen: chosen.min(identities.len().saturating_sub(1)),
            identities,
            ..Self::default()
        }
    }

    /// Whom the field authenticates.
    pub fn who(&self) -> Option<&Identity> {
        self.identities.get(self.chosen)
    }

    /// Applies one line from the helper. `Success` and `Failure` end the
    /// conversation; the agent decides what follows (see
    /// [`AuthDialog::failed`]).
    pub fn apply(&mut self, line: &HelperLine) {
        match line {
            HelperLine::Prompt { text, echo } => {
                self.prompt = Some((text.clone(), *echo));
                self.waiting = false;
            }
            HelperLine::Info(text) => {
                self.notice = Some(Notice {
                    text: text.clone(),
                    error: false,
                });
            }
            HelperLine::Error(text) => {
                self.notice = Some(Notice {
                    text: text.clone(),
                    error: true,
                });
            }
            HelperLine::Success | HelperLine::Failure => {
                self.prompt = None;
                self.waiting = false;
            }
        }
    }

    /// PAM refused what was typed; the agent starts over with the same
    /// person, who is told why.
    pub fn failed(&mut self) {
        let text = match self.ask() {
            Some(Ask::NewPassword) => "The new password was not accepted, try again",
            Some(Ask::Code) => "Wrong password or code, try again",
            Some(Ask::Pin) => "Wrong PIN, try again",
            _ => "Wrong password, try again",
        };
        self.notice = Some(Notice {
            text: text.to_owned(),
            error: true,
        });
        self.prompt = None;
        self.answer.clear();
        self.waiting = false;
    }

    /// Asks someone else: the conversation starts over for them.
    pub fn choose(&mut self, index: usize) -> bool {
        if index >= self.identities.len() || index == self.chosen {
            return false;
        }
        self.chosen = index;
        self.prompt = None;
        self.notice = None;
        self.answer.clear();
        self.waiting = false;
        true
    }

    /// What PAM asks for now, if it asks for anything.
    pub fn ask(&self) -> Option<Ask> {
        self.prompt.as_ref().map(|(text, _)| Ask::of(text))
    }

    /// Whether the field takes typing: PAM asked and nothing is on its way.
    pub fn editable(&self) -> bool {
        self.prompt.is_some() && !self.waiting
    }

    /// Whether what is typed is hidden.
    pub fn secret(&self) -> bool {
        self.prompt.as_ref().is_none_or(|(_, echo)| !echo)
    }

    /// Takes what was typed, to send to the helper.
    pub fn submit(&mut self) -> Option<String> {
        if !self.editable() {
            return None;
        }
        self.waiting = true;
        Some(std::mem::take(&mut self.answer))
    }

    /// The field's placeholder.
    pub fn hint(&self) -> &str {
        match (&self.prompt, self.ask()) {
            (Some(_), Some(Ask::Code)) => "Verification code",
            (Some(_), Some(Ask::Pin)) => "Security key PIN",
            (Some(_), Some(Ask::NewPassword)) => "New password",
            (Some((_, false)), _) => "Password",
            (Some((text, true)), _) => text.trim().trim_end_matches(':'),
            (None, _) => "",
        }
    }

    /// The line under the field: PAM's last message, else what to do.
    pub fn status(&self) -> (&str, bool) {
        if let Some(notice) = &self.notice {
            return (&notice.text, notice.error);
        }
        let text = match (&self.prompt, self.ask()) {
            _ if self.waiting => "Checking…",
            (None, _) => "Starting…",
            (Some(_), Some(Ask::NewPassword)) => "Your password expired; choose a new one",
            (Some(_), Some(Ask::Code)) => "Enter the code from your authenticator app",
            (Some(_), Some(Ask::Pin)) => "Enter your security key's PIN",
            (Some(_), _) => "Enter the password to allow this",
        };
        (text, false)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn people() -> Vec<Identity> {
        vec![
            Identity {
                uid: 1000,
                name: "ada".into(),
            },
            Identity {
                uid: 1001,
                name: "grace".into(),
            },
        ]
    }

    #[test]
    fn the_helpers_lines_parse() {
        assert_eq!(
            HelperLine::parse("PAM_PROMPT_ECHO_OFF Password: "),
            Some(HelperLine::Prompt {
                text: "Password: ".into(),
                echo: false
            })
        );
        assert_eq!(
            HelperLine::parse("PAM_TEXT_INFO Place your finger on the fingerprint reader"),
            Some(HelperLine::Info(
                "Place your finger on the fingerprint reader".into()
            ))
        );
        assert_eq!(
            HelperLine::parse("PAM_ERROR_MSG Password expired\\nChange it"),
            Some(HelperLine::Error("Password expired\nChange it".into()))
        );
        assert_eq!(HelperLine::parse("SUCCESS"), Some(HelperLine::Success));
        assert_eq!(HelperLine::parse("FAILURE"), Some(HelperLine::Failure));
        assert_eq!(HelperLine::parse("HELLO"), None);
    }

    #[test]
    fn escapes_come_back_as_g_strcompress_reads_them() {
        assert_eq!(compress(r#"a\"b\\c\td"#), "a\"b\\c\td");
        // g_strescape writes bytes above 0x7e in octal.
        assert_eq!(compress(r"Caf\303\251"), "Café");
        assert_eq!(compress(r"\101\0"), "A\0");
        assert_eq!(compress("trailing\\"), "trailing");
    }

    #[test]
    fn the_person_at_the_desktop_is_asked_first() {
        assert_eq!(pick(&people(), "grace"), 1);
        assert_eq!(pick(&people(), "root"), 0);
        assert_eq!(pick(&[], "ada"), 0);
    }

    #[test]
    fn the_field_follows_the_prompts() {
        let mut dialog = AuthDialog::new("Install apps", "org.flatpak", people(), 0);
        assert!(!dialog.editable());
        assert_eq!(dialog.status(), ("Starting…", false));
        dialog.apply(&HelperLine::Info(
            "Place your finger on the fingerprint reader".into(),
        ));
        assert_eq!(
            dialog.status(),
            ("Place your finger on the fingerprint reader", false)
        );
        dialog.apply(&HelperLine::Prompt {
            text: "Password: ".into(),
            echo: false,
        });
        assert!(dialog.editable() && dialog.secret());
        assert_eq!(dialog.hint(), "Password");
        dialog.answer = "hunter2".into();
        assert_eq!(dialog.submit().as_deref(), Some("hunter2"));
        assert!(dialog.answer.is_empty());
        assert!(!dialog.editable());
        assert_eq!(dialog.submit(), None);
        dialog.apply(&HelperLine::Prompt {
            text: "Security token PIN: ".into(),
            echo: false,
        });
        assert_eq!(dialog.hint(), "Security key PIN");
        dialog.apply(&HelperLine::Prompt {
            text: "Name: ".into(),
            echo: true,
        });
        assert!(!dialog.secret());
        assert_eq!(dialog.hint(), "Name");
    }

    #[test]
    fn a_refusal_says_what_was_wrong() {
        let mut dialog = AuthDialog::new("", "", people(), 0);
        dialog.apply(&HelperLine::Prompt {
            text: "Verification code: ".into(),
            echo: true,
        });
        dialog.failed();
        assert_eq!(dialog.status(), ("Wrong password or code, try again", true));
        assert!(!dialog.editable());
    }

    #[test]
    fn choosing_someone_else_starts_over() {
        let mut dialog = AuthDialog::new("", "", people(), 0);
        dialog.apply(&HelperLine::Prompt {
            text: "Password: ".into(),
            echo: false,
        });
        dialog.answer = "half".into();
        assert!(!dialog.choose(0));
        assert!(!dialog.choose(5));
        assert!(dialog.choose(1));
        assert_eq!(dialog.who().map(|w| w.uid), Some(1001));
        assert!(dialog.answer.is_empty() && !dialog.editable());
    }
}
