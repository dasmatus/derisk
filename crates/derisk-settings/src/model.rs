use std::{
    fmt, fs, io,
    path::{Path, PathBuf},
};

use mcsapi_ui::{Theme, egui::Color32};

/// Light or dark shell colors.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum ColorScheme {
    /// Dark surfaces with light text (the mcsapi default).
    #[default]
    Dark,
    /// Light surfaces with dark text.
    Light,
}

/// Highlight color for active controls and the focused workspace.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum Accent {
    /// Lime, the mcsapi default.
    #[default]
    Lime,
    /// Sky blue.
    Sky,
    /// Violet.
    Violet,
    /// Rose.
    Rose,
    /// Amber.
    Amber,
}

impl Accent {
    /// Every accent, in display order.
    pub const ALL: [Self; 5] = [Self::Lime, Self::Sky, Self::Violet, Self::Rose, Self::Amber];

    /// The accent's color.
    pub const fn color(self) -> Color32 {
        match self {
            Self::Lime => Color32::from_rgb(163, 230, 53),
            Self::Sky => Color32::from_rgb(56, 189, 248),
            Self::Violet => Color32::from_rgb(167, 139, 250),
            Self::Rose => Color32::from_rgb(251, 113, 133),
            Self::Amber => Color32::from_rgb(251, 191, 36),
        }
    }
}

/// Default tiling layout for new workspaces.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum Layout {
    /// Main pane plus a stack, like xmonad's Tall.
    #[default]
    Tall,
    /// One full-size window at a time.
    Monocle,
}

/// Which adaptive profile the shell uses.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum Profile {
    /// Pick from the output size.
    #[default]
    Automatic,
    /// Monocle, no gaps, large targets.
    Phone,
    /// Touch-friendly tiling.
    Tablet,
    /// Full desktop.
    Desktop,
}

/// When the shell saves power by dropping blur, animations and frame rate.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum LowPower {
    /// Never.
    Off,
    /// While running on battery.
    #[default]
    OnBattery,
    /// Always.
    On,
}

impl LowPower {
    /// Every mode, in display order.
    pub const ALL: [Self; 3] = [Self::Off, Self::OnBattery, Self::On];

    /// The mode's label in the Settings app.
    pub const fn label(self) -> &'static str {
        match self {
            Self::Off => "Off",
            Self::OnBattery => "On battery",
            Self::On => "Always",
        }
    }

    /// Whether low power mode applies, given whether the device runs on
    /// battery right now.
    pub const fn active(self, on_battery: bool) -> bool {
        match self {
            Self::Off => false,
            Self::OnBattery => on_battery,
            Self::On => true,
        }
    }
}

/// How opaque each shell panel is, in percent (20–100). Below 100 the
/// blurred desktop shows through.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PanelOpacity {
    /// The top bar.
    pub top_bar: u8,
    /// The overview backdrop.
    pub overview: u8,
    /// The Snap Assist picker.
    pub snap_assist: u8,
}

/// Appearance preferences.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Appearance {
    /// Light or dark colors.
    pub scheme: ColorScheme,
    /// Highlight color.
    pub accent: Accent,
    /// Interface text scale, from 0.75 to 2.0.
    pub text_scale: f32,
    /// Replace motion with cross-fades.
    pub reduce_motion: bool,
    /// Background blur under translucent panels, 0 (off) to 10.
    pub blur: u8,
    /// Panel opacity.
    pub panels: PanelOpacity,
}

/// Window management preferences.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct DesktopPrefs {
    /// Default layout.
    pub layout: Layout,
    /// Gap between tiled windows in logical pixels, up to 64.
    pub gaps: u8,
    /// Number of workspaces, from 1 to 9.
    pub workspaces: u8,
    /// Adaptive profile.
    pub profile: Profile,
}

/// Keyboard and pointer preferences.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Input {
    /// Content follows the fingers when scrolling.
    pub natural_scroll: bool,
    /// Tap on a touchpad to click.
    pub tap_to_click: bool,
    /// Delay before a held key repeats, 100 to 1000 ms.
    pub repeat_delay_ms: u16,
    /// Repeats per second, 1 to 60.
    pub repeat_rate: u8,
}

/// Notification preferences.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Notifications {
    /// Hide banners; they still go to the notification list.
    pub do_not_disturb: bool,
    /// Play a sound for new notifications.
    pub sounds: bool,
    /// Show message text on the lock screen.
    pub lock_screen_previews: bool,
}

/// Power preferences. Zero minutes means never.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Power {
    /// Minutes of inactivity before the screen dims.
    pub dim_after_min: u16,
    /// Minutes of inactivity before the session locks.
    pub lock_after_min: u16,
    /// Minutes of inactivity before suspending.
    pub suspend_after_min: u16,
    /// When low power mode turns on.
    pub low_power: LowPower,
}

