//! The Wallpaper, Top bar and Shortcuts pages of the Settings app, and the
//! text being typed into them.

use std::path::{Path, PathBuf};

use mcsapi_ui::{Theme, egui};

use crate::{
    model::{BarPosition, Fit, Rgb, Settings, TopBar, Wallpaper, WallpaperKind},
    shortcuts::{Chord, Shortcut, Shortcuts},
};

/// Picture extensions the session can decode.
pub const IMAGE_EXTENSIONS: [&str; 4] = ["png", "jpg", "jpeg", "webp"];

/// Video extensions offered in the file list. ffmpeg plays more; any path
/// typed in works.
const VIDEO_EXTENSIONS: [&str; 5] = ["mp4", "webm", "mkv", "mov", "gif"];

/// Typed text that is only applied once it parses, so a half-typed path
/// or chord never reaches the settings file.
#[derive(Debug, Default)]
pub(crate) struct Drafts {
    path: String,
    chords: Vec<String>,
    /// Files and folders offered for the current wallpaper kind.
    candidates: Option<(WallpaperKind, Vec<PathBuf>)>,
}

impl Drafts {
    /// Drafts showing `settings` as saved.
    pub(crate) fn new(settings: &Settings) -> Self {
        Self {
            path: settings.wallpaper.path.display().to_string(),
            chords: Shortcut::ALL
                .iter()
                .map(|s| chord_text(settings.shortcuts.get(*s)))
                .collect(),
            candidates: None,
        }
    }
}

fn chord_text(chord: Option<Chord>) -> String {
    chord.map_or_else(|| "none".to_owned(), |c| c.to_string())
}

/// Where pictures and videos usually live: the user's Pictures and Videos
/// folders, and `backgrounds` under each `$XDG_DATA_DIRS` entry.
fn wallpaper_dirs() -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    if let Some(home) = std::env::var_os("HOME").map(PathBuf::from) {
        dirs.extend(["Pictures", "Pictures/Wallpapers", "Videos"].map(|d| home.join(d)));
    }
    let data =
        std::env::var("XDG_DATA_DIRS").unwrap_or_else(|_| "/usr/local/share:/usr/share".to_owned());
    dirs.extend(
        data.split(':')
            .filter(|d| Path::new(d).is_absolute())
            .map(|d| Path::new(d).join("backgrounds")),
    );
    dirs
}

/// Files of `kind` (or folders holding pictures, for a slideshow) directly
/// inside `dirs`, sorted, at most 200.
pub fn candidates(kind: WallpaperKind, dirs: &[PathBuf]) -> Vec<PathBuf> {
    let has_ext = |path: &Path, exts: &[&str]| {
        path.extension()
            .and_then(|e| e.to_str())
            .is_some_and(|e| exts.contains(&e.to_ascii_lowercase().as_str()))
    };
    let mut found: Vec<PathBuf> = Vec::new();
    for dir in dirs {
        if kind == WallpaperKind::Slideshow
            && let Some(true) = has_pictures(dir)
        {
            found.push(dir.clone());
        }
        let Ok(entries) = std::fs::read_dir(dir) else {
            continue;
        };
        for path in entries.flatten().map(|e| e.path()) {
            let wanted = match kind {
                WallpaperKind::Image => has_ext(&path, &IMAGE_EXTENSIONS),
                WallpaperKind::Video => has_ext(&path, &VIDEO_EXTENSIONS),
                WallpaperKind::Slideshow => path.is_dir() && has_pictures(&path) == Some(true),
                _ => false,
            };
            if wanted {
                found.push(path);
            }
        }
    }
    found.sort();
    found.dedup();
    found.truncate(200);
    found
}

fn has_pictures(dir: &Path) -> Option<bool> {
    Some(std::fs::read_dir(dir).ok()?.flatten().any(|e| {
        e.path()
            .extension()
            .and_then(|x| x.to_str())
            .is_some_and(|x| IMAGE_EXTENSIONS.contains(&x.to_ascii_lowercase().as_str()))
    }))
}

fn color_row(ui: &mut egui::Ui, label: &str, color: &mut Rgb) {
    ui.label(label);
    let mut rgb = [color.0, color.1, color.2];
    if ui.color_edit_button_srgb(&mut rgb).changed() {
        *color = Rgb(rgb[0], rgb[1], rgb[2]);
    }
    ui.end_row();
}

fn hint(ui: &mut egui::Ui, theme: &Theme, text: &str) {
    ui.label("");
    ui.label(egui::RichText::new(text).small().color(theme.border));
    ui.end_row();
}

pub(crate) fn wallpaper(ui: &mut egui::Ui, w: &mut Wallpaper, drafts: &mut Drafts, theme: &Theme) {
    ui.label("Background");
    ui.horizontal_wrapped(|ui| {
        for kind in WallpaperKind::ALL {
            ui.selectable_value(&mut w.kind, kind, kind.label());
        }
    });
    ui.end_row();
    match w.kind {
        WallpaperKind::Default => {
            hint(
                ui,
                theme,
                "derisk's gradient, tinted with the accent color.",
            );
        }
        WallpaperKind::Color => color_row(ui, "Color", &mut w.color),
        WallpaperKind::Gradient => {
            color_row(ui, "Top", &mut w.color);
            color_row(ui, "Bottom", &mut w.color2);
        }
        WallpaperKind::Image | WallpaperKind::Slideshow | WallpaperKind::Video => {
            file_rows(ui, w, drafts, theme);
            ui.label("Fit");
            ui.horizontal_wrapped(|ui| {
                for fit in Fit::ALL {
                    ui.selectable_value(&mut w.fit, fit, fit.label());
                }
            });
            ui.end_row();
            if w.fit == Fit::Fit {
                color_row(ui, "Bars", &mut w.color);
            }
        }
    }
    if w.kind == WallpaperKind::Slideshow {
        ui.label("Change every");
        ui.add(
            egui::Slider::new(&mut w.interval_min, 1..=1440)
                .logarithmic(true)
                .suffix(" min"),
        );
        ui.end_row();
        ui.label("Order");
        ui.checkbox(&mut w.shuffle, "Shuffle");
        ui.end_row();
    }
    if w.kind == WallpaperKind::Video {
        ui.label("Pause");
        ui.vertical(|ui| {
            ui.checkbox(&mut w.pause_when_covered, "While a window fills the screen");
            ui.checkbox(&mut w.pause_in_low_power, "In low power mode");
        });
        ui.end_row();
        hint(
            ui,
            theme,
            "Videos loop without sound. They are decoded by ffmpeg at up to 30 fps.",
        );
    }
}

