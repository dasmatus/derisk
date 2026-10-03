//! The Smithay compositor host behind `derisk session`.
//!
//! It runs nested in a window of an existing X11 or Wayland session (Smithay's
//! winit backend), owns a Wayland socket for real clients, and feeds outputs,
//! input and xdg-shell toplevels into [`Shell`]. Every frame it draws, per
//! window from bottom to top, the egui title bar and then the client surface,
//! and finally the shell chrome ([`ShellUi::show`]) on top.
//!
//! The agent protocol is served on a Unix socket against the live shell, so
//! `derisk agent`-style requests move real windows.

use std::{
    collections::{HashMap, HashSet},
    ffi::OsString,
    io::{BufRead, BufReader, Write as _},
    os::unix::{
        fs::{DirBuilderExt, FileTypeExt, PermissionsExt},
        net::{UnixListener, UnixStream},
    },
    path::{Path, PathBuf},
    process::{Child, Command},
    sync::{Arc, mpsc},
    time::{Duration, Instant},
};

use derisk::{
    action::{Action, Effect},
    geom::rect,
    ipc,
    keys::{self, Key, Mods, SuperTap},
    overview::Battery,
    shell::{Mode, PointerOutcome, Shell, WindowPlacement},
    snap::{Direction, SnapZone},
    systemd::{self, Priority},
    time::Clock,
    ui::{ShellUi, paint_wallpaper},
};
use mcsapi::{WindowId, toolkit::egui};
use smithay::{
    backend::{
        egl,
        input::{
            AbsolutePositionEvent, Axis, AxisSource, ButtonState, Event, InputEvent, KeyState,
            KeyboardKeyEvent, PointerAxisEvent, PointerButtonEvent,
        },
        renderer::{
            Color32F, Frame, Renderer,
            element::{
                AsRenderElements, Element, RenderElement, surface::WaylandSurfaceRenderElement,
            },
            gles::GlesRenderer,
            utils::on_commit_buffer_handler,
        },
        winit::{self, WinitEvent, WinitGraphicsBackend},
    },
    delegate_compositor, delegate_data_device, delegate_output, delegate_seat, delegate_shm,
    delegate_xdg_decoration, delegate_xdg_shell,
    desktop::{PopupKind, PopupManager, Space, Window, WindowSurfaceType, find_popup_root_surface},
    input::{
        Seat, SeatHandler, SeatState,
        keyboard::{FilterResult, Keysym, KeysymHandle, ModifiersState, XkbConfig},
        pointer::{AxisFrame, ButtonEvent, CursorImageStatus, MotionEvent},
    },
    output::{Mode as OutputMode, Output, PhysicalProperties, Subpixel},
    reexports::{
        calloop::{
            EventLoop, Interest, LoopSignal, Mode as CalloopMode, PostAction,
            channel::{self, Channel},
            generic::Generic,
            timer::{TimeoutAction, Timer},
        },
        wayland_protocols::xdg::{
            decoration::zv1::server::zxdg_toplevel_decoration_v1::Mode as DecorationMode,
            shell::server::xdg_toplevel::State as ToplevelState,
        },
        wayland_server::{
            Client, Display, DisplayHandle, Resource,
            backend::{ClientData, ClientId, DisconnectReason},
            protocol::{wl_buffer, wl_seat, wl_surface::WlSurface},
        },
    },
    utils::{Logical, Point, Rectangle, SERIAL_COUNTER, Scale, Transform},
    wayland::{
        buffer::BufferHandler,
        compositor::{
            CompositorClientState, CompositorHandler, CompositorState, get_parent,
            is_sync_subsurface, with_states,
        },
        output::{OutputHandler, OutputManagerState},
        selection::{
            SelectionHandler,
            data_device::{
                ClientDndGrabHandler, DataDeviceHandler, DataDeviceState, ServerDndGrabHandler,
                set_data_device_focus,
            },
        },
        shell::xdg::{
            PopupSurface, PositionerState, ToplevelSurface, XdgShellHandler, XdgShellState,
            XdgToplevelSurfaceData,
            decoration::{XdgDecorationHandler, XdgDecorationState},
        },
        shm::{ShmHandler, ShmState},
        socket::ListeningSocketSource,
    },
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

/// An egui context plus its GL painter.
struct Egui {
    ctx: egui::Context,
    painter: Option<egui_glow::Painter>,
}

impl Egui {
    fn new() -> Self {
        Self {
            ctx: egui::Context::default(),
            painter: None,
        }
    }
}

/// Primitives ready to paint, with the texture updates they need.
struct Pass {
    primitives: Vec<egui::ClippedPrimitive>,
    textures: egui::TexturesDelta,
}

/// Who receives pointer input until the buttons are released.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Route {
    /// The shell chrome (egui).
    Chrome,
    /// The shell's window management (title bar buttons and drags).
    Shell,
    /// Wayland clients.
    Client,
}

struct AgentRequest {
    line: String,
    reply: mpsc::Sender<String>,
}

/// Compositor state.
pub struct Host {
    start: Instant,
    display: DisplayHandle,
    signal: LoopSignal,
    socket_name: OsString,
    execute: bool,

    compositor_state: CompositorState,
    xdg_shell_state: XdgShellState,
    #[allow(dead_code)]
    xdg_decoration_state: XdgDecorationState,
    shm_state: ShmState,
    #[allow(dead_code)]
    output_manager_state: OutputManagerState,
    seat_state: SeatState<Host>,
    data_device_state: DataDeviceState,
    popups: PopupManager,
    seat: Seat<Host>,
    space: Space<Window>,
    output: Output,
    backend: WinitGraphicsBackend<GlesRenderer>,
    gl: Option<Arc<glow::Context>>,

