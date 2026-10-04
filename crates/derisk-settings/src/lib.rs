//! derisk Settings: appearance, desktop, input, notification, and power
//! preferences, stored as `key = value` lines in
//! `$XDG_CONFIG_HOME/derisk/settings.conf`.
//!
//! [`Settings`] is the model the shell and other apps read; [`SettingsApp`]
//! is the [`App`] that edits it.
//!
//! ```
//! use derisk_settings::{Accent, Settings};
//!
//! let (settings, warnings) = Settings::parse("appearance.accent = sky\nbogus = 1\n");
//! assert_eq!(settings.appearance.accent, Accent::Sky);
//! assert_eq!(warnings.len(), 1);
//! assert_eq!(Settings::parse(&settings.to_text()).0, settings);
//! ```

#![forbid(unsafe_code)]
#![deny(missing_docs)]

mod model;

use std::path::PathBuf;

use mcsapi_ui::{App, Theme, egui};
pub use model::{
    Accent, Appearance, ColorScheme, DesktopPrefs, Input, Layout, LowPower, Notifications,
    PanelOpacity, Power, Profile, Settings, Vrr, Warning, default_path,
};

/// A page of the Settings app.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum Page {
    /// Colors, text size, motion, blur, and panel opacity.
    #[default]
    Appearance,
    /// Layout, gaps, workspaces, profile, and variable refresh rate.
    Desktop,
    /// Keyboard and pointer.
    Input,
    /// Banners and sounds.
    Notifications,
    /// Dimming, locking, suspend, and low power mode.
    Power,
    /// Version and file location.
    About,
}

impl Page {
    /// Every page, in sidebar order.
    pub const ALL: [Self; 6] = [
        Self::Appearance,
        Self::Desktop,
        Self::Input,
        Self::Notifications,
        Self::Power,
        Self::About,
    ];

    /// The page's sidebar label.
    pub const fn label(self) -> &'static str {
        match self {
            Self::Appearance => "Appearance",
            Self::Desktop => "Desktop",
            Self::Input => "Keyboard & pointer",
            Self::Notifications => "Notifications",
            Self::Power => "Power",
            Self::About => "About",
        }
    }
}

/// The Settings app.
#[derive(Debug)]
pub struct SettingsApp {
    path: Option<PathBuf>,
    saved: Settings,
    /// The settings being edited; saved with [`SettingsApp::save`].
    pub settings: Settings,
    /// The visible page.
    pub page: Page,
    status: Option<String>,
}

impl Default for SettingsApp {
    /// Opens the user's settings file from [`default_path`].
    fn default() -> Self {
        Self::open(default_path())
    }
}

impl SettingsApp {
    /// Opens the settings file at `path`, or edits in memory when `None`.
    ///
    /// A missing file starts from defaults; an unreadable or partly invalid
    /// one is reported in the status line.
    pub fn open(path: Option<PathBuf>) -> Self {
        let (saved, status) = match path.as_deref().map(Settings::load) {
            None => (
                Settings::default(),
                Some("Not saved: no config directory".into()),
            ),
            Some(Ok((settings, warnings))) if warnings.is_empty() => (settings, None),
            Some(Ok((settings, warnings))) => {
                let status = format!(
                    "Skipped {} invalid line(s): {}",
                    warnings.len(),
                    warnings[0]
                );
                (settings, Some(status))
            }
            Some(Err(error)) => (
                Settings::default(),
                Some(format!("Could not read: {error}")),
            ),
        };
        Self {
            path,
            saved,
            settings: saved,
            page: Page::default(),
            status,
        }
    }

    /// The file being edited.
    pub fn path(&self) -> Option<&std::path::Path> {
        self.path.as_deref()
    }

    /// Whether there are unsaved changes.
    pub fn is_dirty(&self) -> bool {
        self.settings != self.saved
    }

    /// The latest status or error message.
    pub fn status(&self) -> Option<&str> {
        self.status.as_deref()
    }

    /// Writes the edited settings to the file.
    pub fn save(&mut self) {
        let Some(path) = &self.path else {
            self.status = Some("Not saved: no config directory".into());
            return;
        };
        self.status = Some(match self.settings.save(path) {
            Ok(()) => {
                self.saved = self.settings;
                "Saved".into()
            }
            Err(error) => format!("Could not save: {error}"),
        });
    }

    /// Discards unsaved changes.
    pub fn revert(&mut self) {
        self.settings = self.saved;
        self.status = None;
    }

