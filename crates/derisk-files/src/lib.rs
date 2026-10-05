//! derisk Files: a file manager with places, history, sorting, filtering,
//! copy/cut/paste, renaming, and the freedesktop.org trash.
//!
//! [`Browser`] holds the state and does the filesystem work without any UI,
//! and [`FilesApp`] draws it.
//!
//! ```
//! use derisk_files::{Browser, ClipboardOp, Trash};
//!
//! let root = std::env::temp_dir().join(format!("derisk-files-doc-{}", std::process::id()));
//! std::fs::create_dir_all(root.join("a"))?;
//! let mut files = Browser::with_trash(&root, Some(Trash::at(root.join(".trash"))))?;
//! let notes = files.create_file("notes.txt")?;
//! files.set_clipboard(ClipboardOp::Copy);
//! files.navigate(root.join("a"))?;
//! assert_eq!(files.paste()?, [root.join("a/notes.txt")]);
//! files.go_back()?;
//! files.select(&notes, false);
//! assert_eq!(files.trash_selection()?, 1);
//! assert!(!notes.exists());
//! std::fs::remove_dir_all(&root)?;
//! # Ok::<(), std::io::Error>(())
//! ```

#![forbid(unsafe_code)]
#![deny(missing_docs)]

mod browser;
pub mod fs_ops;

use std::{
    io,
    path::{Path, PathBuf},
};

pub use browser::{Browser, ClipboardOp, SortKey};
pub use fs_ops::{Entry, Kind, Trash};
use mcsapi_ui::{App, Theme, egui};

/// Opens a file with the user's default application.
pub type Opener = Box<dyn FnMut(&Path) -> io::Result<()>>;

/// Opens files with `xdg-open`, without waiting for it.
pub fn xdg_open(path: &Path) -> io::Result<()> {
    std::process::Command::new("xdg-open")
        .arg(path)
        .stdin(std::process::Stdio::null())
        .spawn()
        .map(drop)
}

#[derive(Debug)]
enum PromptKind {
    NewFolder,
    NewFile,
    Rename(PathBuf),
}

#[derive(Debug)]
struct Prompt {
    kind: PromptKind,
    text: String,
}

/// The Files app.
pub struct FilesApp {
    /// The browsing state.
    pub browser: Option<Browser>,
    opener: Opener,
    location: String,
    prompt: Option<Prompt>,
    status: Option<String>,
}

impl std::fmt::Debug for FilesApp {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FilesApp")
            .field("browser", &self.browser)
            .field("status", &self.status)
            .finish_non_exhaustive()
    }
}

impl Default for FilesApp {
    /// Opens the home directory, or `/` when `HOME` is unset.
    fn default() -> Self {
        let home = std::env::var_os("HOME").map_or_else(|| PathBuf::from("/"), PathBuf::from);
        Self::new(Browser::new(home), Box::new(xdg_open))
    }
}

impl FilesApp {
    /// Shows `browser`, opening files with `opener`. An `Err` browser is
    /// reported and leaves the app empty until a place is chosen.
    pub fn new(browser: io::Result<Browser>, opener: Opener) -> Self {
        let (browser, status) = match browser {
            Ok(browser) => (Some(browser), None),
            Err(error) => (None, Some(format!("Could not open folder: {error}"))),
        };
        let location = browser
            .as_ref()
            .map(|b| b.cwd().display().to_string())
            .unwrap_or_default();
        Self {
            browser,
            opener,
            location,
            prompt: None,
            status,
        }
    }

    /// The latest status or error message.
    pub fn status(&self) -> Option<&str> {
        self.status.as_deref()
    }

    /// Opens a folder, or a file with the default application.
    pub fn activate(&mut self, path: &Path) {
        if path.is_dir() {
            self.go(|b| b.navigate(path));
        } else if let Err(error) = (self.opener)(path) {
            self.status = Some(format!("Could not open {}: {error}", path.display()));
        }
    }

    /// Opens a folder, creating the browser if it failed to start.
    pub fn go(&mut self, step: impl FnOnce(&mut Browser) -> io::Result<()>) {
        let result = match &mut self.browser {
            Some(browser) => step(browser),
            None => Err(io::Error::new(io::ErrorKind::NotFound, "no folder open")),
        };
        self.report(result.map(|()| None));
        self.sync_location();
    }

