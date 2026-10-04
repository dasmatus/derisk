//! Logging in through greetd, for `derisk greeter`.
//!
//! greetd (<https://git.sr.ht/~kennylevinsen/greetd>) is the display manager:
//! it owns PAM, opens the logind session and starts the user's session once
//! the greeter exits. The greeter only holds the conversation, over the Unix
//! socket in `$GREETD_SOCK`: each message is JSON preceded by its length as a
//! native-endian `u32`, and every request gets exactly one response.
//!
//! [`Login`] is that conversation as a state machine, kept free of sockets
//! and drawing so it can be tested: the host feeds it what the user typed and
//! what greetd answered, and sends whatever request it returns.

use std::io::{self, Read, Write};

use serde::{Deserialize, Serialize};

/// A request to greetd.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Request {
    /// Start authenticating `username`.
    CreateSession {
        /// Who is logging in.
        username: String,
    },
    /// Answer the last [`Response::AuthMessage`]; `None` for an info or
    /// error message, which only needs acknowledging.
    PostAuthMessageResponse {
        /// What the user typed.
        response: Option<String>,
    },
    /// Start the authenticated session once the greeter exits.
    StartSession {
        /// The session's command line.
        cmd: Vec<String>,
        /// `KEY=VALUE` pairs for the session's environment.
        env: Vec<String>,
    },
    /// Abandon the session being authenticated.
    CancelSession,
}

/// What an auth message asks for.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AuthMessageType {
    /// A prompt whose answer may be shown, such as a user name.
    Visible,
    /// A prompt whose answer is hidden, such as a password.
    Secret,
    /// Something to tell the user.
    Info,
    /// A problem to tell the user about.
    Error,
}

/// greetd's answer to a [`Request`].
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Response {
    /// The request succeeded.
    Success,
    /// The request failed.
    Error {
        /// `auth_error` for a failed authentication, else `error`.
        error_type: String,
        /// greetd's description, usually PAM's.
        description: String,
    },
    /// PAM wants to say or ask something.
    AuthMessage {
        /// What it wants.
        auth_message_type: AuthMessageType,
        /// The text, such as "Password: ".
        auth_message: String,
    },
}

/// Writes one length-prefixed message.
pub fn write_request(w: &mut impl Write, request: &Request) -> io::Result<()> {
    let body = serde_json::to_vec(request)?;
    let len = u32::try_from(body.len()).map_err(|_| io::Error::other("request too long"))?;
    w.write_all(&len.to_ne_bytes())?;
    w.write_all(&body)?;
    w.flush()
}

/// Largest response accepted. greetd's are a few hundred bytes; this bounds
/// what a confused peer can make the greeter allocate.
const MAX_RESPONSE: u32 = 64 * 1024;

/// Reads one length-prefixed message.
pub fn read_response(r: &mut impl Read) -> io::Result<Response> {
    let mut len = [0; 4];
    r.read_exact(&mut len)?;
    let len = u32::from_ne_bytes(len);
    if len > MAX_RESPONSE {
        return Err(io::Error::other(format!("{len}-byte response from greetd")));
    }
    let mut body = vec![0; len as usize];
    r.read_exact(&mut body)?;
    Ok(serde_json::from_slice(&body)?)
}

/// Where the conversation is.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Phase {
    /// Asking who is logging in.
    User,
    /// A request is with greetd.
    Waiting,
    /// PAM asked a question; the answer goes in [`Login::answer`].
    Prompt {
        /// Whether to hide what is typed.
        secret: bool,
        /// PAM's prompt, such as "Password: ".
        text: String,
    },
    /// Authenticated; asked greetd to start the session.
    Starting,
    /// Cancelling greetd's session; `retry` asks again for the same user
    /// once that is done, after a wrong password.
    Cancelling {
        /// Whether to start over for the same user.
        retry: bool,
    },
    /// greetd will start the session when the greeter exits.
    Started,
}

/// A line under the field: PAM's info, or what went wrong.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Notice {
    /// The text.
    pub text: String,
    /// Whether it reports a failure.
    pub error: bool,
}

