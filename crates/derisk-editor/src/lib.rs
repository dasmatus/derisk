//! derisk Text Editor: open, edit, find, and save UTF-8 text files.
//!
//! [`Document`] is the file model; [`EditorApp`] draws it.
//!
//! ```
//! use derisk_editor::Document;
//!
//! let path = std::env::temp_dir().join(format!("derisk-editor-doc-{}.txt", std::process::id()));
//! let mut doc = Document::default();
//! doc.text.push_str("hello");
//! assert!(doc.is_dirty());
//! doc.save_as(&path)?;
//! assert!(!doc.is_dirty());
//! assert_eq!(Document::open(&path)?.text, "hello");
//! # std::fs::remove_file(&path)?;
//! # Ok::<(), std::io::Error>(())
//! ```

#![forbid(unsafe_code)]
#![deny(missing_docs)]

use std::{
    fs,
    io::{self, Write},
    os::unix::fs::{OpenOptionsExt, PermissionsExt},
    path::{Path, PathBuf},
    sync::atomic::{AtomicU32, Ordering},
};

use mcsapi_ui::{App, Theme, egui};

/// Files larger than this are refused rather than loaded into the editor.
pub const MAX_FILE_SIZE: u64 = 8 * 1024 * 1024;

/// A text file being edited.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Document {
    path: Option<PathBuf>,
    saved: String,
    /// The current contents.
    pub text: String,
}

impl Document {
    /// Reads a UTF-8 file of at most [`MAX_FILE_SIZE`] bytes.
    pub fn open(path: &Path) -> io::Result<Self> {
        let size = fs::metadata(path)?.len();
        if size > MAX_FILE_SIZE {
            return Err(io::Error::new(
                io::ErrorKind::FileTooLarge,
                format!("file is larger than {} MiB", MAX_FILE_SIZE / 1024 / 1024),
            ));
        }
        let text = String::from_utf8(fs::read(path)?)
            .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "not a UTF-8 text file"))?;
        Ok(Self {
            path: Some(path.to_owned()),
            saved: text.clone(),
            text,
        })
    }

    /// The file this document is saved to, if any.
    pub fn path(&self) -> Option<&Path> {
        self.path.as_deref()
    }

    /// The file name, or "Untitled".
    pub fn name(&self) -> String {
        self.path
            .as_deref()
            .and_then(Path::file_name)
            .map_or_else(|| "Untitled".into(), |n| n.to_string_lossy().into_owned())
    }

    /// Whether the text differs from what was last opened or saved.
    pub fn is_dirty(&self) -> bool {
        self.text != self.saved
    }

    /// Saves to the document's file. Fails for an untitled document.
    pub fn save(&mut self) -> io::Result<()> {
        let path = self
            .path
            .clone()
            .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "choose where to save"))?;
        self.save_as(&path)
    }

    /// Saves to `path` atomically and makes it the document's file.
    pub fn save_as(&mut self, path: &Path) -> io::Result<()> {
        // Keep the original file's permissions, such as an executable bit,
        // and give a new file what a plain create would under the umask.
        let mode = fs::metadata(path).map_or(0o666 & !umask(), |m| m.permissions().mode() & 0o7777);
        let (temporary, mut file) = create_beside(path)?;
        let written = file
            .set_permissions(fs::Permissions::from_mode(mode))
            .and_then(|()| file.write_all(self.text.as_bytes()))
            .and_then(|()| file.sync_all());
        drop(file);
        written
            .and_then(|()| fs::rename(&temporary, path))
            .inspect_err(|_| {
                let _ = fs::remove_file(&temporary);
            })?;
        self.path = Some(path.to_owned());
        self.saved.clone_from(&self.text);
        Ok(())
    }

    /// Byte offsets of every case-insensitive match of `needle` (exact-case
    /// when the text has characters whose lowercase form changes length).
    pub fn find(&self, needle: &str) -> Vec<usize> {
        if needle.is_empty() {
            return Vec::new();
        }
        let haystack = self.text.to_lowercase();
        // Lowercasing can change byte lengths, which would make the offsets
        // wrong; fall back to an exact-case search then.
        if haystack.len() != self.text.len() {
            return self.text.match_indices(needle).map(|(i, _)| i).collect();
        }
        haystack
            .match_indices(&needle.to_lowercase())
            .map(|(i, _)| i)
            .collect()
    }

    /// Replaces every case-sensitive occurrence of `from`. Returns the count.
    pub fn replace_all(&mut self, from: &str, to: &str) -> usize {
        if from.is_empty() {
            return 0;
        }
        let count = self.text.matches(from).count();
        if count > 0 {
            self.text = self.text.replace(from, to);
        }
        count
    }

    /// 1-based line and column of a character index into the text.
    pub fn line_column(&self, char_index: usize) -> (usize, usize) {
        let before: String = self.text.chars().take(char_index).collect();
        let line = before.matches('\n').count() + 1;
        let column = before.rsplit('\n').next().map_or(0, |l| l.chars().count()) + 1;
        (line, column)
    }
}

