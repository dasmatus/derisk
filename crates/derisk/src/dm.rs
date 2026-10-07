//! `derisk display-manager`: the display manager, with the lock screen as its
//! greeter.
//!
//! It runs as root from a system service, one per seat, and does only what
//! needs root: PAM and starting sessions. It draws nothing. Each round it
//!
//! 1. starts the greeter (`derisk greeter`) as an unprivileged user, in a
//!    logind session of class `greeter` on the seat's VT, where it takes the
//!    display and input devices from logind;
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
    greetd::{self, Request, Response},
    password_age::PasswordAge,
    systemd,
};
use tracing::{error, info};

use crate::pam::{self, Auth, Pam};

type Result<T = ()> = std::result::Result<T, Box<dyn std::error::Error>>;

const PAM_ESTABLISH_CRED: c_int = 0x2;
const PAM_DELETE_CRED: c_int = 0x4;

const VT_ACTIVATE: libc::c_ulong = 0x5606;
const VT_WAITACTIVE: libc::c_ulong = 0x5607;

#[link(name = "pam")]
unsafe extern "C" {
    fn pam_putenv(pamh: *mut c_void, name_value: *const c_char) -> c_int;
    fn pam_getenvlist(pamh: *mut c_void) -> *mut *mut c_char;
    fn pam_setcred(pamh: *mut c_void, flags: c_int) -> c_int;
    fn pam_open_session(pamh: *mut c_void, flags: c_int) -> c_int;
    fn pam_close_session(pamh: *mut c_void, flags: c_int) -> c_int;
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

/// What the display manager needs of a PAM handle beyond authenticating:
/// the session's environment.
impl Pam {
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
                error!("{e}");
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
        return Err("an authenticated user has no account".into());
    };
    // Whose session it is goes to logind, which records it with the session;
    // the log says only that one started, since the name came from the
    // greeter's text field.
    info!("starting a session");
    apply_password_age(&username);
    let worker = spawn_session(pam, &user, options.vt, "user", &cmd, &env, true)?;
    wait(worker);
    info!("the session ended");
    Ok(())
}

/// Brings `user`'s homed record up to the system's password age, now that
/// logging in activated their home and a change can be written into it.
/// Only root may change those fields, so this is where an account made
/// before the policy (or under an older one) gets it. A failure is logged and
/// the login goes on: the policy is a reminder schedule, not a lock.
fn apply_password_age(user: &str) {
    let age = PasswordAge::system();
    let record = std::process::Command::new("homectl")
        .args(["inspect", user, "--json=short", "--no-pager"])
        .stderr(std::process::Stdio::null())
        .output()
        .ok()
        .and_then(|out| serde_json::from_slice(&out.stdout).ok())
        .unwrap_or(serde_json::Value::Null);
    let Some(argv) = age.update_argv(user, &record) else {
        return;
    };
    // Never stops to ask: the display manager has no terminal, and the
    // active home needs no password to update.
    let status = std::process::Command::new(&argv[0])
        .args(&argv[1..])
        .arg("--no-ask-password")
        .stdin(std::process::Stdio::null())
        .status();
    match status {
        Ok(s) if s.success() => info!("applied the password age policy"),
        Ok(s) => error!("homectl update for the password age policy failed: {s}"),
        Err(e) => error!("cannot run homectl for the password age policy: {e}"),
    }
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
                        Auth::Broken => return None,
                        // A cancel that ended the conversation gets its
                        // answer here, as does a refusal.
                        other => other.response().unwrap_or(Response::Success),
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

/// Runs PAM's authentication for `username`, with the greeter answering its
/// prompts over `stream`.
fn authenticate(options: &Options, stream: &mut UnixStream, username: &str) -> Auth {
    let Ok(channel) = stream.try_clone() else {
        return Auth::Broken;
    };
    pam::authenticate(
        &options.service,
        username,
        Some(options.vt),
        pam::Expired::Change,
        Box::new(channel),
    )
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
    // The keyboard layout first-boot setup or localectl saved: a Wayland
    // compositor has no X server to read localed's file, so the login
    // screen and the session get it as xkbcommon's defaults.
    let keyboard = derisk::locale::saved_keyboard(Path::new(derisk::locale::X11_KEYBOARD_CONF));
    for pair in derisk::locale::xkb_environment(&keyboard) {
        pam.putenv(&pair)?;
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
            error!("{e}");
            return 1;
        }
    }
    // Credentials come from authenticating, which the greeter never does:
    // its service's auth stack is free to refuse everything.
    if authenticated {
        // SAFETY: valid handle.
        let status = unsafe { pam_setcred(pam.handle, PAM_ESTABLISH_CRED) };
        if let Err(e) = pam.check("pam_setcred", status) {
            error!("{e}");
            return 1;
        }
    }
    // SAFETY: valid handle.
    let status = unsafe { pam_open_session(pam.handle, 0) };
    if let Err(e) = pam.check("pam_open_session", status) {
        error!("{e}");
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
        error!("fork for the session failed");
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
