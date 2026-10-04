//! `derisk display-manager`: the display manager, with the lock screen as its
//! greeter.
//!
//! It runs as root from a system service, one per seat, and does only what
//! needs root: PAM and starting sessions. It draws nothing. Each round it
//!
//! 1. starts the greeter (`derisk greeter`, in cage) as an unprivileged user,
//!    in a logind session of class `greeter` on the seat's VT;
//! 2. serves greetd's protocol to it over a socket only that user can open
//!    (`$GREETD_SOCK`), running `pam_authenticate` for whoever it names and
//!    relaying PAM's prompts as the conversation;
//! 3. once the greeter asked to start a session for an authenticated user and
//!    exited, opens that user's PAM session (pam_systemd registers it with
//!    logind; pam_systemd_home unlocks a homed home area) and runs the
//!    session command as the user;
//! 4. waits for the session to end, closes it, and goes back to 1.
//!
//! The greeter is the same program greetd would run, so the code that draws
//! the login screen and the lock screen stays shared and unprivileged, and the
//! root half stays small enough to read.
//!
//! The daemon is single-threaded, so forking is safe; each session runs in a
//! worker process that holds its PAM handle open while the session lives,
//! the way login(1) and every other display manager do.

use std::{
    ffi::{CStr, CString, c_char, c_int, c_void},
    fs,
    io::{self, ErrorKind},
    os::unix::{
        fs::{PermissionsExt, chown},
        net::{UnixListener, UnixStream},
    },
    path::{Path, PathBuf},
    ptr,
    time::Duration,
};

use derisk::{
    greetd::{self, AuthMessageType, Request, Response},
    systemd::{self, Priority},
};

use crate::pam::{
    self, PAM_BUF_ERR, PAM_CONV_ERR, PAM_ERROR_MSG, PAM_PROMPT_ECHO_OFF, PAM_PROMPT_ECHO_ON,
    PAM_SUCCESS, PAM_TEXT_INFO, PamConv, PamMessage, PamResponse,
};

type Result<T = ()> = std::result::Result<T, Box<dyn std::error::Error>>;

const PAM_TTY: c_int = 3;
const PAM_ESTABLISH_CRED: c_int = 0x2;
const PAM_DELETE_CRED: c_int = 0x4;
/// Tells modules a pam_end is the parent's copy after a fork, so they free
/// memory without tearing down what the child still uses.
const PAM_DATA_SILENT: c_int = 0x4000_0000;

const VT_ACTIVATE: libc::c_ulong = 0x5606;
const VT_WAITACTIVE: libc::c_ulong = 0x5607;

#[link(name = "pam")]
unsafe extern "C" {
    fn pam_set_item(pamh: *mut c_void, item_type: c_int, item: *const c_void) -> c_int;
    fn pam_putenv(pamh: *mut c_void, name_value: *const c_char) -> c_int;
    fn pam_getenvlist(pamh: *mut c_void) -> *mut *mut c_char;
    fn pam_setcred(pamh: *mut c_void, flags: c_int) -> c_int;
    fn pam_open_session(pamh: *mut c_void, flags: c_int) -> c_int;
    fn pam_close_session(pamh: *mut c_void, flags: c_int) -> c_int;
    fn pam_strerror(pamh: *mut c_void, errnum: c_int) -> *const c_char;
}

/// How the display manager was configured.
#[derive(Debug)]
pub struct Options {
    /// The VT the greeter and sessions run on.
    pub vt: u32,
    /// The unprivileged user the greeter runs as.
    pub greeter_user: String,
    /// The PAM service for the greeter's own session (no authentication;
    /// account and session only, so logind registers it).
    pub greeter_service: String,
    /// The PAM service users log in with.
    pub service: String,
    /// The greeter's command line.
    pub greeter: Vec<String>,
    /// Where the greeter's socket goes.
    pub runtime_dir: PathBuf,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            vt: 1,
            greeter_user: "derisk-greeter".into(),
            greeter_service: "derisk-greeter".into(),
            service: "derisk-login".into(),
            greeter: Vec::new(),
            runtime_dir: PathBuf::from("/run/derisk-dm"),
        }
    }
}