/// The Text Editor app.
#[derive(Debug, Default)]
pub struct EditorApp {
    /// The open document.
    pub document: Document,
    location: String,
    find: String,
    replace: String,
    show_find: bool,
    cursor: usize,
    status: Option<String>,
    confirm_discard: Option<Pending>,
}

#[derive(Clone, Debug)]
enum Pending {
    New,
    Open(PathBuf),
}

impl EditorApp {
    /// Opens `path`, or starts an untitled document when it does not exist yet.
    pub fn open(path: impl Into<PathBuf>) -> Self {
        let path = path.into();
        let mut app = Self {
            location: path.display().to_string(),
            ..Self::default()
        };
        match Document::open(&path) {
            Ok(document) => app.document = document,
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                app.status = Some("New file; it is created when you save".into());
            }
            Err(error) => app.status = Some(format!("Could not open: {error}")),
        }
        app
    }

    /// An untitled document.
    pub fn new() -> Self {
        Self::default()
    }

    /// The latest status or error message.
    pub fn status(&self) -> Option<&str> {
        self.status.as_deref()
    }

    /// Saves to the location field, which may differ from the open file.
    pub fn save(&mut self) {
        let target = PathBuf::from(self.location.trim());
        let result = if self.location.trim().is_empty() {
            Err(io::Error::new(
                io::ErrorKind::NotFound,
                "type a file path to save to",
            ))
        } else if self.document.path() == Some(target.as_path()) {
            self.document.save()
        } else {
            self.document.save_as(&target)
        };
        self.status = Some(match result {
            Ok(()) => format!("Saved {}", self.document.name()),
            Err(error) => format!("Could not save: {error}"),
        });
    }

    fn run(&mut self, pending: Pending) {
        if self.document.is_dirty() && self.confirm_discard.is_none() {
            self.confirm_discard = Some(pending);
            return;
        }
        self.confirm_discard = None;
        match pending {
            Pending::New => {
                self.document = Document::default();
                self.location.clear();
                self.status = None;
            }
            Pending::Open(path) => match Document::open(&path) {
                Ok(document) => {
                    self.document = document;
                    self.status = None;
                }
                Err(error) => self.status = Some(format!("Could not open: {error}")),
            },
        }
    }

    fn toolbar(&mut self, ui: &mut egui::Ui, theme: &Theme) {
        ui.horizontal(|ui| {
            if ui.button("New").clicked() {
                self.run(Pending::New);
            }
            if ui.button("Open").clicked() {
                self.run(Pending::Open(PathBuf::from(self.location.trim())));
            }
            if ui.button("Save").on_hover_text("Ctrl+S").clicked() {
                self.save();
            }
            ui.toggle_value(&mut self.show_find, "Find")
                .on_hover_text("Ctrl+F");
            ui.add(
                egui::TextEdit::singleline(&mut self.location)
                    .hint_text("/path/to/file.txt")
                    .desired_width(f32::INFINITY),
            );
        });
        if let Some(pending) = self.confirm_discard.clone() {
            ui.horizontal(|ui| {
                ui.label(
                    egui::RichText::new(format!("Discard changes to {}?", self.document.name()))
                        .color(theme.accent),
                );
                if ui.button("Discard").clicked() {
                    self.run(pending);
                }
                if ui.button("Keep editing").clicked() {
                    self.confirm_discard = None;
                }
            });
        }
        if self.show_find {
            // Wraps where the window is too narrow, as on a phone.
            ui.horizontal_wrapped(|ui| {
                ui.add(egui::TextEdit::singleline(&mut self.find).hint_text("Find"));
                ui.add(egui::TextEdit::singleline(&mut self.replace).hint_text("Replace with"));
                if ui.button("Replace all").clicked() {
                    let count = self.document.replace_all(&self.find, &self.replace);
                    self.status = Some(format!("Replaced {count} match(es)"));
                }
                let matches = self.document.find(&self.find).len();
                if !self.find.is_empty() {
                    ui.label(format!("{matches} match(es)"));
                }
            });
        }
    }
}

