# derisk

An adaptive, agent-first Wayland desktop environment built on
[mcsapi](https://github.com/dasmatus/mcsapi) that adapts to you.

derisk takes the good parts from many desktops:

| Feature | Inspired by |
| --- | --- |
| Automatic tiling (mcsapi `Tall`/`Monocle` layouts) with floating windows | tiling WMs |
| Drag a title bar to an edge or corner to snap; top edge maximizes; Super+arrows; Snap Assist offers the other half | Windows |
| Window buttons (close, minimize, maximize) on the **left** of the title bar | macOS |
| Global menu in the top bar (app menus + a Window menu) | macOS, KDE |
| Overview with workspaces, an exposé grid and widgets (assistant, clock, suggestions, failed units, calendar, battery, notes) | GNOME, iPadOS |
| Monochrome systray icons recolored to the theme | macOS, GNOME |
| Adaptive profiles: phone (monocle, no gaps), tablet, desktop | mobile shells |
| App suggestions learned from when you launch apps | Android, iOS |
| Startup animation: logo pops in, ring sweeps, then blooms into the desktop (cross-fade with reduced motion) | — |
| Built-in assistant and an agent protocol | agent-first |
| Deep systemd integration | — |

## Agent-first

Everything the shell can do is an `Action` (see `src/action.rs`). The UI,
keyboard, assistant and external agents all go through the same actions, so
anything a person can do an agent can do too, under the same rules.

- **Assistant**: type in the overview's assistant widget, e.g.
  `open firefox and snap it left, then go to workspace 2`.
  Try it from a terminal with `derisk ask open firefox and snap it left`.
- **Agent protocol**: JSON lines over stdio or a Unix socket.

```console
$ derisk agent
{"method":"tools"}                    # MCP-style tool list with JSON schemas
{"method":"state"}                    # windows, workspaces, menus, tray, clock, failed units, ...
{"method":"ask","text":"open kitty and snap it right"}
{"method":"dispatch","actions":[{"action":"snap","zone":"left"}]}
{"method":"register_menu","window":1,"menus":[{"title":"File","entries":[{"kind":"item","id":"open","label":"Open"}]}]}
```

Responses are `{"ok":true,"result":...}` or `{"ok":false,"error":"..."}`.

Guard rails: apps can only be launched by plain command or desktop-file
name (no paths or arguments); reboot, power off, hibernate and log out need
`"confirmed": true`, which the assistant never sets on its own (the UI asks
first); unit actions only apply to units that are currently failed.

## systemd integration

- **Apps run as units.** Each launch is a transient user service
  `app-derisk-<app>@<n>.service` in `app-graphical.slice`
  (`systemd-run --user`, `Type=exec`, `ExitType=cgroup`), following the
  systemd desktop-environment conventions, so every app gets its own cgroup,
  resource accounting and journal stream.
- **Focus boost.** The focused app's unit gets more CPU and IO weight
  (`systemd::FocusBoost`, with the unit found from the client's cgroup via
  `systemd::unit_of_pid`; called by the compositor host on focus changes).
- **Session target.** `derisk-session.target` binds `graphical-session.target`
  and pulls in XDG autostart; the compositor exports `WAYLAND_DISPLAY` and
  friends to the user manager and D-Bus activation environment first
  (`systemd::session_start_argv`).
- **Socket-activated agent.** `derisk-agent.socket` listens on
  `$XDG_RUNTIME_DIR/derisk/agent.sock` (mode 0600) and starts
  `derisk-agent.service` (`Type=notify`, watchdog, hardening) on demand.
- **sd_notify, watchdog and journald.** Readiness, status, watchdog
  keep-alives and stopping are reported to the service manager; logs go to
  the journal with structured fields (stderr outside systemd).
- **logind.** Lock, suspend, hibernate, log out, reboot and power off from the
  system menu, assistant or agents.
- **Failed units widget.** Failed user units show in the top bar and overview
  with restart and reset actions.

Install the units:

```console
$ cargo install --path .
$ cp data/systemd/user/* ~/.config/systemd/user/
$ systemctl --user daemon-reload
$ systemctl --user start derisk-agent.socket
```

## Usage

```console
$ derisk demo          # headless walkthrough: tiling, drag-to-snap, snap assist, assistant, startup animation, UI render
$ derisk ask <text>    # how the assistant interprets a request
$ derisk agent [--socket PATH | --socket-activated] [--execute]
```

Without `--execute` the agent only simulates: launched apps appear as
windows in the headless shell. With `--execute`, effects run through systemd
(launches, session operations, unit restarts) and are logged to the journal.

## Status

This is the shell: window management policy, decorations, the top bar,
overview, assistant, agent protocol and systemd integration, as a library
(`derisk`) plus the `derisk` binary. The compositor host still has to be
written: the Smithay backend and renderer that feed outputs, input and
xdg-shell toplevels into `derisk::shell::Shell`, draw `derisk::ui::ShellUi`
with mcsapi's toolkit, and bridge D-Bus menus (`com.canonical.dbusmenu`) and
StatusNotifierItem to the global menu and tray. Until then `derisk` runs
headless.

## Building

Requires Rust 1.95 and the system libraries mcsapi links against, for
example on Debian/Ubuntu:

```console
$ sudo apt install libxkbcommon-dev libwayland-dev libegl-dev libgles-dev libinput-dev libudev-dev libgbm-dev libseat-dev libdrm-dev
$ cargo build
$ cargo test
$ cargo clippy --all-targets -- -D warnings
```

## License

GPL-3.0-only
