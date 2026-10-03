//! Deep systemd integration, without linking libsystemd.
//!
//! - **Apps as units**: every launch becomes a transient
//!   `app-derisk-<AppID>@<random>.service` in `app-graphical.slice`, named per
//!   the XDG/systemd app-unit convention, so each app gets its own cgroup,
//!   resource accounting, journal stream and clean shutdown.
//! - **Focus-aware resources**: the focused app's unit gets higher CPU and IO
//!   weight; the previously focused unit returns to the default.
//! - **Service manager protocol**: `sd_notify` readiness, status and watchdog
//!   keep-alives; `LISTEN_FDS` socket activation for the agent socket.
//! - **journald**: structured native-protocol logging with a stderr fallback.
//! - **Session**: environment import into the user manager, the
//!   `derisk-session.target` lifecycle, logind lock/suspend/reboot/power-off,
//!   and failed-unit reporting for the overview.

use std::{
    io,
    os::unix::net::UnixDatagram,
    path::Path,
    process::{Command, Output},
    time::Duration,
};

use serde::{Deserialize, Serialize};

use crate::action::Effect;

/// The launcher component of app unit names.
pub const LAUNCHER: &str = "derisk";
/// The slice graphical apps run in.
pub const APP_SLICE: &str = "app-graphical.slice";
/// The session target started once the compositor is ready.
pub const SESSION_TARGET: &str = "derisk-session.target";
/// CPU/IO weight given to the focused app (systemd's default is 100).
pub const FOCUS_WEIGHT: u32 = 400;
/// Default CPU/IO weight.
pub const DEFAULT_WEIGHT: u32 = 100;

/// A logind / service-manager session operation.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SessionOp {
    /// Lock the session.
    Lock,
    /// Suspend to RAM.
    Suspend,
    /// Hibernate.
    Hibernate,
    /// End the session.
    Logout,
    /// Reboot.
    Reboot,
    /// Power off.
    PowerOff,
}

impl SessionOp {
    /// Whether the operation loses unsaved work and needs explicit confirmation.
    pub fn is_destructive(self) -> bool {
        matches!(self, Self::Logout | Self::Reboot | Self::PowerOff)
    }
}

/// Escapes a string for use in a unit name, like `systemd-escape`.
pub fn escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for (i, b) in s.bytes().enumerate() {
        match b {
            b'/' => out.push('-'),
            b'.' if i == 0 => out.push_str("\\x2e"),
            b'a'..=b'z' | b'A'..=b'Z' | b'0'..=b'9' | b':' | b'_' | b'.' => out.push(b as char),
            _ => out.push_str(&format!("\\x{b:02x}")),
        }
    }
    out
}

/// Whether `app` is a plain command name or desktop-file ID we will launch.
///
/// Paths, arguments and shell syntax are rejected so that agents can only
/// start installed applications, not arbitrary command lines.
pub fn is_launchable(app: &str) -> bool {
    !app.is_empty()
        && app.len() <= 128
        && !app.starts_with(['-', '.'])
        && app
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b'+'))
}

/// The transient unit name for an app instance.
pub fn app_unit(app: &str, instance: u64) -> String {
    format!(
        "app-{LAUNCHER}-{}@{instance:x}.service",
        escape(app.strip_suffix(".desktop").unwrap_or(app))
    )
}

/// `systemd-run` arguments that launch `app` as a transient user service.
///
/// A desktop-file ID (`foo.desktop`) runs the command `foo`. Returns `None`
/// for names rejected by [`is_launchable`].
pub fn launch_argv(app: &str, instance: u64) -> Option<Vec<String>> {
    if !is_launchable(app) {
        return None;
    }
    let exec = app.strip_suffix(".desktop").unwrap_or(app);
    Some(
        [
            "systemd-run",
            "--user",
            "--no-block",
            "--collect",
            "--quiet",
            &format!("--unit={}", app_unit(app, instance)),
            &format!("--slice={APP_SLICE}"),
            "--property=Type=exec",
            "--property=ExitType=cgroup",
            &format!("--description={exec} (derisk)"),
            "--",
            exec,
        ]
        .map(str::to_owned)
        .to_vec(),
    )
}

/// Command line for a session operation.
pub fn session_argv(op: SessionOp, session_id: Option<&str>) -> Vec<String> {
    let args: Vec<&str> = match op {
        SessionOp::Lock => match session_id {
            Some(id) => vec!["loginctl", "lock-session", id],
            None => vec!["loginctl", "lock-sessions"],
        },
        SessionOp::Suspend => vec!["systemctl", "suspend"],
        SessionOp::Hibernate => vec!["systemctl", "hibernate"],
        SessionOp::Reboot => vec!["systemctl", "reboot"],
        SessionOp::PowerOff => vec!["systemctl", "poweroff"],
        SessionOp::Logout => match session_id {
            Some(id) => vec!["loginctl", "terminate-session", id],
            None => vec!["systemctl", "--user", "stop", SESSION_TARGET],
        },
    };
    args.into_iter().map(str::to_owned).collect()
}

