//! `derisk auth`: one PAM conversation for the lock screen, over a pipe.
//!
//! The lock screen runs this as a child process, as the session's own user,
//! and speaks greetd's protocol to it on stdin and stdout, exactly as the
//! greeter speaks to `derisk display-manager`: `create_session` starts PAM,
//! every PAM message comes back as an `auth_message`, and `success` means
//! the screen may unlock. A process rather than a thread because a PAM
//! module cannot be interrupted: pam_fprintd waits for a finger, and once the
//! person types their password instead, the lock screen ends the reader's
//! conversation by killing its process, the way GDM ends its worker.
//!
//! It answers only for the user it runs as. Checking anyone else's password
//! is what the display manager is for; here the unprivileged stack could not
//! do it anyway (unix_chkpwd refuses), and saying so plainly is better than
//! a confusing refusal.

use std::io::{self, Read, Write};

use derisk::greetd::{self, Request, Response};

use crate::pam::{self, Auth};

type Result<T = ()> = std::result::Result<T, Box<dyn std::error::Error>>;

/// stdin and stdout as one channel, for the conversation.
struct Stdio;

impl Read for Stdio {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        io::stdin().read(buf)
    }
}

impl Write for Stdio {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        io::stdout().write(buf)
    }

    fn flush(&mut self) -> io::Result<()> {
        io::stdout().flush()
    }
}

/// Serves conversations against `service` until stdin closes.
pub fn run(service: &str) -> Result {
    let user = pam::current_user().ok_or("the user this runs as has no name")?;
    let mut channel = Stdio;
    loop {
        let Ok(request) = greetd::read_request(&mut channel) else {
            // EOF: the lock screen is done with this conversation.
            return Ok(());
        };
        let response = match request {
            Request::CreateSession { username } if username != user => Response::Error {
                error_type: "error".into(),
                description: "derisk auth only checks the user it runs as".into(),
            },
            Request::CreateSession { .. } => {
                match pam::authenticate(service, &user, None, pam::Expired::Remind, Box::new(Stdio))
                {
                    // Nothing to keep open: unlocking has no session to start.
                    Auth::Ok(_) => Response::Success,
                    Auth::Broken => return Ok(()),
                    other => other.response().unwrap_or(Response::Success),
                }
            }
            // Nothing is being set up between requests, so there is nothing
            // to start or cancel; both are acknowledged so a greetd client
            // can end the way it always does.
            Request::StartSession { .. } | Request::CancelSession => Response::Success,
            Request::PostAuthMessageResponse { .. } => Response::Error {
                error_type: "error".into(),
                description: "no question was asked".into(),
            },
        };
        if greetd::write_response(&mut channel, &response).is_err() {
            return Ok(());
        }
    }
}