/// A user account, from NSS (so homed users are found too).
struct Account {
    name: CString,
    uid: libc::uid_t,
    gid: libc::gid_t,
    home: CString,
}

fn account(name: &str) -> Option<Account> {
    let cname = CString::new(name).ok()?;
    // SAFETY: getpwnam_r writes into `entry` and `buf`; the strings are copied
    // out while `buf` is alive.
    unsafe {
        let mut entry: libc::passwd = std::mem::zeroed();
        let mut buf = vec![0 as c_char; 16384];
        let mut result = ptr::null_mut();
        let status = libc::getpwnam_r(
            cname.as_ptr(),
            &mut entry,
            buf.as_mut_ptr(),
            buf.len(),
            &mut result,
        );
        if status != 0 || result.is_null() {
            return None;
        }
        let home = if entry.pw_dir.is_null() {
            c"/".to_owned()
        } else {
            CStr::from_ptr(entry.pw_dir).to_owned()
        };
        Some(Account {
            name: cname,
            uid: entry.pw_uid,
            gid: entry.pw_gid,
            home,
        })
    }
}

/// An open PAM handle. Ended exactly once: on drop, or handed to a session
/// worker by [`Pam::forget`].
struct Pam {
    handle: *mut c_void,
    /// The conversation's state; PAM keeps a pointer to it, so it is boxed
    /// and lives as long as the handle.
    conv: Box<Conversation>,
    status: c_int,
}