    fn open_place(&mut self, dir: &Path) {
        if self.browser.is_some() {
            self.go(|b| b.navigate(dir));
        } else {
            *self = Self::new(
                Browser::new(dir),
                std::mem::replace(&mut self.opener, Box::new(xdg_open)),
            );
        }
    }

    fn sync_location(&mut self) {
        if let Some(browser) = &self.browser {
            self.location = browser.cwd().display().to_string();
        }
    }

    fn report(&mut self, result: io::Result<Option<String>>) {
        match result {
            Ok(Some(message)) => self.status = Some(message),
            Ok(None) => self.status = None,
            Err(error) => self.status = Some(format!("Error: {error}")),
        }
    }

    /// The sidebar's places: symbolic icon, label and folder.
    fn places(&self) -> Vec<(&'static str, &'static str, PathBuf)> {
        let mut places = Vec::new();
        if let Some(home) = std::env::var_os("HOME").map(PathBuf::from) {
            places.push(("user-home", "Home", home.clone()));
            for (icon, dir) in [
                ("user-desktop", "Desktop"),
                ("folder-documents", "Documents"),
                ("folder-download", "Downloads"),
                ("folder-music", "Music"),
                ("folder-pictures", "Pictures"),
                ("folder-videos", "Videos"),
            ] {
                let path = home.join(dir);
                if path.is_dir() {
                    places.push((icon, dir, path));
                }
            }
        }
        if let Some(trash) = self.browser.as_ref().and_then(Browser::trash) {
            places.push(("user-trash", "Trash", trash.files_dir()));
        }
        places.push(("computer", "Computer", PathBuf::from("/")));
        places
    }

    fn submit_prompt(&mut self) {
        let (Some(prompt), Some(browser)) = (self.prompt.take(), &mut self.browser) else {
            return;
        };
        let result = match &prompt.kind {
            PromptKind::NewFolder => browser.create_folder(&prompt.text),
            PromptKind::NewFile => browser.create_file(&prompt.text),
            PromptKind::Rename(path) => browser.rename(path, &prompt.text),
        };
        if result.is_err() {
            self.prompt = Some(prompt);
        }
        self.report(result.map(|_| None));
    }

