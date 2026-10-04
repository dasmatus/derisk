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
| Overview with workspaces, an exposé grid and widgets (clock, suggestions, failed units, calendar, battery, notes) | GNOME, iPadOS |
| Monochrome systray icons recolored to the theme | macOS, GNOME |
| Adaptive profiles: phone (monocle, no gaps), tablet, desktop | mobile shells |
| App suggestions learned from when you launch apps | Android, iOS |
| Startup animation: logo pops in, ring sweeps, then blooms into the desktop (cross-fade with reduced motion) | — |
| Command palette (Super+Space) for apps, app actions, windows, commands, system actions and files, hosting the agent conversation | Raycast, Spotlight, VS Code |
| Built-in assistant and an agent protocol | agent-first |
| Deep systemd integration | — |

## Command palette

Super+Space (or the search field in the top bar) opens one box for
everything. Type to search, in a single ranked list:

- **Apps** from their `.desktop` files: every installed application
  (XDG data dirs, user files first, `Hidden`/`NoDisplay`/`OnlyShowIn`
  respected) and the core apps, plus suggestions learned from your habits.
- **App actions** from each `.desktop` file's `[Desktop Action]` groups,
  such as Firefox's "New Private Window". The core apps ship their own
  `.desktop` files (`crates/derisk-apps/data`), so "Appearance" opens
  Settings on that page and "Downloads" opens Files there. From a terminal:
  `derisk launch org.derisk.settings --action power`.
- **Windows** on every workspace; choosing one switches there and focuses it.
- **App commands**: every item in the focused app's global menus
  (`register_menu`), so apps get palette commands for free.
- **Commands**: window management (snap, maximize, float, tile, close),
  overview, layouts, focus, workspaces and moving the window between them.
- **System actions**: lock, suspend, hibernate, log out, reboot, power off
  (destructive ones need a second Enter), tray items and failed units
  (restart or dismiss). The palette is their home: there is no separate
  system menu, and the top bar's ⚠ count opens the palette on them.
- **Files** under your home folder.

Anything else goes to the assistant: `open firefox and snap it left`,
Enter, done. When a typed sentence is something the assistant understands
and no entry title contains all its words, the assistant row comes first.

**The palette is where the agent lives.** A request opens its
conversation view: what you asked, each planned step with its progress
(✔ done, ⟳ waiting for a launched app's window, ✖ failed and why), and
the outcome. Keep typing to follow up; Backspace on an empty field returns
to search, `?` opens the conversation directly, and "New" starts over.
Requests from external agents (`ask` and `dispatch` over the agent
protocol) appear in the same conversation, and the panel shows a ✨ count
of agent activity you haven't seen (or ✨ while a request is still
working); click it to open the conversation. Typing on the overview opens
the palette with what you typed.

Prefixes narrow the list: `>` commands, `@` windows, `/` or `~` files,
`?` ask the assistant. ↑/↓ (or Tab, Ctrl+N/P) select, Enter runs, Esc
closes. Picks you use often rise to the top. Agents can open and close it
with `{"action":"palette"}` and open files with `{"action":"open","path":...}`
(absolute paths only; executables and `.desktop` files are refused).

## Agent-first

Everything the shell can do is an `Action` (see `src/action.rs`). The UI,
keyboard, assistant and external agents all go through the same actions, so
anything a person can do an agent can do too, under the same rules.

- **Assistant**: type in the command palette, e.g.
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
- **Socket-activated agent.** Without a desktop, `derisk-agent.socket`
  listens on `$XDG_RUNTIME_DIR/derisk/agent.sock` (mode 0600) and starts
  `derisk-agent.service` (`Type=notify`, watchdog, hardening) on demand. A
  `derisk session` serves that socket itself against the live desktop, so it
  stops the headless agent first and the socket unit conflicts with
  `derisk-session.target`.
- **sd_notify, watchdog and journald.** Readiness, status, watchdog
  keep-alives and stopping are reported to the service manager; logs go to
  the journal with structured fields (stderr outside systemd).
- **logind.** Lock, suspend, hibernate, log out, reboot and power off from the
  command palette, assistant or agents, acting on this session
  (`XDG_SESSION_ID`): Log out ends the logind session, not just the target.
- **Lock screen.** Locking hides every window and sends every key to a
  password field checked by PAM (the `derisk` service, `data/pam.d/derisk`).
  With `--execute` the session also locks when logind asks it to (`loginctl
  lock-session`, `lock-sessions`, `busctl wait` on the session's `Lock`
  signal) and sets logind's `LockedHint`. While locked the agent protocol
  answers nothing.
- **Failed units widget.** Failed user units show in the top bar and overview
  with restart and reset actions.

Install the units:

```console
$ cargo install --path .
$ cp data/systemd/user/* ~/.config/systemd/user/
$ systemctl --user daemon-reload
$ systemctl --user start derisk-agent.socket   # headless only; a session serves the socket itself
$ sudo install -m644 data/pam.d/derisk /etc/pam.d/derisk   # for the lock screen
```

## Usage

```console
$ cargo run --release --features host -- session --launch foot
```