    fn page_ui(&mut self, ui: &mut egui::Ui, theme: &Theme) {
        let s = &mut self.settings;
        ui.heading(egui::RichText::new(self.page.label()).color(theme.foreground));
        ui.add_space(8.0);
        egui::Grid::new("settings-page")
            .num_columns(2)
            .spacing([24.0, 10.0])
            .show(ui, |ui| match self.page {
                Page::Appearance => {
                    let a = &mut s.appearance;
                    ui.label("Style");
                    ui.horizontal(|ui| {
                        for scheme in [ColorScheme::Dark, ColorScheme::Light] {
                            let text = if scheme == ColorScheme::Dark {
                                "Dark"
                            } else {
                                "Light"
                            };
                            ui.selectable_value(&mut a.scheme, scheme, text);
                        }
                    });
                    ui.end_row();
                    ui.label("Accent");
                    ui.horizontal(|ui| {
                        for accent in Accent::ALL {
                            swatch(ui, &mut a.accent, accent, theme);
                        }
                    });
                    ui.end_row();
                    ui.label("Text size");
                    ui.add(egui::Slider::new(&mut a.text_scale, 0.75..=2.0).step_by(0.05));
                    ui.end_row();
                    ui.label("Reduce motion");
                    ui.checkbox(&mut a.reduce_motion, "Cross-fade instead of animating");
                    ui.end_row();
                    ui.label("Background blur");
                    ui.add(
                        egui::Slider::new(&mut a.blur, 0..=10).custom_formatter(|v, _| {
                            if v == 0.0 {
                                "Off".into()
                            } else {
                                format!("{v}")
                            }
                        }),
                    );
                    ui.end_row();
                    for (label, value) in [
                        ("Top bar opacity", &mut a.panels.top_bar),
                        ("Overview opacity", &mut a.panels.overview),
                        ("Snap Assist opacity", &mut a.panels.snap_assist),
                    ] {
                        ui.label(label);
                        ui.add(egui::Slider::new(value, 20..=100).suffix(" %"));
                        ui.end_row();
                    }
                }
                Page::Desktop => {
                    let d = &mut s.desktop;
                    ui.label("Default layout");
                    ui.horizontal(|ui| {
                        ui.selectable_value(&mut d.layout, Layout::Tall, "Tall");
                        ui.selectable_value(&mut d.layout, Layout::Monocle, "Monocle");
                    });
                    ui.end_row();
                    ui.label("Window gaps");
                    ui.add(egui::Slider::new(&mut d.gaps, 0..=64).suffix(" px"));
                    ui.end_row();
                    ui.label("Workspaces");
                    ui.add(egui::Slider::new(&mut d.workspaces, 1..=9));
                    ui.end_row();
                    ui.label("Profile");
                    egui::ComboBox::from_id_salt("profile")
                        .selected_text(profile_label(d.profile))
                        .show_ui(ui, |ui| {
                            for profile in [
                                Profile::Automatic,
                                Profile::Phone,
                                Profile::Tablet,
                                Profile::Desktop,
                            ] {
                                ui.selectable_value(
                                    &mut d.profile,
                                    profile,
                                    profile_label(profile),
                                );
                            }
                        });
                    ui.end_row();
                    ui.label("Variable refresh rate");
                    ui.horizontal(|ui| {
                        for vrr in Vrr::ALL {
                            ui.selectable_value(&mut d.vrr, vrr, vrr.label());
                        }
                    });
                    ui.end_row();
                }
                Page::Input => {
                    let i = &mut s.input;
                    ui.label("Scrolling");
                    ui.checkbox(&mut i.natural_scroll, "Natural scrolling");
                    ui.end_row();
                    ui.label("Touchpad");
                    ui.checkbox(&mut i.tap_to_click, "Tap to click");
                    ui.end_row();
                    ui.label("Repeat delay");
                    ui.add(egui::Slider::new(&mut i.repeat_delay_ms, 100..=1000).suffix(" ms"));
                    ui.end_row();
                    ui.label("Repeat rate");
                    ui.add(egui::Slider::new(&mut i.repeat_rate, 1..=60).suffix(" /s"));
                    ui.end_row();
                }
                Page::Notifications => {
                    let n = &mut s.notifications;
                    ui.label("Do not disturb");
                    ui.checkbox(&mut n.do_not_disturb, "Hide banners");
                    ui.end_row();
                    ui.label("Sounds");
                    ui.checkbox(&mut n.sounds, "Play a sound");
                    ui.end_row();
                    ui.label("Lock screen");
                    ui.checkbox(&mut n.lock_screen_previews, "Show message previews");
                    ui.end_row();
                }
                Page::Power => {
                    let p = &mut s.power;
                    for (label, value) in [
                        ("Dim screen after", &mut p.dim_after_min),
                        ("Lock after", &mut p.lock_after_min),
                        ("Suspend after", &mut p.suspend_after_min),
                    ] {
                        ui.label(label);
                        ui.add(
                            egui::Slider::new(value, 0..=240)
                                .suffix(" min")
                                .custom_formatter(|v, _| {
                                    if v == 0.0 {
                                        "Never".into()
                                    } else {
                                        format!("{v}")
                                    }
                                }),
                        );
                        ui.end_row();
                    }
                    ui.label("Low power mode");
                    ui.horizontal(|ui| {
                        for mode in LowPower::ALL {
                            ui.selectable_value(&mut p.low_power, mode, mode.label());
                        }
                    });
                    ui.end_row();
                    ui.label("");
                    ui.label(
                        egui::RichText::new(
                            "Turns off blur and animations and caps the frame rate at 30 fps.",
                        )
                        .small()
                        .color(theme.border),
                    );
                    ui.end_row();
                }
                Page::About => {
                    ui.label("derisk Settings");
                    ui.label(env!("CARGO_PKG_VERSION"));
                    ui.end_row();
                    ui.label("Settings file");
                    ui.label(match &self.path {
                        Some(path) => path.display().to_string(),
                        None => "Not available".into(),
                    });
                    ui.end_row();
                    ui.label("License");
                    ui.label(env!("CARGO_PKG_LICENSE"));
                    ui.end_row();
                }
            });
    }
}

