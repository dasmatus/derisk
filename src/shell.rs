//! Shell state on top of mcsapi: tiling plus floating, snapped and minimized
//! windows, Windows-style dragging, and the overview, menu and tray models.

use std::collections::{BTreeMap, BTreeSet};

use mcsapi::{Desktop, Geometry, Layout, WindowId, WorkspaceId};
use serde::Serialize;

use crate::{
    action::{Action, Effect, LayoutKind},
    adaptive::{FormFactor, Habits, Profile},
    decorations::{Button, ClickTracker, Hit},
    geom::{Point, Rect, centered, contains, inset, rect},
    menu::{self, GlobalMenu},
    overview::Battery,
    snap::{Nudge, SnapZone, zone_at},
    systemd::{self, SessionOp},
    time::Clock,
    tray::Tray,
};

/// Number of workspaces created by [`Shell::new`].
pub const WORKSPACES: u64 = 9;

/// Pointer travel, in logical pixels, before a title bar press becomes a drag.
pub const DRAG_THRESHOLD: i32 = 6;

/// A shell operation failure.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Error {
    /// An mcsapi policy error.
    Policy(mcsapi::Error),
    /// The window is not managed.
    UnknownWindow(u64),
    /// The workspace does not exist.
    UnknownWorkspace(u64),
    /// The action needs a focused window and there is none.
    NoFocusedWindow,
    /// A destructive session operation was not confirmed by the user.
    NeedsConfirmation(SessionOp),
    /// The app name is not a plain command or desktop-file ID.
    NotLaunchable(String),
    /// No tray item has this ID.
    UnknownTrayItem(String),
    /// Unit actions only apply to currently failed user units.
    UnknownUnit(String),
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Policy(e) => e.fmt(f),
            Self::UnknownWindow(id) => write!(f, "unknown window: {id}"),
            Self::UnknownWorkspace(id) => write!(f, "unknown workspace: {id}"),
            Self::NoFocusedWindow => f.write_str("no focused window"),
            Self::NeedsConfirmation(op) => {
                write!(
                    f,
                    "{op:?} needs explicit confirmation (set \"confirmed\": true)"
                )
            }
            Self::NotLaunchable(app) => write!(f, "not a launchable app name: {app:?}"),
            Self::UnknownTrayItem(id) => write!(f, "unknown tray item: {id:?}"),
            Self::UnknownUnit(unit) => write!(f, "not a failed user unit: {unit:?}"),
        }
    }
}

impl std::error::Error for Error {}

impl From<mcsapi::Error> for Error {
    fn from(e: mcsapi::Error) -> Self {
        Self::Policy(e)
    }
}

/// How a window is placed.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(tag = "mode", rename_all = "snake_case")]
pub enum Mode {
    /// Arranged automatically by the workspace layout.
    Tiled,
    /// Free-floating at a fixed frame.
    Floating {
        /// Frame.
        #[serde(flatten)]
        frame: Rect,
    },
    /// Snapped to a zone of the work area.
    Snapped {
        /// Zone.
        zone: SnapZone,
    },
}

#[derive(Clone, Debug)]
struct WindowInfo {
    app_id: String,
    title: String,
    mode: Mode,
    /// Mode to return to when un-maximizing.
    before_maximize: Option<Mode>,
    /// Last free-floating frame, used when dragging a window out of a snap.
    restore: Option<Geometry>,
    minimized: bool,
}

/// Where a window is drawn this frame.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct WindowPlacement {
    /// The window.
    pub window: WindowId,
    /// Full frame including the server-side title bar.
    pub frame: Geometry,
    /// Area for the client surface; configure the toplevel to this size.
    pub client: Geometry,
    /// Placement mode.
    pub mode: Mode,
    /// Keyboard focus.
    pub focused: bool,
}

/// Where the dragged window would land if released now.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DropTarget {
    /// Stay floating where it is.
    Float,
    /// Snap into a zone; `preview` is the outline to draw.
    Snap {
        /// Zone.
        zone: SnapZone,
        /// Preview frame.
        preview: Geometry,
    },
    /// Rejoin tiling at the position of another tiled window.
    Tile {
        /// Window under the pointer.
        onto: WindowId,
        /// Preview frame.
        preview: Geometry,
    },
}

