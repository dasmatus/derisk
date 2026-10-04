//! `derisk` command-line entry point.
//!
//! `derisk session` (built with the `host` feature) runs the desktop as a
//! Smithay compositor. The other commands run the shell headless: windows
//! announced by agents or the demo are simulated, while systemd effects
//! (launching apps as units, session operations) can be executed for real.

#[cfg(feature = "host")]
mod host;

use std::{
    io::{self, BufRead, BufReader, Write},
    os::{
        fd::{FromRawFd, OwnedFd},
        unix::{
            fs::{DirBuilderExt, FileTypeExt, PermissionsExt},
            net::{UnixListener, UnixStream},
        },
    },
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    thread,
};

use derisk::{
    action::{Action, Effect},
    assistant,
    effects::{Effects, SettingsWatch},
    geom::rect,
    ipc,
    menu::{Menu, MenuEntry},
    overview::Battery,
    shell::Shell,
    snap::SnapZone,
    systemd::{self, Priority},
    time::Clock,
    tray::Pixmap,
    ui::ShellUi,
};
use mcsapi::{WindowId, toolkit::egui};

const USAGE: &str = "\
derisk: an adaptive, agent-first Wayland desktop shell built on mcsapi

USAGE:
    derisk session [OPTIONS]      Run the desktop (needs the `host` feature)
    derisk demo                   Run a headless walkthrough
    derisk ask <request...>       Show how the assistant interprets a request
    derisk do <request...>        Ask the running session's assistant to do it
    derisk launch <APP> [--action <ID>]
                                  Launch an app, or one of its desktop actions,
                                  in the running session
    derisk send <json>            Send one agent-protocol request to the session
    derisk agent [OPTIONS]        Serve the JSON-lines agent protocol

SESSION OPTIONS:
    --launch <APP>        Launch an app once the session is up (repeatable)
    --size <WxH>          Window size when nested (default 1280x800)
    --socket <PATH>       Agent socket (default $XDG_RUNTIME_DIR/derisk/agent.sock)
    --reduced-motion      Cross-fade instead of the full startup animation
    --execute             Launch apps as systemd units and run session operations
    --runtime <ROLE> <COMMAND>
                          Start COMMAND (split on spaces) on the compositor's
                          own connection as a panel or overlay: ROLE is app,
                          overlay, or panel:<top|bottom|left|right>:<size>
                          [:keyboard]. For GPUI programs (repeatable)

AGENT OPTIONS:
    --socket <PATH>       Listen on a Unix socket (default: stdin/stdout)
    --socket-activated    Use the socket passed by systemd (derisk-agent.socket)
    --execute             Run systemd effects: launch apps as transient units,
                          lock/suspend/reboot via logind, restart failed units
";

type Result<T = ()> = std::result::Result<T, Box<dyn std::error::Error>>;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let result = match args.first().map(String::as_str) {
        Some("session") => session(&args[1..]),
        Some("demo") => demo(),
        Some("ask") => ask(&args[1..].join(" ")),
        Some("do") => {
            send(&serde_json::json!({"method": "ask", "text": args[1..].join(" ")}).to_string())
        }
        Some("launch") => launch(&args[1..]),
        Some("send") => send(&args[1..].join(" ")),
        Some("agent") => agent(&args[1..]),
        Some("-h" | "--help" | "help") | None => {
            print!("{USAGE}");
            Ok(())
        }
        Some(other) => Err(format!("unknown command: {other}\n\n{USAGE}").into()),
    };
    if let Err(e) = result {
        eprintln!("derisk: {e}");
        std::process::exit(1);
    }
}

#[cfg(feature = "host")]
fn session(args: &[String]) -> Result {
    let mut options = host::Options {
        size: (1280, 800),
        ..Default::default()
    };
    let mut it = args.iter();
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "--launch" => options
                .launch
                .push(it.next().ok_or("--launch needs an app")?.clone()),
            "--size" => {
                let size = it.next().ok_or("--size needs WxH")?;
                let (w, h) = size.split_once('x').ok_or("--size needs WxH")?;
                options.size = (w.parse()?, h.parse()?);
            }
            "--socket" => {
                options.socket = Some(PathBuf::from(it.next().ok_or("--socket needs a path")?));
            }
            "--reduced-motion" => options.reduced_motion = true,
            "--execute" => options.execute = true,
            "--runtime" => {
                let role = host::parse_role(it.next().ok_or("--runtime needs a role")?)?;
                let command = it.next().ok_or("--runtime needs a command")?;
                let argv: Vec<&str> = command.split_whitespace().collect();
                if argv.is_empty() {
                    return Err("--runtime needs a command".into());
                }
                // Panels and overlays are part of the desktop: bring them
                // back if they exit.
                let restart = role != mcsapi_compositor::Role::App;
                options
                    .runtime
                    .push(mcsapi_compositor::RuntimeClient::new(argv, role).restart(restart));
            }
            other => return Err(format!("unknown session option: {other}").into()),
        }
    }
    host::run(options)
}