/// Commands that run an effect through systemd, if it needs any.
///
/// `instance` makes launched unit names unique. Effects handled over Wayland
/// (closing windows, menu activation) return `None`.
pub fn effect_argv(effect: &Effect, instance: u64, session_id: Option<&str>) -> Option<Vec<String>> {
    match effect {
        Effect::Launch { app } => launch_argv(app, instance),
        Effect::Session { op } => Some(session_argv(*op, session_id)),
        Effect::ResetFailed { unit } => Some(
            ["systemctl", "--user", "reset-failed", "--", unit.as_str()]
                .map(str::to_owned)
                .to_vec(),
        ),
        Effect::RestartUnit { unit } => Some(
            ["systemctl", "--user", "restart", "--no-block", "--", unit.as_str()]
                .map(str::to_owned)
                .to_vec(),
        ),
        Effect::Close { .. } | Effect::MenuActivated { .. } | Effect::TrayActivated { .. } => None,
    }
}

/// Commands to run once the compositor's Wayland socket is up.
///
/// Exports the session environment to the user manager and D-Bus activation
/// environment, then starts [`SESSION_TARGET`] (which binds
/// `graphical-session.target`), so portals, tray hosts and autostart units see
/// the right display.
pub fn session_start_argv() -> Vec<Vec<String>> {
    const VARS: [&str; 5] = [
        "WAYLAND_DISPLAY",
        "XDG_CURRENT_DESKTOP",
        "XDG_SESSION_TYPE",
        "XDG_SESSION_DESKTOP",
        "DISPLAY",
    ];
    let with = |prefix: &[&str]| -> Vec<String> {
        prefix
            .iter()
            .chain(VARS.iter())
            .map(|s| (*s).to_owned())
            .collect()
    };
    vec![
        with(&["systemctl", "--user", "import-environment"]),
        with(&["dbus-update-activation-environment", "--systemd"]),
        ["systemctl", "--user", "--no-block", "start", SESSION_TARGET]
            .map(str::to_owned)
            .to_vec(),
    ]
}

/// The unit owning a process, parsed from `/proc/<pid>/cgroup` (cgroup v2).
pub fn unit_from_cgroup(cgroup: &str) -> Option<String> {
    let path = cgroup.lines().find_map(|l| l.strip_prefix("0::"))?;
    path.rsplit('/')
        .find(|c| c.ends_with(".service") || c.ends_with(".scope"))
        .map(str::to_owned)
}

/// The unit owning `pid`, if it can be read.
pub fn unit_of_pid(pid: u32) -> Option<String> {
    unit_from_cgroup(&std::fs::read_to_string(format!("/proc/{pid}/cgroup")).ok()?)
}

/// Gives the focused app's unit more CPU/IO weight than background apps.
#[derive(Clone, Debug, Default)]
pub struct FocusBoost {
    boosted: Option<String>,
}

impl FocusBoost {
    /// Commands to move the boost to `unit`; empty if it already has it.
    ///
    /// Only app units (`app-*`) are touched, never the session or system.
    pub fn focus(&mut self, unit: Option<&str>) -> Vec<Vec<String>> {
        let unit = unit.filter(|u| u.starts_with("app-")).map(str::to_owned);
        if unit == self.boosted {
            return Vec::new();
        }
        let set = |unit: &str, weight: u32| -> Vec<String> {
            [
                "systemctl",
                "--user",
                "set-property",
                "--runtime",
                "--",
                unit,
                &format!("CPUWeight={weight}"),
                &format!("IOWeight={weight}"),
            ]
            .map(str::to_owned)
            .to_vec()
        };
        let mut commands = Vec::new();
        if let Some(old) = self.boosted.take() {
            commands.push(set(&old, DEFAULT_WEIGHT));
        }
        if let Some(new) = &unit {
            commands.push(set(new, FOCUS_WEIGHT));
        }
        self.boosted = unit;
        commands
    }
}

/// Parses `systemctl --user list-units --state=failed --plain --no-legend`.
pub fn parse_failed_units(output: &str) -> Vec<String> {
    output
        .lines()
        .filter_map(|l| l.split_whitespace().next())
        .filter(|u| u.contains('.'))
        .map(str::to_owned)
        .collect()
}

/// Runs a command without a shell, returning its output.
pub fn run(argv: &[String]) -> io::Result<Output> {
    let (program, args) = argv
        .split_first()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "empty command"))?;
    Command::new(program).args(args).output()
}

/// Currently failed user units.
pub fn failed_units() -> Vec<String> {
    let argv = [
        "systemctl",
        "--user",
        "list-units",
        "--state=failed",
        "--plain",
        "--no-legend",
        "--no-pager",
    ]
    .map(str::to_owned);
    run(&argv)
        .ok()
        .filter(|o| o.status.success())
        .map(|o| parse_failed_units(&String::from_utf8_lossy(&o.stdout)))
        .unwrap_or_default()
}