    fn toolbar(&mut self, ui: &mut egui::Ui) {
        let Some(browser) = &self.browser else {
            return;
        };
        let (back, forward) = (browser.can_go_back(), browser.can_go_forward());
        ui.horizontal(|ui| {
            if derisk_icons::tool(ui, back, "go-previous", "Back").clicked() {
                self.go(Browser::go_back);
            }
            if derisk_icons::tool(ui, forward, "go-next", "Forward").clicked() {
                self.go(Browser::go_forward);
            }
            if derisk_icons::tool(ui, true, "go-up", "Parent folder").clicked() {
                self.go(Browser::go_up);
            }
            if derisk_icons::tool(ui, true, "view-refresh", "Refresh").clicked() {
                self.go(Browser::refresh);
            }
            // On a phone the filter and the location share what is left.
            let search_width = (ui.available_width() * 0.35).min(180.0);
            let location = ui.add(
                egui::TextEdit::singleline(&mut self.location)
                    .desired_width((ui.available_width() - search_width - 16.0).max(60.0)),
            );
            if location.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                let target = PathBuf::from(&self.location);
                self.go(|b| b.navigate(target));
            }
            if let Some(browser) = &mut self.browser {
                let color = ui.visuals().weak_text_color();
                derisk_icons::show(ui, "system-search", 16.0, color);
                ui.add(
                    egui::TextEdit::singleline(&mut browser.filter)
                        .hint_text("Filter")
                        .desired_width(search_width - 20.0),
                );
            }
        });
        let Some(browser) = &mut self.browser else {
            return;
        };
        let selected = browser.selection().len();
        let mut action = None;
        // Wraps onto a second line where the window is too narrow for it.
        ui.horizontal_wrapped(|ui| {
            if ui.button("New folder").clicked() {
                action = Some("folder");
            }
            if ui.button("New file").clicked() {
                action = Some("file");
            }
            ui.separator();
            if ui
                .add_enabled(selected == 1, egui::Button::new("Rename"))
                .clicked()
            {
                action = Some("rename");
            }
            if ui
                .add_enabled(selected > 0, egui::Button::new("Copy"))
                .clicked()
            {
                browser.set_clipboard(ClipboardOp::Copy);
            }
            if ui
                .add_enabled(selected > 0, egui::Button::new("Cut"))
                .clicked()
            {
                browser.set_clipboard(ClipboardOp::Cut);
            }
            let can_paste = browser.clipboard().is_some();
            if ui
                .add_enabled(can_paste, egui::Button::new("Paste"))
                .clicked()
            {
                action = Some("paste");
            }
            let can_trash = selected > 0 && browser.trash().is_some();
            if ui
                .add_enabled(can_trash, egui::Button::new("Move to trash"))
                .clicked()
            {
                action = Some("trash");
            }
            ui.separator();
            ui.checkbox(&mut browser.show_hidden, "Hidden files");
        });
        self.run_action(action);
    }

    fn run_action(&mut self, action: Option<&str>) {
        let Some(browser) = &mut self.browser else {
            return;
        };
        match action {
            Some("folder") => {
                self.prompt = Some(Prompt {
                    kind: PromptKind::NewFolder,
                    text: "New folder".into(),
                });
            }
            Some("file") => {
                self.prompt = Some(Prompt {
                    kind: PromptKind::NewFile,
                    text: "New file.txt".into(),
                });
            }
            Some("rename") => {
                if let Some(path) = browser.selection().first().cloned() {
                    let text = path
                        .file_name()
                        .map(|n| n.to_string_lossy().into_owned())
                        .unwrap_or_default();
                    self.prompt = Some(Prompt {
                        kind: PromptKind::Rename(path),
                        text,
                    });
                }
            }
            Some("paste") => {
                let result = browser
                    .paste()
                    .map(|p| Some(format!("Pasted {} item(s)", p.len())));
                self.report(result);
            }
            Some("trash") => {
                let result = browser
                    .trash_selection()
                    .map(|n| Some(format!("Moved {n} item(s) to the trash")));
                self.report(result);
            }
            _ => {}
        }
    }

    fn prompt_ui(&mut self, ui: &mut egui::Ui) {
        let Some(prompt) = &mut self.prompt else {
            return;
        };
        let mut submit = false;
        let mut cancel = false;
        ui.horizontal(|ui| {
            ui.label(match prompt.kind {
                PromptKind::NewFolder => "Folder name",
                PromptKind::NewFile => "File name",
                PromptKind::Rename(_) => "New name",
            });
            let field = ui.text_edit_singleline(&mut prompt.text);
            if !field.has_focus() && !field.lost_focus() {
                field.request_focus();
            }
            submit = field.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
            submit |= ui.button("OK").clicked();
            cancel =
                ui.button("Cancel").clicked() || ui.input(|i| i.key_pressed(egui::Key::Escape));
        });
        if submit {
            self.submit_prompt();
        } else if cancel {
            self.prompt = None;
        }
    }

    fn keyboard(&mut self, ui: &egui::Ui) {
        if ui.ctx().egui_wants_keyboard_input() || self.prompt.is_some() {
            return;
        }
        let (delete, rename, all, up, enter) = ui.input(|i| {
            (
                i.key_pressed(egui::Key::Delete),
                i.key_pressed(egui::Key::F2),
                i.modifiers.command && i.key_pressed(egui::Key::A),
                i.key_pressed(egui::Key::Backspace),
                i.key_pressed(egui::Key::Enter),
            )
        });
        if delete {
            self.run_action(Some("trash"));
        } else if rename {
            self.run_action(Some("rename"));
        } else if all {
            if let Some(browser) = &mut self.browser {
                browser.select_all();
            }
        } else if up {
            self.go(Browser::go_up);
        } else if enter {
            let target = self
                .browser
                .as_ref()
                .filter(|b| b.selection().len() == 1)
                .and_then(|b| b.selection().first().cloned());
            if let Some(path) = target {
                self.activate(&path);
            }
        }
    }

    fn listing(&mut self, ui: &mut egui::Ui, theme: &Theme) {
        let Some(browser) = &mut self.browser else {
            ui.label("Choose a place on the left.");
            return;
        };
        let (key, descending) = browser.sort();
        let arrow = if descending { "pan-down" } else { "pan-up" };
        let mut activate = None;
        egui::ScrollArea::vertical()
            .auto_shrink(false)
            .show(ui, |ui| {
                egui::Grid::new("files-listing")
                    .num_columns(3)
                    .striped(true)
                    .spacing([24.0, 4.0])
                    .min_col_width(80.0)
                    .show(ui, |ui| {
                        for (label, column) in [
                            ("Name", SortKey::Name),
                            ("Size", SortKey::Size),
                            ("Modified", SortKey::Modified),
                        ] {
                            let header = if key == column {
                                let color = ui.visuals().text_color();
                                derisk_icons::button_with_text(ui.ctx(), arrow, label, 12.0, color)
                            } else {
                                egui::Button::new(label)
                            };
                            if ui.add(header).clicked() {
                                browser.sort_by(column);
                            }
                        }
                        ui.end_row();
                        let mut clicked = None;
                        for entry in browser.visible() {
                            let icon = match (entry.kind, entry.symlink) {
                                (Kind::Directory, _) => "folder",
                                (_, true) => "insert-link",
                                (Kind::File, _) => "text-x-generic",
                                (Kind::Other, _) => "dialog-question",
                            };
                            let selected = browser.is_selected(&entry.path);
                            let mut text = egui::RichText::new(&entry.name);
                            let mut color = ui.visuals().text_color();
                            if entry.is_hidden() {
                                text = text.color(theme.border);
                                color = theme.border;
                            }
                            let icon = derisk_icons::atom(ui.ctx(), icon, 16.0, color);
                            let row = ui.add(egui::Button::selectable(selected, (icon, text)));
                            if row.double_clicked() {
                                activate = Some(entry.path.clone());
                            } else if row.clicked() {
                                let extend = ui.input(|i| i.modifiers.command || i.modifiers.shift);
                                clicked = Some((entry.path.clone(), extend));
                            }
                            ui.label(match entry.kind {
                                Kind::Directory => "—".to_owned(),
                                _ => fs_ops::human_size(entry.size),
                            });
                            ui.label(entry.modified.map_or_else(
                                || "—".to_owned(),
                                |t| fs_ops::timestamp(t)[..16].replace('T', " "),
                            ));
                            ui.end_row();
                        }
                        if let Some((path, extend)) = clicked {
                            browser.select(&path, extend);
                        }
                    });
            });
        if let Some(path) = activate {
            self.activate(&path);
        }
    }
}