/// All derisk preferences.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Settings {
    /// Appearance.
    pub appearance: Appearance,
    /// Window management.
    pub desktop: DesktopPrefs,
    /// Keyboard and pointer.
    pub input: Input,
    /// Notifications.
    pub notifications: Notifications,
    /// Power.
    pub power: Power,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            appearance: Appearance {
                scheme: ColorScheme::Dark,
                accent: Accent::Lime,
                text_scale: 1.0,
                reduce_motion: false,
                blur: 6,
                panels: PanelOpacity {
                    top_bar: 70,
                    overview: 80,
                    snap_assist: 85,
                },
            },
            desktop: DesktopPrefs {
                layout: Layout::Tall,
                gaps: 8,
                workspaces: 9,
                profile: Profile::Automatic,
            },
            input: Input {
                natural_scroll: true,
                tap_to_click: true,
                repeat_delay_ms: 400,
                repeat_rate: 25,
            },
            notifications: Notifications {
                do_not_disturb: false,
                sounds: true,
                lock_screen_previews: false,
            },
            power: Power {
                dim_after_min: 5,
                lock_after_min: 10,
                suspend_after_min: 30,
                low_power: LowPower::OnBattery,
            },
        }
    }
}

/// A line of a settings file that was ignored while loading.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Warning {
    /// 1-based line number.
    pub line: usize,
    /// What was wrong.
    pub message: String,
}

impl fmt::Display for Warning {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "line {}: {}", self.line, self.message)
    }
}

macro_rules! enum_text {
    ($ty:ty { $($variant:ident => $text:literal),+ $(,)? }) => {
        impl $ty {
            /// The value as written in the settings file.
            pub const fn as_str(self) -> &'static str {
                match self { $(Self::$variant => $text),+ }
            }

            fn parse(text: &str) -> Option<Self> {
                match text { $($text => Some(Self::$variant),)+ _ => None }
            }
        }
    };
}

enum_text!(ColorScheme { Dark => "dark", Light => "light" });
enum_text!(Accent { Lime => "lime", Sky => "sky", Violet => "violet", Rose => "rose", Amber => "amber" });
enum_text!(Layout { Tall => "tall", Monocle => "monocle" });
enum_text!(LowPower { Off => "off", OnBattery => "on_battery", On => "on" });
enum_text!(Profile { Automatic => "automatic", Phone => "phone", Tablet => "tablet", Desktop => "desktop" });

fn parse_bool(text: &str) -> Option<bool> {
    match text {
        "true" => Some(true),
        "false" => Some(false),
        _ => None,
    }
}

fn parse_in<T: std::str::FromStr + PartialOrd>(text: &str, min: T, max: T) -> Option<T> {
    text.parse().ok().filter(|v| *v >= min && *v <= max)
}

impl Settings {
    /// Parses `key = value` lines. `#` starts a comment.
    ///
    /// Unknown keys and invalid values are skipped with a warning and leave
    /// the default in place, so an old or hand-edited file never blocks login.
    pub fn parse(text: &str) -> (Self, Vec<Warning>) {
        let mut settings = Self::default();
        let mut warnings = Vec::new();
        for (index, raw) in text.lines().enumerate() {
            let line = raw.split('#').next().unwrap_or_default().trim();
            if line.is_empty() {
                continue;
            }
            let warn = |message: String| Warning {
                line: index + 1,
                message,
            };
            let Some((key, value)) = line.split_once('=') else {
                warnings.push(warn(format!("expected `key = value`, found `{line}`")));
                continue;
            };
            let (key, value) = (key.trim(), value.trim());
            if !settings.set(key, value) {
                warnings.push(warn(format!("ignored `{key} = {value}`")));
            }
        }
        (settings, warnings)
    }

    /// Sets one key from its text value. Returns `false` if the key is
    /// unknown or the value is invalid, leaving the setting unchanged.
    pub fn set(&mut self, key: &str, value: &str) -> bool {
        fn put<T>(slot: &mut T, value: Option<T>) -> bool {
            value.map(|v| *slot = v).is_some()
        }
        let (a, d, i, n, p) = (
            &mut self.appearance,
            &mut self.desktop,
            &mut self.input,
            &mut self.notifications,
            &mut self.power,
        );
        match key {
            "appearance.scheme" => put(&mut a.scheme, ColorScheme::parse(value)),
            "appearance.accent" => put(&mut a.accent, Accent::parse(value)),
            "appearance.text_scale" => put(&mut a.text_scale, parse_in(value, 0.75, 2.0)),
            "appearance.reduce_motion" => put(&mut a.reduce_motion, parse_bool(value)),
            "appearance.blur" => put(&mut a.blur, parse_in(value, 0, 10)),
            "appearance.top_bar_opacity" => put(&mut a.panels.top_bar, parse_in(value, 20, 100)),
            "appearance.overview_opacity" => put(&mut a.panels.overview, parse_in(value, 20, 100)),
            "appearance.snap_assist_opacity" => {
                put(&mut a.panels.snap_assist, parse_in(value, 20, 100))
            }
            "desktop.layout" => put(&mut d.layout, Layout::parse(value)),
            "desktop.gaps" => put(&mut d.gaps, parse_in(value, 0, 64)),
            "desktop.workspaces" => put(&mut d.workspaces, parse_in(value, 1, 9)),
            "desktop.profile" => put(&mut d.profile, Profile::parse(value)),
            "input.natural_scroll" => put(&mut i.natural_scroll, parse_bool(value)),
            "input.tap_to_click" => put(&mut i.tap_to_click, parse_bool(value)),
            "input.repeat_delay_ms" => put(&mut i.repeat_delay_ms, parse_in(value, 100, 1000)),
            "input.repeat_rate" => put(&mut i.repeat_rate, parse_in(value, 1, 60)),
            "notifications.do_not_disturb" => put(&mut n.do_not_disturb, parse_bool(value)),
            "notifications.sounds" => put(&mut n.sounds, parse_bool(value)),
            "notifications.lock_screen_previews" => {
                put(&mut n.lock_screen_previews, parse_bool(value))
            }
            "power.dim_after_min" => put(&mut p.dim_after_min, parse_in(value, 0, 240)),
            "power.lock_after_min" => put(&mut p.lock_after_min, parse_in(value, 0, 240)),
            "power.suspend_after_min" => put(&mut p.suspend_after_min, parse_in(value, 0, 240)),
            "power.low_power" => put(&mut p.low_power, LowPower::parse(value)),
            _ => false,
        }
    }