#[cfg(not(feature = "host"))]
fn session(_args: &[String]) -> Result {
    Err("this derisk was built without the compositor host; rebuild with `--features host`".into())
}

fn ask(text: &str) -> Result {
    let actions = assistant::interpret(text).map_err(|e| format!("{e:?}"))?;
    println!("{}", serde_json::to_string_pretty(&actions)?);
    Ok(())
}

/// The running session's agent socket: `$DERISK_AGENT_SOCKET`, else
/// `$XDG_RUNTIME_DIR/derisk/agent.sock`.
fn session_socket() -> Result<PathBuf> {
    if let Some(path) = std::env::var_os("DERISK_AGENT_SOCKET") {
        return Ok(path.into());
    }
    let dir = std::env::var_os("XDG_RUNTIME_DIR").ok_or("XDG_RUNTIME_DIR is not set")?;
    Ok(PathBuf::from(dir).join("derisk").join("agent.sock"))
}

/// Sends one request line to the running session and prints the response.
/// `derisk launch <app> [--action <id>]`, the `Exec` of derisk's own
/// `.desktop` files.
fn launch(args: &[String]) -> Result {
    let action = match args {
        [app] => serde_json::json!({"action": "launch", "app": app}),
        [app, flag, id] if flag == "--action" => {
            serde_json::json!({"action": "launch_action", "app": app, "id": id})
        }
        _ => return Err(format!("usage: derisk launch <APP> [--action <ID>]\n\n{USAGE}").into()),
    };
    send(&serde_json::json!({"method": "dispatch", "actions": [action]}).to_string())
}

fn send(line: &str) -> Result {
    let line = line.trim();
    if line.is_empty() {
        return Err("send needs a JSON request".into());
    }
    let path = session_socket()?;
    let mut stream = UnixStream::connect(&path)
        .map_err(|e| format!("no derisk session on {}: {e}", path.display()))?;
    writeln!(stream, "{line}")?;
    let mut response = String::new();
    BufReader::new(stream).read_line(&mut response)?;
    let value: serde_json::Value = serde_json::from_str(&response)?;
    println!("{}", serde_json::to_string_pretty(&value)?);
    if value["ok"] == false {
        std::process::exit(1);
    }
    Ok(())
}

fn refresh(shell: &mut Shell) {
    shell.clock = Clock::now_utc();
    shell.battery = Battery::read(Path::new("/sys/class/power_supply"));
    if let Some(settings) = SettingsWatch::new(derisk_settings::default_path()).poll() {
        shell.effects = Effects::from_settings(&settings);
    }
}

