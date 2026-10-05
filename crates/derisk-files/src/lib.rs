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

    fn places(&self) -> Vec<(&'static str, PathBuf)> {
        let mut places = Vec::new();
        if let Some(home) = std::env::var_os("HOME").map(PathBuf::from) {
            places.push(("🏠 Home", home.clone()));
            for (label, dir) in [
                ("🖥 Desktop", "Desktop"),
                ("🗐 Documents", "Documents"),
                ("⬇ Downloads", "Downloads"),
                ("🎵 Music", "Music"),
                ("🖼 Pictures", "Pictures"),
                ("🎞 Videos", "Videos"),
            ] {
                let path = home.join(dir);
                if path.is_dir() {
                    places.push((label, path));
                }
            }
        }
        if let Some(trash) = self.browser.as_ref().and_then(Browser::trash) {
            places.push(("🗑 Trash", trash.files_dir()));
        }
        places.push(("💻 Computer", PathBuf::from("/")));
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
            if ui
                .add_enabled(back, egui::Button::new("⬅"))
                .on_hover_text("Back")
                .clicked()
            {
                self.go(Browser::go_back);
            }
            if ui
                .add_enabled(forward, egui::Button::new("➡"))
                .on_hover_text("Forward")
                .clicked()
            {
                self.go(Browser::go_forward);
            }
            if ui.button("⬆").on_hover_text("Parent folder").clicked() {
                self.go(Browser::go_up);
            }
            if ui.button("⟳").on_hover_text("Refresh").clicked() {
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
                ui.add(
                    egui::TextEdit::singleline(&mut browser.filter)
                        .hint_text("🔍 Filter")
                        .desired_width(search_width),
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
        let arrow = if descending { " ⏷" } else { " ⏶" };
        let header = |label: &str, column: SortKey| {
            if key == column {
                format!("{label}{arrow}")
            } else {
                label.to_owned()
            }
        };
        let mut activate = None;
        let mut clicked = None;
        let row_height = ui.spacing().interact_size.y;
        // The name column takes whatever the fixed columns leave, so the
        // table spans the window instead of huddling in its left third.
        let columns = |width: f32| {
            let size = 80.0_f32.min(width * 0.15);
            let modified = 136.0_f32.min(width * 0.4);
            [(width - size - modified).max(0.0), size, modified]
        };
        let width = ui.available_width();
        let header_rect = ui
            .allocate_exact_size(egui::vec2(width, row_height), egui::Sense::hover())
            .0;
        let mut x = header_rect.left();
        for ((label, column), w) in [
            ("Name", SortKey::Name),
            ("Size", SortKey::Size),
            ("Modified", SortKey::Modified),
        ]
        .into_iter()
        .zip(columns(width))
        {
            let cell = egui::Rect::from_min_size(
                egui::pos2(x, header_rect.top()),
                egui::vec2(w, row_height),
            );
            // Flat headers under one thin rule, like the table below them,
            // rather than a row of framed buttons.
            let button = egui::Button::new(header(label, column)).frame(false);
            if ui
                .put(cell, |ui: &mut egui::Ui| {
                    ui.with_layout(egui::Layout::left_to_right(egui::Align::Center), |ui| {
                        ui.add(button)
                    })
                    .inner
                })
                .clicked()
            {
                browser.sort_by(column);
            }
            x += w;
        }
        ui.painter().hline(
            header_rect.x_range(),
            header_rect.bottom(),
            egui::Stroke::new(1.0, theme.border),
        );
        egui::ScrollArea::vertical()
            .auto_shrink(false)
            .show(ui, |ui| {
                ui.spacing_mut().item_spacing.y = 0.0;
                let width = ui.available_width();
                let [name_w, size_w, _] = columns(width);
                for (i, entry) in browser.visible().enumerate() {
                    let icon = match (entry.kind, entry.symlink) {
                        (Kind::Directory, _) => "🗀",
                        (_, true) => "🔗",
                        (Kind::File, _) => "🗋",
                        (Kind::Other, _) => "❓",
                    };
                    let selected = browser.is_selected(&entry.path);
                    // The whole row is one target, so a click anywhere along
                    // it selects the entry, not only on the name's text.
                    let (rect, row) =
                        ui.allocate_exact_size(egui::vec2(width, row_height), egui::Sense::click());
                    let name = entry.name.clone();
                    row.widget_info(|| {
                        egui::WidgetInfo::selected(
                            egui::WidgetType::SelectableLabel,
                            true,
                            selected,
                            &name,
                        )
                    });
                    let visuals = ui.visuals();
                    let fill = if selected {
                        Some(visuals.selection.bg_fill)
                    } else if row.hovered() {
                        Some(visuals.widgets.hovered.weak_bg_fill)
                    } else if i % 2 == 1 {
                        Some(visuals.faint_bg_color)
                    } else {
                        None
                    };
                    if let Some(fill) = fill {
                        ui.painter().rect_filled(rect, 2.0, fill);
                    }
                    let mut text = egui::RichText::new(format!("{icon} {}", entry.name));
                    if entry.is_hidden() {
                        text = text.color(theme.border);
                    }
                    let size = match entry.kind {
                        Kind::Directory => "—".to_owned(),
                        _ => fs_ops::human_size(entry.size),
                    };
                    let modified = entry.modified.map_or_else(
                        || "—".to_owned(),
                        |t| fs_ops::timestamp(t)[..16].replace('T', " "),
                    );
                    // Cells keep a small inset whatever the touch padding, so
                    // a full date still fits beside the name on a phone.
                    let pad = 4.0;
                    let mut x = rect.left();
                    for (text, w) in [
                        (text, name_w),
                        (egui::RichText::new(size), size_w),
                        (
                            egui::RichText::new(modified),
                            rect.right() - (x + name_w + size_w),
                        ),
                    ] {
                        let cell = egui::Rect::from_min_size(
                            egui::pos2(x + pad, rect.top()),
                            egui::vec2((w - 2.0 * pad).max(0.0), row_height),
                        );
                        // `put` centers what it adds; cells read from the left.
                        ui.scope_builder(
                            egui::UiBuilder::new()
                                .max_rect(cell)
                                .layout(egui::Layout::left_to_right(egui::Align::Center)),
                            |ui| ui.add(egui::Label::new(text).truncate().selectable(false)),
                        );
                        x += w;
                    }
                    if row.double_clicked() {
                        activate = Some(entry.path.clone());
                    } else if row.clicked() {
                        let extend = ui.input(|i| i.modifiers.command || i.modifiers.shift);
                        clicked = Some((entry.path.clone(), extend));
                    }
                }
            });
        if let Some((path, extend)) = clicked {
            browser.select(&path, extend);
        }
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
            for (label, path) in self.places() {
                if ui
                    .selectable_label(cwd.as_ref() == Some(&path), label)
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
            // Wide enough for the longest place name, and no wider: on a big
            // window the listing gets the room, not the sidebar.
            let sidebar = (ui.available_width() * 0.16).clamp(140.0, 220.0);
            egui::Panel::left("files-places")
                .resizable(false)
                .exact_size(sidebar)
                .show(ui, places);
        }
        if let Some(path) = place {
            self.open_place(&path);
        }
        egui::CentralPanel::default_margins().show(ui, |ui| self.listing(ui, theme));
    }
}
