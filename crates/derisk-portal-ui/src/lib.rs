//! derisk portal dialogs.
//!
//! Sandboxed and agent-driven apps ask the desktop for files, screens and
//! permissions through [xdg-desktop-portal]. This crate draws derisk's
//! answers to those requests, one dialog per portal:
//!
//! | Dialog | Portal |
//! | --- | --- |
//! | [`file_chooser`] | `org.freedesktop.portal.FileChooser` |
//! | [`screen_cast`] | `org.freedesktop.portal.ScreenCast` |
//! | [`screenshot`] | `org.freedesktop.portal.Screenshot` |
//! | [`access`] | `org.freedesktop.portal.Camera` and other `Access` prompts |
//! | [`app_chooser`] | `org.freedesktop.portal.OpenURI` |
//! | [`background`] | `org.freedesktop.portal.Background` |
//!
//! Each dialog is a [`mcsapi_ui::App`] built from a serializable [`Request`]
//! that finishes with a [`Reply`]. They use the spacious control sizes
//! (44 px fields, 48 px buttons) and stretch their lists, grids and previews
//! to whatever size the window has. With the `window` feature,
//! `derisk-portal-dialog` shows one in its own window; the portal backend,
//! `xdg-desktop-portal-derisk` (crate `derisk-portal`), runs it once per
//! request.
//!
//! ```
//! use derisk_portal_ui::{Dialog, Reply, Request, background};
//! use mcsapi_ui::{Theme, egui};
//!
//! let mut dialog = derisk_portal_ui::open(Request::Background(background::Request {
//!     app_id: "org.example.Sync".into(),
//!     app_name: "Sync".into(),
//!     ..Default::default()
//! }));
//! assert_eq!(dialog.title(), "Background Activity");
//! let context = egui::Context::default();
//! let mut output = mcsapi_ui::run_frame(&mut *dialog, &context, Default::default(), &Theme::default());
//! output.textures_delta.clear();
//! assert!(dialog.reply().is_none());
//! ```
//!
//! [xdg-desktop-portal]: https://flatpak.github.io/xdg-desktop-portal/

#![forbid(unsafe_code)]
#![deny(missing_docs)]

pub mod access;
pub mod app_chooser;
pub mod background;
pub mod file_chooser;
pub mod icons;
pub mod screen_cast;
pub mod screenshot;
pub mod widgets;
#[cfg(feature = "window")]
pub mod window;

use mcsapi_components::Tokens;
use mcsapi_ui::{App, Theme, egui};
use serde::{Deserialize, Serialize};

/// A request for one dialog.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(tag = "dialog", rename_all = "snake_case")]
pub enum Request {
    /// Open or save files.
    FileChooser(file_chooser::Request),
    /// Pick screens or windows to share.
    ScreenCast(screen_cast::Request),
    /// Take a screenshot.
    Screenshot(screenshot::Request),
    /// Grant or deny a device or permission.
    Access(access::Request),
    /// Pick the app that opens a file or link.
    AppChooser(app_chooser::Request),
    /// Let an app run in the background.
    Background(background::Request),
}

/// How a dialog ended.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(tag = "outcome", rename_all = "snake_case")]
pub enum Reply {
    /// Dismissed: Cancel, Deny, Don't Allow, Escape or the window closed.
    Cancelled,
    /// Files chosen in the [`file_chooser`].
    Files(file_chooser::Choice),
    /// Sources chosen in the [`screen_cast`] dialog.
    ScreenCast(screen_cast::Choice),
    /// What to capture, from the [`screenshot`] dialog.
    Screenshot(screenshot::Choice),
    /// Access granted, with the [`access`] dialog's choices.
    Access(access::Choice),
    /// The app chosen in the [`app_chooser`].
    App(app_chooser::Choice),
    /// Background activity allowed, from the [`background`] dialog.
    Background(background::Choice),
}

/// A change to an open dialog while it is showing.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(tag = "update", rename_all = "snake_case")]
pub enum Update {
    /// The [`app_chooser`]'s apps changed (`UpdateChoices`).
    Choices {
        /// The new apps.
        choices: Vec<app_chooser::AppEntry>,
    },
}