    /// Serializes every setting, one `key = value` per line.
    pub fn to_text(&self) -> String {
        let (a, d, i, n, p) = (
            &self.appearance,
            &self.desktop,
            &self.input,
            &self.notifications,
            &self.power,
        );
        format!(
            "# derisk settings, written by the Settings app.\n\
             appearance.scheme = {}\n\
             appearance.accent = {}\n\
             appearance.text_scale = {}\n\
             appearance.reduce_motion = {}\n\
             appearance.blur = {}\n\
             appearance.top_bar_opacity = {}\n\
             appearance.overview_opacity = {}\n\
             appearance.snap_assist_opacity = {}\n\
             desktop.layout = {}\n\
             desktop.gaps = {}\n\
             desktop.workspaces = {}\n\
             desktop.profile = {}\n\
             input.natural_scroll = {}\n\
             input.tap_to_click = {}\n\
             input.repeat_delay_ms = {}\n\
             input.repeat_rate = {}\n\
             notifications.do_not_disturb = {}\n\
             notifications.sounds = {}\n\
             notifications.lock_screen_previews = {}\n\
             power.dim_after_min = {}\n\
             power.lock_after_min = {}\n\
             power.suspend_after_min = {}\n\
             power.low_power = {}\n",
            a.scheme.as_str(),
            a.accent.as_str(),
            a.text_scale,
            a.reduce_motion,
            a.blur,
            a.panels.top_bar,
            a.panels.overview,
            a.panels.snap_assist,
            d.layout.as_str(),
            d.gaps,
            d.workspaces,
            d.profile.as_str(),
            i.natural_scroll,
            i.tap_to_click,
            i.repeat_delay_ms,
            i.repeat_rate,
            n.do_not_disturb,
            n.sounds,
            n.lock_screen_previews,
            p.dim_after_min,
            p.lock_after_min,
            p.suspend_after_min,
            p.low_power.as_str(),
        )
    }

    /// Loads a settings file. A missing file yields the defaults.
    pub fn load(path: &Path) -> io::Result<(Self, Vec<Warning>)> {
        match fs::read_to_string(path) {
            Ok(text) => Ok(Self::parse(&text)),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok((Self::default(), vec![])),
            Err(error) => Err(error),
        }
    }

    /// Writes the settings file atomically, creating its directory.
    pub fn save(&self, path: &Path) -> io::Result<()> {
        if let Some(dir) = path.parent() {
            fs::create_dir_all(dir)?;
        }
        let mut temporary = path.as_os_str().to_owned();
        temporary.push(".tmp");
        let temporary = PathBuf::from(temporary);
        fs::write(&temporary, self.to_text())?;
        fs::rename(&temporary, path)
    }

    /// The shell and app colors these settings select.
    pub fn theme(&self) -> Theme {
        let accent = self.appearance.accent.color();
        match self.appearance.scheme {
            ColorScheme::Dark => Theme {
                accent,
                ..Theme::default()
            },
            ColorScheme::Light => Theme {
                background: Color32::from_rgb(248, 250, 252),
                surface: Color32::from_rgb(226, 232, 240),
                foreground: Color32::from_rgb(15, 23, 42),
                border: Color32::from_rgb(148, 163, 184),
                // Accents are tuned for dark backgrounds; darken for contrast.
                accent: Color32::from_rgb(accent.r() / 2, accent.g() / 2, accent.b() / 2),
            },
        }
    }
}

/// Where derisk keeps its settings: `$XDG_CONFIG_HOME/derisk/settings.conf`,
/// falling back to `~/.config`. `None` when neither variable is set.
pub fn default_path() -> Option<PathBuf> {
    let config = std::env::var_os("XDG_CONFIG_HOME")
        .filter(|dir| Path::new(dir).is_absolute())
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|home| Path::new(&home).join(".config")))?;
    Some(config.join("derisk").join("settings.conf"))
}
