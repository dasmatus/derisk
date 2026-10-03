//! The single command vocabulary shared by keybindings, menus, the overview,
//! the built-in assistant, and external agents.

use serde::{Deserialize, Serialize};

use crate::snap::{Direction, SnapZone};

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
    /// Choose a global-menu item.
    ActivateMenu {
        /// Window owning the menu.
        #[serde(default)]
        window: Option<u64>,
        /// Item ID.
        item: String,
    },
}

impl Action {
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
    /// Send the client a close request (e.g. `xdg_toplevel.close`).
    Close {
        /// Window.
        window: u64,
    },
    /// Forward a global-menu activation to the app (e.g. dbusmenu `Event`).
    MenuActivated {
        /// Window.
        window: u64,
        /// Item ID.
        item: String,
    },
}