/// A portal dialog.
pub trait Dialog: App + Send {
    /// How the dialog ended, once it has.
    fn reply(&self) -> Option<&Reply>;

    /// The window's content size in points when it opens.
    fn size(&self) -> [f32; 2];

    /// Applies a change from the requesting app. Most dialogs ignore them.
    fn update(&mut self, update: Update) {
        let _ = update;
    }
}

/// Builds the dialog for `request`.
pub fn open(request: Request) -> Box<dyn Dialog> {
    match request {
        Request::FileChooser(r) => Box::new(file_chooser::FileChooser::new(r)),
        Request::ScreenCast(r) => Box::new(screen_cast::ScreenCast::new(r)),
        Request::Screenshot(r) => Box::new(screenshot::Screenshot::new(r)),
        Request::Access(r) => Box::new(access::Access::new(r)),
        Request::AppChooser(r) => Box::new(app_chooser::AppChooser::new(r)),
        Request::Background(r) => Box::new(background::Background::new(r)),
    }
}

/// Draws `dialog` filling `ui`, on the theme background, with the theme's
/// tokens installed. Hosts call this once per frame.
pub fn show(dialog: &mut dyn Dialog, ui: &mut egui::Ui, theme: &Theme) {
    Tokens::from_theme(theme).install(ui.ctx());
    let rect = ui.max_rect();
    ui.painter().rect_filled(rect, 0, theme.background);
    widgets::region(ui, rect, |ui| dialog.ui(ui, theme));
    mcsapi_ui::paint_focus_ring(ui.ctx(), theme.accent);
}

/// Whether Escape was pressed this frame (and consumes it).
fn escape(ui: &egui::Ui) -> bool {
    ui.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Escape))
}

/// The sample request for each dialog, with the design's placeholder
/// content, by name: `file-chooser`, `screen-cast`, `screenshot`, `access`,
/// `app-chooser` or `background`.
pub fn sample(name: &str) -> Option<Request> {
    Some(match name {
        "file-chooser" => Request::FileChooser(file_chooser::Request {
            app_name: "Firefox".into(),
            title: "Open File".into(),
            filters: vec![
                file_chooser::Filter {
                    name: "All files".into(),
                    patterns: vec![file_chooser::Pattern::Glob("*".into())],
                },
                file_chooser::Filter {
                    name: "PDF documents".into(),
                    patterns: vec![file_chooser::Pattern::Mime("application/pdf".into())],
                },
                file_chooser::Filter {
                    name: "Office documents".into(),
                    patterns: ["odt", "ods", "odp", "docx", "xlsx"]
                        .iter()
                        .map(|e| file_chooser::Pattern::Glob(format!("*.{e}")))
                        .collect(),
                },
                file_chooser::Filter {
                    name: "Images".into(),
                    patterns: vec![file_chooser::Pattern::Mime("image/*".into())],
                },
            ],
            ..Default::default()
        }),
        "screen-cast" => Request::ScreenCast(screen_cast::Request::sample()),
        "screenshot" => Request::Screenshot(screenshot::Request {
            app_name: "Agent".into(),
            heading: "An agent wants a screenshot".into(),
            ..Default::default()
        }),
        "access" => Request::Access(access::Request::sample()),
        "app-chooser" => Request::AppChooser(app_chooser::Request::sample()),
        "background" => Request::Background(background::Request {
            app_id: "com.github.syncthing.Syncthing".into(),
            app_name: "Syncthing".into(),
            description:
                "It runs as a systemd user unit and keeps syncing after its window closes.".into(),
            command: "syncthing serve --no-browser".into(),
            autostart: true,
        }),
        _ => return None,
    })
}

/// Names [`sample`] accepts.
pub const SAMPLES: [&str; 6] = [
    "file-chooser",
    "screen-cast",
    "screenshot",
    "access",
    "app-chooser",
    "background",
];