#[derive(Clone, Copy, Debug)]
struct Drag {
    window: WindowId,
    origin: Point,
    grab: Point,
    started: bool,
    target: DropTarget,
}

/// After snapping, the other windows offered for the complementary zone.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SnapAssist {
    /// Zone to fill.
    pub zone: SnapZone,
    /// Its frame.
    pub frame: Geometry,
    /// Candidate windows.
    pub candidates: Vec<WindowId>,
}

/// What a pointer press did.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum PointerOutcome {
    /// Nothing was under the pointer.
    Desktop,
    /// The press hit a client surface; forward it at these surface-local coordinates.
    Client {
        /// Window.
        window: WindowId,
        /// Surface-local position.
        local: Point,
    },
    /// The shell handled the press (decorations, drag start); `effects` are for the host.
    Handled {
        /// Host work to do.
        effects: Vec<Effect>,
    },
}

/// The desktop shell state.
#[derive(Debug)]
pub struct Shell {
    desktop: Desktop,
    windows: BTreeMap<WindowId, WindowInfo>,
    stack: Vec<WindowId>,
    next_id: u64,
    output: Geometry,
    profile: Profile,
    drag: Option<Drag>,
    clicks: ClickTracker,
    overview: bool,
    snap_assist: Option<SnapAssist>,
    pending: Vec<(String, Vec<Action>)>,
    /// Global menus registered by apps.
    pub menus: GlobalMenu,
    /// System tray items.
    pub tray: Tray,
    /// Learned launch habits.
    pub habits: Habits,
    /// Current time, updated by the host.
    pub clock: Clock,
    /// Battery state, updated by the host.
    pub battery: Option<Battery>,
    /// Failed user units, updated by the host (see [`systemd::failed_units`]).
    pub failed_units: Vec<String>,
}

fn workspace(id: u64) -> Result<WorkspaceId, Error> {
    WorkspaceId::new(id).ok_or(Error::UnknownWorkspace(id))
}

impl Shell {
    /// Creates a shell for an output with [`WORKSPACES`] workspaces.
    pub fn new(output: Geometry, touch: bool) -> Self {
        let desktop = Desktop::new((1..=WORKSPACES).filter_map(WorkspaceId::new))
            .expect("workspace IDs are unique and nonzero");
        let mut shell = Self {
            desktop,
            windows: BTreeMap::new(),
            stack: Vec::new(),
            next_id: 1,
            output,
            profile: Profile::detect(output.size.w, output.size.h, touch),
            drag: None,
            clicks: ClickTracker::default(),
            overview: false,
            snap_assist: None,
            pending: Vec::new(),
            menus: GlobalMenu::default(),
            tray: Tray::default(),
            habits: Habits::default(),
            clock: Clock::default(),
            battery: None,
            failed_units: Vec::new(),
        };
        shell.apply_profile_layout();
        shell
    }

    /// Updates the output (resolution change, rotation, dock/undock).
    pub fn set_output(&mut self, output: Geometry, touch: bool) {
        let before = self.profile.form_factor;
        self.output = output;
        self.profile = Profile::detect(output.size.w, output.size.h, touch);
        if before != self.profile.form_factor {
            self.apply_profile_layout();
        }
    }

    fn apply_profile_layout(&mut self) {
        let active = self.desktop.active().id();
        let ids: Vec<WorkspaceId> = self.desktop.workspaces().map(|w| w.id()).collect();
        for id in ids {
            self.desktop.switch_to(id).expect("existing workspace");
            self.desktop.set_layout(self.profile.layout);
        }
        self.desktop.switch_to(active).expect("existing workspace");
    }

    /// The adaptive metrics in use.
    pub fn profile(&self) -> &Profile {
        &self.profile
    }

    /// The whole output.
    pub fn output(&self) -> Geometry {
        self.output
    }

    /// The output minus the top bar.
    pub fn work_area(&self) -> Geometry {
        let bar = self.profile.top_bar.min(self.output.size.h - 1);
        rect(
            self.output.loc.x,
            self.output.loc.y + bar,
            self.output.size.w,
            self.output.size.h - bar,
        )
    }

    /// The underlying mcsapi policy (workspaces, focus, layout).
    pub fn desktop(&self) -> &Desktop {
        &self.desktop
    }