    shell: Shell,
    ui: ShellUi,
    windows: HashMap<WindowId, Window>,
    /// Toplevels that have not committed yet (no app ID or size known).
    unmanaged: Vec<Window>,
    keyboard_focus: Option<WindowId>,

    chrome: Egui,
    decorations: Egui,
    events: Vec<egui::Event>,
    pointer: Point<f64, Logical>,
    route: Option<Route>,
    buttons: u32,
    mods: Mods,
    egui_mods: egui::Modifiers,
    super_tap: SuperTap,
    suppressed_keys: HashSet<u32>,
    last_tick: Instant,

    children: Vec<Child>,
    launches: u64,
}

/// Per-client Wayland state.
#[derive(Default)]
struct ClientState {
    compositor_state: CompositorClientState,
}

impl ClientData for ClientState {
    fn initialized(&self, _client_id: ClientId) {}
    fn disconnected(&self, _client_id: ClientId, _reason: DisconnectReason) {}
}

/// Runs the session until the window is closed.
pub fn run(options: Options) -> Result {
    let mut event_loop: EventLoop<Host> = EventLoop::try_new()?;
    let display: Display<Host> = Display::new()?;
    let dh = display.handle();

    let (w, h) = options.size;
    let (backend, winit_loop) = winit::init_from_attributes::<GlesRenderer>(
        smithay::reexports::winit::window::Window::default_attributes()
            .with_title("derisk")
            .with_inner_size(smithay::reexports::winit::dpi::LogicalSize::new(w, h))
            .with_visible(true),
    )
    .map_err(|e| format!("cannot open a window for the session: {e}"))?;
    let size = backend.window_size();

    let output = Output::new(
        "derisk-0".into(),
        PhysicalProperties {
            size: (0, 0).into(),
            subpixel: Subpixel::Unknown,
            make: "derisk".into(),
            model: "nested".into(),
        },
    );
    let _global = output.create_global::<Host>(&dh);
    let mode = OutputMode {
        size,
        refresh: 60_000,
    };
    output.change_current_state(
        Some(mode),
        Some(Transform::Normal),
        None,
        Some((0, 0).into()),
    );
    output.set_preferred(mode);

    let mut seat_state = SeatState::new();
    let mut seat = seat_state.new_wl_seat(&dh, "seat0");
    seat.add_keyboard(XkbConfig::default(), 400, 30)?;
    seat.add_pointer();

    let mut space = Space::default();
    space.map_output(&output, (0, 0));

    let socket = ListeningSocketSource::new_auto()?;
    let socket_name = socket.socket_name().to_os_string();
    event_loop
        .handle()
        .insert_source(socket, |stream, _, host| {
            if let Err(e) = host
                .display
                .insert_client(stream, Arc::new(ClientState::default()))
            {
                log(Priority::Warning, &format!("rejected client: {e}"));
            }
        })?;
    event_loop.handle().insert_source(
        Generic::new(display, Interest::READ, CalloopMode::Level),
        |_, display, host| {
            // SAFETY: the display is never dropped while the loop runs.
            unsafe { display.get_mut().dispatch_clients(host)? };
            Ok(PostAction::Continue)
        },
    )?;

    let shell = Shell::new(rect(0, 0, size.w, size.h), false);
    let ui = ShellUi::new(&shell, options.reduced_motion);
    let mut host = Host {
        start: Instant::now(),
        display: dh.clone(),
        signal: event_loop.get_signal(),
        socket_name,
        execute: options.execute,
        compositor_state: CompositorState::new::<Host>(&dh),
        xdg_shell_state: XdgShellState::new::<Host>(&dh),
        xdg_decoration_state: XdgDecorationState::new::<Host>(&dh),
        shm_state: ShmState::new::<Host>(&dh, vec![]),
        output_manager_state: OutputManagerState::new_with_xdg_output::<Host>(&dh),
        data_device_state: DataDeviceState::new::<Host>(&dh),
        seat_state,
        popups: PopupManager::default(),
        seat,
        space,
        output,
        backend,
        gl: None,
        shell,
        ui,
        windows: HashMap::new(),
        unmanaged: Vec::new(),
        keyboard_focus: None,
        chrome: Egui::new(),
        decorations: Egui::new(),
        events: Vec::new(),
        pointer: (0.0, 0.0).into(),
        route: None,
        buttons: 0,
        mods: Mods::default(),
        egui_mods: egui::Modifiers::default(),
        super_tap: SuperTap::default(),
        suppressed_keys: HashSet::new(),
        last_tick: Instant::now() - Duration::from_secs(5),
        children: Vec::new(),
        launches: 0,
    };
    host.tick();

    event_loop
        .handle()
        .insert_source(winit_loop, |event, _, host| host.winit_event(event))?;

    let agents = agent_socket(options.socket)?;
    if let Some((path, channel)) = agents {
        log(
            Priority::Notice,
            &format!("agent protocol on {}", path.display()),
        );
        event_loop
            .handle()
            .insert_source(channel, |event, _, host| {
                if let channel::Event::Msg(request) = event {
                    let (response, effects) = ipc::handle_line(&mut host.shell, &request.line);
                    host.perform(effects);
                    let _ = request.reply.send(response);
                }
            })?;
    }

    if options.execute {
        for argv in systemd::session_start_argv(&host.socket_name.to_string_lossy()) {
            let _ = systemd::run(&argv);
        }
    }
    log(
        Priority::Notice,
        &format!(
            "derisk session on WAYLAND_DISPLAY={}",
            host.socket_name.to_string_lossy()
        ),
    );
    systemd::notify_ready("derisk session running");

    let launch = options.launch;
    event_loop.handle().insert_source(
        Timer::from_duration(Duration::from_millis(300)),
        move |_, _, host| {
            let actions = launch
                .iter()
                .map(|app| Action::Launch { app: app.clone() })
                .collect::<Vec<_>>();
            host.dispatch(actions);
            TimeoutAction::Drop
        },
    )?;
    event_loop
        .handle()
        .insert_source(Timer::immediate(), |_, _, host| {
            host.render();
            TimeoutAction::ToDuration(Duration::from_millis(16))
        })?;

    event_loop.run(None, &mut host, |host| {
        host.space.refresh();
        host.popups.cleanup();
        let _ = host.display.flush_clients();
    })?;
    systemd::notify_stopping();
    for child in &mut host.children {
        let _ = child.kill();
    }
    Ok(())
}

