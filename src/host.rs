//! `derisk session`: the derisk shell running on
//! [`mcsapi_compositor`], with the derisk core apps from `derisk-apps`.
//!
//! The compositor (in the mcsapi workspace) owns the display, Wayland socket,
//! input and rendering. This module adapts [`Shell`] and [`ShellUi`] to its
//! [`compositor::Shell`] trait, provides the core apps as in-process
//! windows, and serves the agent protocol against the live desktop.

use std::{
    io::{BufRead, BufReader, Write as _},
    os::unix::{
        fs::{DirBuilderExt, FileTypeExt, PermissionsExt},
        net::{UnixListener, UnixStream},
    },
    path::{Path, PathBuf},
    sync::mpsc,
    time::{Duration, Instant},
};

use derisk::{
    action::{Action, Effect},
    effects::SettingsWatch,
    geom::rect,
    ipc,
    keys::{self, Key, Mods, SuperTap},
    overview::Battery,
    shell::{Mode, PointerOutcome, Shell},
    snap::{Direction, SnapZone},
    systemd::{self, Priority},
    time::Clock,
    ui::{ShellUi, paint_wallpaper},
};
use mcsapi::WindowId;
use mcsapi_compositor::{
    self as compositor, AppId, Apps, Blur, ClientRequest, Command, Compositor, Edges, InstanceId,
    KeyInput, KeyRoute, Keysym, Placement, Press, Remote, Theme, egui,
};

type Result<T = ()> = std::result::Result<T, Box<dyn std::error::Error>>;

/// `derisk session` options.
#[derive(Debug, Default)]
pub struct Options {
    /// Run systemd effects (apps as transient units, logind session operations).
    pub execute: bool,
    /// Agent socket path (default `$XDG_RUNTIME_DIR/derisk/agent.sock`).
    pub socket: Option<PathBuf>,
    /// Apps to launch once the session is up.
    pub launch: Vec<String>,
    /// Initial window size.
    pub size: (i32, i32),
    /// Shorten the startup animation.
    pub reduced_motion: bool,
}

/// The derisk shell as seen by the compositor.
pub struct Session {
    shell: Shell,
    ui: ShellUi,
    execute: bool,
    wayland_display: String,
    launches: u64,
    commands: Vec<Command>,
    super_tap: SuperTap,
    start: Instant,
    last_tick: Option<Instant>,
    settings: SettingsWatch,
}

impl Session {
    fn new(options: &Options) -> Self {
        let (w, h) = options.size;
        let shell = Shell::new(rect(0, 0, w, h), false);
        let ui = ShellUi::new(&shell, options.reduced_motion);
        Self {
            shell,
            ui,
            execute: options.execute,
            wayland_display: String::new(),
            launches: 0,
            commands: Vec::new(),
            super_tap: SuperTap::default(),
            start: Instant::now(),
            last_tick: None,
            settings: SettingsWatch::new(derisk_settings::default_path()),
        }
    }

    /// Applies actions from any source and carries out their effects.
    fn dispatch(&mut self, actions: Vec<Action>) {
        if actions.is_empty() {
            return;
        }
        match self.shell.run(actions) {
            Ok(effects) => self.perform(effects),
            Err(e) => log(Priority::Info, &format!("action failed: {e}")),
        }
    }

    fn perform(&mut self, effects: Vec<Effect>) {
        for effect in effects {
            match &effect {
                Effect::Launch { app } => self.commands.push(Command::Launch(app.clone())),
                Effect::Close { window } => {
                    if let Some(id) = WindowId::new(*window) {
                        self.commands.push(Command::Close(id));
                    }
                }
                Effect::Session { .. }
                | Effect::RestartUnit { .. }
                | Effect::ResetFailed { .. } => {
                    if self.execute {
                        self.launches += 1;
                        if let Some(argv) = systemd::effect_argv(&effect, self.launches, None) {
                            let _ = systemd::run(&argv);
                        }
                    } else {
                        log(
                            Priority::Info,
                            &format!(
                                "not executing {} (run with --execute)",
                                serde_json::to_string(&effect).unwrap_or_default()
                            ),
                        );
                    }
                }
                Effect::MenuActivated { .. } | Effect::TrayActivated { .. } => log(
                    Priority::Info,
                    &serde_json::to_string(&effect).unwrap_or_default(),
                ),
            }
        }
    }

