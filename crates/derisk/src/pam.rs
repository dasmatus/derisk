//! PAM, for the display manager and the lock screen.
//!
//! This is where derisk calls C directly, which is why it lives in the
//! binary and not in the library (`#![forbid(unsafe_code)]`). Both callers
//! hold the same conversation: [`authenticate`] runs `pam_authenticate` and
//! `pam_acct_mgmt` for a user and relays every PAM message over greetd's
//! protocol to whoever draws the screen, so whatever the system's stack asks
//! for -- a password, a security key's PIN and touch through
//! pam_systemd_home, a verification code through pam_google_authenticator,
//! a finger through pam_fprintd -- reaches the person, and an expired
//! password is changed in the same conversation before the login goes on.
//!
//! `derisk display-manager` serves the greeter this way as root; the lock
//! screen runs `derisk auth` ([`crate::auth`]) as the session's user and
//! talks to it the same way over a pipe.

use std::{
    ffi::{CStr, CString, c_char, c_int, c_void},
    io::{Read, Write},
    ptr,
};

use derisk::greetd::{self, AuthMessageType, Request, Response};
use tracing::info;

/// The PAM service the lock screen authenticates against.
pub const SERVICE: &str = "derisk";

/// The PAM service the lock screen listens to the fingerprint reader
/// through, beside [`SERVICE`]. The lock screen offers the reader only when
/// the system installed this service (`/etc/pam.d/derisk-fingerprint`).
pub const FINGERPRINT_SERVICE: &str = "derisk-fingerprint";

pub(crate) const PAM_SUCCESS: c_int = 0;
pub(crate) const PAM_BUF_ERR: c_int = 5;
pub(crate) const PAM_AUTH_ERR: c_int = 7;
pub(crate) const PAM_MAXTRIES: c_int = 11;
pub(crate) const PAM_NEW_AUTHTOK_REQD: c_int = 12;
pub(crate) const PAM_CONV_ERR: c_int = 19;
pub(crate) const PAM_PROMPT_ECHO_OFF: c_int = 1;
pub(crate) const PAM_PROMPT_ECHO_ON: c_int = 2;
pub(crate) const PAM_ERROR_MSG: c_int = 3;
pub(crate) const PAM_TEXT_INFO: c_int = 4;
const PAM_TTY: c_int = 3;
/// Asks the password modules to change only a token that has expired.
const PAM_CHANGE_EXPIRED_AUTHTOK: c_int = 0x20;
/// Tells modules a pam_end is the parent's copy after a fork, so they free
/// memory without tearing down what the child still uses.
const PAM_DATA_SILENT: c_int = 0x4000_0000;

#[repr(C)]
pub(crate) struct PamMessage {
    pub(crate) msg_style: c_int,
    pub(crate) msg: *const c_char,
}

#[repr(C)]
pub(crate) struct PamResponse {
    pub(crate) resp: *mut c_char,
    pub(crate) resp_retcode: c_int,
}

pub(crate) type ConvFn = unsafe extern "C" fn(
    num_msg: c_int,
    msg: *mut *const PamMessage,
    resp: *mut *mut PamResponse,
    appdata_ptr: *mut c_void,
) -> c_int;

#[repr(C)]
pub(crate) struct PamConv {
    pub(crate) conv: ConvFn,
    pub(crate) appdata_ptr: *mut c_void,
}

#[link(name = "pam")]
unsafe extern "C" {
    fn pam_start(
        service_name: *const c_char,
        user: *const c_char,
        pam_conversation: *const PamConv,
        pamh: *mut *mut c_void,
    ) -> c_int;
    pub(crate) fn pam_authenticate(pamh: *mut c_void, flags: c_int) -> c_int;
    pub(crate) fn pam_acct_mgmt(pamh: *mut c_void, flags: c_int) -> c_int;
    fn pam_chauthtok(pamh: *mut c_void, flags: c_int) -> c_int;
    fn pam_end(pamh: *mut c_void, pam_status: c_int) -> c_int;
    fn pam_set_item(pamh: *mut c_void, item_type: c_int, item: *const c_void) -> c_int;
    fn pam_strerror(pamh: *mut c_void, errnum: c_int) -> *const c_char;
}

/// Where the conversation's messages go and its answers come from: the
/// greeter's socket, or the lock screen's pipe.
pub(crate) trait Channel: Read + Write {}

