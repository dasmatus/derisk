//! The single command vocabulary shared by keybindings, menus, the overview,
//! the built-in assistant, and external agents.

use serde::{Deserialize, Serialize};

use crate::{
    snap::{Direction, SnapZone},
    systemd::SessionOp,
};

/// Window arrangement for a workspace's tiled windows.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LayoutKind {
    /// Main window left, others stacked right.
    Tall,
    /// One window at a time.
    Monocle,
}

/// Something the shell can do. `window: None` targets the focused window.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "action", rename_all = "snake_case")]
pub enum Action {
    /// Start an application (performed by the host).
    Launch {
        /// App ID or command name.
        app: String,
    },
    /// Run one of an app's desktop actions (performed by the host), such as
    /// Firefox's `new-private-window`.
    LaunchAction {
        /// Desktop file ID (`firefox.desktop`) or core app ID.
        app: String,
        /// Action ID from the app's `.desktop` file.
        id: String,
    },
    /// Ask a window to close (performed by the host).
    Close {
        /// Target window.
        #[serde(default)]
        window: Option<u64>,
    },
    /// Focus and raise a window, switching workspace if needed.
    Focus {
        /// Target window.
        window: u64,
    },
    /// Focus the next window on the active workspace.
    FocusNext,
    /// Focus the previous window on the active workspace.
    FocusPrevious,
    /// Make the focused window the main tile.
    Promote,
    /// Snap a window into a zone.
    Snap {
        /// Target window.
        #[serde(default)]
        window: Option<u64>,
        /// Zone.
        zone: SnapZone,
    },
    /// Windows-style Super+arrow.
    Nudge {
        /// Target window.
        #[serde(default)]
        window: Option<u64>,
        /// Arrow.
        direction: Direction,
    },
    /// Return a window to automatic tiling.
    Tile {
        /// Target window.
        #[serde(default)]
        window: Option<u64>,
    },
    /// Let a window float freely.
    Float {
        /// Target window.
        #[serde(default)]
        window: Option<u64>,
    },
    /// Maximize, or restore a maximized window.
    ToggleMaximize {
        /// Target window.
        #[serde(default)]
        window: Option<u64>,
    },
    /// Hide a window until restored.
    Minimize {
        /// Target window.
        #[serde(default)]
        window: Option<u64>,
    },
    /// Show a minimized window again and focus it.
    Restore {
        /// Target window.
        window: u64,
    },
    /// Activate a workspace.
    SwitchWorkspace {
        /// Workspace number (1-based).
        workspace: u64,
    },
    /// Send a window to a workspace.
    MoveToWorkspace {
        /// Target window.
        #[serde(default)]
        window: Option<u64>,
        /// Workspace number (1-based).
        workspace: u64,
    },
    /// Change the active workspace's layout.
    SetLayout {
        /// Layout.
        layout: LayoutKind,
    },
    /// Show, hide (`Some`) or toggle (`None`) the overview.
    Overview {
        /// Desired visibility.
        #[serde(default)]
        visible: Option<bool>,
    },
    /// Show, hide (`Some`) or toggle (`None`) the command palette.
    Palette {
        /// Desired visibility.
        #[serde(default)]
        visible: Option<bool>,
    },
    /// Show, hide (`Some`) or toggle (`None`) the on-screen keyboard.
    Keyboard {
        /// Desired visibility.
        #[serde(default)]
        visible: Option<bool>,
    },
    /// Open a file or folder with its default application.
    ///
    /// Only absolute paths to existing, non-executable files and folders are
    /// accepted, so this cannot be used to run programs.
    Open {
        /// Absolute path.
        path: String,
    },
    /// Search the web for `query` with the search engine chosen in
    /// Settings, in the default browser. The engine decides the URL and the
    /// query only fills in its query string, so this opens nothing but a
    /// results page.
    SearchWeb {
        /// What to search for.
        query: String,
    },
    /// Hand a request the built-in assistant can't follow to the agent set
    /// up in Sonne, with whichever model key or agent CLI the person chose
    /// there. It opens in Sonne's agent panel for the person to send, so
    /// nothing runs on an agent's say-so, and that agent drives the desktop
    /// through `derisk mcp`.
    AskAgent {
        /// The request, as typed.
        text: String,
    },
    /// Choose a global-menu item.
    ActivateMenu {
        /// Window owning the menu.
        #[serde(default)]
        window: Option<u64>,
        /// Item ID.
        item: String,
    },
    /// Lock, suspend, log out, reboot or power off through logind/systemd.
    Session {
        /// Operation.
        op: SessionOp,
        /// Destructive operations (log out, reboot, power off) are refused
        /// unless the user explicitly confirmed them.
        #[serde(default)]
        confirmed: bool,
    },
    /// Activate a tray item, or one of its menu entries.
    ActivateTray {
        /// Tray item ID.
        id: String,
        /// Menu item ID; `None` for a plain click.
        #[serde(default)]
        item: Option<String>,
    },
    /// Restart a failed user unit.
    RestartUnit {
        /// Unit name.
        unit: String,
    },
    /// Clear a user unit's failed state.
    ResetFailed {
        /// Unit name.
        unit: String,
    },
    /// Answer the on-screen confirmation of a session operation an agent
    /// or injected input asked for ([`crate::shell::Shell::pending_confirmation`]).
    /// Accepting only works from real input.
    Confirm {
        /// Go ahead, or cancel.
        accept: bool,
    },
}