fn notify_socket() -> Option<UnixDatagram> {
    let path = std::env::var_os("NOTIFY_SOCKET")?;
    let socket = UnixDatagram::unbound().ok()?;
    let bytes = path.as_encoded_bytes();
    if let Some(name) = bytes.strip_prefix(b"@") {
        use std::os::linux::net::SocketAddrExt;
        let addr = std::os::unix::net::SocketAddr::from_abstract_name(name).ok()?;
        socket.connect_addr(&addr).ok()?;
    } else {
        socket.connect(Path::new(&path)).ok()?;
    }
    Some(socket)
}

/// Sends an `sd_notify` message (e.g. `READY=1`). Returns false outside systemd.
pub fn notify(state: &str) -> bool {
    notify_socket().is_some_and(|s| s.send(state.as_bytes()).is_ok())
}

/// Tells the service manager the shell is ready, with a status line.
pub fn notify_ready(status: &str) -> bool {
    notify(&format!("READY=1\nSTATUS={}", status.replace('\n', " ")))
}

/// Updates the status shown by `systemctl status`.
pub fn notify_status(status: &str) -> bool {
    notify(&format!("STATUS={}", status.replace('\n', " ")))
}

/// Sends a watchdog keep-alive.
pub fn notify_watchdog() -> bool {
    notify("WATCHDOG=1")
}

/// Announces shutdown.
pub fn notify_stopping() -> bool {
    notify("STOPPING=1")
}

/// How often to send watchdog keep-alives (half of `WatchdogSec=`), if enabled.
pub fn watchdog_interval() -> Option<Duration> {
    watchdog_interval_from(
        std::env::var("WATCHDOG_USEC").ok().as_deref(),
        std::env::var("WATCHDOG_PID").ok().as_deref(),
        std::process::id(),
    )
}

/// [`watchdog_interval`] from explicit values.
pub fn watchdog_interval_from(usec: Option<&str>, pid: Option<&str>, me: u32) -> Option<Duration> {
    if pid.is_some_and(|p| p.parse::<u32>().ok() != Some(me)) {
        return None;
    }
    let usec: u64 = usec?.parse().ok().filter(|u| *u > 0)?;
    Some(Duration::from_micros(usec / 2))
}

/// The first socket-activation file descriptor passed to this process.
///
/// Returns `None` unless `LISTEN_PID` names this process and `LISTEN_FDS`
/// is at least one. The descriptor is always 3 (`SD_LISTEN_FDS_START`).
pub fn listen_fd() -> Option<i32> {
    listen_fd_from(
        std::env::var("LISTEN_PID").ok().as_deref(),
        std::env::var("LISTEN_FDS").ok().as_deref(),
        std::process::id(),
    )
}

/// [`listen_fd`] from explicit values.
pub fn listen_fd_from(pid: Option<&str>, fds: Option<&str>, me: u32) -> Option<i32> {
    (pid?.parse::<u32>().ok()? == me && fds?.parse::<u32>().ok()? >= 1).then_some(3)
}

/// Journal priority.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Priority {
    /// Errors.
    Error = 3,
    /// Warnings.
    Warning = 4,
    /// Notable events.
    Notice = 5,
    /// Informational.
    Info = 6,
    /// Debugging.
    Debug = 7,
}

/// Encodes fields in journald's native protocol.
pub fn journal_payload(priority: Priority, message: &str, fields: &[(&str, &str)]) -> Vec<u8> {
    let mut out = Vec::new();
    let priority = (priority as u8).to_string();
    let all = [
        ("MESSAGE", message),
        ("PRIORITY", priority.as_str()),
        ("SYSLOG_IDENTIFIER", LAUNCHER),
    ];
    for (key, value) in all.iter().chain(fields) {
        out.extend_from_slice(key.as_bytes());
        if value.contains('\n') {
            out.push(b'\n');
            out.extend_from_slice(&(value.len() as u64).to_le_bytes());
            out.extend_from_slice(value.as_bytes());
        } else {
            out.push(b'=');
            out.extend_from_slice(value.as_bytes());
        }
        out.push(b'\n');
    }
    out
}

/// Logs a structured message to journald, or to stderr if it is unavailable.
///
/// Field names must be uppercase ASCII letters, digits and underscores.
pub fn log(priority: Priority, message: &str, fields: &[(&str, &str)]) {
    let fields: Vec<(&str, &str)> = fields
        .iter()
        .copied()
        .filter(|(k, _)| {
            !k.is_empty()
                && !k.starts_with('_')
                && k.bytes()
                    .all(|b| b.is_ascii_uppercase() || b.is_ascii_digit() || b == b'_')
        })
        .collect();
    let sent = UnixDatagram::unbound().is_ok_and(|s| {
        s.send_to(
            &journal_payload(priority, message, &fields),
            "/run/systemd/journal/socket",
        )
        .is_ok()
    });
    if !sent {
        eprintln!("<{}>{LAUNCHER}: {message}", priority as u8);
    }
}
