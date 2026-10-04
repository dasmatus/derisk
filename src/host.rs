//! `derisk session`: the derisk shell running on
//! [`mcsapi_compositor`], with the derisk core apps from `derisk-apps`.
//!
//! The compositor (in the mcsapi workspace) owns the display, Wayland socket,
//! input and rendering. This module adapts [`Shell`] and [`ShellUi`] to its
//! [`compositor::Shell`] trait, provides the core apps as in-process
//! windows, and serves the agent protocol against the live desktop.

use std::{
    collections::HashMap,
    io::{BufRead, BufReader, Write as _},
    os::unix::{
        fs::{DirBuilderExt, FileTypeExt, PermissionsExt},
        net::{UnixListener, UnixStream},
    },
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex,
        atomic::{AtomicU64, Ordering},
        mpsc,
    },
    time::{Duration, Instant},
};

use derisk::{
    action::{Action, Effect},
    conversation::Source,
    decorations::Button,
    desktop::{self, DesktopEntry},
    effects::SettingsWatch,
    geom::rect,
    ipc::{self, LiveRequest},
    keys::{self, Key, Mods, SuperTap},
    overview::Battery,
    palette::{self, Entry},
    shell::{Mode, PointerOutcome, Shell},
    snap::{Direction, SnapZone},
    systemd::{self, Priority},
    time::Clock,
    ui::{ShellUi, paint_wallpaper},
};
use mcsapi::WindowId;
use mcsapi_compositor::{
    self as compositor, AppId, Apps, Blur, Capture, ClientRequest, Command, Compositor, Edges,
    InstanceId, KeyInput, KeyRoute, Keysym, OutputTiming, Placement, Press, Remote, Theme,
    a11y::{Origin, Snapshot, Subtree},
    accesskit::{self, NodeId, Role},
    egui,
};
use serde_json::{Value, json};

use crate::computer;

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
    palette_open: bool,
    index: Option<mpsc::Receiver<Index>>,
    core_apps: Vec<DesktopEntry>,
    installed: Vec<DesktopEntry>,
    pending_actions: PendingActions,
    settings: SettingsWatch,
    /// Request numbers for [`Command::Describe`] and [`Command::Capture`].
    requests: u64,
    /// Agents waiting for the accessibility tree.
    trees: HashMap<u64, (mpsc::Sender<String>, TreeQuery)>,
    /// Agents waiting for a screenshot, and whether they want it inline.
    shots: HashMap<u64, (mpsc::Sender<String>, bool)>,
    /// Accessibility nodes programs registered for their windows, with
    /// the agent connection that owns them.
    registered: HashMap<WindowId, (u64, Subtree)>,
    /// Event lines to each agent connection that registered a tree.
    listeners: HashMap<u64, mpsc::Sender<String>>,
}

/// What an agent asked of the accessibility tree.
enum TreeQuery {
    Tree {
        window: Option<u64>,
    },
    Find {
        role: Option<String>,
        name: Option<String>,
        window: Option<u64>,
    },
}

/// Node IDs of the title-bar buttons the shell paints, in the space above
/// [`computer::REGISTERED_ID_LIMIT`] so they never meet registered nodes.
const TITLE_BAR_NODES: u64 = u64::MAX - 8;

/// Core-app actions waiting for the compositor to launch their app, shared
/// between [`Session`] (which queues them) and [`CoreApps`] (which takes
/// them when the launch arrives).
type PendingActions = Arc<Mutex<Vec<(String, String)>>>;

/// What the palette indexes in the background: files under home and the
/// installed applications.
type Index = (Vec<PathBuf>, Vec<DesktopEntry>);

/// The core apps' own `.desktop` files, parsed.
fn core_desktop_entries() -> Vec<DesktopEntry> {
    derisk_apps::APPS
        .iter()
        .filter_map(|app| DesktopEntry::parse(&app.desktop_id(), app.desktop_file))
        .collect()
}

/// Installed applications except hidden ones and copies of the core apps.
fn installed_apps() -> Vec<DesktopEntry> {
    desktop::scan(&desktop::application_dirs())
        .into_iter()
        .filter(|e| !e.no_display && derisk_apps::find(&e.id).is_none())
        .collect()
}

/// Palette entries for the core apps and installed apps, with their
/// desktop actions.
fn palette_entries(core: &[DesktopEntry], installed: &[DesktopEntry]) -> Vec<Entry> {
    let core = core.iter().flat_map(|e| {
        let icon = derisk_apps::find(&e.id).map_or("🖥", |app| app.icon);
        palette::desktop_app(e, icon)
    });
    let installed = installed.iter().flat_map(|e| palette::desktop_app(e, "🖥"));
    core.chain(installed).collect()
}

