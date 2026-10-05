//! Rebindable keyboard shortcuts, written `shortcut.<id> = Super+Shift+M`.
//!
//! Only the single-chord shortcuts are rebindable. The families (Super+1…9,
//! Super+Shift+1…9, Super+arrows, Alt+Tab) stay fixed: they are one rule
//! over ten keys each, and a per-key table for them would be noise.

use std::fmt;

/// A key, independent of keyboard layout.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
pub enum KeyName {
    /// A letter, lowercase.
    Letter(char),
    /// A digit 0–9.
    Digit(u8),
    /// Enter or Return.
    Enter,
    /// Tab.
    Tab,
    /// Escape.
    Escape,
    /// The space bar.
    Space,
    /// Left arrow.
    Left,
    /// Right arrow.
    Right,
    /// Up arrow.
    Up,
    /// Down arrow.
    Down,
}

/// A key with modifiers, such as `Super+Shift+M`.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
pub struct Chord {
    /// Super (logo) key.
    pub logo: bool,
    /// Shift.
    pub shift: bool,
    /// Control.
    pub ctrl: bool,
    /// Alt.
    pub alt: bool,
    /// The key.
    pub key: KeyName,
}

impl Chord {
    /// Parses `Super+Shift+M`: modifiers in any order and case, then one key.
    /// A chord needs at least one modifier, so a plain letter can never be
    /// taken away from the focused app.
    pub fn parse(text: &str) -> Option<Self> {
        let mut chord = Self {
            logo: false,
            shift: false,
            ctrl: false,
            alt: false,
            key: KeyName::Space,
        };
        let mut parts: Vec<&str> = text.split('+').map(str::trim).collect();
        let key = parts.pop()?;
        for part in parts {
            let flag = match part.to_ascii_lowercase().as_str() {
                "super" | "logo" | "meta" | "win" => &mut chord.logo,
                "shift" => &mut chord.shift,
                "ctrl" | "control" => &mut chord.ctrl,
                "alt" => &mut chord.alt,
                _ => return None,
            };
            if std::mem::replace(flag, true) {
                return None;
            }
        }
        chord.key = match key.to_ascii_lowercase().as_str() {
            "enter" | "return" => KeyName::Enter,
            "tab" => KeyName::Tab,
            "escape" | "esc" => KeyName::Escape,
            "space" => KeyName::Space,
            "left" => KeyName::Left,
            "right" => KeyName::Right,
            "up" => KeyName::Up,
            "down" => KeyName::Down,
            k => {
                let mut chars = k.chars();
                match (chars.next(), chars.next()) {
                    (Some(c @ 'a'..='z'), None) => KeyName::Letter(c),
                    (Some(c @ '0'..='9'), None) => KeyName::Digit(c as u8 - b'0'),
                    _ => return None,
                }
            }
        };
        (chord.logo || chord.ctrl || chord.alt).then_some(chord)
    }
}

impl fmt::Display for Chord {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for (on, name) in [
            (self.logo, "Super+"),
            (self.ctrl, "Ctrl+"),
            (self.alt, "Alt+"),
            (self.shift, "Shift+"),
        ] {
            if on {
                f.write_str(name)?;
            }
        }
        match self.key {
            KeyName::Letter(c) => write!(f, "{}", c.to_ascii_uppercase()),
            KeyName::Digit(d) => write!(f, "{d}"),
            KeyName::Enter => f.write_str("Enter"),
            KeyName::Tab => f.write_str("Tab"),
            KeyName::Escape => f.write_str("Escape"),
            KeyName::Space => f.write_str("Space"),
            KeyName::Left => f.write_str("Left"),
            KeyName::Right => f.write_str("Right"),
            KeyName::Up => f.write_str("Up"),
            KeyName::Down => f.write_str("Down"),
        }
    }
}

/// A rebindable shortcut.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash, PartialOrd, Ord)]
pub enum Shortcut {
    /// Toggle the command palette.
    Palette,
    /// Toggle the overview.
    Overview,
    /// Focus the next window.
    FocusNext,
    /// Focus the previous window.
    FocusPrevious,
    /// Move the focused window to the main pane.
    Promote,
    /// Close the focused window.
    Close,
    /// Float the focused window.
    Float,
    /// Tile the focused window.
    Tile,
    /// Switch to the monocle layout.
    Monocle,
    /// Switch to the tall layout.
    Tall,
}