    fn startup_done(&self) -> bool {
        let elapsed = self.start.elapsed().as_millis() as u32;
        self.ui.startup.frame(elapsed).done
    }
}

impl compositor::Shell for Session {
    fn map_window(&mut self, app_id: &str, title: &str) -> WindowId {
        let (id, effects) = self.shell.map_window(app_id, title);
        self.perform(effects);
        id
    }

    fn unmap_window(&mut self, window: WindowId) {
        let _ = self.shell.unmap_window(window);
    }

    fn set_output(&mut self, (w, h): (i32, i32)) {
        self.shell.set_output(rect(0, 0, w, h), false);
    }

    fn focused(&self) -> Option<WindowId> {
        self.shell.focused()
    }

    fn placements(&self) -> Vec<Placement> {
        self.shell
            .placements()
            .into_iter()
            .map(|p| Placement {
                window: p.window,
                frame: p.frame,
                client: p.client,
                focused: p.focused,
                tiled: tiled_edges(p.mode),
                maximized: p.mode
                    == (Mode::Snapped {
                        zone: SnapZone::Maximize,
                    }),
            })
            .collect()
    }

    fn set_title(&mut self, window: WindowId, title: &str) {
        let _ = self.shell.set_title(window, title);
    }

    fn focus(&mut self, window: WindowId) {
        self.dispatch(vec![Action::Focus {
            window: window.get(),
        }]);
    }

    fn session_started(&mut self, wayland_display: &str) {
        self.wayland_display = wayland_display.to_owned();
        if self.execute {
            for argv in systemd::session_start_argv(wayland_display) {
                let _ = systemd::run(&argv);
            }
        }
        log(
            Priority::Notice,
            &format!("derisk session on WAYLAND_DISPLAY={wayland_display}"),
        );
        systemd::notify_ready("derisk session running");
    }

    /// Refreshes the clock, battery, effect settings and failed units about
    /// once a second.
    fn tick(&mut self) {
        if self
            .last_tick
            .is_some_and(|t| t.elapsed() < Duration::from_secs(1))
        {
            return;
        }
        self.last_tick = Some(Instant::now());
        self.shell.clock = Clock::now_utc();
        self.shell.battery = Battery::read(Path::new("/sys/class/power_supply"));
        if let Some(effects) = self.settings.poll() {
            self.shell.effects = effects;
        }
        if self.execute {
            self.shell.failed_units = systemd::failed_units();
        }
    }

    fn chrome_wants_pointer(&self, (x, y): (i32, i32)) -> bool {
        let inside = |g: mcsapi::Geometry| {
            x >= g.loc.x && y >= g.loc.y && x < g.loc.x + g.size.w && y < g.loc.y + g.size.h
        };
        self.shell.overview_visible()
            || y < self.shell.profile().top_bar
            || self.shell.snap_assist().is_some_and(|a| inside(a.frame))
            || !self.startup_done()
    }

    fn pointer_down(&mut self, at: (i32, i32), time_ms: u64) -> Press {
        self.super_tap.cancel();
        match self.shell.pointer_down(at, time_ms) {
            Ok(PointerOutcome::Handled { effects }) => {
                self.perform(effects);
                Press::Handled
            }
            Ok(PointerOutcome::Client { .. } | PointerOutcome::Desktop) | Err(_) => Press::Client,
        }
    }

    fn pointer_motion(&mut self, at: (i32, i32)) {
        self.shell.pointer_motion(at);
    }

    fn pointer_up(&mut self) {
        self.shell.pointer_up();
    }

    fn key(&mut self, key: &KeyInput) -> KeyRoute {
        let is_super = matches!(key.sym, Keysym::Super_L | Keysym::Super_R);
        if self.super_tap.key(is_super, key.pressed) {
            self.dispatch(vec![Action::Overview { visible: None }]);
            return KeyRoute::Consume;
        }
        if is_super {
            return KeyRoute::Consume;
        }
        let mods = Mods {
            logo: key.mods.logo,
            shift: key.mods.shift,
            ctrl: key.mods.ctrl,
            alt: key.mods.alt,
        };
        if let Some(action) = layout_key(key.sym).and_then(|k| keys::binding(mods, k)) {
            if key.pressed {
                self.dispatch(vec![action]);
            }
            return KeyRoute::Consume;
        }
        if self.shell.overview_visible() {
            if key.pressed && key.sym == Keysym::Escape {
                self.dispatch(vec![Action::Overview {
                    visible: Some(false),
                }]);
                return KeyRoute::Consume;
            }
            return KeyRoute::Chrome;
        }
        KeyRoute::Client
    }