`derisk session` runs the desktop as a Smithay compositor nested in a window
of your current X11 or Wayland session. The core apps (Files, Settings, Text
Editor, System Monitor, Calculator, in `crates/`) run inside
the session and launch by name: `--launch files`, `derisk do open calculator`,
or the command palette. Wayland apps launched from it (or from any
terminal with `WAYLAND_DISPLAY` set to the socket it prints) get derisk's
title bars, tiling, snapping, overview and assistant. The agent protocol is
served on `$XDG_RUNTIME_DIR/derisk/agent.sock` against the live desktop:

```console
$ derisk do open files and snap it right   # natural language, from any terminal
$ derisk send '{"method":"state"}'           # raw agent protocol
```

Session options: `--launch <app>` (repeatable), `--size WxH`,
`--socket PATH`, `--reduced-motion`, and `--execute` to launch apps as
systemd units and run logind session operations.

### Keyboard

| Chord | Action |
| --- | --- |
| Super+Space | Command palette |
| Super (tap), Super+A | Overview (typing opens the palette) |
| Super+←/→/↑/↓ | Snap halves and quarters, maximize, restore, minimize |
| Super+1…9 / Super+Shift+1…9 | Switch workspace / move the window there (one past the last opens a new workspace) |
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
- **Compositor host** (`derisk session`, feature `host`): derisk's desktop
  implemented on mcsapi's `mcsapi-compositor` (Smithay, winit backend, GLES
  renderer). The host manages xdg-shell toplevels and popups with server-side
  decorations, input and outputs, and runs the `derisk-apps` core apps in
  process next to Wayland clients; derisk supplies window placement, keys,
  title bars, chrome and the agent socket on the live desktop.

Still to do: a DRM/KMS + libinput backend to run on a bare TTY (today the
session runs nested), layer-shell, XWayland, popup grabs, and bridging D-Bus
menus (`com.canonical.dbusmenu`) and StatusNotifierItem to the global menu
and tray. Apps that insist on client-side decorations (GTK, weston clients)
draw their own title bar inside derisk's.

## Showcase video

`scripts/showcase.py` records the desktop in a virtual X server: it starts
Xvfb, runs `derisk session`, drives it with xdotool and the agent socket, and
encodes with ffmpeg. It shows Wayland clients and the core apps side by
side. It needs Xvfb, xdotool, ffmpeg, foot, neofetch and htop.

```console
$ cargo build --release --features host
$ PATH=$PWD/target/release:$PATH scripts/showcase.py derisk-showcase.mp4
```

## Core apps

The repository is a Cargo workspace: the shell is the root package and the
core apps live under `crates/`. Each app is an `mcsapi_ui::App` with its logic
in a UI-free model and its own tests, and has an ID under `org.derisk.*`.

| App | Crate | What it does |
| --- | --- | --- |
| Files | `derisk-files` | Places, back/forward/up, sort, filter, hidden files, new folder/file, rename (never overwrites), copy/cut/paste with `name (copy).ext` on clashes, the freedesktop.org trash, and `xdg-open`. |
| Settings | `derisk-settings` | Dark/light, accent, text size, reduced motion, layout, gaps, workspaces, adaptive profile, input, notifications, and power, saved to `$XDG_CONFIG_HOME/derisk/settings.conf`. `Settings::theme()` gives the shell its colors. |
| Text Editor | `derisk-editor` | UTF-8 files up to 8 MiB, atomic saves that keep permissions, find and replace, line/column, unsaved-change prompts. |
| System Monitor | `derisk-monitor` | CPU graph, memory, swap, load, uptime, and a sortable, filterable process table with End process, from `/proc`. |
| Calculator | `derisk-calculator` | Expressions with precedence, `^`, `%`, functions, `pi`, `e`, `ans`, and history. |

`derisk-apps` lists them, registers them with `mcsapi-runtime`, and keeps the
running app objects next to their instances (`derisk_apps::Session`), so the
host only gives each instance a surface and calls `mcsapi_ui::run_frame`.
To try them in one window without the compositor:

```console
$ cargo run -p derisk-apps --features preview --bin derisk-preview -- org.derisk.files
```

## Building

Requires Rust 1.95 and the system libraries mcsapi links against, for
example on Debian/Ubuntu:

```console
$ sudo apt install libxkbcommon-dev libwayland-dev libegl-dev libgles-dev libinput-dev libudev-dev libgbm-dev libseat-dev libdrm-dev libpam0g-dev
$ cargo build
$ cargo test --workspace
$ cargo clippy --workspace --all-targets -- -D warnings
$ cargo clippy --workspace --all-targets --features derisk/host -- -D warnings
```

### Nix

`flake.nix` has a dev shell with the Rust toolchain and the Wayland, libinput,
GPU and windowing libraries. With [nix-direnv](https://github.com/nix-community/nix-direnv),
`direnv allow` enters it on `cd` (see `.envrc`).

```console
$ nix develop                # the dev shell
$ nix build                  # derisk with the host feature, plus its systemd units in share/systemd/user
$ nix build .#derisk-preview # the apps' preview window
$ nix run . -- session --launch foot
$ nix flake check            # rustfmt, clippy and tests (default and host + preview)
$ nix fmt                    # nixfmt and rustfmt
```

mcsapi comes from Cargo.lock, not a flake input: `cargo update -p mcsapi`
moves it for Cargo and Nix alike.

## License

GPL-3.0-only