fn demo() -> Result {
    let mut shell = Shell::new(rect(0, 0, 1920, 1080), false);
    refresh(&mut shell);
    println!("profile: {:?}", shell.profile().form_factor);

    let (editor, _) = shell.map_window("editor", "notes.md");
    let (term, _) = shell.map_window("terminal", "~/src/derisk");
    let (browser, _) = shell.map_window("browser", "mcsapi on GitHub");
    shell.menus.register(
        editor.get(),
        vec![Menu {
            title: "File".into(),
            entries: vec![
                MenuEntry::item("save", "Save").with_shortcut("Ctrl+S"),
                MenuEntry::Separator,
                MenuEntry::item("quit", "Quit"),
            ],
        }],
    );
    let dot = Pixmap::from_rgba(
        2,
        2,
        vec![255, 0, 0, 255, 0, 0, 0, 0, 0, 0, 0, 0, 0, 255, 0, 255],
    )
    .ok_or("bad pixmap")?;
    shell
        .tray
        .insert("network", "Wi-Fi", &dot, Vec::new(), [248, 250, 252]);

    println!("\nauto-tiled:");
    print_placements(&shell);

    let frame = shell
        .placements()
        .into_iter()
        .find(|p| p.window == browser)
        .ok_or("browser not visible")?
        .frame;
    let grab = (frame.loc.x + frame.size.w / 2, frame.loc.y + 10);
    shell.pointer_down(grab, 0)?;
    shell.pointer_motion((grab.0 - 100, grab.1 + 40));
    let target = shell.pointer_motion((2, 500));
    println!("\ndragging the browser title bar to the left edge -> {target:?}");
    shell.pointer_up();
    print_placements(&shell);
    if let Some(assist) = shell.snap_assist() {
        println!(
            "snap assist offers {:?} for windows {:?}",
            assist.zone,
            assist
                .candidates
                .iter()
                .map(|w| w.get())
                .collect::<Vec<_>>()
        );
    }
    shell.apply(Action::Snap {
        window: Some(term.get()),
        zone: SnapZone::Right,
    })?;
    println!("\nterminal picked for the other half:");
    print_placements(&shell);

    let request = "open firefox and snap it top left";
    println!("\nassistant: {request:?}");
    let actions = assistant::interpret(request).map_err(|e| format!("{e:?}"))?;
    let effects = shell.run(actions)?;
    for effect in &effects {
        println!("  effect: {}", serde_json::to_string(effect)?);
        if let Some(argv) = systemd::effect_argv(effect, 1, None) {
            println!("  via systemd: {}", argv.join(" "));
        }
    }
    let (firefox, _) = shell.map_window("firefox", "New Tab");
    println!(
        "  firefox mapped as window {firefox}: {:?}",
        shell.mode(firefox)
    );

    let mut ui = ShellUi::new(&shell, false);
    println!("\nstartup animation:");
    for ms in [0, 300, 700, 1200, 1500, 1800] {
        let f = ui.startup.frame(ms);
        println!(
            "  {ms:>4} ms  cover {:.2}  logo {:.2} x{:.2}  ring {:.2}  bar {:+.1}px  done {}",
            f.cover, f.logo_opacity, f.logo_scale, f.ring, f.bar_offset, f.done
        );
    }

    shell.apply(Action::Overview {
        visible: Some(true),
    })?;
    let ctx = egui::Context::default();
    let input = egui::RawInput {
        screen_rect: Some(egui::Rect::from_min_size(
            egui::Pos2::ZERO,
            egui::vec2(1920.0, 1080.0),
        )),
        ..Default::default()
    };
    let mut output = ctx.run_ui(input, |root| {
        ui.paint_decorations(root.painter(), &shell);
        ui.show(root, &shell, 5_000);
    });
    println!(
        "\nrendered left-button title bars, top bar and overview widgets: {} shapes",
        output.shapes.len()
    );
    // Headless: there is no renderer to upload the tray textures to.
    output.textures_delta.clear();
    Ok(())
}

fn print_placements(shell: &Shell) {
    for p in shell.placements() {
        let (app, _) = shell.window_label(p.window).unwrap_or_default();
        println!(
            "  {:>2} {:<9} {:>4},{:<4} {:>4}x{:<4} {:?}{}",
            p.window,
            app,
            p.frame.loc.x,
            p.frame.loc.y,
            p.frame.size.w,
            p.frame.size.h,
            p.mode,
            if p.focused { "  (focused)" } else { "" }
        );
    }
}

/// The headless shell plus effect execution.
struct Host {
    shell: Shell,
    execute: bool,
    launches: u64,
    session_id: Option<String>,
}

impl Host {
    fn handle(&mut self, line: &str) -> String {
        refresh(&mut self.shell);
        if self.execute && line.contains("\"state\"") {
            self.shell.failed_units = systemd::failed_units();
        }
        let (response, effects) = ipc::handle_line(&mut self.shell, line);
        for effect in effects {
            self.perform(&effect);
        }
        response
    }

    fn perform(&mut self, effect: &Effect) {
        // Headless stand-in for the compositor side.
        match effect {
            Effect::Launch { app } => {
                self.shell.map_window(app, app);
            }
            Effect::Close { window } => {
                if let Some(id) = WindowId::new(*window) {
                    let _ = self.shell.unmap_window(id);
                }
            }
            Effect::RestartUnit { unit } | Effect::ResetFailed { unit } => {
                self.shell.failed_units.retain(|u| u != unit);
            }
            _ => {}
        }
        if !self.execute {
            return;
        }
        self.launches += 1;
        let instance = (u64::from(std::process::id()) << 20) | self.launches;
        if let Some(argv) = systemd::effect_argv(effect, instance, self.session_id.as_deref()) {
            let ok = systemd::run(&argv).is_ok_and(|o| o.status.success());
            let effect_json = serde_json::to_string(effect).unwrap_or_default();
            systemd::log(
                if ok {
                    Priority::Info
                } else {
                    Priority::Warning
                },
                &format!("{} {}", if ok { "ran" } else { "failed" }, argv.join(" ")),
                &[("DERISK_EFFECT", &effect_json)],
            );
        }
    }
}