impl Session {
    fn new(options: &Options, pending_actions: PendingActions) -> Self {
        let (w, h) = options.size;
        let shell = Shell::new(rect(0, 0, w, h), false);
        let mut ui = ShellUi::new(&shell, options.reduced_motion);
        let core_apps = core_desktop_entries();
        let installed = installed_apps();
        ui.palette.extra = palette_entries(&core_apps, &installed);
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
            palette_open: false,
            index: None,
            core_apps,
            installed,
            pending_actions,
            settings: SettingsWatch::new(derisk_settings::default_path()),
            requests: 0,
            trees: HashMap::new(),
            shots: HashMap::new(),
            registered: HashMap::new(),
            listeners: HashMap::new(),
        }
    }

    /// Handles one agent request line from connection `conn`; the response
    /// goes to `reply`, now or once the compositor has answered.
    fn agent_line(
        &mut self,
        conn: u64,
        line: &str,
        reply: mpsc::Sender<String>,
        events: &mpsc::Sender<String>,
    ) {
        let respond = |result: std::result::Result<Value, String>| {
            let line = match result {
                Ok(result) => json!({"ok": true, "result": result}),
                Err(error) => json!({"ok": false, "error": error}),
            };
            let _ = reply.send(line.to_string());
        };
        let request = match ipc::live_request(line) {
            None => {
                let (response, effects) = ipc::handle_line(&mut self.shell, line);
                self.perform(effects);
                let _ = reply.send(response);
                return;
            }
            Some(Err(e)) => return respond(Err(e)),
            Some(Ok(request)) => request,
        };
        match request {
            LiveRequest::Tree { window } => {
                self.requests += 1;
                self.trees
                    .insert(self.requests, (reply, TreeQuery::Tree { window }));
                self.commands.push(Command::Describe(self.requests));
            }
            LiveRequest::Find { role, name, window } => {
                self.requests += 1;
                self.trees.insert(
                    self.requests,
                    (reply, TreeQuery::Find { role, name, window }),
                );
                self.commands.push(Command::Describe(self.requests));
            }
            LiveRequest::Act {
                element,
                action,
                value,
            } => {
                self.commands.push(Command::Act {
                    element,
                    action: computer::element_action(action),
                    value,
                });
                respond(Ok(json!({"queued": true})));
            }
            LiveRequest::Screenshot { inline } => {
                self.requests += 1;
                self.shots.insert(self.requests, (reply, inline));
                self.commands.push(Command::Capture(self.requests));
            }
            LiveRequest::Input { events } => match computer::inputs(&events) {
                Ok(inputs) => {
                    let n = inputs.len();
                    self.commands.extend(inputs.into_iter().map(Command::Input));
                    respond(Ok(json!({"queued": n})));
                }
                Err(e) => respond(Err(e)),
            },
            LiveRequest::RegisterTree { window, nodes } => {
                let Some(id) =
                    WindowId::new(window).filter(|w| self.shell.window_label(*w).is_some())
                else {
                    return respond(Err(format!("unknown window: {window}")));
                };
                if let Some((owner, _)) = self.registered.get(&id)
                    && *owner != conn
                {
                    return respond(Err(format!(
                        "another connection registered window {window}'s tree"
                    )));
                }
                match computer::subtree(id, &nodes) {
                    Ok(subtree) => {
                        self.registered.insert(id, (conn, subtree));
                        self.listeners.insert(conn, events.clone());
                        respond(Ok(json!({"nodes": nodes.len()})));
                    }
                    Err(e) => respond(Err(e)),
                }
            }
        }
    }

    /// An agent connection closed: its registered trees go with it.
    fn agent_disconnected(&mut self, conn: u64) {
        self.registered.retain(|_, (owner, _)| *owner != conn);
        self.listeners.remove(&conn);
    }

    /// The title-bar buttons of each visible window, which the shell
    /// paints itself, so screen readers and agents can press them.
    fn title_bar_nodes(&self) -> Vec<Subtree> {
        let bar = self.shell.profile().title_bar;
        self.shell
            .placements()
            .into_iter()
            .map(|p| {
                let nodes: Vec<(NodeId, accesskit::Node)> = bar
                    .buttons(p.frame)
                    .map(|(button, area)| {
                        let mut node = accesskit::Node::new(Role::Button);
                        node.set_label(match button {
                            Button::Close => "Close",
                            Button::Minimize => "Minimize",
                            Button::Maximize => "Maximize",
                        });
                        node.set_bounds(accesskit::Rect {
                            x0: f64::from(area.loc.x),
                            y0: f64::from(area.loc.y),
                            x1: f64::from(area.loc.x + area.size.w),
                            y1: f64::from(area.loc.y + area.size.h),
                        });
                        node.add_action(accesskit::Action::Click);
                        (NodeId(TITLE_BAR_NODES + button_index(button)), node)
                    })
                    .collect();
                Subtree {
                    window: p.window,
                    roots: nodes.iter().map(|(id, _)| *id).collect(),
                    nodes,
                    origin: Origin::Shell,
                }
            })
            .collect()
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
        self.palette_opened();
    }

    /// Re-indexes home on a background thread each time the palette opens,
    /// so file results stay fresh without stalling a frame.
    fn palette_opened(&mut self) {
        let open = self.shell.palette_visible();
        if open && !self.palette_open {
            let (tx, rx) = mpsc::channel();
            std::thread::spawn(move || {
                let files = std::env::var_os("HOME")
                    .map(|home| palette::index_files(Path::new(&home), 4, 20_000))
                    .unwrap_or_default();
                let _ = tx.send((files, installed_apps()));
            });
            self.index = Some(rx);
        }
        self.palette_open = open;
        if let Some((files, installed)) = self.index.as_ref().and_then(|rx| rx.try_recv().ok()) {
            self.ui.palette.files = files;
            self.ui.palette.extra = palette_entries(&self.core_apps, &installed);
            self.installed = installed;
            self.index = None;
        }
    }

    /// Runs `argv` for `app`: as a transient unit with `--execute`,
    /// otherwise as a child with this session's `WAYLAND_DISPLAY`.
    fn spawn_command(&mut self, app: &str, argv: &[String]) {
        self.launches += 1;
        if self.execute {
            if let Some(mut unit) = systemd::launch_command_argv(app, self.launches, argv) {
                let split = unit.iter().position(|a| a == "--").unwrap_or(unit.len());
                unit.insert(
                    split,
                    format!("--setenv=WAYLAND_DISPLAY={}", self.wayland_display),
                );
                let _ = systemd::run(&unit);
            }
        } else if let Some((program, args)) = argv.split_first()
            && let Err(e) = std::process::Command::new(program)
                .args(args)
                .env("WAYLAND_DISPLAY", &self.wayland_display)
                .spawn()
        {
            log(Priority::Warning, &format!("{program}: {e}"));
        }
    }

    /// Runs an app's desktop action: core apps open in-process through
    /// [`CoreApps`], installed apps run the action's `Exec`.
    fn launch_action(&mut self, app: &str, id: &str) {
        if let Some(core) = derisk_apps::find(app) {
            if core.create_action(id).is_none() {
                log(Priority::Info, &format!("{} has no action {id:?}", core.id));
                return;
            }
            if let Ok(mut pending) = self.pending_actions.lock() {
                pending.push((core.id.to_owned(), id.to_owned()));
            }
            self.commands.push(Command::Launch(core.id.to_owned()));
            return;
        }
        let Some(argv) = self
            .installed
            .iter()
            .find(|e| e.id == app)
            .and_then(|e| e.action_argv(id))
        else {
            log(Priority::Info, &format!("{app} has no action {id:?}"));
            return;
        };
        self.spawn_command(app, &argv);
    }

    fn perform(&mut self, effects: Vec<Effect>) {
        for effect in effects {
            match &effect {
                Effect::Launch { app } => self.commands.push(Command::Launch(app.clone())),
                Effect::LaunchAction { app, id } => self.launch_action(app, id),
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
                Effect::Open { path } => {
                    if self.execute {
                        self.launches += 1;
                        if let Some(mut argv) = systemd::effect_argv(&effect, self.launches, None) {
                            let split = argv.iter().position(|a| a == "--").unwrap_or(argv.len());
                            argv.insert(
                                split,
                                format!("--setenv=WAYLAND_DISPLAY={}", self.wayland_display),
                            );
                            let _ = systemd::run(&argv);
                        }
                    } else if let Err(e) = std::process::Command::new("xdg-open")
                        .arg(path)
                        .env("WAYLAND_DISPLAY", &self.wayland_display)
                        .spawn()
                    {
                        log(Priority::Warning, &format!("xdg-open {path}: {e}"));
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
        self.ui.startup_frame(elapsed).done
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
            || self.shell.palette_visible()
            || self.shell.pending_confirmation().is_some()
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
        // The confirmation dialog takes every key until it is answered.
        if self.shell.pending_confirmation().is_some() {
            if key.pressed && key.sym == Keysym::Escape {
                self.dispatch(vec![Action::Confirm { accept: false }]);
                return KeyRoute::Consume;
            }
            return KeyRoute::Chrome;
        }
        // Super+B moves keyboard focus into the top bar; Tab and Shift+Tab
        // walk it, Escape hands the keyboard back to the window.
        if key.mods.logo && !key.mods.ctrl && !key.mods.alt && key.sym == Keysym::b {
            if key.pressed {
                self.ui.focus_top_bar();
            }
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
        if self.shell.palette_visible() {
            if key.pressed && key.sym == Keysym::Escape {
                self.dispatch(vec![Action::Palette {
                    visible: Some(false),
                }]);
                return KeyRoute::Consume;
            }
            return KeyRoute::Chrome;
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
        let action = match request {
            ClientRequest::Maximize => Action::ToggleMaximize { window },
            ClientRequest::Minimize => Action::Minimize { window },
            // Requests added to the compositor later are ignored until handled.
            _ => return,
        };
        self.dispatch(vec![action]);
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
        // Requests typed in the palette; their progress shows in its
        // conversation.
        for ask in self.ui.take_asks() {
            match self.shell.ask(&ask.text, Source::User, ask.confirmed) {
                Ok(effects) => self.perform(effects),
                Err(e) => log(Priority::Info, &format!("request failed: {e}")),
            }
        }
        self.palette_opened();
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

    fn frame_interval(&self, timing: &OutputTiming) -> Duration {
        let look = self.shell.look();
        let timing = OutputTiming {
            vrr: look.vrr.unwrap_or(timing.vrr),
            ..*timing
        };
        match look.max_fps {
            Some(fps) => timing.interval_for(fps),
            None => timing.refresh_interval(),
        }
    }

    fn spawn_argv(&mut self, app: &str) -> Vec<String> {
        if !systemd::is_launchable(app) {
            log(Priority::Warning, &format!("refusing to launch {app:?}"));
            return Vec::new();
        }
        // An installed app's desktop file ID runs its `Exec`; any other name
        // runs as a plain command.
        let command = match self.installed.iter().find(|e| e.id == app) {
            Some(entry) => entry.argv(),
            None => vec![app.strip_suffix(".desktop").unwrap_or(app).to_owned()],
        };
        if command.is_empty() {
            log(Priority::Warning, &format!("invalid Exec for {app:?}"));
            return Vec::new();
        }
        self.launches += 1;
        if self.execute
            && let Some(mut argv) = systemd::launch_command_argv(app, self.launches, &command)
        {
            // systemd-run takes its options before the `--` and the command.
            let split = argv.iter().position(|a| a == "--").unwrap_or(argv.len());
            argv.insert(
                split,
                format!("--setenv=WAYLAND_DISPLAY={}", self.wayland_display),
            );
            return argv;
        }
        command
    }

    fn take_commands(&mut self) -> Vec<Command> {
        std::mem::take(&mut self.commands)
    }

    fn access_subtrees(&mut self) -> Vec<Subtree> {
        let mut subtrees = self.title_bar_nodes();
        subtrees.extend(self.registered.values().map(|(_, s)| s.clone()));
        subtrees
    }

    fn access_action(
        &mut self,
        window: WindowId,
        node: NodeId,
        action: accesskit::Action,
        value: Option<&str>,
    ) {
        if node.0 >= TITLE_BAR_NODES {
            if action != accesskit::Action::Click {
                return;
            }
            let w = Some(window.get());
            let action = match node.0 - TITLE_BAR_NODES {
                0 => Action::Close { window: w },
                1 => Action::Minimize { window: w },
                2 => Action::ToggleMaximize { window: w },
                _ => return,
            };
            self.dispatch(vec![action]);
            return;
        }
        // A node a program registered: tell that program.
        if let Some((conn, _)) = self.registered.get(&window)
            && let Some(events) = self.listeners.get(conn)
        {
            let event = computer::action_event(window, node, action, value);
            let _ = events.send(event.to_string());
        }
    }

    fn described(&mut self, request: u64, tree: Snapshot) {
        let Some((reply, query)) = self.trees.remove(&request) else {
            return;
        };
        let result = match query {
            TreeQuery::Tree { window } => computer::tree_json(&tree, window),
            TreeQuery::Find { role, name, window } => {
                computer::find_json(&tree, role.as_deref(), name.as_deref(), window)
            }
        };
        let _ = reply.send(json!({"ok": true, "result": result}).to_string());
    }

    fn captured(&mut self, request: u64, frame: std::result::Result<Capture, String>) {
        let Some((reply, inline)) = self.shots.remove(&request) else {
            return;
        };
        // Encoding takes a while; keep it off the compositor thread.
        std::thread::spawn(move || {
            let result = frame.and_then(|capture| {
                let png = computer::png(&capture)?;
                let dir = computer::screenshot_dir()
                    .ok_or_else(|| "XDG_RUNTIME_DIR is not set".to_owned())?;
                let path = computer::save_screenshot(&dir, request, &png)
                    .map_err(|e| format!("cannot save the screenshot: {e}"))?;
                let mut result = json!({
                    "path": path,
                    "width": capture.width,
                    "height": capture.height,
                });
                if inline {
                    result["png_base64"] = json!(computer::base64(&png));
                }
                Ok(result)
            });
            let line = match result {
                Ok(result) => json!({"ok": true, "result": result}),
                Err(error) => json!({"ok": false, "error": error}),
            };
            let _ = reply.send(line.to_string());
        });
    }

    fn input_source(&mut self, synthetic: bool) {
        self.shell.set_synthetic_input(synthetic);
    }
}

fn button_index(button: Button) -> u64 {
    match button {
        Button::Close => 0,
        Button::Minimize => 1,
        Button::Maximize => 2,
    }
}

/// The derisk core apps, as in-process windows.
struct CoreApps {
    session: derisk_apps::Session,
    pending_actions: PendingActions,
}

impl Apps for CoreApps {
    /// Accepts app IDs (`org.derisk.files`), desktop file IDs
    /// (`org.derisk.files.desktop`), their last segment (`files`), and
    /// display names (`text editor`, `texteditor`).
    fn resolve(&self, name: &str) -> Option<AppId> {
        let name = name.trim().to_lowercase();
        let name = name.strip_suffix(".desktop").unwrap_or(&name).to_owned();
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

    /// Launches `app`, through its desktop action if one was queued for it.
    fn launch(&mut self, app: &AppId) -> std::result::Result<InstanceId, mcsapi_runtime::Error> {
        let action = self.pending_actions.lock().ok().and_then(|mut pending| {
            let i = pending.iter().position(|(id, _)| id == app.as_str())?;
            Some(pending.remove(i).1)
        });
        match action {
            Some(action) => self.session.launch_action(app, &action),
            None => self.session.launch(app),
        }
    }

    fn stop(&mut self, instance: InstanceId) {
        let _ = self.session.stop(instance);
    }

    fn app_mut(&mut self, instance: InstanceId) -> Option<&mut dyn mcsapi_compositor::App> {
        self.session
            .app_mut(instance)
            .map(|app| app as &mut dyn mcsapi_compositor::App)
    }

    fn prepare(&mut self, ctx: &egui::Context, theme: &Theme) {
        ctx.set_visuals(derisk_apps::visuals(theme));
    }
}

/// Runs the session until the window is closed.
pub fn run(options: Options) -> Result {
    let pending_actions = PendingActions::default();
    let session = Session::new(&options, pending_actions.clone());
    let apps = CoreApps {
        session: derisk_apps::Session::new()?,
        pending_actions,
    };
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

/// Numbers agent connections, so registered trees know their owner.
static CONNECTIONS: AtomicU64 = AtomicU64::new(0);

fn serve_agent(stream: UnixStream, remote: &Remote<Session>) {
    let Ok(writer) = stream.try_clone() else {
        return;
    };
    let conn = CONNECTIONS.fetch_add(1, Ordering::Relaxed);
    let writer = Arc::new(Mutex::new(writer));
    // Events for trees this connection registered arrive between responses.
    let (events, event_lines) = mpsc::channel::<String>();
    {
        let writer = writer.clone();
        std::thread::spawn(move || {
            for line in event_lines {
                let Ok(mut w) = writer.lock() else { return };
                if writeln!(w, "{line}").is_err() {
                    return;
                }
            }
        });
    }
    for line in BufReader::new(stream).lines() {
        let Ok(line) = line else { break };
        if line.trim().is_empty() {
            continue;
        }
        let (reply, response) = mpsc::channel();
        let events = events.clone();
        let queued = remote.run(move |session: &mut Session| {
            session.agent_line(conn, &line, reply, &events);
        });
        if !queued {
            return;
        }
        let Ok(response) = response.recv() else {
            break;
        };
        let Ok(mut w) = writer.lock() else { break };
        if writeln!(w, "{response}").is_err() {
            break;
        }
    }
    remote.run(move |session: &mut Session| session.agent_disconnected(conn));
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
        Keysym::space => Key::Space,
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