    fn client_request(&mut self, window: WindowId, request: ClientRequest) {
        let window = Some(window.get());
        self.dispatch(vec![match request {
            ClientRequest::Maximize => Action::ToggleMaximize { window },
            ClientRequest::Minimize => Action::Minimize { window },
        }]);
    }

    fn theme(&self) -> Theme {
        self.ui.theme
    }

    fn paint_background(&mut self, painter: &egui::Painter, screen: egui::Rect) {
        paint_wallpaper(painter, screen, &self.ui.theme);
    }

    fn paint_decoration(&mut self, painter: &egui::Painter, placement: &Placement) {
        if let Some(p) = self
            .shell
            .placements()
            .into_iter()
            .find(|p| p.window == placement.window)
        {
            self.ui.paint_decoration(painter, &self.shell, &p);
        }
    }

    fn chrome(&mut self, ui: &mut egui::Ui, elapsed_ms: u32) {
        let actions = self.ui.show(ui, &self.shell, elapsed_ms);
        self.dispatch(actions);
    }

    fn blur_regions(&self) -> Vec<Blur> {
        self.ui
            .blur_regions()
            .iter()
            .map(|b| Blur {
                area: b.area,
                corner_radius: b.corner_radius,
                strength: b.strength,
            })
            .collect()
    }

    fn frame_interval(&self) -> Duration {
        self.shell.look().frame_interval
    }

    fn spawn_argv(&mut self, app: &str) -> Vec<String> {
        if !systemd::is_launchable(app) {
            log(Priority::Warning, &format!("refusing to launch {app:?}"));
            return Vec::new();
        }
        self.launches += 1;
        if self.execute
            && let Some(mut argv) = systemd::launch_argv(app, self.launches)
        {
            // systemd-run takes its options before the `--` and the command.
            let split = argv.iter().position(|a| a == "--").unwrap_or(argv.len());
            argv.insert(
                split,
                format!("--setenv=WAYLAND_DISPLAY={}", self.wayland_display),
            );
            return argv;
        }
        vec![app.strip_suffix(".desktop").unwrap_or(app).to_owned()]
    }

    fn take_commands(&mut self) -> Vec<Command> {
        std::mem::take(&mut self.commands)
    }
}

/// The derisk core apps, as in-process windows.
struct CoreApps(derisk_apps::Session);

impl Apps for CoreApps {
    /// Accepts app IDs (`org.derisk.files`), their last segment (`files`),
    /// and display names (`text editor`, `texteditor`).
    fn resolve(&self, name: &str) -> Option<AppId> {
        let name = name.trim().to_lowercase();
        derisk_apps::APPS
            .iter()
            .find(|app| {
                let display = app.name.to_lowercase();
                app.id == name
                    || app.id.rsplit('.').next() == Some(name.as_str())
                    || display == name
                    || display.replace(' ', "") == name
            })
            .map(|app| app.app_id())
    }

    fn launch(&mut self, app: &AppId) -> std::result::Result<InstanceId, mcsapi_runtime::Error> {
        self.0.launch(app)
    }

    fn stop(&mut self, instance: InstanceId) {
        let _ = self.0.stop(instance);
    }

    fn app_mut(&mut self, instance: InstanceId) -> Option<&mut dyn mcsapi_compositor::App> {
        self.0
            .app_mut(instance)
            .map(|app| app as &mut dyn mcsapi_compositor::App)
    }

    fn prepare(&mut self, ctx: &egui::Context, theme: &Theme) {
        ctx.set_visuals(derisk_apps::visuals(theme));
    }
}

/// Runs the session until the window is closed.
pub fn run(options: Options) -> Result {
    let session = Session::new(&options);
    let apps = CoreApps(derisk_apps::Session::new()?);
    let (w, h) = options.size;
    let mut compositor = Compositor::new(session)
        .title("derisk")
        .size(w, h)
        .apps(apps);
    if let Some(path) = agent_socket(options.socket, compositor.remote())? {
        log(
            Priority::Notice,
            &format!("agent protocol on {}", path.display()),
        );
    }
    for app in options.launch {
        compositor = compositor.launch(app);
    }
    compositor.run()?;
    systemd::notify_stopping();
    Ok(())
}