impl Pam {
    fn start(service: &str, user: &str, stream: Option<UnixStream>) -> io::Result<Self> {
        let service = CString::new(service).map_err(io::Error::other)?;
        let user = CString::new(user).map_err(io::Error::other)?;
        let mut conv = Box::new(Conversation {
            stream,
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
        let status =
            unsafe { pam::pam_start(service.as_ptr(), user.as_ptr(), &*pam_conv, &mut handle) };
        if status != PAM_SUCCESS || handle.is_null() {
            return Err(io::Error::other(format!("pam_start failed ({status})")));
        }
        Ok(Self {
            handle,
            conv,
            status,
        })
    }

    fn error(&self, status: c_int) -> String {
        // SAFETY: pam_strerror returns a static string.
        unsafe { CStr::from_ptr(pam_strerror(self.handle, status)) }
            .to_string_lossy()
            .into_owned()
    }

    fn check(&mut self, what: &str, status: c_int) -> std::result::Result<(), String> {
        self.status = status;
        if status == PAM_SUCCESS {
            Ok(())
        } else {
            Err(format!("{what}: {}", self.error(status)))
        }
    }

    fn set_tty(&mut self, vt: u32) -> std::result::Result<(), String> {
        let tty = CString::new(format!("tty{vt}")).unwrap_or_default();
        // SAFETY: PAM copies the string.
        let status = unsafe { pam_set_item(self.handle, PAM_TTY, tty.as_ptr().cast()) };
        self.check("pam_set_item(PAM_TTY)", status)
    }

    fn putenv(&mut self, pair: &str) -> std::result::Result<(), String> {
        let pair = CString::new(pair).map_err(|e| e.to_string())?;
        // SAFETY: PAM copies the string.
        let status = unsafe { pam_putenv(self.handle, pair.as_ptr()) };
        self.check("pam_putenv", status)
    }

    /// The environment PAM's session modules built (XDG_RUNTIME_DIR,
    /// XDG_SESSION_ID and the like).
    fn env(&self) -> Vec<CString> {
        let mut out = Vec::new();
        // SAFETY: pam_getenvlist returns a NULL-terminated array of malloc'd
        // strings, which the caller frees along with the array.
        unsafe {
            let list = pam_getenvlist(self.handle);
            if list.is_null() {
                return out;
            }
            let mut i = 0;
            while !(*list.add(i)).is_null() {
                let entry = *list.add(i);
                out.push(CStr::from_ptr(entry).to_owned());
                libc::free(entry.cast());
                i += 1;
            }
            libc::free(list.cast());
        }
        out
    }

    /// Lets go of the handle without ending the PAM transaction, in the
    /// process that forked a worker to own it.
    fn forget(mut self) {
        // SAFETY: the handle is valid; PAM_DATA_SILENT frees this copy only.
        unsafe { pam::pam_end(self.handle, self.status | PAM_DATA_SILENT) };
        self.handle = ptr::null_mut();
    }
}

impl Drop for Pam {
    fn drop(&mut self) {
        if !self.handle.is_null() {
            // SAFETY: the handle is valid and ended exactly once.
            unsafe { pam::pam_end(self.handle, self.status) };
        }
    }
}

/// PAM's conversation, relayed to the greeter as greetd's protocol: each PAM
/// message becomes an `auth_message` response, and the greeter's next
/// request answers it.
struct Conversation {
    stream: Option<UnixStream>,
    /// The greeter cancelled mid-conversation; its cancel is still to be
    /// answered.
    cancelled: bool,
    /// The greeter sent something other than an answer or a cancel.
    unexpected: bool,
    /// The socket failed; the greeter is gone.
    broken: bool,
}

impl Conversation {
    /// Asks the greeter one thing. `Ok(None)` when it cancelled.
    fn ask(&mut self, kind: AuthMessageType, text: String) -> Option<Option<String>> {
        let stream = self.stream.as_mut()?;
        let message = Response::AuthMessage {
            auth_message_type: kind,
            auth_message: text,
        };
        let request =
            greetd::write_response(stream, &message).and_then(|()| greetd::read_request(stream));
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
            pam::wipe(&mut answer);
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

/// Who is in the middle of logging in, and what to start for them.
struct Pending {
    pam: Pam,
    username: String,
    start: Option<(Vec<String>, Vec<String>)>,
}

/// Runs the display manager until it is stopped.
pub fn run(options: Options) -> Result {
    if options.greeter.is_empty() {
        return Err("derisk display-manager needs the greeter's command after `--`".into());
    }
    // SAFETY: geteuid cannot fail.
    if unsafe { libc::geteuid() } != 0 {
        return Err("derisk display-manager must run as root".into());
    }
    let greeter = account(&options.greeter_user)
        .ok_or_else(|| format!("no greeter user {}", options.greeter_user))?;
    systemd::notify_ready("derisk display manager running");
    loop {
        activate_vt(options.vt);
        match round(&options, &greeter) {
            Ok(()) => {}
            Err(e) => {
                log(Priority::Error, &format!("{e}"));
                // Don't spin if something is persistently broken.
                std::thread::sleep(Duration::from_secs(2));
            }
        }
    }
}

/// One greeter, and the session it asks for, if any.
fn round(options: &Options, greeter: &Account) -> Result {
    fs::create_dir_all(&options.runtime_dir)?;
    fs::set_permissions(&options.runtime_dir, fs::Permissions::from_mode(0o711))?;
    let socket = options
        .runtime_dir
        .join(format!("greeter-vt{}.sock", options.vt));
    match fs::remove_file(&socket) {
        Err(e) if e.kind() != ErrorKind::NotFound => return Err(e.into()),
        _ => {}
    }
    let listener = UnixListener::bind(&socket)?;
    // Only the greeter user may talk to the display manager: it is what can
    // ask for anyone's password to be checked.
    chown(&socket, Some(greeter.uid), Some(greeter.gid))?;
    fs::set_permissions(&socket, fs::Permissions::from_mode(0o600))?;
    listener.set_nonblocking(true)?;

    let greeter_name = greeter.name.to_string_lossy().into_owned();
    let pam = Pam::start(&options.greeter_service, &greeter_name, None)?;
    let env = vec![format!("GREETD_SOCK={}", socket.display())];
    let worker = spawn_session(
        pam,
        greeter,
        options.vt,
        "greeter",
        &options.greeter,
        &env,
        false,
    )?;

    // Wait for the greeter to connect, or to die before it does.
    let stream = loop {
        match listener.accept() {
            Ok((stream, _)) => break Some(stream),
            Err(e) if e.kind() == ErrorKind::WouldBlock => {
                if exited(worker) {
                    break None;
                }
                std::thread::sleep(Duration::from_millis(100));
            }
            Err(e) => return Err(e.into()),
        }
    };
    let _ = fs::remove_file(&socket);
    let pending = match stream {
        Some(stream) => {
            stream.set_nonblocking(false)?;
            let pending = serve(options, stream);
            wait(worker);
            pending
        }
        None => {
            // Most likely it cannot start at all; retrying at once would
            // only fill the journal.
            return Err("the greeter exited before it connected".into());
        }
    };
    let Some(Pending {
        pam,
        username,
        start: Some((cmd, env)),
    }) = pending
    else {
        return Ok(());
    };
    let Some(user) = account(&username) else {
        return Err(format!("{username} authenticated but has no account").into());
    };
    log(
        Priority::Notice,
        &format!("starting a session for {username}"),
    );
    let worker = spawn_session(pam, &user, options.vt, "user", &cmd, &env, true)?;
    wait(worker);
    log(Priority::Notice, &format!("session for {username} ended"));
    Ok(())
}

/// Serves the greeter until it hangs up. Returns the authenticated user and
/// the session they asked for, if the greeter got that far.
fn serve(options: &Options, mut stream: UnixStream) -> Option<Pending> {
    let mut pending: Option<Pending> = None;
    loop {
        let request = match greetd::read_request(&mut stream) {
            Ok(request) => request,
            // EOF: the greeter is done (it exits after a start).
            Err(_) => return pending,
        };
        let response = match request {
            Request::CreateSession { username } => {
                if pending.is_some() {
                    error("a session is already being set up")
                } else {
                    match authenticate(options, &mut stream, &username) {
                        Auth::Ok(pam) => {
                            pending = Some(Pending {
                                pam,
                                username,
                                start: None,
                            });
                            Response::Success
                        }
                        // The cancel that ended the conversation gets its
                        // answer here.
                        Auth::Cancelled => Response::Success,
                        Auth::Failed(description) => Response::Error {
                            error_type: "auth_error".into(),
                            description,
                        },
                        Auth::Error(description) => error(&description),
                        Auth::Broken => return None,
                    }
                }
            }
            Request::StartSession { cmd, env } => match pending.as_mut() {
                Some(p) if !cmd.is_empty() => {
                    p.start = Some((cmd, env));
                    Response::Success
                }
                Some(_) => error("no command to start"),
                None => error("no authenticated session to start"),
            },
            Request::CancelSession => {
                if pending.as_ref().is_some_and(|p| p.start.is_some()) {
                    error("the session is already started")
                } else {
                    pending = None;
                    Response::Success
                }
            }
            Request::PostAuthMessageResponse { .. } => error("no question was asked"),
        };
        if greetd::write_response(&mut stream, &response).is_err() {
            return pending;
        }
    }
}

fn error(description: &str) -> Response {
    Response::Error {
        error_type: "error".into(),
        description: description.into(),
    }
}

enum Auth {
    Ok(Pam),
    Cancelled,
    Failed(String),
    Error(String),
    Broken,
}

/// Runs PAM's authentication for `username`, with the greeter answering its
/// prompts over `stream`.
fn authenticate(options: &Options, stream: &mut UnixStream, username: &str) -> Auth {
    let Ok(conv_stream) = stream.try_clone() else {
        return Auth::Broken;
    };
    let mut pam = match Pam::start(&options.service, username, Some(conv_stream)) {
        Ok(pam) => pam,
        Err(e) => return Auth::Error(e.to_string()),
    };
    if let Err(e) = pam.set_tty(options.vt) {
        return Auth::Error(e);
    }
    // SAFETY: the handle is valid; the conversation lives in `pam`.
    let status = unsafe { pam::pam_authenticate(pam.handle, 0) };
    let auth = pam.check("pam_authenticate", status);
    let conv = &pam.conv;
    if conv.broken {
        return Auth::Broken;
    }
    if conv.cancelled {
        return Auth::Cancelled;
    }
    if conv.unexpected {
        return Auth::Error("unexpected request during authentication".into());
    }
    if let Err(e) = auth {
        log(
            Priority::Notice,
            &format!("login for {username} failed: {e}"),
        );
        return Auth::Failed(e);
    }
    // SAFETY: as above.
    let status = unsafe { pam::pam_acct_mgmt(pam.handle, 0) };
    if let Err(e) = pam.check("pam_acct_mgmt", status) {
        return Auth::Failed(e);
    }
    // The conversation is over; the session modules get no greeter to talk
    // to, and fail rather than block if one asks.
    pam.conv.stream = None;
    Auth::Ok(pam)
}

/// Opens `pam`'s session for `user` on `vt` and runs `cmd` as them, in a
/// worker process that closes the session when the command exits. Returns
/// the worker's PID. `authenticated` sessions establish credentials (the
/// greeter's has none to establish).
fn spawn_session(
    mut pam: Pam,
    user: &Account,
    vt: u32,
    class: &str,
    cmd: &[String],
    env: &[String],
    authenticated: bool,
) -> Result<libc::pid_t> {
    pam.set_tty(vt)?;
    for pair in [
        "XDG_SEAT=seat0".to_owned(),
        format!("XDG_VTNR={vt}"),
        format!("XDG_SESSION_CLASS={class}"),
        "XDG_SESSION_TYPE=wayland".to_owned(),
    ] {
        pam.putenv(&pair)?;
    }
    // Session type and desktop come from the greeter for a user session.
    for pair in env {
        if pair.starts_with("XDG_SESSION_TYPE=") || pair.starts_with("XDG_SESSION_DESKTOP=") {
            pam.putenv(pair)?;
        }
    }
    let argv: Vec<CString> = shell_argv(cmd)?;
    // SAFETY: the daemon is single-threaded, so the child may allocate.
    match unsafe { libc::fork() } {
        -1 => Err(io::Error::last_os_error().into()),
        0 => {
            let code = session_worker(pam, user, vt, &argv, env, authenticated);
            // SAFETY: leaves without running the parent's destructors twice.
            unsafe { libc::_exit(code) }
        }
        pid => {
            pam.forget();
            Ok(pid)
        }
    }
}

/// The command, run through `sh` so /etc/profile sets up PATH and the rest
/// of a login environment first, as greetd and login(1) do.
fn shell_argv(cmd: &[String]) -> Result<Vec<CString>> {
    let script = "[ -f /etc/profile ] && . /etc/profile; exec \"$@\"";
    ["/bin/sh", "-c", script, "sh"]
        .into_iter()
        .map(str::to_owned)
        .chain(cmd.iter().cloned())
        .map(|a| CString::new(a).map_err(Into::into))
        .collect()
}

fn session_worker(
    mut pam: Pam,
    user: &Account,
    vt: u32,
    argv: &[CString],
    env: &[String],
    authenticated: bool,
) -> c_int {
    // A new session with the VT as its controlling terminal, so logind sees
    // the session on that VT and the kernel delivers its signals.
    // SAFETY: plain syscalls on descriptors this process owns.
    unsafe {
        libc::setsid();
        let tty = CString::new(format!("/dev/tty{vt}")).unwrap_or_default();
        let fd = libc::open(tty.as_ptr(), libc::O_RDWR | libc::O_NOCTTY);
        if fd >= 0 {
            libc::ioctl(fd, libc::TIOCSCTTY, 1);
            libc::dup2(fd, 0);
            if fd > 2 {
                libc::close(fd);
            }
        }
    }
    if !authenticated {
        // SAFETY: valid handle.
        let status = unsafe { pam::pam_acct_mgmt(pam.handle, 0) };
        if let Err(e) = pam.check("pam_acct_mgmt", status) {
            log(Priority::Error, &e);
            return 1;
        }
    }
    // Credentials come from authenticating, which the greeter never does:
    // its service's auth stack is free to refuse everything.
    if authenticated {
        // SAFETY: valid handle.
        let status = unsafe { pam_setcred(pam.handle, PAM_ESTABLISH_CRED) };
        if let Err(e) = pam.check("pam_setcred", status) {
            log(Priority::Error, &e);
            return 1;
        }
    }
    // SAFETY: valid handle.
    let status = unsafe { pam_open_session(pam.handle, 0) };
    if let Err(e) = pam.check("pam_open_session", status) {
        log(Priority::Error, &e);
        if authenticated {
            // SAFETY: valid handle.
            unsafe { pam_setcred(pam.handle, PAM_DELETE_CRED) };
        }
        return 1;
    }
    let mut envp = pam.env();
    let set = |envp: &mut Vec<CString>, pair: String| {
        let key = pair.split('=').next().unwrap_or_default().to_owned() + "=";
        envp.retain(|e| !e.to_bytes().starts_with(key.as_bytes()));
        if let Ok(pair) = CString::new(pair) {
            envp.push(pair);
        }
    };
    let name = user.name.to_string_lossy().into_owned();
    let home = user.home.to_string_lossy().into_owned();
    for pair in env {
        set(&mut envp, pair.clone());
    }
    set(&mut envp, format!("HOME={home}"));
    set(&mut envp, format!("USER={name}"));
    set(&mut envp, format!("LOGNAME={name}"));
    if !envp.iter().any(|e| e.to_bytes().starts_with(b"PATH=")) {
        set(
            &mut envp,
            "PATH=/run/current-system/sw/bin:/usr/bin:/bin".into(),
        );
    }

    // SAFETY: still single-threaded after the first fork.
    let child = unsafe { libc::fork() };
    if child == 0 {
        // SAFETY: dropping privileges for good, then exec; only _exit on
        // failure, never return into the worker's code.
        unsafe {
            if libc::initgroups(user.name.as_ptr(), user.gid) != 0
                || libc::setgid(user.gid) != 0
                || libc::setuid(user.uid) != 0
                || libc::setuid(0) == 0
            {
                libc::_exit(126);
            }
            if libc::chdir(user.home.as_ptr()) != 0 {
                libc::chdir(c"/".as_ptr());
            }
            let mut args: Vec<*const c_char> = argv.iter().map(|a| a.as_ptr()).collect();
            args.push(ptr::null());
            let mut vars: Vec<*const c_char> = envp.iter().map(|e| e.as_ptr()).collect();
            vars.push(ptr::null());
            libc::execve(args[0], args.as_ptr(), vars.as_ptr());
            libc::_exit(127);
        }
    }
    if child > 0 {
        wait(child);
    } else {
        log(Priority::Error, "fork for the session failed");
    }
    // SAFETY: valid handle; close what was opened above.
    unsafe {
        let status = pam_close_session(pam.handle, 0);
        if authenticated {
            pam_setcred(pam.handle, PAM_DELETE_CRED);
        }
        pam.status = status;
    }
    0
}

/// Whether `pid` has exited, reaping it if so.
fn exited(pid: libc::pid_t) -> bool {
    let mut status = 0;
    // SAFETY: waitpid on our own child.
    let r = unsafe { libc::waitpid(pid, &mut status, libc::WNOHANG) };
    r == pid || r == -1
}

/// Waits for `pid` to exit.
fn wait(pid: libc::pid_t) {
    let mut status = 0;
    loop {
        // SAFETY: waitpid on our own child.
        let r = unsafe { libc::waitpid(pid, &mut status, 0) };
        if r == pid || (r == -1 && io::Error::last_os_error().kind() != ErrorKind::Interrupted) {
            return;
        }
    }
}

/// Brings `vt` to the front, so the session on it is the seat's active one.
fn activate_vt(vt: u32) {
    let Ok(console) = fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(Path::new("/dev/tty0"))
    else {
        return;
    };
    use std::os::fd::AsRawFd;
    let fd = console.as_raw_fd();
    // SAFETY: VT ioctls on an open console descriptor, with an int argument.
    unsafe {
        libc::ioctl(fd, VT_ACTIVATE, vt as c_int);
        libc::ioctl(fd, VT_WAITACTIVE, vt as c_int);
    }
}

fn log(priority: Priority, message: &str) {
    systemd::log(priority, message, &[]);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_command_runs_after_the_profile() {
        let argv = shell_argv(&["derisk".into(), "greeter".into()]).unwrap();
        let argv: Vec<_> = argv.iter().map(|a| a.to_str().unwrap()).collect();
        assert_eq!(argv[0], "/bin/sh");
        assert_eq!(argv[1], "-c");
        assert!(argv[2].ends_with("exec \"$@\""));
        assert_eq!(&argv[3..], ["sh", "derisk", "greeter"]);
    }
}
