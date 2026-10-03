//! A global (top bar) menu registry, macOS/Unity style.
//!
//! Apps (or a `com.canonical.AppMenu.Registrar` / dbusmenu bridge in the host)
//! register menus per window. The top bar shows the focused window's menus,
//! followed by a shell-provided "Window" menu.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::{action::Action, snap::SnapZone};

/// One entry in a menu.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum MenuEntry {
    /// An activatable item. `id` is sent back to the app when chosen.
    Item {
        /// Identifier reported on activation.
        id: String,
        /// Visible text.
        label: String,
        /// Optional shortcut hint, e.g. `Ctrl+S`.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        shortcut: Option<String>,
        /// Disabled items are shown but cannot be chosen.
        #[serde(default = "enabled")]
        enabled: bool,
    },
    /// A nested menu.
    Submenu {
        /// Visible text.
        label: String,
        /// Child entries.
        entries: Vec<MenuEntry>,
    },
    /// A divider.
    Separator,
}

fn enabled() -> bool {
    true
}

impl MenuEntry {
    /// An enabled item without a shortcut.
    pub fn item(id: impl Into<String>, label: impl Into<String>) -> Self {
        Self::Item {
            id: id.into(),
            label: label.into(),
            shortcut: None,
            enabled: true,
        }
    }

    /// Adds a shortcut hint to an item; other entries are returned unchanged.
    pub fn with_shortcut(mut self, hint: impl Into<String>) -> Self {
        if let Self::Item { shortcut, .. } = &mut self {
            *shortcut = Some(hint.into());
        }
        self
    }
}

/// A top-level menu such as "File".
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct Menu {
    /// Title shown in the bar.
    pub title: String,
    /// Entries.
    pub entries: Vec<MenuEntry>,
}

/// Prefix of item IDs handled by the shell rather than the app.
pub const SHELL_PREFIX: &str = "derisk.";

/// The shell's own "Window" menu, appended after every app's menus.
pub fn window_menu() -> Menu {
    Menu {
        title: "Window".into(),
        entries: vec![
            MenuEntry::item("derisk.minimize", "Minimize").with_shortcut("Super+↓"),
            MenuEntry::item("derisk.maximize", "Maximize").with_shortcut("Super+↑"),
            MenuEntry::Submenu {
                label: "Snap".into(),
                entries: vec![
                    MenuEntry::item("derisk.snap.left", "Left Half").with_shortcut("Super+←"),
                    MenuEntry::item("derisk.snap.right", "Right Half").with_shortcut("Super+→"),
                    MenuEntry::item("derisk.snap.top_left", "Top Left"),
                    MenuEntry::item("derisk.snap.top_right", "Top Right"),
                    MenuEntry::item("derisk.snap.bottom_left", "Bottom Left"),
                    MenuEntry::item("derisk.snap.bottom_right", "Bottom Right"),
                ],
            },
            MenuEntry::item("derisk.tile", "Tile"),
            MenuEntry::item("derisk.float", "Float"),
            MenuEntry::item("derisk.promote", "Make Main Window"),
            MenuEntry::Separator,
            MenuEntry::item("derisk.overview", "Overview").with_shortcut("Super"),
            MenuEntry::item("derisk.close", "Close").with_shortcut("Alt+F4"),
        ],
    }
}

/// Maps a shell menu item ID to the action it performs on `window`.
pub fn shell_action(item: &str, window: u64) -> Option<Action> {
    let window = Some(window);
    let snap = |zone| Action::Snap { window, zone };
    Some(match item.strip_prefix(SHELL_PREFIX)? {
        "minimize" => Action::Minimize { window },
        "maximize" => Action::ToggleMaximize { window },
        "close" => Action::Close { window },
        "tile" => Action::Tile { window },
        "float" => Action::Float { window },
        "promote" => Action::Promote,
        "overview" => Action::Overview { visible: None },
        "snap.left" => snap(SnapZone::Left),
        "snap.right" => snap(SnapZone::Right),
        "snap.top_left" => snap(SnapZone::TopLeft),
        "snap.top_right" => snap(SnapZone::TopRight),
        "snap.bottom_left" => snap(SnapZone::BottomLeft),
        "snap.bottom_right" => snap(SnapZone::BottomRight),
        _ => return None,
    })
}

/// Menus registered per window.
#[derive(Clone, Debug, Default)]
pub struct GlobalMenu {
    menus: BTreeMap<u64, Vec<Menu>>,
}

impl GlobalMenu {
    /// Registers (or replaces) a window's menus.
    pub fn register(&mut self, window: u64, menus: Vec<Menu>) {
        self.menus.insert(window, menus);
    }

    /// Forgets a window's menus.
    pub fn unregister(&mut self, window: u64) {
        self.menus.remove(&window);
    }

    /// A window's own menus, if it registered any.
    pub fn app_menus(&self, window: u64) -> &[Menu] {
        self.menus.get(&window).map_or(&[], Vec::as_slice)
    }

    /// Everything the bar shows for `window`: app menus then the Window menu.
    pub fn bar(&self, window: Option<u64>) -> Vec<Menu> {
        let mut menus = window.map_or_else(Vec::new, |w| self.app_menus(w).to_vec());
        if window.is_some() {
            menus.push(window_menu());
        }
        menus
    }
}