fn log(priority: Priority, message: &str) {
    systemd::log(priority, message, &[]);
}

/// Binds the agent socket and serves each connection on its own thread,
/// running requests on the live shell through `remote`.
fn agent_socket(path: Option<PathBuf>, remote: Remote<Session>) -> Result<Option<PathBuf>> {
    let Some(path) = path.or_else(|| {
        std::env::var_os("XDG_RUNTIME_DIR")
            .map(|dir| PathBuf::from(dir).join("derisk").join("agent.sock"))
    }) else {
        return Ok(None);
    };
    if let Some(dir) = path.parent().filter(|d| !d.as_os_str().is_empty()) {
        std::fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(dir)?;
    }
    if let Ok(meta) = std::fs::symlink_metadata(&path) {
        if !meta.file_type().is_socket() {
            return Err(format!("{} exists and is not a socket", path.display()).into());
        }
        // Refuse to take over a live session's socket; remove a stale one.
        match UnixStream::connect(&path) {
            Ok(_) => return Err(format!("{} is already in use", path.display()).into()),
            Err(e) if e.kind() == std::io::ErrorKind::ConnectionRefused => {
                std::fs::remove_file(&path)?;
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(e.into()),
        }
    }
    let listener = UnixListener::bind(&path)?;
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600))?;
    std::thread::spawn(move || {
        for stream in listener.incoming().flatten() {
            let remote = remote.clone();
            std::thread::spawn(move || serve_agent(stream, &remote));
        }
    });
    Ok(Some(path))
}

fn serve_agent(stream: UnixStream, remote: &Remote<Session>) {
    let Ok(mut writer) = stream.try_clone() else {
        return;
    };
    for line in BufReader::new(stream).lines() {
        let Ok(line) = line else { return };
        if line.trim().is_empty() {
            continue;
        }
        let (reply, response) = mpsc::channel();
        let queued = remote.run(move |session: &mut Session| {
            let (response, effects) = ipc::handle_line(&mut session.shell, &line);
            session.perform(effects);
            let _ = reply.send(response);
        });
        if !queued {
            return;
        }
        let Ok(response) = response.recv() else {
            return;
        };
        if writeln!(writer, "{response}").is_err() {
            return;
        }
    }
}

/// Layout-independent key for shortcuts, from the unmodified keysym.
fn layout_key(sym: Keysym) -> Option<Key> {
    Some(match sym {
        Keysym::Left => Key::Arrow(Direction::Left),
        Keysym::Right => Key::Arrow(Direction::Right),
        Keysym::Up => Key::Arrow(Direction::Up),
        Keysym::Down => Key::Arrow(Direction::Down),
        Keysym::Return | Keysym::KP_Enter | Keysym::Linefeed => Key::Enter,
        Keysym::Tab | Keysym::ISO_Left_Tab => Key::Tab,
        Keysym::Escape => Key::Escape,
        _ => {
            let c = sym.key_char()?.to_ascii_lowercase();
            match c {
                '0'..='9' => Key::Digit(c as u8 - b'0'),
                'a'..='z' => Key::Letter(c),
                _ => return None,
            }
        }
    })
}

/// Which edges of a window touch a neighbour or the screen edge.
fn tiled_edges(mode: Mode) -> Edges {
    let edges = |left, right, top, bottom| Edges {
        left,
        right,
        top,
        bottom,
    };
    match mode {
        Mode::Tiled => Edges::ALL,
        Mode::Floating { .. } => Edges::NONE,
        Mode::Snapped { zone } => match zone {
            SnapZone::Left => edges(true, false, true, true),
            SnapZone::Right => edges(false, true, true, true),
            SnapZone::TopLeft => edges(true, false, true, false),
            SnapZone::TopRight => edges(false, true, true, false),
            SnapZone::BottomLeft => edges(true, false, false, true),
            SnapZone::BottomRight => edges(false, true, false, true),
            SnapZone::Maximize => Edges::NONE,
        },
    }
}
