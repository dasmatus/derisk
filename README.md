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
$ cargo run --release --features host -- session --launch foot
```

`derisk session` runs the desktop as a Smithay compositor nested in a window
of your current X11 or Wayland session. Apps launched from it (or from any
terminal with `WAYLAND_DISPLAY` set to the socket it prints) get derisk's
title bars, tiling, snapping, overview and assistant. The agent protocol is
served on `$XDG_RUNTIME_DIR/derisk/agent.sock` against the live desktop:

```console
$ derisk do open foot and snap it right     # natural language, from any terminal
$ derisk send '{"method":"state"}'           # raw agent protocol
```

Session options: `--launch <app>` (repeatable), `--size WxH`,
`--socket PATH`, `--reduced-motion`, and `--execute` to launch apps as
systemd units and run logind session operations.

### Keyboard

| Chord | Action |
| --- | --- |
| Super (tap), Super+A | Overview (the assistant is focused, just type) |
| Super+←/→/↑/↓ | Snap halves and quarters, maximize, restore, minimize |
| Super+1…9 / Super+Shift+1…9 | Switch workspace / move the window there |
| Super+J / Super+K, Alt+Tab | Focus next / previous |
| Super+Enter | Promote to the main pane |
| Super+Q | Close |
| Super+F / Super+T | Float / tile |
| Super+M / Super+Shift+M | Monocle / tall |
| Escape | Leave the overview |

### Headless commands

```console
$ derisk demo          # headless walkthrough: tiling, drag-to-snap, snap assist, assistant, startup animation, UI render
$ derisk ask <text>    # how the assistant interprets a request
$ derisk agent [--socket PATH | --socket-activated] [--execute]
```

Without `--execute` the agent only simulates: launched apps appear as
windows in the headless shell. With `--execute`, effects run through systemd
(launches, session operations, unit restarts) and are logged to the journal.

## Status

- **Shell** (library `derisk`): window management policy, decorations, the
  top bar, overview, assistant, agent protocol, keyboard shortcuts and
  systemd integration.
- **Compositor host** (`derisk session`, feature `host`): Smithay with the
  winit backend and the GLES renderer. It manages xdg-shell toplevels and
  popups with server-side decorations (xdg-decoration), shm buffers, seat
  input, data device and outputs; draws title bars, client surfaces and the
  egui chrome per window in stacking order; and serves the agent socket on
  the live desktop.

Still to do: a DRM/KMS + libinput backend to run on a bare TTY (today the
session runs nested), layer-shell, XWayland, popup grabs, and bridging D-Bus
menus (`com.canonical.dbusmenu`) and StatusNotifierItem to the global menu
and tray. Apps that insist on client-side decorations (GTK, weston clients)
draw their own title bar inside derisk's.

## Showcase video

`scripts/showcase.py` records the desktop in a virtual X server: it starts
Xvfb, runs `derisk session`, drives it with xdotool and the agent socket, and
encodes with ffmpeg. It needs Xvfb, xdotool, ffmpeg, foot, neofetch, htop and
cmatrix.

```console
$ cargo build --release --features host
$ PATH=$PWD/target/release:$PATH scripts/showcase.py derisk-showcase.mp4
```

## Building

Requires Rust 1.95 and the system libraries mcsapi links against, for
example on Debian/Ubuntu:

```console
$ sudo apt install libxkbcommon-dev libwayland-dev libegl-dev libgles-dev libinput-dev libudev-dev libgbm-dev libseat-dev libdrm-dev
$ cargo build
$ cargo test
$ cargo clippy --all-targets -- -D warnings
$ cargo clippy --all-targets --features host -- -D warnings
```

## License

GPL-3.0-only