    /// Whether the overview is showing.
    pub fn overview_visible(&self) -> bool {
        self.overview
    }

    /// The pending Snap Assist offer, if any.
    pub fn snap_assist(&self) -> Option<&SnapAssist> {
        self.snap_assist.as_ref()
    }

    /// The active drag's drop target, for drawing a preview.
    pub fn drag_target(&self) -> Option<DropTarget> {
        self.drag.filter(|d| d.started).map(|d| d.target)
    }

    /// A window's app ID and title.
    pub fn window_label(&self, window: WindowId) -> Option<(&str, &str)> {
        self.windows
            .get(&window)
            .map(|i| (i.app_id.as_str(), i.title.as_str()))
    }

    /// Whether a window is minimized.
    pub fn is_minimized(&self, window: WindowId) -> bool {
        self.windows.get(&window).is_some_and(|i| i.minimized)
    }

    /// A window's placement mode.
    pub fn mode(&self, window: WindowId) -> Option<Mode> {
        self.windows.get(&window).map(|i| i.mode)
    }

    /// The workspace containing a window.
    pub fn workspace_of(&self, window: WindowId) -> Option<WorkspaceId> {
        self.desktop
            .workspaces()
            .find(|ws| ws.windows().any(|w| w == window))
            .map(|ws| ws.id())
    }

    /// The focused, visible window on the active workspace.
    pub fn focused(&self) -> Option<WindowId> {
        self.desktop
            .active()
            .focused()
            .filter(|w| !self.is_minimized(*w))
    }

    /// Starts managing a new toplevel on the active workspace and focuses it.
    ///
    /// Actions an agent queued after launching `app_id` are applied now.
    pub fn map_window(&mut self, app_id: &str, title: &str) -> (WindowId, Vec<Effect>) {
        let id = WindowId::new(self.next_id).expect("IDs start at 1");
        self.next_id += 1;
        self.desktop.insert(id).expect("fresh IDs are unique");
        self.windows.insert(
            id,
            WindowInfo {
                app_id: app_id.to_owned(),
                title: title.to_owned(),
                mode: Mode::Tiled,
                before_maximize: None,
                restore: None,
                minimized: false,
            },
        );
        self.stack.push(id);

        let key = app_id.to_lowercase();
        let mut effects = Vec::new();
        if let Some(i) = self
            .pending
            .iter()
            .position(|(app, _)| *app == key || key.contains(app.as_str()))
        {
            let (_, actions) = self.pending.remove(i);
            for action in actions {
                // Queued actions are best-effort: the window may already be gone.
                if let Ok(more) = self.apply(action) {
                    effects.extend(more);
                }
            }
        }
        (id, effects)
    }

    /// Stops managing a destroyed toplevel.
    pub fn unmap_window(&mut self, window: WindowId) -> Result<(), Error> {
        self.desktop.remove(window)?;
        self.windows.remove(&window);
        self.stack.retain(|w| *w != window);
        self.menus.unregister(window.get());
        if self.drag.is_some_and(|d| d.window == window) {
            self.drag = None;
        }
        if let Some(assist) = &mut self.snap_assist {
            assist.candidates.retain(|w| *w != window);
            if assist.candidates.is_empty() {
                self.snap_assist = None;
            }
        }
        Ok(())
    }

    /// Updates a window's title.
    pub fn set_title(&mut self, window: WindowId, title: &str) -> Result<(), Error> {
        self.info_mut(window)?.title = title.to_owned();
        Ok(())
    }

    fn info_mut(&mut self, window: WindowId) -> Result<&mut WindowInfo, Error> {
        self.windows
            .get_mut(&window)
            .ok_or(Error::UnknownWindow(window.get()))
    }

    fn target(&self, window: Option<u64>) -> Result<WindowId, Error> {
        match window {
            Some(raw) => WindowId::new(raw)
                .filter(|id| self.windows.contains_key(id))
                .ok_or(Error::UnknownWindow(raw)),
            None => self.focused().ok_or(Error::NoFocusedWindow),
        }
    }

    fn raise(&mut self, window: WindowId) {
        self.stack.retain(|w| *w != window);
        self.stack.push(window);
    }