/// The greeter's side of a greetd login.
#[derive(Debug)]
pub struct Login {
    /// Who is logging in.
    pub username: String,
    /// The answer being typed to PAM's prompt.
    pub answer: String,
    phase: Phase,
    notice: Option<Notice>,
    cmd: Vec<String>,
    env: Vec<String>,
}

impl Login {
    /// A login that starts `cmd` with `env` once authenticated, with
    /// `username` filled in.
    pub fn new(username: String, cmd: Vec<String>, env: Vec<String>) -> Self {
        Self {
            username,
            answer: String::new(),
            phase: Phase::User,
            notice: None,
            cmd,
            env,
        }
    }

    /// Where the conversation is.
    pub fn phase(&self) -> &Phase {
        &self.phase
    }

    /// The line to show under the field, if any.
    pub fn notice(&self) -> Option<&Notice> {
        self.notice.as_ref()
    }

    /// Whether the user can type: picking a user, or answering a prompt.
    pub fn editable(&self) -> bool {
        matches!(self.phase, Phase::User | Phase::Prompt { .. })
    }

    /// Enter in the field. Returns the request to send, or `None` when there
    /// is nothing to send yet.
    pub fn submit(&mut self) -> Option<Request> {
        match self.phase {
            Phase::User => {
                let username = self.username.trim().to_owned();
                if username.is_empty() {
                    return None;
                }
                self.username = username.clone();
                self.notice = None;
                self.phase = Phase::Waiting;
                Some(Request::CreateSession { username })
            }
            Phase::Prompt { .. } => {
                self.phase = Phase::Waiting;
                Some(Request::PostAuthMessageResponse {
                    response: Some(std::mem::take(&mut self.answer)),
                })
            }
            _ => None,
        }
    }

    /// Escape: back to picking a user. Returns the request that abandons
    /// greetd's session, when there is one to abandon.
    pub fn back(&mut self) -> Option<Request> {
        match self.phase {
            Phase::Prompt { .. } => {
                self.answer.clear();
                self.notice = None;
                self.phase = Phase::Cancelling { retry: false };
                Some(Request::CancelSession)
            }
            _ => None,
        }
    }

    /// Applies greetd's response to the last request, and returns the next
    /// request to send, if any.
    pub fn respond(&mut self, response: Response) -> Option<Request> {
        match (response, &self.phase) {
            (
                Response::AuthMessage {
                    auth_message_type,
                    auth_message,
                },
                Phase::Waiting,
            ) => match auth_message_type {
                AuthMessageType::Visible | AuthMessageType::Secret => {
                    self.answer.clear();
                    self.phase = Phase::Prompt {
                        secret: auth_message_type == AuthMessageType::Secret,
                        text: auth_message.trim().to_owned(),
                    };
                    None
                }
                // Shown, and acknowledged at once: greetd waits for an
                // answer to every message, even one that asks nothing.
                AuthMessageType::Info | AuthMessageType::Error => {
                    self.notice = Some(Notice {
                        text: auth_message.trim().to_owned(),
                        error: auth_message_type == AuthMessageType::Error,
                    });
                    Some(Request::PostAuthMessageResponse { response: None })
                }
            },
            (Response::Success, Phase::Waiting) => {
                self.phase = Phase::Starting;
                Some(Request::StartSession {
                    cmd: self.cmd.clone(),
                    env: self.env.clone(),
                })
            }
            (Response::Success, Phase::Starting) => {
                self.phase = Phase::Started;
                None
            }
            (Response::Success, Phase::Cancelling { retry: true }) => {
                self.phase = Phase::Waiting;
                Some(Request::CreateSession {
                    username: self.username.clone(),
                })
            }
            (Response::Success, Phase::Cancelling { retry: false }) => {
                self.phase = Phase::User;
                None
            }
            // A wrong password ends greetd's session; cancel what is left of
            // it and ask the same user again, so trying again is just typing.
            (
                Response::Error {
                    error_type,
                    description,
                },
                Phase::Waiting | Phase::Starting,
            ) => {
                let wrong_password = error_type == "auth_error" && self.phase == Phase::Waiting;
                self.notice = Some(Notice {
                    text: if wrong_password {
                        "Wrong password, try again".to_owned()
                    } else {
                        description
                    },
                    error: true,
                });
                self.answer.clear();
                self.phase = Phase::Cancelling {
                    retry: wrong_password,
                };
                Some(Request::CancelSession)
            }
            (Response::Error { description, .. }, Phase::Cancelling { .. }) => {
                self.notice = Some(Notice {
                    text: description,
                    error: true,
                });
                self.phase = Phase::User;
                None
            }
            // Nothing was asked; greetd is out of step. Starting over is the
            // one way back to a conversation both sides agree on.
            (response, _) => {
                self.notice = Some(Notice {
                    text: format!("unexpected answer from greetd: {response:?}"),
                    error: true,
                });
                self.answer.clear();
                self.phase = Phase::Cancelling { retry: false };
                Some(Request::CancelSession)
            }
        }
    }