fn agent(args: &[String]) -> Result {
    let mut socket = None;
    let mut activated = false;
    let mut execute = false;
    let mut it = args.iter();
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "--socket" => socket = Some(PathBuf::from(it.next().ok_or("--socket needs a path")?)),
            "--socket-activated" => activated = true,
            "--execute" => execute = true,
            other => return Err(format!("unknown agent option: {other}").into()),
        }
    }
    let host = Arc::new(Mutex::new(Host {
        shell: Shell::new(rect(0, 0, 1920, 1080), false),
        execute,
        launches: 0,
        session_id: std::env::var("XDG_SESSION_ID").ok(),
    }));

    if let Some(interval) = systemd::watchdog_interval() {
        thread::spawn(move || {
            loop {
                systemd::notify_watchdog();
                thread::sleep(interval / 2);
            }
        });
    }

    let listener = if activated {
        let fd =
            systemd::listen_fd().ok_or("no socket passed by systemd (LISTEN_PID/LISTEN_FDS)")?;
        // SAFETY: systemd handed this listening socket to this very process
        // (LISTEN_PID matched) and nothing else in the process owns the fd.
        let passed = UnixListener::from(unsafe { OwnedFd::from_raw_fd(fd) });
        // systemd passes fd 3 without close-on-exec, so every systemctl,
        // systemd-run and loginctl this process spawns would inherit the
        // listening socket. try_clone duplicates with F_DUPFD_CLOEXEC, and
        // dropping the original closes fd 3.
        Some(passed.try_clone()?)
    } else if let Some(path) = socket {
        Some(bind(&path)?)
    } else {
        None
    };

    let Some(listener) = listener else {
        systemd::notify_ready("serving the agent protocol on stdio");
        let mut stdout = io::stdout();
        for line in io::stdin().lock().lines() {
            let line = line?;
            if line.trim().is_empty() {
                continue;
            }
            let response = host.lock().map_err(|_| "shell poisoned")?.handle(&line);
            writeln!(stdout, "{response}")?;
            stdout.flush()?;
        }
        systemd::notify_stopping();
        return Ok(());
    };

    systemd::notify_ready("serving the agent protocol");
    systemd::log(Priority::Notice, "derisk agent socket ready", &[]);
    for stream in listener.incoming() {
        match stream {
            Ok(stream) => {
                let host = Arc::clone(&host);
                thread::spawn(move || {
                    if let Err(e) = serve(stream, &host) {
                        systemd::log(Priority::Info, &format!("agent disconnected: {e}"), &[]);
                    }
                });
            }
            Err(e) => systemd::log(Priority::Warning, &format!("accept failed: {e}"), &[]),
        }
    }
    systemd::notify_stopping();
    Ok(())
}

fn serve(stream: UnixStream, host: &Mutex<Host>) -> io::Result<()> {
    let mut writer = stream.try_clone()?;
    let mut reader = BufReader::new(stream);
    while let Some(line) = derisk::ipc::read_request(&mut reader)? {
        if line.trim().is_empty() {
            continue;
        }
        let response = host
            .lock()
            .map_err(|_| io::Error::other("shell poisoned"))?
            .handle(&line);
        writeln!(writer, "{response}")?;
    }
    Ok(())
}

/// Binds a user-only socket, replacing a stale socket (never any other file).
fn bind(path: &Path) -> Result<UnixListener> {
    if let Some(dir) = path.parent().filter(|d| !d.as_os_str().is_empty()) {
        std::fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(dir)?;
        derisk::ipc::check_socket_dir(dir)?;
    }
    if let Ok(meta) = std::fs::symlink_metadata(path) {
        if !meta.file_type().is_socket() {
            return Err(format!("{} exists and is not a socket", path.display()).into());
        }
        // Never take over a socket something is still listening on.
        match UnixStream::connect(path) {
            Ok(_) => return Err(format!("{} is already in use", path.display()).into()),
            Err(e) if e.kind() == io::ErrorKind::ConnectionRefused => std::fs::remove_file(path)?,
            Err(e) if e.kind() == io::ErrorKind::NotFound => {}
            Err(e) => return Err(e.into()),
        }
    }
    let listener = UnixListener::bind(path)?;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))?;
    Ok(listener)
}