impl<T: Read + Write> Channel for T {}

/// An open PAM handle. Ended exactly once: on drop, or handed to a session
/// worker by [`Pam::forget`].
pub(crate) struct Pam {
    pub(crate) handle: *mut c_void,
    /// The conversation's state; PAM keeps a pointer to it, so it is boxed
    /// and lives as long as the handle.
    pub(crate) conv: Box<Conversation>,
    pub(crate) status: c_int,
}

impl Pam {
    pub(crate) fn start(
        service: &str,
        user: &str,
        channel: Option<Box<dyn Channel>>,
    ) -> std::io::Result<Self> {
        let service = CString::new(service).map_err(std::io::Error::other)?;
        let user = CString::new(user).map_err(std::io::Error::other)?;
        let mut conv = Box::new(Conversation {
            channel,
            cancelled: false,
            unexpected: false,
            broken: false,
        });
        let pam_conv = Box::new(PamConv {
            conv: converse,
            appdata_ptr: (&raw mut *conv).cast(),
        });
        let mut handle = ptr::null_mut();
        // SAFETY: Linux-PAM copies the conversation struct; the appdata it
        // points at is boxed and owned by the returned Pam.
        let status = unsafe { pam_start(service.as_ptr(), user.as_ptr(), &*pam_conv, &mut handle) };
        if status != PAM_SUCCESS || handle.is_null() {
            return Err(std::io::Error::other(format!(
                "pam_start failed ({status})"
            )));
        }
        Ok(Self {
            handle,
            conv,
            status,
        })
    }

    pub(crate) fn error(&self, status: c_int) -> String {
        // SAFETY: pam_strerror returns a static string.
        unsafe { CStr::from_ptr(pam_strerror(self.handle, status)) }
            .to_string_lossy()
            .into_owned()
    }

    pub(crate) fn check(&mut self, what: &str, status: c_int) -> Result<(), String> {
        self.status = status;
        if status == PAM_SUCCESS {
            Ok(())
        } else {
            Err(format!("{what}: {}", self.error(status)))
        }
    }

    pub(crate) fn set_tty(&mut self, vt: u32) -> Result<(), String> {
        let tty = CString::new(format!("tty{vt}")).unwrap_or_default();
        // SAFETY: PAM copies the string.
        let status = unsafe { pam_set_item(self.handle, PAM_TTY, tty.as_ptr().cast()) };
        self.check("pam_set_item(PAM_TTY)", status)
    }

    /// Lets go of the handle without ending the PAM transaction, in the
    /// process that forked a worker to own it.
    pub(crate) fn forget(mut self) {
        // SAFETY: the handle is valid; PAM_DATA_SILENT frees this copy only.
        unsafe { pam_end(self.handle, self.status | PAM_DATA_SILENT) };
        self.handle = ptr::null_mut();
    }
}

impl Drop for Pam {
    fn drop(&mut self) {
        if !self.handle.is_null() {
            // SAFETY: the handle is valid and ended exactly once.
            unsafe { pam_end(self.handle, self.status) };
        }
    }
}

/// PAM's conversation, relayed as greetd's protocol: each PAM message
/// becomes an `auth_message` response, and the next request answers it.
pub(crate) struct Conversation {
    pub(crate) channel: Option<Box<dyn Channel>>,
    /// The other side cancelled mid-conversation; its cancel is still to be
    /// answered.
    pub(crate) cancelled: bool,
    /// The other side sent something other than an answer or a cancel.
    pub(crate) unexpected: bool,
    /// The channel failed; the other side is gone.
    pub(crate) broken: bool,
}

impl Conversation {
    /// Asks one thing. `None` when the other side cancelled or is gone.
    fn ask(&mut self, kind: AuthMessageType, text: String) -> Option<Option<String>> {
        let channel = self.channel.as_mut()?;
        let message = Response::AuthMessage {
            auth_message_type: kind,
            auth_message: text,
        };
        let request =
            greetd::write_response(channel, &message).and_then(|()| greetd::read_request(channel));
        match request {
            Ok(Request::PostAuthMessageResponse { response }) => Some(response),
            Ok(Request::CancelSession) => {
                self.cancelled = true;
                None
            }
            Ok(_) => {
                self.unexpected = true;
                None
            }
            Err(_) => {
                self.broken = true;
                None
            }
        }
    }
}

