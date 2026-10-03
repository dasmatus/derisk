use std::{os::unix::net::UnixDatagram, process::Command, time::Duration};

use derisk::{
    action::Effect,
    systemd::{self, FocusBoost, Priority, SessionOp},
};

#[test]
fn apps_launch_as_transient_units_in_the_app_slice() {
    let argv = systemd::launch_argv("org.gnome.Nautilus.desktop", 7).unwrap();
    assert_eq!(&argv[..2], ["systemd-run", "--user"]);
    assert!(argv.contains(&"--slice=app-graphical.slice".to_owned()));
    let unit = argv.iter().find_map(|a| a.strip_prefix("--unit=")).unwrap();
    assert!(unit.starts_with("app-derisk-org.gnome.Nautilus@"));
    assert!(unit.ends_with(".service"));
    assert_eq!(argv.last().unwrap(), "org.gnome.Nautilus");
    assert_eq!(argv[argv.len() - 2], "--");
}

#[test]
fn only_plain_command_names_are_launchable() {
    for ok in ["firefox", "org.kde.dolphin.desktop", "gtk4-demo", "g++"] {
        assert!(systemd::is_launchable(ok), "{ok}");
        assert!(systemd::launch_argv(ok, 1).is_some());
    }
    for bad in [
        "",
        "/usr/bin/x",
        "../x",
        ".hidden",
        "-rf",
        "a b",
        "a;b",
        "$(x)",
        "é",
    ] {
        assert!(!systemd::is_launchable(bad), "{bad}");
        assert!(systemd::launch_argv(bad, 1).is_none());
    }
}

#[test]
fn escape_follows_systemd_escape() {
    assert_eq!(systemd::escape("foo-bar"), "foo\\x2dbar");
    assert_eq!(systemd::escape("a b"), "a\\x20b");
    assert_eq!(systemd::escape("x.y_z"), "x.y_z");
}

#[test]
fn session_ops_go_through_logind_and_the_user_manager() {
    assert_eq!(
        systemd::session_argv(SessionOp::Lock, Some("3")),
        ["loginctl", "lock-session", "3"]
    );
    assert_eq!(
        systemd::session_argv(SessionOp::Suspend, None),
        ["systemctl", "suspend"]
    );
    assert_eq!(
        systemd::session_argv(SessionOp::Logout, None),
        ["systemctl", "--user", "stop", "derisk-session.target"]
    );
    assert!(SessionOp::Reboot.is_destructive());
    assert!(!SessionOp::Lock.is_destructive());
}

#[test]
fn effects_map_to_systemd_commands() {
    let restart = systemd::effect_argv(
        &Effect::RestartUnit {
            unit: "x.service".into(),
        },
        0,
        None,
    );
    assert_eq!(
        restart.unwrap(),
        [
            "systemctl",
            "--user",
            "restart",
            "--no-block",
            "--",
            "x.service"
        ]
    );
    assert!(systemd::effect_argv(&Effect::Close { window: 1 }, 0, None).is_none());
}

#[test]
fn session_start_exports_environment_then_starts_the_target() {
    let cmds = systemd::session_start_argv("wayland-7");
    assert_eq!(cmds[0][..3], ["systemctl", "--user", "set-environment"]);
    assert!(cmds[0].contains(&"WAYLAND_DISPLAY=wayland-7".to_owned()));
    assert!(cmds[0].contains(&"XDG_SESSION_TYPE=wayland".to_owned()));
    assert!(cmds[0].contains(&"DISPLAY=".to_owned()));
    assert_eq!(cmds[1][0], "dbus-update-activation-environment");
    assert!(cmds[1].contains(&"WAYLAND_DISPLAY=wayland-7".to_owned()));
    assert!(cmds[1].contains(&"DISPLAY=".to_owned()));
    assert_eq!(
        cmds.last().unwrap().last().unwrap(),
        "derisk-session.target"
    );
}