    fn focus(&mut self, window: WindowId) -> Result<(), Error> {
        let ws = self
            .workspace_of(window)
            .ok_or(Error::UnknownWindow(window.get()))?;
        self.desktop.switch_to(ws)?;
        self.desktop.focus(window)?;
        self.info_mut(window)?.minimized = false;
        self.raise(window);
        Ok(())
    }

    fn focus_visible(&mut self, forward: bool) {
        let count = self.desktop.active().windows().len();
        for _ in 0..count {
            let next = if forward {
                self.desktop.focus_next()
            } else {
                self.desktop.focus_previous()
            };
            if let Some(w) = next.filter(|w| !self.is_minimized(*w)) {
                self.raise(w);
                return;
            }
        }
    }

    fn default_float(&self) -> Geometry {
        let area = self.work_area();
        centered(area, area.size.w * 3 / 5, area.size.h * 7 / 10)
    }

    fn frame_of(&self, window: WindowId) -> Option<Geometry> {
        self.placements()
            .into_iter()
            .find(|p| p.window == window)
            .map(|p| p.frame)
    }

    fn set_mode(&mut self, window: WindowId, mode: Mode) -> Result<(), Error> {
        let info = self.info_mut(window)?;
        if let Mode::Floating { frame } = info.mode {
            info.restore = Some(frame.into());
        }
        if mode
            != (Mode::Snapped {
                zone: SnapZone::Maximize,
            })
        {
            info.before_maximize = None;
        }
        info.mode = mode;
        Ok(())
    }

    fn snap(&mut self, window: WindowId, zone: SnapZone) -> Result<(), Error> {
        if zone == SnapZone::Maximize {
            let info = self.info_mut(window)?;
            if info.mode != (Mode::Snapped { zone }) {
                info.before_maximize = Some(info.mode);
            }
        }
        self.set_mode(window, Mode::Snapped { zone })?;
        self.focus(window)?;
        self.offer_snap_assist(window, zone);
        Ok(())
    }

    fn offer_snap_assist(&mut self, snapped: WindowId, zone: SnapZone) {
        let assist = self
            .snap_assist
            .take()
            .filter(|a| a.zone == zone && a.candidates.contains(&snapped));
        if assist.is_some() {
            // The offer was just accepted.
            return;
        }
        self.snap_assist = zone.complement().and_then(|zone| {
            let candidates: Vec<WindowId> = self
                .desktop
                .active()
                .windows()
                .filter(|w| *w != snapped && !self.is_minimized(*w))
                .filter(|w| self.mode(*w) != Some(Mode::Snapped { zone }))
                .collect();
            (!candidates.is_empty()).then(|| SnapAssist {
                zone,
                frame: zone.geometry(self.work_area(), self.profile.gap),
                candidates,
            })
        });
    }

    /// Dismisses the Snap Assist offer.
    pub fn dismiss_snap_assist(&mut self) {
        self.snap_assist = None;
    }

    fn float(&mut self, window: WindowId) -> Result<(), Error> {
        let frame = self.windows[&window]
            .restore
            .unwrap_or_else(|| self.default_float());
        self.set_mode(
            window,
            Mode::Floating {
                frame: frame.into(),
            },
        )
    }

    fn check_failed_unit(&self, unit: &str) -> Result<(), Error> {
        if self.failed_units.iter().any(|u| u == unit) {
            Ok(())
        } else {
            Err(Error::UnknownUnit(unit.to_owned()))
        }
    }

