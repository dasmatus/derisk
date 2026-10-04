//! Keyboard shortcuts, as a pure mapping from a key chord to an [`Action`].
//!
//! The compositor host translates keysyms into [`Key`] and asks
//! [`binding`] before forwarding a key to the focused client. Keys that map
//! to an action are consumed by the shell.
//!
//! | Chord | Action |
//! | --- | --- |
//! | Super+Space | Toggle the command palette |
//! | Super (tap), Super+A | Toggle the overview |
//! | Super+←/→/↑/↓ | Snap, maximize, restore or minimize (Windows-style) |
//! | Super+1…9 | Switch workspace |
//! | Super+Shift+1…9 | Move the focused window to a workspace (one past the last opens a new one) |
//! | Super+J / Super+K, Alt+Tab | Focus next / previous |
//! | Super+Enter | Promote to the main pane |
//! | Super+Q | Close |
//! | Super+F / Super+T | Float / tile |
//! | Super+M / Super+Shift+M | Monocle / tall layout |

use crate::{
    action::{Action, LayoutKind},
    snap::Direction,
};

/// Modifier state for a chord.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct Mods {
    /// Super (logo) key.
    pub logo: bool,
    /// Shift.
    pub shift: bool,
    /// Control.
    pub ctrl: bool,
    /// Alt.
    pub alt: bool,
}

/// A key, independent of keyboard layout.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Key {
    /// An arrow key.
    Arrow(Direction),
    /// A digit 0–9 (top row or keypad).
    Digit(u8),
    /// A letter, lowercase.
    Letter(char),
    /// Enter or Return.
    Enter,
    /// Tab.
    Tab,
    /// Escape.
    Escape,
    /// The space bar.
    Space,
}

/// The action bound to a chord, if any.
pub fn binding(mods: Mods, key: Key) -> Option<Action> {
    if mods.alt && !mods.logo && !mods.ctrl && key == Key::Tab {
        return Some(if mods.shift {
            Action::FocusPrevious
        } else {
            Action::FocusNext
        });
    }
    if !mods.logo || mods.ctrl || mods.alt {
        return None;
    }
    Some(match (mods.shift, key) {
        (false, Key::Arrow(direction)) => Action::Nudge {
            window: None,
            direction,
        },
        (false, Key::Digit(d @ 1..=9)) => Action::SwitchWorkspace {
            workspace: u64::from(d),
        },
        (true, Key::Digit(d @ 1..=9)) => Action::MoveToWorkspace {
            window: None,
            workspace: u64::from(d),
        },
        (false, Key::Letter('j')) | (false, Key::Tab) => Action::FocusNext,
        (false, Key::Letter('k')) | (true, Key::Tab) => Action::FocusPrevious,
        (false, Key::Enter) => Action::Promote,
        (false, Key::Letter('q')) => Action::Close { window: None },
        (false, Key::Letter('f')) => Action::Float { window: None },
        (false, Key::Letter('t')) => Action::Tile { window: None },
        (false, Key::Letter('m')) => Action::SetLayout {
            layout: LayoutKind::Monocle,
        },
        (true, Key::Letter('m')) => Action::SetLayout {
            layout: LayoutKind::Tall,
        },
        (false, Key::Letter('a')) => Action::Overview { visible: None },
        (false, Key::Space) => Action::Palette { visible: None },
        _ => return None,
    })
}

/// Detects a bare tap of the Super key (press and release with nothing in
/// between), which toggles the overview like GNOME and Windows.
#[derive(Clone, Copy, Debug, Default)]
pub struct SuperTap {
    armed: bool,
}

impl SuperTap {
    /// Feeds a key event; `is_super` says whether it is a Super key.
    /// Returns `true` when a release completes a tap.
    pub fn key(&mut self, is_super: bool, pressed: bool) -> bool {
        match (is_super, pressed) {
            (true, true) => {
                self.armed = true;
                false
            }
            (true, false) => std::mem::take(&mut self.armed),
            (false, _) => {
                self.armed = false;
                false
            }
        }
    }

    /// Cancels a pending tap (for example on a pointer click while held).
    pub fn cancel(&mut self) {
        self.armed = false;
    }
}