#[test]
fn units_are_found_from_cgroups_and_failed_lists() {
    let cg = "0::/user.slice/user-1000.slice/user@1000.service/app.slice/app-graphical.slice/app-derisk-foot@1.service\n";
    assert_eq!(
        systemd::unit_from_cgroup(cg).as_deref(),
        Some("app-derisk-foot@1.service")
    );
    assert_eq!(systemd::unit_from_cgroup("12:cpu:/x\n"), None);
    let out = "foo.service loaded failed failed Foo\nbar.socket loaded failed failed Bar\n";
    assert_eq!(
        systemd::parse_failed_units(out),
        ["foo.service", "bar.socket"]
    );
}

#[test]
fn focus_boost_moves_weight_between_app_units_only() {
    let mut boost = FocusBoost::default();
    let first = boost.focus(Some("app-a.service"));
    assert_eq!(first.len(), 1);
    assert!(first[0].contains(&"CPUWeight=400".to_owned()));
    assert!(boost.focus(Some("app-a.service")).is_empty());
    let second = boost.focus(Some("app-b.service"));
    assert_eq!(second.len(), 2);
    assert!(second[0].contains(&"app-a.service".to_owned()));
    assert!(second[0].contains(&"CPUWeight=100".to_owned()));
    // Never boosts system or session units.
    let third = boost.focus(Some("dbus.service"));
    assert_eq!(third.len(), 1);
    assert!(third[0].contains(&"app-b.service".to_owned()));
}

#[test]
fn socket_activation_and_watchdog_check_the_pid() {
    assert_eq!(systemd::listen_fd_from(Some("42"), Some("1"), 42), Some(3));
    assert_eq!(systemd::listen_fd_from(Some("41"), Some("1"), 42), None);
    assert_eq!(systemd::listen_fd_from(Some("42"), Some("0"), 42), None);
    assert_eq!(
        systemd::watchdog_interval_from(Some("30000000"), Some("42"), 42),
        Some(Duration::from_secs(15))
    );
    assert_eq!(
        systemd::watchdog_interval_from(Some("30000000"), Some("1"), 42),
        None
    );
    assert_eq!(systemd::watchdog_interval_from(None, None, 42), None);
}

#[test]
fn journal_payload_uses_the_native_protocol() {
    let p = systemd::journal_payload(Priority::Warning, "hi\nthere", &[("UNIT", "x")]);
    let mut expected = b"MESSAGE\n".to_vec();
    expected.extend_from_slice(&8u64.to_le_bytes());
    expected.extend_from_slice(b"hi\nthere\nPRIORITY=4\n");
    assert!(p.starts_with(&expected));
    assert!(p.ends_with(b"UNIT=x\n"));
}

#[test]
fn agent_reports_readiness_over_notify_socket() {
    let dir = std::env::temp_dir().join(format!("derisk-notify-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("notify");
    let _ = std::fs::remove_file(&path);
    let socket = UnixDatagram::bind(&path).unwrap();
    socket
        .set_read_timeout(Some(Duration::from_secs(10)))
        .unwrap();
    let status = Command::new(env!("CARGO_BIN_EXE_derisk"))
        .arg("agent")
        .env("NOTIFY_SOCKET", &path)
        .stdin(std::process::Stdio::null())
        .status()
        .unwrap();
    assert!(status.success());
    let mut buf = [0; 256];
    let n = socket.recv(&mut buf).unwrap();
    assert!(
        std::str::from_utf8(&buf[..n])
            .unwrap()
            .starts_with("READY=1\nSTATUS=")
    );
    let n = socket.recv(&mut buf).unwrap();
    assert_eq!(&buf[..n], b"STOPPING=1");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn shipped_units_wire_socket_activation() {
    let root = env!("CARGO_MANIFEST_DIR");
    let socket =
        std::fs::read_to_string(format!("{root}/data/systemd/user/derisk-agent.socket")).unwrap();
    let service =
        std::fs::read_to_string(format!("{root}/data/systemd/user/derisk-agent.service")).unwrap();
    assert!(socket.contains("SocketMode=0600"));
    assert!(service.contains("Type=notify"));
    assert!(service.contains("--socket-activated"));
    assert!(service.contains("WatchdogSec="));
}