/// Below this width the places move from a sidebar to a row on top.
const NARROW: f32 = 560.0;

impl App for FilesApp {
    fn title(&self) -> &str {
        "Files"
    }

    fn ui(&mut self, ui: &mut egui::Ui, theme: &Theme) {
        self.keyboard(ui);
        egui::Panel::top("files-toolbar").show(ui, |ui| {
            self.toolbar(ui);
            self.prompt_ui(ui);
        });
        egui::Panel::bottom("files-status").show(ui, |ui| {
            let summary = self.browser.as_ref().map(|b| {
                let selected = b.selection().len();
                let shown = b.visible().count();
                let clip = match b.clipboard() {
                    Some((ClipboardOp::Copy, p)) => format!(" · {} to copy", p.len()),
                    Some((ClipboardOp::Cut, p)) => format!(" · {} to move", p.len()),
                    None => String::new(),
                };
                format!("{shown} items · {selected} selected{clip}")
            });
            ui.horizontal(|ui| {
                ui.label(summary.unwrap_or_default());
                if let Some(status) = &self.status {
                    ui.separator();
                    ui.label(egui::RichText::new(status).color(theme.accent));
                }
            });
        });
        let mut place = None;
        let mut places = |ui: &mut egui::Ui| {
            let cwd = self.browser.as_ref().map(|b| b.cwd().to_owned());
            let color = ui.visuals().text_color();
            for (icon, label, path) in self.places() {
                let atom = derisk_icons::atom(ui.ctx(), icon, 16.0, color);
                if ui
                    .add(egui::Button::selectable(
                        cwd.as_ref() == Some(&path),
                        (atom, label),
                    ))
                    .clicked()
                {
                    place = Some(path);
                }
            }
        };
        if ui.available_width() < NARROW {
            // On a phone the places sidebar would take half the screen, so
            // they become a row across the top that scrolls sideways.
            egui::Panel::top("files-places").show(ui, |ui| {
                egui::ScrollArea::horizontal().show(ui, |ui| ui.horizontal(&mut places));
            });
        } else {
            egui::Panel::left("files-places")
                .resizable(false)
                .exact_size(170.0)
                .show(ui, places);
        }
        if let Some(path) = place {
            self.open_place(&path);
        }
        egui::CentralPanel::default_margins().show(ui, |ui| self.listing(ui, theme));
    }
}