unsafe extern "C" fn converse(
    num_msg: c_int,
    msg: *mut *const PamMessage,
    resp: *mut *mut PamResponse,
    appdata_ptr: *mut c_void,
) -> c_int {
    let Ok(count) = usize::try_from(num_msg) else {
        return PAM_CONV_ERR;
    };
    if count == 0 || msg.is_null() || resp.is_null() || appdata_ptr.is_null() {
        return PAM_CONV_ERR;
    }
    // SAFETY: appdata_ptr is the boxed Conversation owned by the Pam whose
    // call this is.
    let conv = unsafe { &mut *appdata_ptr.cast::<Conversation>() };
    // SAFETY: calloc returns zeroed memory or null, checked below.
    let replies = unsafe { libc::calloc(count, size_of::<PamResponse>()) }.cast::<PamResponse>();
    if replies.is_null() {
        return PAM_BUF_ERR;
    }
    for i in 0..count {
        // SAFETY: Linux-PAM passes an array of `count` message pointers.
        let message = unsafe { &**msg.add(i) };
        let text = if message.msg.is_null() {
            String::new()
        } else {
            // SAFETY: PAM messages are NUL-terminated strings.
            unsafe { CStr::from_ptr(message.msg) }
                .to_string_lossy()
                .into_owned()
        };
        let kind = match message.msg_style {
            PAM_PROMPT_ECHO_OFF => AuthMessageType::Secret,
            PAM_PROMPT_ECHO_ON => AuthMessageType::Visible,
            PAM_ERROR_MSG => AuthMessageType::Error,
            PAM_TEXT_INFO => AuthMessageType::Info,
            _ => AuthMessageType::Info,
        };
        let Some(answer) = conv.ask(kind, text) else {
            // SAFETY: free the responses filled so far, then the array.
            unsafe {
                for j in 0..i {
                    let r = (*replies.add(j)).resp;
                    if !r.is_null() {
                        libc::free(r.cast());
                    }
                }
                libc::free(replies.cast());
            }
            return PAM_CONV_ERR;
        };
        if matches!(kind, AuthMessageType::Secret | AuthMessageType::Visible) {
            let mut answer = answer.unwrap_or_default();
            let copy = CString::new(answer.as_bytes()).unwrap_or_default();
            wipe(&mut answer);
            // SAFETY: `replies` holds `count` zeroed responses; strdup copies
            // a NUL-terminated string, which PAM frees.
            unsafe { (*replies.add(i)).resp = libc::strdup(copy.as_ptr()) };
            let mut bytes = copy.into_bytes();
            bytes.fill(0);
        }
    }
    // SAFETY: resp is PAM's out-pointer for the response array.
    unsafe { *resp = replies };
    PAM_SUCCESS
}

/// How an authentication ended.
pub(crate) enum Auth {
    /// Accepted; the handle is ready for a session (or simply done, for an
    /// unlock).
    Ok(Pam),
    /// The other side cancelled.
    Cancelled,
    /// PAM refused what was answered; asking again may work.
    Failed(String),
    /// PAM could not authenticate at all (no reader, a timeout, a broken
    /// stack); asking again at once would fail the same way.
    Error(String),
    /// The channel broke.
    Broken,
}

impl Auth {
    /// greetd's answer for everything but `Ok` and `Broken`. A refusal is
    /// `auth_error`, which a greeter answers by asking again; anything else
    /// is `error`, which it shows and stops on.
    pub(crate) fn response(&self) -> Option<Response> {
        match self {
            Self::Cancelled => Some(Response::Success),
            Self::Failed(description) => Some(Response::Error {
                error_type: "auth_error".into(),
                description: description.clone(),
            }),
            Self::Error(description) => Some(Response::Error {
                error_type: "error".into(),
                description: description.clone(),
            }),
            Self::Ok(_) | Self::Broken => None,
        }
    }
}

/// What to do when the account checks say the password has expired.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Expired {
    /// Ask for a new one in the same conversation, as login(1) does, and let
    /// the account in only once it is set: logging in.
    Change,
    /// Let the account in and say so: unlocking a session that is already
    /// running. The person proved who they are; refusing them their own
    /// session over a date would lock them out of it for good wherever the
    /// stack cannot change a password unprivileged (pam_unix can't), and the
    /// next login asks for the change anyway.
    Remind,
}