impl Shortcut {
    /// Every shortcut, in the Settings app's order.
    pub const ALL: [Self; 10] = [
        Self::Palette,
        Self::Overview,
        Self::FocusNext,
        Self::FocusPrevious,
        Self::Promote,
        Self::Close,
        Self::Float,
        Self::Tile,
        Self::Monocle,
        Self::Tall,
    ];

    /// The ID in `shortcut.<id>`.
    pub const fn id(self) -> &'static str {
        match self {
            Self::Palette => "palette",
            Self::Overview => "overview",
            Self::FocusNext => "focus_next",
            Self::FocusPrevious => "focus_previous",
            Self::Promote => "promote",
            Self::Close => "close",
            Self::Float => "float",
            Self::Tile => "tile",
            Self::Monocle => "monocle",
            Self::Tall => "tall",
        }
    }

    /// What it does, for the Settings app.
    pub const fn label(self) -> &'static str {
        match self {
            Self::Palette => "Command palette",
            Self::Overview => "Overview",
            Self::FocusNext => "Focus next window",
            Self::FocusPrevious => "Focus previous window",
            Self::Promote => "Move to main pane",
            Self::Close => "Close window",
            Self::Float => "Float window",
            Self::Tile => "Tile window",
            Self::Monocle => "Monocle layout",
            Self::Tall => "Tall layout",
        }
    }

    /// The chord it has until rebound.
    pub fn default_chord(self) -> Chord {
        let text = match self {
            Self::Palette => "Super+Space",
            Self::Overview => "Super+A",
            Self::FocusNext => "Super+J",
            Self::FocusPrevious => "Super+K",
            Self::Promote => "Super+Enter",
            Self::Close => "Super+Q",
            Self::Float => "Super+F",
            Self::Tile => "Super+T",
            Self::Monocle => "Super+M",
            Self::Tall => "Super+Shift+M",
        };
        Chord::parse(text).expect("default chords parse")
    }

    fn from_id(id: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|s| s.id() == id)
    }
}

/// The chord for each rebindable shortcut. `None` turns one off.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Shortcuts {
    chords: [Option<Chord>; Shortcut::ALL.len()],
}

impl Default for Shortcuts {
    fn default() -> Self {
        Self {
            chords: Shortcut::ALL.map(|s| Some(s.default_chord())),
        }
    }
}

impl Shortcuts {
    /// The chord bound to `shortcut`.
    pub fn get(&self, shortcut: Shortcut) -> Option<Chord> {
        self.chords[shortcut as usize]
    }

    /// Binds `shortcut` to `chord`, or turns it off with `None`.
    pub fn set(&mut self, shortcut: Shortcut, chord: Option<Chord>) {
        self.chords[shortcut as usize] = chord;
    }

    /// The shortcut a chord triggers. When two share a chord, the first in
    /// [`Shortcut::ALL`] wins; the Settings app flags the clash.
    pub fn lookup(&self, chord: Chord) -> Option<Shortcut> {
        Shortcut::ALL
            .into_iter()
            .find(|s| self.get(*s) == Some(chord))
    }

    /// Shortcuts that share their chord with another one.
    pub fn clashes(&self) -> Vec<Shortcut> {
        Shortcut::ALL
            .into_iter()
            .filter(|s| {
                self.get(*s).is_some_and(|c| {
                    Shortcut::ALL
                        .into_iter()
                        .any(|other| other != *s && self.get(other) == Some(c))
                })
            })
            .collect()
    }

    /// Sets `shortcut.<id>` from its text: a chord or `none`.
    pub(crate) fn set_text(&mut self, id: &str, value: &str) -> bool {
        let Some(shortcut) = Shortcut::from_id(id) else {
            return false;
        };
        if value.eq_ignore_ascii_case("none") {
            self.set(shortcut, None);
            return true;
        }
        Chord::parse(value)
            .map(|c| self.set(shortcut, Some(c)))
            .is_some()
    }

    /// `shortcut.<id> = <chord>` lines for every shortcut.
    pub(crate) fn to_text(self) -> String {
        Shortcut::ALL
            .into_iter()
            .map(|s| match self.get(s) {
                Some(chord) => format!("shortcut.{} = {chord}\n", s.id()),
                None => format!("shortcut.{} = none\n", s.id()),
            })
            .collect()
    }
}