    /// Applies one action, returning work for the host.
    pub fn apply(&mut self, action: Action) -> Result<Vec<Effect>, Error> {
        if !matches!(action, Action::Snap { .. }) {
            self.snap_assist = None;
        }
        match action {
            Action::Launch { app } => {
                if !systemd::is_launchable(&app) {
                    return Err(Error::NotLaunchable(app));
                }
                self.habits.record(&app, self.clock.hour);
                return Ok(vec![Effect::Launch { app }]);
            }
            Action::Close { window } => {
                let window = self.target(window)?.get();
                return Ok(vec![Effect::Close { window }]);
            }
            Action::Focus { window } => {
                let w = self.target(Some(window))?;
                self.focus(w)?;
            }
            Action::FocusNext => self.focus_visible(true),
            Action::FocusPrevious => self.focus_visible(false),
            Action::Promote => {
                let w = self.target(None)?;
                self.set_mode(w, Mode::Tiled)?;
                self.desktop.promote_focused();
            }
            Action::Snap { window, zone } => {
                let w = self.target(window)?;
                self.snap(w, zone)?;
            }
            Action::Nudge { window, direction } => {
                let w = self.target(window)?;
                let current = match self.windows[&w].mode {
                    Mode::Snapped { zone } => Some(zone),
                    _ => None,
                };
                match SnapZone::nudge(current, direction) {
                    Nudge::Snap(zone) => self.snap(w, zone)?,
                    Nudge::Restore => {
                        let back = self.windows[&w].before_maximize.unwrap_or(Mode::Tiled);
                        match back {
                            Mode::Floating { .. } => self.float(w)?,
                            other => self.set_mode(w, other)?,
                        }
                    }
                    Nudge::Minimize => {
                        return self.apply(Action::Minimize {
                            window: Some(w.get()),
                        });
                    }
                }
            }
            Action::Tile { window } => {
                let w = self.target(window)?;
                self.set_mode(w, Mode::Tiled)?;
            }
            Action::Float { window } => {
                let w = self.target(window)?;
                self.float(w)?;
            }
            Action::ToggleMaximize { window } => {
                let w = self.target(window)?;
                let info = &self.windows[&w];
                if info.mode
                    == (Mode::Snapped {
                        zone: SnapZone::Maximize,
                    })
                {
                    match info.before_maximize.unwrap_or(Mode::Tiled) {
                        Mode::Floating { .. } => self.float(w)?,
                        other => self.set_mode(w, other)?,
                    }
                } else {
                    self.snap(w, SnapZone::Maximize)?;
                    self.snap_assist = None;
                }
            }
            Action::Minimize { window } => {
                let w = self.target(window)?;
                self.info_mut(w)?.minimized = true;
                if self.desktop.active().focused() == Some(w) {
                    self.focus_visible(true);
                }
            }
            Action::Restore { window } => {
                let w = self.target(Some(window))?;
                self.focus(w)?;
            }
            Action::SwitchWorkspace { workspace: n } => {
                self.desktop.switch_to(workspace(n)?).map_err(|e| match e {
                    mcsapi::Error::UnknownWorkspace(_) => Error::UnknownWorkspace(n),
                    other => other.into(),
                })?;
            }
            Action::MoveToWorkspace {
                window,
                workspace: n,
            } => {
                let w = self.target(window)?;
                self.desktop
                    .move_window(w, workspace(n)?)
                    .map_err(|e| match e {
                        mcsapi::Error::UnknownWorkspace(_) => Error::UnknownWorkspace(n),
                        other => other.into(),
                    })?;
            }
            Action::SetLayout { layout } => self.desktop.set_layout(match layout {
                LayoutKind::Tall => Layout::Tall,
                LayoutKind::Monocle => Layout::Monocle,
            }),
            Action::Overview { visible } => {
                self.overview = visible.unwrap_or(!self.overview);
                self.drag = None;
            }
            Action::ActivateMenu { window, item } => {
                let w = self.target(window)?;
                if let Some(action) = menu::shell_action(&item, w.get()) {
                    return self.apply(action);
                }
                return Ok(vec![Effect::MenuActivated {
                    window: w.get(),
                    item,
                }]);
            }
            Action::Session { op, confirmed } => {
                if op.is_destructive() && !confirmed {
                    return Err(Error::NeedsConfirmation(op));
                }
                return Ok(vec![Effect::Session { op }]);
            }
            Action::ActivateTray { id, item } => {
                if !self.tray.items().any(|i| i.id == id) {
                    return Err(Error::UnknownTrayItem(id));
                }
                return Ok(vec![Effect::TrayActivated { id, item }]);
            }
            Action::RestartUnit { unit } => {
                self.check_failed_unit(&unit)?;
                return Ok(vec![Effect::RestartUnit { unit }]);
            }
            Action::ResetFailed { unit } => {
                self.check_failed_unit(&unit)?;
                self.failed_units.retain(|u| *u != unit);
                return Ok(vec![Effect::ResetFailed { unit }]);
            }
        }
        Ok(Vec::new())
    }