fn file_rows(ui: &mut egui::Ui, w: &mut Wallpaper, drafts: &mut Drafts, theme: &Theme) {
    ui.label(if w.kind == WallpaperKind::Slideshow {
        "Folder"
    } else {
        "File"
    });
    let edit = ui.add(
        egui::TextEdit::singleline(&mut drafts.path)
            .hint_text("/home/you/Pictures/wallpaper.jpg")
            .desired_width(320.0),
    );
    let typed = PathBuf::from(drafts.path.trim());
    let valid = drafts.path.trim().is_empty() || typed.is_absolute();
    if edit.changed() && valid {
        w.path = typed;
    }
    ui.end_row();
    if !valid {
        hint(ui, theme, "Type a full path, starting with /.");
    } else if !w.path.as_os_str().is_empty() && !w.path.exists() {
        hint(
            ui,
            theme,
            "Nothing is there yet; the default shows instead.",
        );
    }
    if drafts.candidates.as_ref().is_none_or(|(k, _)| *k != w.kind) {
        drafts.candidates = Some((w.kind, candidates(w.kind, &wallpaper_dirs())));
    }
    let Some((_, found)) = &drafts.candidates else {
        return;
    };
    if found.is_empty() {
        hint(
            ui,
            theme,
            "Nothing found in Pictures, Videos or backgrounds.",
        );
        return;
    }
    ui.label("Found");
    ui.vertical(|ui| {
        egui::ScrollArea::vertical()
            .id_salt("wallpaper-candidates")
            .max_height(160.0)
            .show(ui, |ui| {
                for path in found {
                    let name = path.file_name().map_or_else(
                        || path.display().to_string(),
                        |n| n.to_string_lossy().into_owned(),
                    );
                    if ui
                        .selectable_label(w.path == *path, name)
                        .on_hover_text(path.display().to_string())
                        .clicked()
                    {
                        w.path = path.clone();
                        drafts.path = path.display().to_string();
                    }
                }
            });
    });
    ui.end_row();
}

pub(crate) fn top_bar(ui: &mut egui::Ui, b: &mut TopBar, theme: &Theme) {
    ui.label("Position");
    ui.horizontal(|ui| {
        ui.selectable_value(&mut b.position, BarPosition::Top, "Top");
        ui.selectable_value(&mut b.position, BarPosition::Bottom, "Bottom");
    });
    ui.end_row();
    ui.label("Auto-hide");
    ui.checkbox(&mut b.autohide, "Hide until the pointer touches the edge");
    ui.end_row();
    ui.label("Show");
    ui.vertical(|ui| {
        ui.checkbox(&mut b.search, "Search field");
        ui.checkbox(&mut b.app_name, "Focused app's name");
        ui.checkbox(&mut b.date, "Date");
        ui.checkbox(&mut b.battery, "Battery");
    });
    ui.end_row();
    ui.label("Clock");
    ui.horizontal(|ui| {
        ui.selectable_value(&mut b.clock_24h, true, "24-hour");
        ui.selectable_value(&mut b.clock_24h, false, "AM/PM");
    });
    ui.end_row();
    hint(
        ui,
        theme,
        "The overview button, menus, tray and warnings always show.",
    );
}

pub(crate) fn shortcuts(ui: &mut egui::Ui, s: &mut Shortcuts, drafts: &mut Drafts, theme: &Theme) {
    let clashes = s.clashes();
    for (i, shortcut) in Shortcut::ALL.into_iter().enumerate() {
        ui.label(shortcut.label());
        ui.horizontal(|ui| {
            let text = &mut drafts.chords[i];
            let edit = ui.add(egui::TextEdit::singleline(text).desired_width(160.0));
            let parsed = if text.trim().eq_ignore_ascii_case("none") {
                Some(None)
            } else {
                Chord::parse(text).map(Some)
            };
            match parsed {
                Some(chord) if edit.changed() => s.set(shortcut, chord),
                None => {
                    ui.label(egui::RichText::new("Not a shortcut").color(theme.accent));
                }
                _ => {}
            }
            if clashes.contains(&shortcut) {
                ui.label(egui::RichText::new("Used twice").color(theme.accent));
            }
            if s.get(shortcut) != Some(shortcut.default_chord())
                && ui.small_button("Reset").clicked()
            {
                s.set(shortcut, Some(shortcut.default_chord()));
                *text = chord_text(s.get(shortcut));
            }
        });
        ui.end_row();
    }
    hint(
        ui,
        theme,
        "Write chords like Super+Shift+M; \"none\" turns one off. Each needs Super, Ctrl or Alt. \
         Super+1…9, Super+arrows and Alt+Tab are fixed.",
    );
}
