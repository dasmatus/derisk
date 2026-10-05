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
//!
//! The single-chord shortcuts (palette, overview, focus, promote, close,
//! float, tile, layouts) can be rebound in the Settings app; see
//! [`binding_with`]. The rest are fixed.

use derisk_settings::{Chord, KeyName, Shortcut, Shortcuts};

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

/// The action bound to a chord with the default shortcuts, if any.
pub fn binding(mods: Mods, key: Key) -> Option<Action> {
    binding_with(&Shortcuts::default(), mods, key)
}

/// The action bound to a chord, with the rebindable shortcuts taken from
/// `shortcuts` (the Settings app's Shortcuts page). A rebound shortcut
/// leaves its old chord free.
pub fn binding_with(shortcuts: &Shortcuts, mods: Mods, key: Key) -> Option<Action> {
    if mods.alt && !mods.logo && !mods.ctrl && key == Key::Tab {
        return Some(if mods.shift {
            Action::FocusPrevious
        } else {
            Action::FocusNext
        });
    }
    if mods.logo && !mods.ctrl && !mods.alt {
        match (mods.shift, key) {
            (false, Key::Arrow(direction)) => {
                return Some(Action::Nudge {
                    window: None,
                    direction,
                });
            }
            (false, Key::Digit(d @ 1..=9)) => {
                return Some(Action::SwitchWorkspace {
                    workspace: u64::from(d),
                });
            }
            (true, Key::Digit(d @ 1..=9)) => {
                return Some(Action::MoveToWorkspace {
                    window: None,
                    workspace: u64::from(d),
                });
            }
            (false, Key::Tab) => return Some(Action::FocusNext),
            (true, Key::Tab) => return Some(Action::FocusPrevious),
            _ => {}
        }
    }
    let chord = Chord {
        logo: mods.logo,
        shift: mods.shift,
        ctrl: mods.ctrl,
        alt: mods.alt,
        key: match key {
            Key::Arrow(Direction::Left) => KeyName::Left,
            Key::Arrow(Direction::Right) => KeyName::Right,
            Key::Arrow(Direction::Up) => KeyName::Up,
            Key::Arrow(Direction::Down) => KeyName::Down,
            Key::Digit(d) => KeyName::Digit(d),
            Key::Letter(c) => KeyName::Letter(c),
            Key::Enter => KeyName::Enter,
            Key::Tab => KeyName::Tab,
            Key::Escape => KeyName::Escape,
            Key::Space => KeyName::Space,
        },
    };
    Some(match shortcuts.lookup(chord)? {
        Shortcut::Palette => Action::Palette { visible: None },
        Shortcut::Overview => Action::Overview { visible: None },
        Shortcut::FocusNext => Action::FocusNext,
        Shortcut::FocusPrevious => Action::FocusPrevious,
        Shortcut::Promote => Action::Promote,
        Shortcut::Close => Action::Close { window: None },
        Shortcut::Float => Action::Float { window: None },
        Shortcut::Tile => Action::Tile { window: None },
        Shortcut::Monocle => Action::SetLayout {
            layout: LayoutKind::Monocle,
        },
        Shortcut::Tall => Action::SetLayout {
            layout: LayoutKind::Tall,
        },
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