    /// Applies a sequence of actions.
    ///
    /// Window actions without an explicit window that follow a `Launch` are
    /// deferred until that app maps a window, so "open firefox and snap it
    /// left" acts on Firefox. Stops at the first error; earlier actions stay
    /// applied.
    pub fn run(&mut self, actions: impl IntoIterator<Item = Action>) -> Result<Vec<Effect>, Error> {
        let mut effects = Vec::new();
        let mut actions = actions.into_iter().peekable();
        while let Some(action) = actions.next() {
            if let Action::Launch { app } = &action {
                let key = app.to_lowercase();
                effects.extend(self.apply(action)?);
                let mut deferred = Vec::new();
                while let Some(next) = actions.next_if(Action::targets_new_window) {
                    deferred.push(next);
                }
                if !deferred.is_empty() {
                    self.pending.push((key, deferred));
                }
            } else {
                effects.extend(self.apply(action)?);
            }
        }
        Ok(effects)
    }

    /// Frames for the active workspace, bottom to top.
    ///
    /// Tiled windows come first (only the topmost in monocle), then snapped and
    /// floating windows in stacking order. Minimized windows are omitted.
    pub fn placements(&self) -> Vec<WindowPlacement> {
        let ws = self.desktop.active();
        let members: BTreeSet<WindowId> = ws.windows().collect();
        let focused = self.focused();
        let area = self.work_area();
        let gap = self.profile.gap;
        let title = self.profile.title_bar;
        let visible = |w: &WindowId| !self.is_minimized(*w);
        let place = |window: WindowId, frame: Geometry| WindowPlacement {
            window,
            frame,
            client: title.client(frame),
            mode: self.windows[&window].mode,
            focused: focused == Some(window),
        };

        let tiled: Vec<WindowId> = ws
            .windows()
            .filter(visible)
            .filter(|w| self.windows[w].mode == Mode::Tiled)
            .collect();
        let topmost_tiled = self.stack.iter().rev().find(|w| tiled.contains(w)).copied();
        let tile_area = inset(area, gap / 2);
        let layout = ws.layout();
        let mut out: Vec<WindowPlacement> = match layout.arrange(tile_area, tiled.iter().copied()) {
            Ok(placements) if layout != Layout::Monocle => placements
                .map(|p| place(p.window, inset(p.geometry, gap - gap / 2)))
                .collect(),
            // Monocle, or too little space to tile: show the topmost tiled window.
            _ => topmost_tiled
                .map(|w| place(w, inset(tile_area, gap - gap / 2)))
                .into_iter()
                .collect(),
        };

        for &w in &self.stack {
            if !members.contains(&w) || !visible(&w) {
                continue;
            }
            match self.windows[&w].mode {
                Mode::Tiled => {}
                Mode::Floating { frame } => out.push(place(w, frame.into())),
                Mode::Snapped { zone } => out.push(place(w, zone.geometry(area, gap))),
            }
        }
        out
    }

    /// The topmost window frame under `point`.
    pub fn window_at(&self, point: Point) -> Option<WindowPlacement> {
        self.placements()
            .into_iter()
            .rev()
            .find(|p| contains(p.frame, point))
    }

    /// Handles a primary-button press at `point` (logical, output-relative).
    ///
    /// Focuses the window, runs title bar buttons, toggles maximize on double
    /// click, and arms a drag on the title area.
    pub fn pointer_down(&mut self, point: Point, time_ms: u64) -> Result<PointerOutcome, Error> {
        let Some(hit) = self.window_at(point) else {
            self.snap_assist = None;
            return Ok(PointerOutcome::Desktop);
        };
        let window = hit.window;
        self.focus(window)?;
        match self.profile.title_bar.hit(hit.frame, point) {
            Some(Hit::Button(button)) => {
                let window = Some(window.get());
                let action = match button {
                    Button::Close => Action::Close { window },
                    Button::Minimize => Action::Minimize { window },
                    Button::Maximize => Action::ToggleMaximize { window },
                };
                Ok(PointerOutcome::Handled {
                    effects: self.apply(action)?,
                })
            }
            Some(Hit::Title) => {
                if self.clicks.click(window.get(), time_ms) {
                    self.drag = None;
                    let effects = self.apply(Action::ToggleMaximize {
                        window: Some(window.get()),
                    })?;
                    return Ok(PointerOutcome::Handled { effects });
                }
                self.drag = Some(Drag {
                    window,
                    origin: point,
                    grab: (point.0 - hit.frame.loc.x, point.1 - hit.frame.loc.y),
                    started: false,
                    target: DropTarget::Float,
                });
                Ok(PointerOutcome::Handled {
                    effects: Vec::new(),
                })
            }
            Some(Hit::Client) | None => Ok(PointerOutcome::Client {
                window,
                local: (point.0 - hit.client.loc.x, point.1 - hit.client.loc.y),
            }),
        }
    }

