//! The lock screen's PAM conversations, each in a `derisk auth` process
//! ([`crate::auth`]) that the session talks greetd's protocol to.

use std::{
    io::{self, Read, Write},
    path::Path,
    process::{Child, ChildStdin, ChildStdout, Command, Stdio},
    sync::mpsc,
};

use derisk::greetd::{Request, Response};
use tracing::warn;

use crate::{greeter::spawn_worker, pam};

/// One running conversation. Dropping it kills the process, which is how a
/// conversation PAM is still blocked in (a reader waiting for a finger) is
/// ended once the other one unlocked.
pub(crate) struct Conversation {
    child: Child,
    to: mpsc::Sender<Request>,
    from: mpsc::Receiver<io::Result<Response>>,
}

/// A child's stdin and stdout as one stream.
struct Pipe {
    stdin: ChildStdin,
    stdout: ChildStdout,
}

impl Read for Pipe {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        self.stdout.read(buf)
    }
}

impl Write for Pipe {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.stdin.write(buf)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.stdin.flush()
    }
}

impl Conversation {
    /// Starts `derisk auth` against `service`.
    pub(crate) fn start(service: &str) -> io::Result<Self> {
        let mut child = Command::new(std::env::current_exe()?)
            .args(["auth", "--service", service])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()?;
        let (Some(stdin), Some(stdout)) = (child.stdin.take(), child.stdout.take()) else {
            let _ = child.kill();
            return Err(io::Error::other("derisk auth has no pipes"));
        };
        let (to, from) = spawn_worker(Pipe { stdin, stdout });
        Ok(Self { child, to, from })
    }

    /// Sends `request`. `false` once the process is gone.
    pub(crate) fn send(&self, request: Request) -> bool {
        self.to.send(request).is_ok()
    }

    /// The next answer, if one arrived. `Some(Err)` once the process is
    /// gone.
    pub(crate) fn poll(&self) -> Option<io::Result<Response>> {
        match self.from.try_recv() {
            Ok(answer) => Some(answer),
            Err(mpsc::TryRecvError::Empty) => None,
            Err(mpsc::TryRecvError::Disconnected) => {
                Some(Err(io::Error::other("derisk auth stopped")))
            }
        }
    }
}

impl Drop for Conversation {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// Whether the system offers the fingerprint reader on the lock screen: it
/// does by installing the PAM service for it.
pub(crate) fn fingerprint_offered() -> bool {
    ["/etc/pam.d", "/usr/lib/pam.d"]
        .iter()
        .any(|dir| Path::new(dir).join(pam::FINGERPRINT_SERVICE).exists())
}

/// Starts a conversation, logging why one could not start.
pub(crate) fn start(service: &str) -> Option<Conversation> {
    Conversation::start(service)
        .inspect_err(|e| warn!("cannot start derisk auth for {service}: {e}"))
        .ok()
}