impl Action {
    /// A short human description, for the palette's agent conversation.
    pub fn label(&self) -> String {
        let which = |window: &Option<u64>| match window {
            Some(w) => format!("window {w}"),
            None => "the window".to_owned(),
        };
        let zone = |z: SnapZone| match z {
            SnapZone::Left => "left",
            SnapZone::Right => "right",
            SnapZone::TopLeft => "top left",
            SnapZone::TopRight => "top right",
            SnapZone::BottomLeft => "bottom left",
            SnapZone::BottomRight => "bottom right",
            SnapZone::Maximize => "full screen",
        };
        match self {
            Self::Launch { app } => format!("Open {app}"),
            Self::LaunchAction { app, id } => format!("Run {app} action {id}"),
            Self::Close { window } => format!("Close {}", which(window)),
            Self::Focus { window } => format!("Focus window {window}"),
            Self::FocusNext => "Focus the next window".into(),
            Self::FocusPrevious => "Focus the previous window".into(),
            Self::Promote => "Make the window the main tile".into(),
            Self::Snap { window, zone: z } => format!("Snap {} {}", which(window), zone(*z)),
            Self::Nudge { window, direction } => {
                format!(
                    "Nudge {} {}",
                    which(window),
                    format!("{direction:?}").to_lowercase()
                )
            }
            Self::Tile { window } => format!("Tile {}", which(window)),
            Self::Float { window } => format!("Float {}", which(window)),
            Self::ToggleMaximize { window } => format!("Maximize or restore {}", which(window)),
            Self::Minimize { window } => format!("Minimize {}", which(window)),
            Self::Restore { window } => format!("Restore window {window}"),
            Self::SwitchWorkspace { workspace } => format!("Go to workspace {workspace}"),
            Self::MoveToWorkspace { window, workspace } => {
                format!("Move {} to workspace {workspace}", which(window))
            }
            Self::SetLayout { layout } => {
                format!("Use the {} layout", format!("{layout:?}").to_lowercase())
            }
            Self::Overview {
                visible: Some(true),
            } => "Show the overview".into(),
            Self::Overview {
                visible: Some(false),
            } => "Hide the overview".into(),
            Self::Overview { visible: None } => "Toggle the overview".into(),
            Self::Palette { .. } => "Toggle the command palette".into(),
            Self::Keyboard {
                visible: Some(true),
            } => "Show the on-screen keyboard".into(),
            Self::Keyboard {
                visible: Some(false),
            } => "Hide the on-screen keyboard".into(),
            Self::Keyboard { visible: None } => "Toggle the on-screen keyboard".into(),
            Self::Open { path } => format!("Open {path}"),
            Self::SearchWeb { query } => format!("Search the web for {query}"),
            Self::AskAgent { text } => format!("Ask Sonne's agent: {text}"),
            Self::ActivateMenu { item, .. } => format!("Choose menu item {item}"),
            Self::Session { op, .. } => format!("{op:?}"),
            Self::ActivateTray { id, .. } => format!("Activate tray item {id}"),
            Self::RestartUnit { unit } => format!("Restart {unit}"),
            Self::ResetFailed { unit } => format!("Dismiss {unit}"),
            Self::Confirm { accept: true } => "Confirm".into(),
            Self::Confirm { accept: false } => "Cancel".into(),
        }
    }

    /// Whether this action operates on "the focused window" implicitly, so
    /// after a `Launch` it should wait for the launched app's window.
    pub fn targets_new_window(&self) -> bool {
        matches!(
            self,
            Self::Close { window: None }
                | Self::Snap { window: None, .. }
                | Self::Nudge { window: None, .. }
                | Self::Tile { window: None }
                | Self::Float { window: None }
                | Self::ToggleMaximize { window: None }
                | Self::Minimize { window: None }
                | Self::MoveToWorkspace { window: None, .. }
                | Self::Promote
        )
    }
}

/// Work the host must perform after the shell handled an action.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "effect", rename_all = "snake_case")]
pub enum Effect {
    /// Spawn an application.
    Launch {
        /// App ID or command name.
        app: String,
    },
    /// Run an app's desktop action.
    LaunchAction {
        /// Desktop file ID or core app ID.
        app: String,
        /// Action ID.
        id: String,
    },
    /// Send the client a close request (e.g. `xdg_toplevel.close`).
    Close {
        /// Window.
        window: u64,
    },
    /// Open a file or folder with its default application (`xdg-open`), or
    /// a web search's results page in the default browser.
    Open {
        /// Absolute path, checked by the shell, or the https URL the chosen
        /// search engine builds.
        path: String,
    },
    /// Forward a global-menu activation to the app (e.g. dbusmenu `Event`).
    MenuActivated {
        /// Window.
        window: u64,
        /// Item ID.
        item: String,
    },
    /// Perform a session operation.
    Session {
        /// Operation.
        op: SessionOp,
    },
    /// Forward a tray activation to its StatusNotifierItem.
    TrayActivated {
        /// Tray item ID.
        id: String,
        /// Menu item ID, if a menu entry was chosen.
        item: Option<String>,
    },
    /// Restart a user unit.
    RestartUnit {
        /// Unit name.
        unit: String,
    },
    /// Clear a user unit's failed state.
    ResetFailed {
        /// Unit name.
        unit: String,
    },
}