    /// Moves an armed or active drag. Returns the drop target for previewing.
    ///
    /// Like Windows, a snapped, maximized or tiled window detaches into its
    /// restore size on the first real movement, keeping the pointer at the same
    /// relative position along the title bar.
    pub fn pointer_motion(&mut self, point: Point) -> Option<DropTarget> {
        let mut drag = self.drag?;
        if !drag.started {
            let (dx, dy) = (point.0 - drag.origin.0, point.1 - drag.origin.1);
            if dx.abs().max(dy.abs()) < DRAG_THRESHOLD {
                return None;
            }
            drag.started = true;
            self.snap_assist = None;
            let frame = self.frame_of(drag.window)?;
            let info = &self.windows[&drag.window];
            if !matches!(info.mode, Mode::Floating { .. }) {
                let restore = info.restore.unwrap_or_else(|| self.default_float());
                let ratio = f64::from(drag.grab.0) / f64::from(frame.size.w.max(1));
                let gx = (ratio * f64::from(restore.size.w)) as i32;
                let gy = drag.grab.1.min(self.profile.title_bar.height / 2);
                drag.grab = (gx, gy);
            }
        }
        let size = match self.windows[&drag.window].mode {
            Mode::Floating { frame } => (frame.w, frame.h),
            _ => {
                let restore = self.windows[&drag.window]
                    .restore
                    .unwrap_or_else(|| self.default_float());
                (restore.size.w, restore.size.h)
            }
        };
        let frame = rect(point.0 - drag.grab.0, point.1 - drag.grab.1, size.0, size.1);
        let info = self.windows.get_mut(&drag.window)?;
        info.mode = Mode::Floating {
            frame: frame.into(),
        };
        info.before_maximize = None;
        self.raise(drag.window);

        let area = self.work_area();
        drag.target =
            if let Some(zone) = zone_at(point, area, self.profile.snap) {
                DropTarget::Snap {
                    zone,
                    preview: zone.geometry(area, self.profile.gap),
                }
            } else if let Some(under) = self.placements().into_iter().rev().find(|p| {
                p.window != drag.window && p.mode == Mode::Tiled && contains(p.frame, point)
            }) {
                DropTarget::Tile {
                    onto: under.window,
                    preview: under.frame,
                }
            } else {
                DropTarget::Float
            };
        self.drag = Some(drag);
        Some(drag.target)
    }

    /// Ends a drag, applying its drop target. Returns it if a drag happened.
    pub fn pointer_up(&mut self) -> Option<DropTarget> {
        let drag = self.drag.take().filter(|d| d.started)?;
        let window = drag.window;
        if let Mode::Floating { frame } = self.windows.get(&window)?.mode {
            self.windows.get_mut(&window)?.restore = Some(frame.into());
        }
        match drag.target {
            DropTarget::Float => {}
            DropTarget::Snap { zone, .. } => {
                self.snap(window, zone).ok()?;
            }
            DropTarget::Tile { onto, .. } => {
                let main = self.desktop.active().windows().next();
                self.windows.get_mut(&window)?.mode = Mode::Tiled;
                if main == Some(onto) {
                    self.desktop.focus(window).ok()?;
                    self.desktop.promote_focused();
                }
            }
        }
        Some(drag.target)
    }

    /// Windows on a workspace, in tiling order.
    pub fn windows_on(&self, workspace: WorkspaceId) -> Vec<WindowId> {
        self.desktop
            .workspaces()
            .find(|ws| ws.id() == workspace)
            .map(|ws| ws.windows().collect())
            .unwrap_or_default()
    }

    /// Whether the device is a phone-sized screen.
    pub fn is_phone(&self) -> bool {
        self.profile.form_factor == FormFactor::Phone
    }
}