/// The reminder [`Expired::Remind`] sends.
const EXPIRED_REMINDER: &str =
    "Your password has expired. Change it in Settings, or you will be asked to at your next login.";

/// Runs PAM's authentication and account checks for `username` against
/// `service`, with `channel` answering its prompts. `expired` says what an
/// expired password means here. `tty` is the VT a login happens on.
pub(crate) fn authenticate(
    service: &str,
    username: &str,
    tty: Option<u32>,
    expired: Expired,
    channel: Box<dyn Channel>,
) -> Auth {
    let mut pam = match Pam::start(service, username, Some(channel)) {
        Ok(pam) => pam,
        Err(e) => return Auth::Error(e.to_string()),
    };
    if let Some(vt) = tty
        && let Err(e) = pam.set_tty(vt)
    {
        return Auth::Error(e);
    }
    // SAFETY: the handle is valid; the conversation lives in `pam`.
    let status = unsafe { pam_authenticate(pam.handle, 0) };
    let auth = pam.check("pam_authenticate", status);
    if let Some(ended) = ended(&pam) {
        return ended;
    }
    if let Err(e) = auth {
        // No name: a failed login's user name is whatever was typed, which
        // is sometimes the password typed into the wrong field.
        info!("authentication failed: {e}");
        return if matches!(status, PAM_AUTH_ERR | PAM_MAXTRIES) {
            Auth::Failed(e)
        } else {
            Auth::Error(e)
        };
    }
    // SAFETY: as above.
    let mut status = unsafe { pam_acct_mgmt(pam.handle, 0) };
    if status == PAM_NEW_AUTHTOK_REQD && expired == Expired::Remind {
        info!("the password has expired; unlocking anyway");
        let _ = pam
            .conv
            .ask(AuthMessageType::Error, EXPIRED_REMINDER.into());
        if let Some(ended) = ended(&pam) {
            return ended;
        }
        status = PAM_SUCCESS;
    } else if status == PAM_NEW_AUTHTOK_REQD {
        // The password expired (or an administrator asked for a new one):
        // the module has said so through the conversation, and now asks
        // for the new one. Only once that is set may the login go on.
        info!("the password has expired; asking for a new one");
        // SAFETY: as above.
        status = unsafe { pam_chauthtok(pam.handle, PAM_CHANGE_EXPIRED_AUTHTOK) };
        let changed = pam.check("pam_chauthtok", status);
        if let Some(ended) = ended(&pam) {
            return ended;
        }
        if let Err(e) = changed {
            return Auth::Failed(e);
        }
        info!("the password was changed");
        // SAFETY: as above.
        status = unsafe { pam_acct_mgmt(pam.handle, 0) };
    }
    if let Err(e) = pam.check("pam_acct_mgmt", status) {
        return Auth::Failed(e);
    }
    // The conversation is over; the session modules get no one to talk to,
    // and fail rather than block if one asks.
    pam.conv.channel = None;
    Auth::Ok(pam)
}

/// How the conversation ended early, if it did.
fn ended(pam: &Pam) -> Option<Auth> {
    let conv = &pam.conv;
    if conv.broken {
        Some(Auth::Broken)
    } else if conv.cancelled {
        Some(Auth::Cancelled)
    } else if conv.unexpected {
        Some(Auth::Error(
            "unexpected request during authentication".into(),
        ))
    } else {
        None
    }
}

/// Overwrites a secret's bytes before the allocation is freed.
pub(crate) fn wipe(secret: &mut str) {
    // SAFETY: zero bytes are valid UTF-8.
    unsafe { secret.as_bytes_mut() }.fill(0);
}

/// The user the process runs as.
pub fn current_user() -> Option<String> {
    // SAFETY: getuid cannot fail. getpwuid_r writes into `entry` and `buf`,
    // and `name` is read only while `buf` is alive.
    unsafe {
        let mut entry: libc::passwd = std::mem::zeroed();
        let mut buf = vec![0 as c_char; 4096];
        let mut result = ptr::null_mut();
        let status = libc::getpwuid_r(
            libc::getuid(),
            &mut entry,
            buf.as_mut_ptr(),
            buf.len(),
            &mut result,
        );
        if status != 0 || result.is_null() || entry.pw_name.is_null() {
            return None;
        }
        CStr::from_ptr(entry.pw_name)
            .to_str()
            .ok()
            .map(str::to_owned)
    }
}
