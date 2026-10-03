//! Translucent, blurred panels and low power mode.
//!
//! [`Effects`] holds what the user picked in the Settings app (blur
//! strength, per-panel opacity, when to save power); [`Effects::resolve`]
//! combines it with the battery into a [`Look`] for this frame. In low power
//! mode the blur, window shadows and animations go away, panels turn nearly
//! opaque so text stays readable without the blur, and the frame rate
//! halves.
//!
//! ```
//! use derisk::{effects::Effects, overview::Battery};
//! use derisk_settings::LowPower;
//!
//! let effects = Effects { low_power: LowPower::OnBattery, ..Effects::default() };
//! let plugged = effects.resolve(Some(Battery { percent: 80, charging: true }));
//! let unplugged = effects.resolve(Some(Battery { percent: 80, charging: false }));
//! assert!(plugged.blur > 0 && !plugged.low_power);
//! assert!(unplugged.low_power && unplugged.blur == 0);
//! assert!(unplugged.frame_interval > plugged.frame_interval);
//! ```

use std::{
    path::{Path, PathBuf},
    time::{Duration, SystemTime},
};

use derisk_settings::{LowPower, PanelOpacity, Settings};
use mcsapi::Geometry;

use crate::overview::Battery;

/// Time between frames normally, about 60 per second.
pub const FRAME_INTERVAL: Duration = Duration::from_millis(16);

/// Time between frames in low power mode, about 30 per second.
pub const LOW_POWER_FRAME_INTERVAL: Duration = Duration::from_millis(33);

/// The least panel opacity in low power mode, where nothing is blurred.
const LOW_POWER_MIN_OPACITY: f32 = 0.97;

/// The user's effect preferences.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Effects {
    /// Blur strength under translucent panels, 0 (off) to 10.
    pub blur: u8,
    /// Opacity of each panel in percent.
    pub panels: PanelOpacity,
    /// Replace motion with cross-fades.
    pub reduce_motion: bool,
    /// When low power mode turns on.
    pub low_power: LowPower,
}

impl Default for Effects {
    fn default() -> Self {
        Self::from_settings(&Settings::default())
    }
}

/// Effects in force for one frame.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Look {
    /// Whether low power mode is on.
    pub low_power: bool,
    /// Blur strength for panels, 0 for none.
    pub blur: u8,
    /// Top bar opacity, 0–1.
    pub top_bar: f32,
    /// Overview backdrop opacity, 0–1.
    pub overview: f32,
    /// Snap Assist opacity, 0–1.
    pub snap_assist: f32,
    /// Whether to animate (the startup sequence).
    pub animate: bool,
    /// Whether windows cast shadows.
    pub shadows: bool,
    /// Time between frames.
    pub frame_interval: Duration,
}

/// An area of the screen to blur under a panel, in logical pixels.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct BlurArea {
    /// The panel's area.
    pub area: Geometry,
    /// The panel's corner radius.
    pub corner_radius: u8,
    /// Blur strength, 1–10.
    pub strength: u8,
}

impl Effects {
    /// Takes the effect preferences from the Settings app's model.
    pub fn from_settings(settings: &Settings) -> Self {
        let a = &settings.appearance;
        Self {
            blur: a.blur.min(10),
            panels: a.panels,
            reduce_motion: a.reduce_motion,
            low_power: settings.power.low_power,
        }
    }

    /// The effects for this frame. A device without a battery is never on
    /// battery.
    pub fn resolve(&self, battery: Option<Battery>) -> Look {
        let on_battery = battery.is_some_and(|b| !b.charging);
        let low_power = self.low_power.active(on_battery);
        let opacity = |percent: u8| {
            let o = f32::from(percent.clamp(20, 100)) / 100.0;
            if low_power {
                o.max(LOW_POWER_MIN_OPACITY)
            } else {
                o
            }
        };
        Look {
            low_power,
            blur: if low_power { 0 } else { self.blur },
            top_bar: opacity(self.panels.top_bar),
            overview: opacity(self.panels.overview),
            snap_assist: opacity(self.panels.snap_assist),
            animate: !low_power && !self.reduce_motion,
            shadows: !low_power,
            frame_interval: if low_power {
                LOW_POWER_FRAME_INTERVAL
            } else {
                FRAME_INTERVAL
            },
        }
    }
}

/// Reloads [`Effects`] when the settings file changes, so the Settings app
/// applies live.
#[derive(Debug)]
pub struct SettingsWatch {
    path: Option<PathBuf>,
    modified: Option<SystemTime>,
    loaded: bool,
}