fn profile_label(profile: Profile) -> &'static str {
    match profile {
        Profile::Automatic => "Automatic",
        Profile::Phone => "Phone",
        Profile::Tablet => "Tablet",
        Profile::Desktop => "Desktop",
    }
}

fn swatch(ui: &mut egui::Ui, current: &mut Accent, accent: Accent, theme: &Theme) {
    let (rect, response) = ui.allocate_exact_size(egui::vec2(24.0, 24.0), egui::Sense::click());
    let selected = *current == accent;
    let painter = ui.painter();
    painter.circle_filled(rect.center(), 10.0, accent.color());
    if selected || response.hovered() {
        painter.circle_stroke(
            rect.center(),
            12.0,
            egui::Stroke::new(2.0, theme.foreground),
        );
    }
    if response.on_hover_text(accent.as_str()).clicked() {
        *current = accent;
    }
}

impl App for SettingsApp {
    fn title(&self) -> &str {
        "Settings"
    }

    fn ui(&mut self, ui: &mut egui::Ui, theme: &Theme) {
        let narrow = ui.available_width() < NARROW;
        if narrow {
            // A 180-pixel sidebar would leave a phone half a page, so the
            // pages become a row of tabs across the top that scrolls sideways.
            egui::Panel::top("settings-pages").show(ui, |ui| {
                egui::ScrollArea::horizontal().show(ui, |ui| {
                    ui.horizontal(|ui| self.page_buttons(ui));
                });
            });
        } else {
            egui::Panel::left("settings-pages")
                .resizable(false)
                .exact_size(180.0)
                .show(ui, |ui| self.page_buttons(ui));
        }
        egui::Panel::bottom("settings-actions").show(ui, |ui| {
            ui.horizontal(|ui| {
                let dirty = self.is_dirty();
                if ui.add_enabled(dirty, egui::Button::new("Save")).clicked() {
                    self.save();
                }
                if ui.add_enabled(dirty, egui::Button::new("Revert")).clicked() {
                    self.revert();
                }
                if dirty {
                    ui.label(egui::RichText::new("Unsaved changes").color(theme.accent));
                } else if let Some(status) = &self.status {
                    ui.label(status);
                }
            });
        });
        egui::CentralPanel::default_margins().show(ui, |ui| {
            // A row of controls wider than a phone scrolls rather than being
            // cut off where nothing can reach it.
            let scroll = if narrow {
                egui::ScrollArea::both()
            } else {
                egui::ScrollArea::vertical()
            };
            scroll.show(ui, |ui| self.page_ui(ui, theme));
        });
    }
}

/// Below this width the page list moves from a sidebar to tabs on top.
const NARROW: f32 = 560.0;

impl SettingsApp {
    fn page_buttons(&mut self, ui: &mut egui::Ui) {
        for page in Page::ALL {
            if ui
                .selectable_label(self.page == page, page.label())
                .clicked()
            {
                self.page = page;
            }
        }
    }
}
