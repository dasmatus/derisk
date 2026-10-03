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
    derisk send <json>            Send one agent-protocol request to the session
    derisk agent [OPTIONS]        Serve the JSON-lines agent protocol

SESSION OPTIONS:
    --launch <APP>        Launch an app once the session is up (repeatable)
    --size <WxH>          Window size when nested (default 1280x800)
    --socket <PATH>       Agent socket (default $XDG_RUNTIME_DIR/derisk/agent.sock)
    --reduced-motion      Cross-fade instead of the full startup animation
    --execute             Launch apps as systemd units and run session operations

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
fn send(line: &str) -> Result {
    let path = session_socket()?;
    let mut stream = UnixStream::connect(&path)
        .map_err(|e| format!("no derisk session on {}: {e}", path.display()))?;
    writeln!(stream, "{}", line.trim())?;
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
        Some(UnixListener::from(unsafe { OwnedFd::from_raw_fd(fd) }))
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
    for line in BufReader::new(stream).lines() {
        let line = line?;
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
    }
    if let Ok(meta) = std::fs::symlink_metadata(path) {
        if !meta.file_type().is_socket() {
            return Err(format!("{} exists and is not a socket", path.display()).into());
        }
        std::fs::remove_file(path)?;
    }
    let listener = UnixListener::bind(path)?;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))?;
    Ok(listener)
}