fn log(priority: Priority, message: &str) {
    systemd::log(priority, message, &[]);
}

/// Binds the agent socket and serves each connection on its own thread,
/// handing requests to the compositor loop.
fn agent_socket(path: Option<PathBuf>) -> Result<Option<(PathBuf, Channel<AgentRequest>)>> {
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
    let (sender, channel) = channel::channel::<AgentRequest>();
    std::thread::spawn(move || {
        for stream in listener.incoming().flatten() {
            let sender = sender.clone();
            std::thread::spawn(move || serve_agent(stream, &sender));
        }
    });
    Ok(Some((path, channel)))
}

fn serve_agent(stream: UnixStream, sender: &channel::Sender<AgentRequest>) {
    let Ok(mut writer) = stream.try_clone() else {
        return;
    };
    for line in BufReader::new(stream).lines() {
        let Ok(line) = line else { return };
        if line.trim().is_empty() {
            continue;
        }
        let (reply, response) = mpsc::channel();
        if sender.send(AgentRequest { line, reply }).is_err() {
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

impl Host {
    fn now_ms(&self) -> u32 {
        self.start.elapsed().as_millis() as u32
    }

    /// Refreshes the clock, battery and failed units about once a second.
    fn tick(&mut self) {
        if self.last_tick.elapsed() < Duration::from_secs(1) {
            return;
        }
        self.last_tick = Instant::now();
        self.shell.clock = Clock::now_utc();
        self.shell.battery = Battery::read(Path::new("/sys/class/power_supply"));
        if self.execute {
            self.shell.failed_units = systemd::failed_units();
        }
        self.children
            .retain_mut(|child| child.try_wait().is_ok_and(|status| status.is_none()));
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
                Effect::Launch { app } => self.launch(app),
                Effect::Close { window } => {
                    if let Some(toplevel) = WindowId::new(*window)
                        .and_then(|id| self.windows.get(&id))
                        .and_then(|w| w.toplevel())
                    {
                        toplevel.send_close();
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

    /// Launches an app on this session's Wayland socket.
    fn launch(&mut self, app: &str) {
        if !systemd::is_launchable(app) {
            log(Priority::Warning, &format!("refusing to launch {app:?}"));
            return;
        }
        self.launches += 1;
        if self.execute
            && let Some(argv) = systemd::launch_argv(app, self.launches)
        {
            // systemd-run takes options before the command; rebuild in order.
            let mut args: Vec<String> = argv[1..].to_vec();
            let split = args.iter().position(|a| a == "--").unwrap_or(args.len());
            args.insert(
                split,
                format!(
                    "--setenv=WAYLAND_DISPLAY={}",
                    self.socket_name.to_string_lossy()
                ),
            );
            match Command::new(&argv[0]).args(&args).status() {
                Ok(status) if status.success() => return,
                _ => log(Priority::Warning, "systemd-run failed, launching directly"),
            }
        }
        let executable = app.strip_suffix(".desktop").unwrap_or(app);
        let mut command = Command::new(executable);
        command
            .env("WAYLAND_DISPLAY", &self.socket_name)
            .env("XDG_SESSION_TYPE", "wayland")
            .env(
                "LANG",
                std::env::var("LANG")
                    .ok()
                    .filter(|l| l.contains("UTF-8"))
                    .unwrap_or_else(|| "C.UTF-8".into()),
            )
            .env("GDK_BACKEND", "wayland")
            .env("QT_QPA_PLATFORM", "wayland")
            .env_remove("DISPLAY");
        match command.spawn() {
            Ok(child) => self.children.push(child),
            Err(e) => log(Priority::Warning, &format!("cannot launch {app}: {e}")),
        }
    }

    fn window_of(&self, surface: &WlSurface) -> Option<(WindowId, &Window)> {
        self.windows
            .iter()
            .find(|(_, w)| w.toplevel().is_some_and(|t| t.wl_surface() == surface))
            .map(|(id, w)| (*id, w))
    }

    /// Pushes shell placements to toplevels and the space.
    fn sync(&mut self) {
        let placements = self.shell.placements();
        for (id, window) in &self.windows {
            if !placements.iter().any(|p| p.window == *id) {
                self.space.unmap_elem(window);
            }
        }
        for p in &placements {
            let Some(window) = self.windows.get(&p.window) else {
                continue;
            };
            if let Some(toplevel) = window.toplevel() {
                toplevel.with_pending_state(|state| {
                    state.size = Some(p.client.size);
                    let tiled_edges = match p.mode {
                        Mode::Tiled => [true, true, true, true],
                        Mode::Floating { .. } => [false, false, false, false],
                        Mode::Snapped { zone } => match zone {
                            SnapZone::Left => [true, false, true, true],
                            SnapZone::Right => [false, true, true, true],
                            SnapZone::TopLeft => [true, false, true, false],
                            SnapZone::TopRight => [false, true, true, false],
                            SnapZone::BottomLeft => [true, false, false, true],
                            SnapZone::BottomRight => [false, true, false, true],
                            SnapZone::Maximize => [false, false, false, false],
                        },
                    };
                    for (s, tiled) in [
                        ToplevelState::TiledLeft,
                        ToplevelState::TiledRight,
                        ToplevelState::TiledTop,
                        ToplevelState::TiledBottom,
                    ]
                    .into_iter()
                    .zip(tiled_edges)
                    {
                        if tiled {
                            state.states.set(s);
                        } else {
                            state.states.unset(s);
                        }
                    }
                    if p.mode
                        == (Mode::Snapped {
                            zone: SnapZone::Maximize,
                        })
                    {
                        state.states.set(ToplevelState::Maximized);
                    } else {
                        state.states.unset(ToplevelState::Maximized);
                    }
                    if p.focused {
                        state.states.set(ToplevelState::Activated);
                    } else {
                        state.states.unset(ToplevelState::Activated);
                    }
                });
                toplevel.send_pending_configure();
            }
            let offset = window.geometry().loc;
            self.space
                .map_element(window.clone(), p.client.loc - offset, false);
            self.space.raise_element(window, false);
        }

        let focused = self.shell.focused();
        if focused != self.keyboard_focus {
            self.keyboard_focus = focused;
            let surface = focused
                .and_then(|id| self.windows.get(&id))
                .and_then(|w| w.toplevel())
                .map(|t| t.wl_surface().clone());
            if let Some(keyboard) = self.seat.get_keyboard() {
                keyboard.set_focus(self, surface, SERIAL_COUNTER.next_serial());
            }
        }
    }

    fn winit_event(&mut self, event: WinitEvent) {
        match event {
            WinitEvent::Resized { size, .. } => {
                let mode = OutputMode {
                    size,
                    refresh: 60_000,
                };
                self.output
                    .change_current_state(Some(mode), None, None, None);
                self.output.set_preferred(mode);
                self.shell.set_output(rect(0, 0, size.w, size.h), false);
            }
            WinitEvent::Input(event) => self.input(event),
            WinitEvent::CloseRequested => self.signal.stop(),
            WinitEvent::Focus(_) | WinitEvent::Redraw => {}
        }
    }

    /// Whether the chrome owns the pointer at its current position.
    fn chrome_wants_pointer(&self) -> bool {
        let (x, y) = (self.pointer.x as i32, self.pointer.y as i32);
        let in_rect = |g: smithay::utils::Rectangle<i32, Logical>| {
            x >= g.loc.x && y >= g.loc.y && x < g.loc.x + g.size.w && y < g.loc.y + g.size.h
        };
        self.shell.overview_visible()
            || y < self.shell.profile().top_bar
            || self.shell.snap_assist().is_some_and(|a| in_rect(a.frame))
            || self.chrome.ctx.egui_wants_pointer_input()
            || egui::Popup::is_any_open(&self.chrome.ctx)
            || !self.ui.startup.frame(self.now_ms()).done
    }

    fn input(&mut self, event: InputEvent<smithay::backend::winit::WinitInput>) {
        match event {
            InputEvent::Keyboard { event } => {
                let serial = SERIAL_COUNTER.next_serial();
                let time = Event::time_msec(&event);
                let pressed = event.state() == KeyState::Pressed;
                let Some(keyboard) = self.seat.get_keyboard() else {
                    return;
                };
                let action = keyboard.input::<Option<Action>, _>(
                    self,
                    event.key_code(),
                    event.state(),
                    serial,
                    time,
                    |host, modifiers, handle| {
                        host.filter_key(modifiers, &handle, pressed, event.key_code().into())
                    },
                );
                if let Some(Some(action)) = action {
                    self.dispatch(vec![action]);
                }
            }
            InputEvent::PointerMotionAbsolute { event } => {
                let size = self.backend.window_size();
                self.pointer = event.position_transformed((size.w, size.h).into());
                self.pointer_motion(Event::time_msec(&event));
            }
            InputEvent::PointerButton { event } => {
                let pressed = event.state() == ButtonState::Pressed;
                self.pointer_button(
                    event.button_code(),
                    event.state(),
                    pressed,
                    Event::time_msec(&event),
                );
            }
            InputEvent::PointerAxis { event } => {
                let h = event
                    .amount(Axis::Horizontal)
                    .or_else(|| {
                        event
                            .amount_v120(Axis::Horizontal)
                            .map(|v| v * 15.0 / 120.0)
                    })
                    .unwrap_or(0.0);
                let v = event
                    .amount(Axis::Vertical)
                    .or_else(|| event.amount_v120(Axis::Vertical).map(|v| v * 15.0 / 120.0))
                    .unwrap_or(0.0);
                if self.chrome_wants_pointer() {
                    self.events.push(egui::Event::MouseWheel {
                        unit: egui::MouseWheelUnit::Point,
                        delta: egui::vec2(-h as f32, -v as f32),
                        phase: egui::TouchPhase::Move,
                        modifiers: self.egui_mods,
                    });
                    return;
                }
                let mut frame = AxisFrame::new(Event::time_msec(&event)).source(AxisSource::Wheel);
                if h != 0.0 {
                    frame = frame.value(Axis::Horizontal, h);
                }
                if v != 0.0 {
                    frame = frame.value(Axis::Vertical, v);
                }
                if let Some(pointer) = self.seat.get_pointer() {
                    pointer.axis(self, frame);
                    pointer.frame(self);
                }
            }
            _ => {}
        }
    }

    fn surface_under(&self) -> Option<(WlSurface, Point<f64, Logical>)> {
        let (window, loc) = self.space.element_under(self.pointer)?;
        window
            .surface_under(self.pointer - loc.to_f64(), WindowSurfaceType::ALL)
            .map(|(surface, offset)| (surface, (offset + loc).to_f64()))
    }

    fn pointer_motion(&mut self, time: u32) {
        let pos = egui::pos2(self.pointer.x as f32, self.pointer.y as f32);
        self.events.push(egui::Event::PointerMoved(pos));
        let point = (self.pointer.x as i32, self.pointer.y as i32);
        let route = self.route.unwrap_or(if self.chrome_wants_pointer() {
            Route::Chrome
        } else {
            Route::Client
        });
        if route == Route::Shell {
            self.shell.pointer_motion(point);
        }
        let focus = if route == Route::Client {
            self.surface_under()
        } else {
            None
        };
        if let Some(pointer) = self.seat.get_pointer() {
            pointer.motion(
                self,
                focus,
                &MotionEvent {
                    location: self.pointer,
                    serial: SERIAL_COUNTER.next_serial(),
                    time,
                },
            );
            pointer.frame(self);
        }
    }

    fn pointer_button(&mut self, button: u32, state: ButtonState, pressed: bool, time: u32) {
        const BTN_LEFT: u32 = 0x110;
        const BTN_RIGHT: u32 = 0x111;
        self.super_tap.cancel();
        if pressed {
            self.buttons += 1;
        } else {
            self.buttons = self.buttons.saturating_sub(1);
        }
        let egui_button = match button {
            BTN_LEFT => Some(egui::PointerButton::Primary),
            BTN_RIGHT => Some(egui::PointerButton::Secondary),
            0x112 => Some(egui::PointerButton::Middle),
            _ => None,
        };
        let point = (self.pointer.x as i32, self.pointer.y as i32);
        if pressed && self.route.is_none() {
            self.route = Some(if self.chrome_wants_pointer() {
                Route::Chrome
            } else if button == BTN_LEFT {
                match self.shell.pointer_down(point, u64::from(time)) {
                    Ok(PointerOutcome::Handled { effects }) => {
                        self.perform(effects);
                        Route::Shell
                    }
                    Ok(PointerOutcome::Client { .. } | PointerOutcome::Desktop) | Err(_) => {
                        Route::Client
                    }
                }
            } else {
                // Other buttons focus the window under the pointer too.
                if let Some(p) = self.shell.window_at(point) {
                    self.dispatch(vec![Action::Focus {
                        window: p.window.get(),
                    }]);
                }
                Route::Client
            });
        }
        let route = self.route.unwrap_or(Route::Client);
        match route {
            Route::Chrome => {
                if let Some(button) = egui_button {
                    self.events.push(egui::Event::PointerButton {
                        pos: egui::pos2(self.pointer.x as f32, self.pointer.y as f32),
                        button,
                        pressed,
                        modifiers: self.egui_mods,
                    });
                }
            }
            Route::Shell => {
                if !pressed {
                    self.shell.pointer_up();
                }
            }
            Route::Client => {
                if let Some(pointer) = self.seat.get_pointer() {
                    pointer.button(
                        self,
                        &ButtonEvent {
                            button,
                            state,
                            serial: SERIAL_COUNTER.next_serial(),
                            time,
                        },
                    );
                    pointer.frame(self);
                }
            }
        }
        if self.buttons == 0 {
            self.route = None;
        }
    }

    /// Decides what a key does: a shell shortcut, input for the chrome, or
    /// input for the focused client.
    fn filter_key(
        &mut self,
        modifiers: &ModifiersState,
        handle: &KeysymHandle<'_>,
        pressed: bool,
        keycode: u32,
    ) -> FilterResult<Option<Action>> {
        if !pressed && self.suppressed_keys.remove(&keycode) {
            return FilterResult::Intercept(None);
        }
        self.mods = Mods {
            logo: modifiers.logo,
            shift: modifiers.shift,
            ctrl: modifiers.ctrl,
            alt: modifiers.alt,
        };
        self.egui_mods = egui::Modifiers {
            alt: modifiers.alt,
            ctrl: modifiers.ctrl,
            shift: modifiers.shift,
            mac_cmd: false,
            command: modifiers.ctrl,
        };
        let sym = handle.modified_sym();
        let raw = handle.raw_latin_sym_or_raw_current_sym().unwrap_or(sym);
        let is_super = matches!(raw, Keysym::Super_L | Keysym::Super_R);
        if self.super_tap.key(is_super, pressed) {
            return FilterResult::Intercept(Some(Action::Overview { visible: None }));
        }
        if is_super {
            return FilterResult::Intercept(None);
        }
        if pressed
            && let Some(key) = layout_key(raw)
            && let Some(action) = keys::binding(self.mods, key)
        {
            self.suppressed_keys.insert(keycode);
            return FilterResult::Intercept(Some(action));
        }
        if pressed && raw == Keysym::Escape && self.shell.overview_visible() {
            self.suppressed_keys.insert(keycode);
            return FilterResult::Intercept(Some(Action::Overview {
                visible: Some(false),
            }));
        }
        if self.shell.overview_visible() || self.chrome.ctx.egui_wants_keyboard_input() {
            if let Some(key) = egui_key(raw) {
                self.events.push(egui::Event::Key {
                    key,
                    physical_key: Some(key),
                    pressed,
                    repeat: false,
                    modifiers: self.egui_mods,
                });
            }
            if pressed
                && !modifiers.ctrl
                && !modifiers.alt
                && let Some(c) = sym.key_char().filter(|c| !c.is_control())
            {
                self.events.push(egui::Event::Text(c.to_string()));
            }
            return FilterResult::Intercept(None);
        }
        FilterResult::Forward
    }

    /// Draws one frame: wallpaper, then per window its title bar and surface,
    /// then the chrome.
    fn render(&mut self) {
        self.tick();
        self.sync();
        let size = self.backend.window_size();
        let screen =
            egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(size.w as f32, size.h as f32));
        let elapsed = self.now_ms();
        let time = self.start.elapsed().as_secs_f64();

        // Chrome first, so its actions take effect this frame.
        let input = egui::RawInput {
            screen_rect: Some(screen),
            events: std::mem::take(&mut self.events),
            time: Some(time),
            focused: true,
            ..Default::default()
        };
        let (ui, shell) = (&mut self.ui, &self.shell);
        let mut actions = Vec::new();
        let pointer = egui::pos2(self.pointer.x as f32, self.pointer.y as f32);
        let output = self.chrome.ctx.run_ui(input, |root| {
            actions = ui.show(root, shell, elapsed);
            paint_cursor(root.ctx(), pointer);
        });
        let chrome = Pass {
            primitives: self
                .chrome
                .ctx
                .tessellate(output.shapes, output.pixels_per_point),
            textures: output.textures_delta,
        };

        let placements = self.shell.placements();
        let mut passes = Vec::with_capacity(placements.len() + 1);
        let wallpaper = |painter: &egui::Painter| paint_wallpaper(painter, screen, &self.ui.theme);
        passes.push(self.paint_only(screen, time, wallpaper));
        for p in &placements {
            passes.push(self.paint_only(screen, time, |painter| {
                self.ui.paint_decoration(painter, &self.shell, p)
            }));
        }

        if let Err(e) = self.draw(size, &placements, passes, chrome) {
            log(Priority::Warning, &format!("render failed: {e}"));
        }

        let now = self.start.elapsed();
        for window in self.space.elements() {
            window.send_frame(&self.output, now, Some(Duration::ZERO), |_, _| {
                Some(self.output.clone())
            });
        }
        self.dispatch(actions);
    }

    fn paint_only(&self, screen: egui::Rect, time: f64, paint: impl Fn(&egui::Painter)) -> Pass {
        let input = egui::RawInput {
            screen_rect: Some(screen),
            time: Some(time),
            ..Default::default()
        };
        let output = self
            .decorations
            .ctx
            .run_ui(input, |root| paint(root.painter()));
        Pass {
            primitives: self
                .decorations
                .ctx
                .tessellate(output.shapes, output.pixels_per_point),
            textures: output.textures_delta,
        }
    }

    fn draw(
        &mut self,
        size: smithay::utils::Size<i32, smithay::utils::Physical>,
        placements: &[WindowPlacement],
        passes: Vec<Pass>,
        chrome: Pass,
    ) -> Result {
        let scale = Scale::from(1.0);
        let screen_px = [size.w as u32, size.h as u32];
        let full = Rectangle::from_size(size);

        let (renderer, mut framebuffer) = self.backend.bind()?;
        if self.gl.is_none() {
            // SAFETY: the renderer's EGL context is current inside with_context,
            // and the loader only resolves GL symbols through EGL.
            let gl = renderer.with_context(|_| unsafe {
                glow::Context::from_loader_function(|s| egl::get_proc_address(s))
            })?;
            let gl = Arc::new(gl);
            self.chrome.painter = Some(egui_glow::Painter::new(gl.clone(), "", None, false)?);
            self.decorations.painter = Some(egui_glow::Painter::new(gl.clone(), "", None, false)?);
            self.gl = Some(gl);
        }

        // Import client buffers before starting the frame.
        let mut surfaces: Vec<Vec<WaylandSurfaceRenderElement<GlesRenderer>>> = Vec::new();
        for p in placements {
            let elements = self
                .windows
                .get(&p.window)
                .and_then(|w| Some((w, self.space.element_location(w)?)))
                .map(|(w, loc)| {
                    w.render_elements(renderer, loc.to_physical_precise_round(scale), scale, 1.0)
                })
                .unwrap_or_default();
            surfaces.push(elements);
        }

        let gl = self.gl.clone().expect("initialized above");
        let deco = self
            .decorations
            .painter
            .as_mut()
            .expect("initialized above");
        let chrome_painter = self.chrome.painter.as_mut().expect("initialized above");
        let mut frame = renderer.render(&mut framebuffer, size, Transform::Flipped180)?;
        frame.clear(Color32F::new(0.06, 0.09, 0.16, 1.0), &[full])?;

        let mut passes = passes.into_iter();
        if let Some(mut wallpaper) = passes.next() {
            paint(&gl, deco, screen_px, &mut wallpaper);
        }
        for (mut decoration, elements) in passes.zip(&surfaces) {
            paint(&gl, deco, screen_px, &mut decoration);
            // Elements come topmost first.
            for element in elements.iter().rev() {
                let dst = element.geometry(scale);
                element.draw(
                    &mut frame,
                    element.src(),
                    dst,
                    &[Rectangle::from_size(dst.size)],
                    &[],
                )?;
            }
        }
        let mut chrome = chrome;
        paint(&gl, chrome_painter, screen_px, &mut chrome);
        let _sync = frame.finish()?;
        drop(framebuffer);
        self.backend.submit(Some(&[full]))?;
        Ok(())
    }
}

/// Draws the pointer above everything (the nested window hides the host cursor).
fn paint_cursor(ctx: &egui::Context, at: egui::Pos2) {
    let painter = ctx.layer_painter(egui::LayerId::new(
        egui::Order::Debug,
        egui::Id::new("derisk-cursor"),
    ));
    let points = [
        (0.0, 0.0),
        (0.0, 17.0),
        (4.5, 13.0),
        (7.5, 19.5),
        (10.0, 18.5),
        (7.0, 12.0),
        (12.5, 12.0),
    ]
    .map(|(x, y)| at + egui::vec2(x, y))
    .to_vec();
    painter.add(egui::Shape::convex_polygon(
        points.clone(),
        egui::Color32::BLACK,
        egui::Stroke::NONE,
    ));
    painter.add(egui::Shape::closed_line(
        points,
        egui::Stroke::new(1.5, egui::Color32::WHITE),
    ));
}

/// Paints an egui pass into the current framebuffer and restores the GL state
/// Smithay's renderer relies on.
fn paint(gl: &glow::Context, painter: &mut egui_glow::Painter, screen: [u32; 2], pass: &mut Pass) {
    use glow::HasContext as _;
    painter.paint_and_update_textures(screen, 1.0, &pass.primitives, &mut pass.textures);
    // SAFETY: plain state resets on the current context.
    unsafe {
        gl.disable(glow::SCISSOR_TEST);
        gl.enable(glow::BLEND);
        gl.blend_func(glow::ONE, glow::ONE_MINUS_SRC_ALPHA);
        gl.bind_vertex_array(None);
        gl.bind_buffer(glow::ARRAY_BUFFER, None);
        gl.use_program(None);
        gl.active_texture(glow::TEXTURE0);
        gl.bind_texture(glow::TEXTURE_2D, None);
        gl.viewport(0, 0, screen[0] as i32, screen[1] as i32);
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

fn egui_key(sym: Keysym) -> Option<egui::Key> {
    use egui::Key as K;
    Some(match sym {
        Keysym::BackSpace => K::Backspace,
        Keysym::Return | Keysym::KP_Enter | Keysym::Linefeed => K::Enter,
        Keysym::Tab => K::Tab,
        Keysym::Escape => K::Escape,
        Keysym::Delete => K::Delete,
        Keysym::Home => K::Home,
        Keysym::End => K::End,
        Keysym::Left => K::ArrowLeft,
        Keysym::Right => K::ArrowRight,
        Keysym::Up => K::ArrowUp,
        Keysym::Down => K::ArrowDown,
        Keysym::space => K::Space,
        _ => {
            let c = sym.key_char()?.to_ascii_uppercase();
            K::from_name(&c.to_string())?
        }
    })
}

impl BufferHandler for Host {
    fn buffer_destroyed(&mut self, _buffer: &wl_buffer::WlBuffer) {}
}

impl CompositorHandler for Host {
    fn compositor_state(&mut self) -> &mut CompositorState {
        &mut self.compositor_state
    }

    fn client_compositor_state<'a>(&self, client: &'a Client) -> &'a CompositorClientState {
        &client
            .get_data::<ClientState>()
            .expect("every client is inserted with ClientState")
            .compositor_state
    }

    fn commit(&mut self, surface: &WlSurface) {
        on_commit_buffer_handler::<Self>(surface);
        if !is_sync_subsurface(surface) {
            let mut root = surface.clone();
            while let Some(parent) = get_parent(&root) {
                root = parent;
            }
            if let Some((_, window)) = self.window_of(&root) {
                window.on_commit();
            }
        }
        self.manage_on_first_commit(surface);
        self.popups.commit(surface);
        if let Some(PopupKind::Xdg(popup)) = self.popups.find_popup(surface)
            && !popup.is_initial_configure_sent()
        {
            let _ = popup.send_configure();
        }
    }
}

impl Host {
    /// Starts managing a toplevel on its initial commit, when its app ID and
    /// title are known, so the first configure already has its tiled size.
    fn manage_on_first_commit(&mut self, surface: &WlSurface) {
        let Some(i) = self
            .unmanaged
            .iter()
            .position(|w| w.toplevel().is_some_and(|t| t.wl_surface() == surface))
        else {
            return;
        };
        let window = self.unmanaged.remove(i);
        let Some(toplevel) = window.toplevel().cloned() else {
            return;
        };
        let (app_id, title) = with_states(surface, |states| {
            states
                .data_map
                .get::<XdgToplevelSurfaceData>()
                .and_then(|d| d.lock().ok())
                .map(|d| (d.app_id.clone(), d.title.clone()))
                .unwrap_or_default()
        });
        let app_id = app_id.unwrap_or_else(|| "app".into());
        let (id, effects) = self.shell.map_window(&app_id, &title.unwrap_or_default());
        self.windows.insert(id, window);
        self.perform(effects);
        self.sync();
        if !toplevel.is_initial_configure_sent() {
            toplevel.send_configure();
        }
    }

    fn label_changed(&mut self, surface: &ToplevelSurface) {
        let Some((id, _)) = self.window_of(surface.wl_surface()) else {
            return;
        };
        let title = with_states(surface.wl_surface(), |states| {
            states
                .data_map
                .get::<XdgToplevelSurfaceData>()
                .and_then(|d| d.lock().ok())
                .and_then(|d| d.title.clone())
        });
        let _ = self.shell.set_title(id, &title.unwrap_or_default());
    }
}

impl XdgShellHandler for Host {
    fn xdg_shell_state(&mut self) -> &mut XdgShellState {
        &mut self.xdg_shell_state
    }

    fn new_toplevel(&mut self, surface: ToplevelSurface) {
        surface.with_pending_state(|state| {
            state.decoration_mode = Some(DecorationMode::ServerSide);
        });
        self.unmanaged.push(Window::new_wayland_window(surface));
    }

    fn toplevel_destroyed(&mut self, surface: ToplevelSurface) {
        self.unmanaged
            .retain(|w| w.toplevel().is_some_and(|t| t != &surface));
        if let Some((id, window)) = self
            .window_of(surface.wl_surface())
            .map(|(id, w)| (id, w.clone()))
        {
            self.space.unmap_elem(&window);
            self.windows.remove(&id);
            let _ = self.shell.unmap_window(id);
            if self.keyboard_focus == Some(id) {
                self.keyboard_focus = None;
            }
        }
    }

    fn title_changed(&mut self, surface: ToplevelSurface) {
        self.label_changed(&surface);
    }

    fn new_popup(&mut self, surface: PopupSurface, _positioner: PositionerState) {
        let _ = self.popups.track_popup(PopupKind::Xdg(surface));
    }

    fn reposition_request(
        &mut self,
        surface: PopupSurface,
        positioner: PositionerState,
        token: u32,
    ) {
        surface.with_pending_state(|state| {
            state.geometry = positioner.get_geometry();
            state.positioner = positioner;
        });
        surface.send_repositioned(token);
    }

    fn grab(
        &mut self,
        surface: PopupSurface,
        _seat: wl_seat::WlSeat,
        _serial: smithay::utils::Serial,
    ) {
        // Popup grabs are not implemented; dismiss when focus leaves instead.
        let _ = find_popup_root_surface(&PopupKind::Xdg(surface));
    }

    fn maximize_request(&mut self, surface: ToplevelSurface) {
        if let Some((id, _)) = self.window_of(surface.wl_surface()) {
            self.dispatch(vec![Action::ToggleMaximize {
                window: Some(id.get()),
            }]);
        }
    }

    fn minimize_request(&mut self, surface: ToplevelSurface) {
        if let Some((id, _)) = self.window_of(surface.wl_surface()) {
            self.dispatch(vec![Action::Minimize {
                window: Some(id.get()),
            }]);
        }
    }
}

impl XdgDecorationHandler for Host {
    fn new_decoration(&mut self, toplevel: ToplevelSurface) {
        toplevel.with_pending_state(|state| {
            state.decoration_mode = Some(DecorationMode::ServerSide);
        });
        if toplevel.is_initial_configure_sent() {
            toplevel.send_pending_configure();
        }
    }

    fn request_mode(&mut self, toplevel: ToplevelSurface, _mode: DecorationMode) {
        // derisk always draws its own title bars (buttons on the left).
        self.new_decoration(toplevel);
    }

    fn unset_mode(&mut self, toplevel: ToplevelSurface) {
        self.new_decoration(toplevel);
    }
}

impl ShmHandler for Host {
    fn shm_state(&self) -> &ShmState {
        &self.shm_state
    }
}

impl SeatHandler for Host {
    type KeyboardFocus = WlSurface;
    type PointerFocus = WlSurface;
    type TouchFocus = WlSurface;

    fn seat_state(&mut self) -> &mut SeatState<Host> {
        &mut self.seat_state
    }

    fn cursor_image(&mut self, _seat: &Seat<Self>, _image: CursorImageStatus) {}

    fn focus_changed(&mut self, seat: &Seat<Self>, focused: Option<&WlSurface>) {
        let client = focused.and_then(|s| self.display.get_client(s.id()).ok());
        set_data_device_focus(&self.display, seat, client);
    }
}

impl SelectionHandler for Host {
    type SelectionUserData = ();
}

impl DataDeviceHandler for Host {
    fn data_device_state(&self) -> &DataDeviceState {
        &self.data_device_state
    }
}

impl ClientDndGrabHandler for Host {}
impl ServerDndGrabHandler for Host {}
impl OutputHandler for Host {}

delegate_compositor!(Host);
delegate_xdg_shell!(Host);
delegate_xdg_decoration!(Host);
delegate_shm!(Host);
delegate_seat!(Host);
delegate_data_device!(Host);
delegate_output!(Host);