impl SettingsWatch {
    /// Watches `path`, usually [`derisk_settings::default_path`].
    pub fn new(path: Option<PathBuf>) -> Self {
        Self {
            path,
            modified: None,
            loaded: false,
        }
    }

    /// The watched file.
    pub fn path(&self) -> Option<&Path> {
        self.path.as_deref()
    }

    /// New effects on the first call and whenever the file's modification
    /// time changes (including when it appears or goes away), else `None`.
    pub fn poll(&mut self) -> Option<Effects> {
        let Some(path) = &self.path else {
            return (!std::mem::replace(&mut self.loaded, true)).then(Effects::default);
        };
        let modified = std::fs::metadata(path).and_then(|m| m.modified()).ok();
        if self.loaded && modified == self.modified {
            return None;
        }
        self.loaded = true;
        self.modified = modified;
        let settings = Settings::load(path).map_or_else(|_| Settings::default(), |(s, _)| s);
        Some(Effects::from_settings(&settings))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const PLUGGED: Option<Battery> = Some(Battery {
        percent: 50,
        charging: true,
    });
    const UNPLUGGED: Option<Battery> = Some(Battery {
        percent: 50,
        charging: false,
    });

    #[test]
    fn default_look_is_translucent_blurred_and_animated() {
        let look = Effects::default().resolve(None);
        assert!(!look.low_power);
        assert!(look.blur > 0);
        assert!(look.top_bar < 1.0 && look.top_bar >= 0.2);
        assert!(look.animate && look.shadows);
        assert_eq!(look.frame_interval, FRAME_INTERVAL);
    }

    #[test]
    fn low_power_drops_blur_shadows_animation_and_frames() {
        let effects = Effects {
            low_power: LowPower::On,
            ..Effects::default()
        };
        let look = effects.resolve(PLUGGED);
        assert!(look.low_power);
        assert_eq!(look.blur, 0);
        assert!(!look.animate && !look.shadows);
        assert_eq!(look.frame_interval, LOW_POWER_FRAME_INTERVAL);
        assert!(look.top_bar >= LOW_POWER_MIN_OPACITY);
    }

    #[test]
    fn on_battery_mode_needs_a_discharging_battery() {
        let effects = Effects {
            low_power: LowPower::OnBattery,
            ..Effects::default()
        };
        assert!(!effects.resolve(None).low_power);
        assert!(!effects.resolve(PLUGGED).low_power);
        assert!(effects.resolve(UNPLUGGED).low_power);
        let off = Effects {
            low_power: LowPower::Off,
            ..effects
        };
        assert!(!off.resolve(UNPLUGGED).low_power);
    }

    #[test]
    fn opacity_follows_settings_within_bounds() {
        let effects = Effects {
            panels: PanelOpacity {
                top_bar: 100,
                overview: 35,
                snap_assist: 5,
            },
            ..Effects::default()
        };
        let look = effects.resolve(None);
        assert_eq!(look.top_bar, 1.0);
        assert_eq!(look.overview, 0.35);
        assert_eq!(look.snap_assist, 0.2);
    }

    #[test]
    fn reduce_motion_stops_animation_but_keeps_blur() {
        let effects = Effects {
            reduce_motion: true,
            ..Effects::default()
        };
        let look = effects.resolve(None);
        assert!(!look.animate);
        assert!(look.blur > 0);
    }

    #[test]
    fn watch_reloads_when_the_file_changes() {
        let dir = std::env::temp_dir().join(format!("derisk-effects-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("settings.conf");
        let mut watch = SettingsWatch::new(Some(path.clone()));
        assert_eq!(watch.poll(), Some(Effects::default()));
        assert_eq!(watch.poll(), None);

        std::fs::write(&path, "appearance.blur = 2\npower.low_power = on\n").unwrap();
        let effects = watch.poll().expect("file appeared");
        assert_eq!(effects.blur, 2);
        assert_eq!(effects.low_power, LowPower::On);
        assert_eq!(watch.poll(), None);

        std::fs::remove_file(&path).unwrap();
        assert_eq!(watch.poll(), Some(Effects::default()));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn watch_without_a_path_yields_defaults_once() {
        let mut watch = SettingsWatch::new(None);
        assert_eq!(watch.poll(), Some(Effects::default()));
        assert_eq!(watch.poll(), None);
    }
}