impl App for EditorApp {
    fn title(&self) -> &str {
        "Text Editor"
    }

    fn ui(&mut self, ui: &mut egui::Ui, theme: &Theme) {
        let (save, find) = ui.input_mut(|i| {
            (
                i.consume_key(egui::Modifiers::COMMAND, egui::Key::S),
                i.consume_key(egui::Modifiers::COMMAND, egui::Key::F),
            )
        });
        if save {
            self.save();
        }
        if find {
            self.show_find = !self.show_find;
        }
        egui::Panel::top("editor-toolbar").show(ui, |ui| self.toolbar(ui, theme));
        egui::Panel::bottom("editor-status").show(ui, |ui| {
            ui.horizontal(|ui| {
                let dirty = if self.document.is_dirty() { " •" } else { "" };
                ui.label(format!("{}{dirty}", self.document.name()));
                ui.separator();
                let (line, column) = self.document.line_column(self.cursor);
                ui.label(format!("Ln {line}, Col {column}"));
                ui.separator();
                ui.label(format!(
                    "{} lines · {} chars",
                    self.document.text.lines().count().max(1),
                    self.document.text.chars().count()
                ));
                if let Some(status) = &self.status {
                    ui.separator();
                    ui.label(status);
                }
            });
        });
        egui::CentralPanel::default_margins().show(ui, |ui| {
            egui::ScrollArea::both().auto_shrink(false).show(ui, |ui| {
                let output = egui::TextEdit::multiline(&mut self.document.text)
                    .code_editor()
                    .desired_width(f32::INFINITY)
                    .desired_rows(30)
                    .frame(egui::Frame::NONE)
                    .show(ui);
                if let Some(range) = output.cursor_range {
                    self.cursor = range.primary.index.into();
                }
            });
        });
    }
}

/// Creates a new, empty file next to `path` for an atomic save, readable only
/// by its owner until the caller sets its mode.
///
/// The name is fresh each time and the file is created with `O_EXCL`, which
/// never follows a symlink and never opens a file that is already there. A
/// fixed name (`<file>.derisk-save`, as this used to be) could be planted in a
/// directory another user can write, as a symlink to one of this user's files
/// or as a file the other user owns, and the save would then write the
/// document through it.
fn create_beside(path: &Path) -> io::Result<(PathBuf, fs::File)> {
    static SAVES: AtomicU32 = AtomicU32::new(0);
    let name = path
        .file_name()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "not a file name"))?;
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.subsec_nanos());
    for _ in 0..64 {
        let mut temporary = std::ffi::OsString::from(".");
        temporary.push(name);
        temporary.push(format!(
            ".derisk-save-{}-{}-{nanos:x}",
            std::process::id(),
            SAVES.fetch_add(1, Ordering::Relaxed)
        ));
        let temporary = path.with_file_name(temporary);
        match fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&temporary)
        {
            Ok(file) => return Ok((temporary, file)),
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(e),
        }
    }
    Err(io::Error::new(
        io::ErrorKind::AlreadyExists,
        "no free name to save through",
    ))
}

/// The process umask, read from /proc so it is not changed to find out.
fn umask() -> u32 {
    fs::read_to_string("/proc/self/status")
        .ok()
        .and_then(|s| {
            s.lines()
                .find_map(|l| l.strip_prefix("Umask:"))
                .and_then(|v| u32::from_str_radix(v.trim(), 8).ok())
        })
        .unwrap_or(0o022)
}