    /// The request could not be sent or answered: greetd is gone.
    pub fn disconnected(&mut self, error: &str) {
        self.answer.clear();
        self.notice = Some(Notice {
            text: format!("lost greetd: {error}"),
            error: true,
        });
        self.phase = Phase::User;
    }

    /// The hint in the field.
    pub fn hint(&self) -> &str {
        match &self.phase {
            Phase::Prompt { text, .. } if !text.is_empty() => text.trim_end_matches(':'),
            Phase::Prompt { secret: true, .. } => "Password",
            Phase::Prompt { .. } => "Answer",
            _ => "User name",
        }
    }

    /// The line under the field when there is no notice.
    pub fn status(&self) -> &'static str {
        match self.phase {
            Phase::User => "Enter your user name",
            Phase::Waiting | Phase::Cancelling { .. } => "Checking…",
            Phase::Prompt { secret: true, .. } => "Enter your password to log in",
            Phase::Prompt { .. } => "Press Escape to log in as someone else",
            Phase::Starting | Phase::Started => "Starting your session…",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ANSWER: &str = "typed-answer";

    fn login() -> Login {
        Login::new(
            "alice".into(),
            vec!["derisk-session".into()],
            vec!["XDG_SESSION_TYPE=wayland".into()],
        )
    }

    fn secret() -> Response {
        Response::AuthMessage {
            auth_message_type: AuthMessageType::Secret,
            auth_message: "Password: ".into(),
        }
    }

    fn auth_error() -> Response {
        Response::Error {
            error_type: "auth_error".into(),
            description: "pam_authenticate: AUTH_ERR".into(),
        }
    }

    fn at_prompt() -> Login {
        let mut login = login();
        login.submit();
        assert_eq!(login.respond(secret()), None);
        login
    }

    #[test]
    fn frames_requests_like_greetd() {
        let mut buf = Vec::new();
        write_request(&mut buf, &Request::CancelSession).unwrap();
        let body = br#"{"type":"cancel_session"}"#;
        assert_eq!(&buf[..4], &(body.len() as u32).to_ne_bytes());
        assert_eq!(&buf[4..], body);

        let json = |r: &Request| serde_json::to_value(r).unwrap();
        assert_eq!(
            json(&Request::PostAuthMessageResponse { response: None }),
            serde_json::json!({"type": "post_auth_message_response", "response": null})
        );
        assert_eq!(
            json(&Request::StartSession {
                cmd: vec!["a".into()],
                env: vec![]
            }),
            serde_json::json!({"type": "start_session", "cmd": ["a"], "env": []})
        );
    }

    #[test]
    fn reads_greetd_responses() {
        let mut framed = Vec::new();
        let body =
            br#"{"type":"auth_message","auth_message_type":"secret","auth_message":"Password: "}"#;
        framed.extend((body.len() as u32).to_ne_bytes());
        framed.extend(body);
        assert_eq!(read_response(&mut &framed[..]).unwrap(), secret());

        let mut huge = (MAX_RESPONSE + 1).to_ne_bytes().to_vec();
        huge.extend(b"{}");
        assert!(read_response(&mut &huge[..]).is_err());
    }

    #[test]
    fn password_login_starts_the_session() {
        let mut login = at_prompt();
        assert!(login.editable());
        assert_eq!(login.hint(), "Password");
        login.answer = ANSWER.into();
        assert_eq!(
            login.submit(),
            Some(Request::PostAuthMessageResponse {
                response: Some(ANSWER.into())
            })
        );
        assert!(login.answer.is_empty());
        assert_eq!(
            login.respond(Response::Success),
            Some(Request::StartSession {
                cmd: vec!["derisk-session".into()],
                env: vec!["XDG_SESSION_TYPE=wayland".into()],
            })
        );
        assert_eq!(login.respond(Response::Success), None);
        assert_eq!(login.phase(), &Phase::Started);
    }

    #[test]
    fn no_user_name_sends_nothing() {
        let mut login = login();
        login.username = "  ".into();
        assert_eq!(login.submit(), None);
        login.username = " bob ".into();
        assert_eq!(
            login.submit(),
            Some(Request::CreateSession {
                username: "bob".into()
            })
        );
    }

    #[test]
    fn wrong_password_asks_the_same_user_again() {
        let mut login = at_prompt();
        login.answer = ANSWER.into();
        login.submit();
        assert_eq!(login.respond(auth_error()), Some(Request::CancelSession));
        assert_eq!(login.notice().unwrap().text, "Wrong password, try again");
        assert_eq!(
            login.respond(Response::Success),
            Some(Request::CreateSession {
                username: "alice".into()
            })
        );
        assert_eq!(login.respond(secret()), None);
        assert!(login.editable());
        // The notice stays up while the user types again.
        assert!(login.notice().unwrap().error);
    }

    #[test]
    fn info_messages_are_shown_and_acknowledged() {
        let mut login = login();
        login.submit();
        assert_eq!(
            login.respond(Response::AuthMessage {
                auth_message_type: AuthMessageType::Info,
                auth_message: "Please touch the security key".into(),
            }),
            Some(Request::PostAuthMessageResponse { response: None })
        );
        assert_eq!(
            login.notice(),
            Some(&Notice {
                text: "Please touch the security key".into(),
                error: false
            })
        );
        assert!(!login.editable());
    }

    #[test]
    fn escape_cancels_back_to_the_user_name() {
        let mut login = at_prompt();
        login.answer = ANSWER.into();
        assert_eq!(login.back(), Some(Request::CancelSession));
        assert!(login.answer.is_empty());
        assert_eq!(login.respond(Response::Success), None);
        assert_eq!(login.phase(), &Phase::User);
        assert_eq!(login.back(), None);
    }

    #[test]
    fn a_failed_start_goes_back_to_the_user_name() {
        let mut login = at_prompt();
        login.submit();
        login.respond(Response::Success);
        let failed = Response::Error {
            error_type: "error".into(),
            description: "session already started".into(),
        };
        assert_eq!(login.respond(failed), Some(Request::CancelSession));
        assert_eq!(login.notice().unwrap().text, "session already started");
        assert_eq!(login.respond(Response::Success), None);
        assert_eq!(login.phase(), &Phase::User);
    }

    #[test]
    fn an_unasked_answer_cancels() {
        let mut login = at_prompt();
        assert_eq!(
            login.respond(Response::Success),
            Some(Request::CancelSession)
        );
        assert!(login.notice().unwrap().error);
    }

    #[test]
    fn typing_is_ignored_while_waiting() {
        let mut login = login();
        login.submit();
        assert!(!login.editable());
        assert_eq!(login.submit(), None);
        assert_eq!(login.back(), None);
    }

    #[test]
    fn losing_greetd_returns_to_the_user_name() {
        let mut login = at_prompt();
        login.answer = ANSWER.into();
        login.disconnected("broken pipe");
        assert_eq!(login.phase(), &Phase::User);
        assert!(login.answer.is_empty());
        assert_eq!(login.username, "alice");
    }
}
