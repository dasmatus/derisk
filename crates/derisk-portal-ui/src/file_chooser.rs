//! 1a File Chooser (`org.freedesktop.portal.FileChooser`): a Places
//! sidebar, a breadcrumb, a search field and the folder's files, with the
//! file type filter, a status line and Cancel / Open in the footer.

use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    time::SystemTime,
};

use derisk_files::{Browser, Kind, fs_ops};
use egui::{Align2, Key, Modifiers, Rect, Stroke, Ui, pos2, vec2};
use mcsapi_ui::egui;
use mcsapi_ui::{App, Theme};
use serde::{Deserialize, Serialize};

use crate::{
    Reply,
    icons::{self, Icon},
    widgets::{self as w, Variant},
};

/// One pattern of a [`Filter`].
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Pattern {
    /// A shell glob such as `*.pdf`, matched case-insensitively.
    Glob(String),
    /// A MIME type such as `application/pdf` or `image/*`.
    Mime(String),
}

/// A named file type filter.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct Filter {
    /// Display name, for example "PDF documents".
    pub name: String,
    /// A file matches when any pattern matches.
    pub patterns: Vec<Pattern>,
}

impl Filter {
    /// Whether a file named `name` passes.
    pub fn matches(&self, name: &str) -> bool {
        self.patterns.iter().any(|p| match p {
            Pattern::Glob(glob) => glob_matches(glob, name),
            Pattern::Mime(mime) => mime_matches(mime, name),
        })
    }
}

/// What the app asked for.
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
pub struct Request {
    /// The requesting app's name, shown in the status line.
    #[serde(default)]
    pub app_name: String,
    /// Window title, for example "Open File".
    pub title: String,
    /// Label of the confirm button; "Open" or "Save" when unset.
    #[serde(default)]
    pub accept_label: Option<String>,
    /// Allow choosing several files.
    #[serde(default)]
    pub multiple: bool,
    /// Choose folders instead of files.
    #[serde(default)]
    pub directory: bool,
    /// Saving: the suggested file name. `None` opens files.
    #[serde(default)]
    pub save_name: Option<String>,
    /// File type filters; empty shows everything.
    #[serde(default)]
    pub filters: Vec<Filter>,
    /// Index into `filters` to start with.
    #[serde(default)]
    pub current_filter: Option<usize>,
    /// Folder to start in; the home folder when unset.
    #[serde(default)]
    pub current_folder: Option<PathBuf>,
}

/// What the person chose.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct Choice {
    /// Chosen files or folders, absolute.
    pub paths: Vec<PathBuf>,
    /// Index of the filter in use, if the app gave any.
    pub filter: Option<usize>,
}

/// A sidebar place.
#[derive(Clone, Debug)]
struct Place {
    name: &'static str,
    icon: Icon<'static>,
    path: PathBuf,
}

fn home() -> PathBuf {
    std::env::var_os("HOME").map_or_else(|| PathBuf::from("/"), PathBuf::from)
}

/// `XDG_<key>_DIR` from `user-dirs.dirs`, else `~/<fallback>`.
pub fn user_dir(key: &str, fallback: &str) -> PathBuf {
    let home = home();
    let config = std::env::var_os("XDG_CONFIG_HOME")
        .filter(|v| !v.is_empty())
        .map_or_else(|| home.join(".config"), PathBuf::from);
    let found = std::fs::read_to_string(config.join("user-dirs.dirs"))
        .ok()
        .and_then(|text| parse_user_dir(&text, key, &home));
    found.unwrap_or_else(|| home.join(fallback))
}

/// The `XDG_<key>_DIR="$HOME/..."` line of a `user-dirs.dirs` file.
pub fn parse_user_dir(text: &str, key: &str, home: &Path) -> Option<PathBuf> {
    let prefix = format!("XDG_{key}_DIR=");
    let value = text
        .lines()
        .map(str::trim)
        .find_map(|l| l.strip_prefix(&prefix))?
        .trim_matches('"');
    if let Some(rest) = value.strip_prefix("$HOME") {
        Some(home.join(rest.trim_start_matches('/')))
    } else if value.starts_with('/') {
        Some(PathBuf::from(value))
    } else {
        None
    }
}

fn places() -> Vec<Place> {
    let home = home();
    let data = std::env::var_os("XDG_DATA_HOME")
        .filter(|v| !v.is_empty())
        .map_or_else(|| home.join(".local/share"), PathBuf::from);
    vec![
        Place {
            name: "Home",
            icon: icons::HOME,
            path: home,
        },
        Place {
            name: "Documents",
            icon: icons::DOCUMENTS,
            path: user_dir("DOCUMENTS", "Documents"),
        },
        Place {
            name: "Downloads",
            icon: icons::DOWNLOADS,
            path: user_dir("DOWNLOAD", "Downloads"),
        },
        Place {
            name: "Pictures",
            icon: icons::PICTURES,
            path: user_dir("PICTURES", "Pictures"),
        },
        Place {
            name: "Videos",
            icon: icons::VIDEOS,
            path: user_dir("VIDEOS", "Videos"),
        },
        Place {
            name: "Trash",
            icon: icons::TRASH,
            path: data.join("Trash/files"),
        },
    ]
}

/// The File Chooser dialog.
pub struct FileChooser {
    request: Request,
    places: Vec<Place>,
    browser: Result<Browser, String>,
    /// Folder item counts, by path, for the Size column.
    counts: HashMap<PathBuf, usize>,
    filters: Vec<String>,
    filter: usize,
    search: String,
    name: String,
    error: Option<String>,
    reply: Option<Reply>,
}

impl FileChooser {
    /// Opens the dialog in the requested folder.
    pub fn new(request: Request) -> Self {
        let places = places();
        let start = request
            .current_folder
            .clone()
            .filter(|p| p.is_dir())
            .unwrap_or_else(home);
        let mut filters: Vec<String> = request.filters.iter().map(|f| f.name.clone()).collect();
        if filters.is_empty() {
            filters.push("All files".into());
        }
        let filter = request
            .current_filter
            .filter(|&i| i < filters.len())
            .unwrap_or(0);
        let mut chooser = Self {
            name: request.save_name.clone().unwrap_or_default(),
            request,
            places,
            browser: Err(String::new()),
            counts: HashMap::new(),
            filters,
            filter,
            search: String::new(),
            error: None,
            reply: None,
        };
        chooser.browser = Browser::new(&start).map_err(|e| e.to_string());
        chooser.count_folders();
        chooser
    }

    fn count_folders(&mut self) {
        self.counts.clear();
        if let Ok(browser) = &self.browser {
            for entry in browser.visible().filter(|e| e.kind == Kind::Directory) {
                if let Ok(dir) = std::fs::read_dir(&entry.path) {
                    self.counts.insert(entry.path.clone(), dir.count());
                }
            }
        }
    }

    fn navigate(&mut self, dir: &Path) {
        let result = match &mut self.browser {
            Ok(browser) => browser.navigate(dir),
            Err(_) => Browser::new(dir).map(|b| self.browser = Ok(b)),
        };
        match result {
            Ok(()) => {
                self.error = None;
                self.search.clear();
                self.count_folders();
            }
            Err(e) => self.error = Some(format!("Could not open {}: {e}", dir.display())),
        }
    }

    fn saving(&self) -> bool {
        self.request.save_name.is_some()
    }

    /// The entries shown: the search applied by the browser, then the
    /// type filter (folders always pass).
    fn shown(&self) -> Vec<&fs_ops::Entry> {
        let Ok(browser) = &self.browser else {
            return Vec::new();
        };
        let filter = self.request.filters.get(self.filter);
        browser
            .visible()
            .filter(|e| {
                e.kind == Kind::Directory
                    || (!self.request.directory && filter.is_none_or(|f| f.matches(&e.name)))
            })
            .collect()
    }

    fn selection(&self) -> Vec<PathBuf> {
        self.browser
            .as_ref()
            .map(|b| b.selection().iter().cloned().collect())
            .unwrap_or_default()
    }

    fn accept_label(&self) -> String {
        self.request.accept_label.clone().unwrap_or_else(|| {
            if self.saving() {
                "Save".into()
            } else {
                "Open".into()
            }
        })
    }

    /// Confirms: saves to the typed name, opens the selection, or enters
    /// a selected folder when files are wanted.
    fn accept(&mut self) {
        let filter = (!self.request.filters.is_empty()).then_some(self.filter);
        let Ok(browser) = &self.browser else {
            return;
        };
        let cwd = browser.cwd().to_owned();
        if self.saving() {
            let name = self.name.trim();
            if let Err(e) = fs_ops::validate_name(name) {
                self.error = Some(e.to_string());
                return;
            }
            self.reply = Some(Reply::Files(Choice {
                paths: vec![cwd.join(name)],
                filter,
            }));
            return;
        }
        let selection = self.selection();
        if !self.request.directory
            && let [only] = selection.as_slice()
            && only.is_dir()
        {
            let only = only.clone();
            self.navigate(&only);
            return;
        }
        let paths = if selection.is_empty() && self.request.directory {
            vec![cwd]
        } else {
            selection
        };
        if !paths.is_empty() {
            self.reply = Some(Reply::Files(Choice { paths, filter }));
        }
    }

    fn can_accept(&self) -> bool {
        if self.saving() {
            !self.name.trim().is_empty()
        } else {
            self.request.directory || !self.selection().is_empty()
        }
    }

    fn sidebar(&mut self, ui: &mut Ui, rect: Rect) {
        let t = w::tokens(ui);
        ui.painter().rect_filled(rect, 0, t.card);
        ui.painter()
            .vline(rect.right() - 0.5, rect.y_range(), t.border_stroke());
        let inner = rect.shrink2(vec2(16.0, 20.0));
        w::text(
            ui.painter(),
            Rect::from_min_size(inner.min + vec2(12.0, 0.0), vec2(inner.width(), 11.0)),
            Align2::LEFT_TOP,
            "PLACES",
            w::sans(11.0),
            t.muted_foreground,
        );
        let cwd = self.browser.as_ref().ok().map(|b| b.cwd().to_owned());
        let mut y = inner.top() + 11.0 + 8.0 + 4.0;
        let mut go = None;
        for (index, place) in self.places.iter().enumerate() {
            let row = Rect::from_min_size(pos2(inner.left(), y), vec2(inner.width(), 48.0));
            let selected = cwd.as_deref() == Some(place.path.as_path());
            let response = w::row(
                ui,
                row,
                ui.id().with(("place", index)),
                selected,
                place.name,
            );
            icons::paint(
                ui,
                Rect::from_min_size(
                    pos2(row.left() + 14.0, row.center().y - 10.0),
                    vec2(20.0, 20.0),
                ),
                place.icon,
                t.foreground,
            );
            w::text(
                ui.painter(),
                Rect::from_min_max(pos2(row.left() + 48.0, row.top()), row.max),
                Align2::LEFT_CENTER,
                place.name,
                w::sans(15.0),
                t.foreground,
            );
            if response.clicked() {
                go = Some(place.path.clone());
            }
            y += 48.0 + 4.0;
        }
        if let Some(path) = go {
            self.navigate(&path);
        }
    }

    /// Breadcrumb items for the current folder: from Home when inside it.
    fn crumbs(&self) -> Vec<(String, PathBuf)> {
        let Ok(browser) = &self.browser else {
            return Vec::new();
        };
        crumbs(browser.cwd(), &home())
    }

    fn toolbar(&mut self, ui: &mut Ui, rect: Rect) {
        let t = w::tokens(ui);
        ui.painter()
            .hline(rect.x_range(), rect.bottom() - 0.5, t.border_stroke());
        let inner = rect.shrink2(vec2(24.0, 16.0));
        let back = Rect::from_min_size(
            pos2(inner.left(), inner.center().y - 18.0),
            vec2(36.0, 36.0),
        );
        let can_back = self.browser.as_ref().is_ok_and(Browser::can_go_back);
        if w::icon_button(ui, back, "back", icons::BACK, "Back", can_back).clicked()
            && let Ok(browser) = &mut self.browser
        {
            if let Err(e) = browser.go_back() {
                self.error = Some(e.to_string());
            }
            self.count_folders();
        }

        // Crumbs: links in muted text, the current folder in the foreground.
        let mut x = back.right() + 16.0;
        let crumbs = self.crumbs();
        let search_min = 120.0_f32.max(inner.width() * 0.35);
        let crumb_right = inner.right() - search_min - 16.0;
        let mut go = None;
        for (index, (name, path)) in crumbs.iter().enumerate() {
            let last = index + 1 == crumbs.len();
            let color = if last {
                t.foreground
            } else {
                t.muted_foreground
            };
            let g = w::galley(ui.painter(), name, w::sans(14.0), color);
            let width = g.size().x.min((crumb_right - x).max(0.0));
            let r = Rect::from_min_size(pos2(x, inner.center().y - 10.0), vec2(width, 20.0));
            w::text_truncated(
                ui.painter(),
                r,
                Align2::LEFT_CENTER,
                name,
                w::sans(14.0),
                color,
            );
            if !last {
                let link = ui.interact(r, ui.id().with(("crumb", index)), egui::Sense::click());
                link.widget_info(|| {
                    egui::WidgetInfo::labeled(egui::WidgetType::Link, true, name.as_str())
                });
                if link.hovered() {
                    ui.painter()
                        .hline(r.x_range(), r.bottom() - 2.0, Stroke::new(1.0, color));
                }
                if link.clicked() {
                    go = Some(path.clone());
                }
                x = r.right() + 8.0;
                let sep = w::text(
                    ui.painter(),
                    Rect::from_min_size(pos2(x, r.top()), vec2(12.0, 20.0)),
                    Align2::LEFT_CENTER,
                    "›",
                    w::sans(14.0),
                    t.muted_foreground,
                );
                x = sep.right() + 8.0;
            } else {
                x = r.right();
            }
        }
        if let Some(path) = go {
            self.navigate(&path);
        }

        let search = Rect::from_min_max(
            pos2(
                (x + 16.0).max(inner.right() - inner.width()),
                inner.center().y - 22.0,
            ),
            pos2(inner.right(), inner.center().y + 22.0),
        );
        let search = Rect::from_min_max(
            pos2(search.left().min(inner.right() - search_min), search.top()),
            search.max,
        );
        let before = self.search.clone();
        w::text_field(ui, search, "search", &mut self.search, "Search in folder…");
        if self.search != before
            && let Ok(browser) = &mut self.browser
        {
            browser.filter = self.search.clone();
        }
    }

    fn header(ui: &Ui, rect: Rect) {
        let t = w::tokens(ui);
        ui.painter()
            .hline(rect.x_range(), rect.bottom() - 0.5, t.border_stroke());
        let [name, size, date] = columns(rect.shrink2(vec2(40.0, 0.0)));
        for (r, label) in [(name, "Name"), (size, "Size"), (date, "Modified")] {
            w::text(
                ui.painter(),
                r,
                Align2::LEFT_CENTER,
                label,
                w::sans(12.0),
                t.muted_foreground,
            );
        }
    }

    fn list(&mut self, ui: &mut Ui, rect: Rect) {
        let t = w::tokens(ui);
        let now = SystemTime::now();
        let shown: Vec<(fs_ops::Entry, bool)> = {
            let selection = self.selection();
            self.shown()
                .into_iter()
                .map(|e| (e.clone(), selection.contains(&e.path)))
                .collect()
        };
        let mut clicked = None;
        let mut opened = None;
        w::region(ui, rect, |ui| {
            if shown.is_empty() {
                let message = match (&self.browser, self.search.is_empty()) {
                    (Err(e), _) => format!("Could not open this folder: {e}"),
                    (Ok(_), false) => "No files match the search".into(),
                    (Ok(_), true) => "This folder is empty".into(),
                };
                w::text(
                    ui.painter(),
                    rect,
                    Align2::CENTER_CENTER,
                    &message,
                    w::sans(15.0),
                    t.muted_foreground,
                );
                return;
            }
            egui::ScrollArea::vertical()
                .auto_shrink(false)
                .show(ui, |ui| {
                    ui.add_space(8.0);
                    for (entry, selected) in &shown {
                        let (slot, _) = ui.allocate_exact_size(
                            vec2(ui.available_width(), 52.0 + 2.0),
                            egui::Sense::hover(),
                        );
                        let row = Rect::from_min_size(
                            slot.min + vec2(16.0, 0.0),
                            vec2(slot.width() - 32.0, 52.0),
                        );
                        let response = w::row(
                            ui,
                            row,
                            ui.id().with(("file", &entry.path)),
                            *selected,
                            &entry.name,
                        );
                        let [name, size, date] = columns(row.shrink2(vec2(24.0, 0.0)));
                        let is_dir = entry.kind == Kind::Directory;
                        icons::paint(
                            ui,
                            Rect::from_min_size(
                                pos2(name.left(), name.center().y - 10.0),
                                vec2(20.0, 20.0),
                            ),
                            icons::for_file(&entry.name, is_dir),
                            t.foreground,
                        );
                        let painter = ui.painter();
                        w::text_truncated(
                            painter,
                            Rect::from_min_max(pos2(name.left() + 34.0, name.top()), name.max),
                            Align2::LEFT_CENTER,
                            &entry.name,
                            w::sans(15.0),
                            t.foreground,
                        );
                        let size_text = if is_dir {
                            self.counts
                                .get(&entry.path)
                                .map_or_else(String::new, |&n| items(n))
                        } else {
                            fs_ops::human_size(entry.size)
                        };
                        w::text_truncated(
                            painter,
                            size,
                            Align2::LEFT_CENTER,
                            &size_text,
                            w::sans(15.0),
                            t.muted_foreground,
                        );
                        let date_text = entry.modified.map(|m| when(m, now)).unwrap_or_default();
                        w::text_truncated(
                            painter,
                            date,
                            Align2::LEFT_CENTER,
                            &date_text,
                            w::sans(15.0),
                            t.muted_foreground,
                        );
                        if response.double_clicked() {
                            opened = Some((entry.path.clone(), is_dir));
                        } else if response.clicked() {
                            let extend = ui.input(|i| i.modifiers.command || i.modifiers.shift);
                            clicked = Some((entry.path.clone(), extend));
                        }
                    }
                    ui.add_space(8.0);
                });
        });
        let multiple = self.request.multiple;
        if let Some((path, extend)) = clicked
            && let Ok(browser) = &mut self.browser
        {
            browser.select(&path, extend && multiple);
            if self.saving() && !path.is_dir() {
                self.name = path
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_default();
            }
        }
        if let Some((path, is_dir)) = opened {
            if is_dir {
                self.navigate(&path);
            } else if !self.saving() {
                if let Ok(browser) = &mut self.browser {
                    browser.select(&path, false);
                }
                self.accept();
            } else {
                self.accept();
            }
        }
    }

    fn footer(&mut self, ui: &mut Ui, rect: Rect) {
        let t = w::tokens(ui);
        ui.painter()
            .hline(rect.x_range(), rect.top() + 0.5, t.border_stroke());
        let inner = rect.shrink2(vec2(24.0, 20.0));
        let buttons = 160.0 * 2.0 + 16.0;
        let select_width = 260.0_f32
            .min((inner.width() - buttons - 32.0) * 0.5)
            .max(140.0);
        let select = Rect::from_min_size(
            pos2(inner.left(), inner.center().y - 22.0),
            vec2(select_width, 44.0),
        );
        let before = self.filter;
        let filters = self.filters.clone();
        w::select(ui, select, "filter", &mut self.filter, &filters);
        if self.filter != before {
            self.error = None;
        }

        let middle = Rect::from_min_max(
            pos2(select.right() + 16.0, inner.top()),
            pos2(inner.right() - buttons - 16.0, inner.bottom()),
        );
        if self.saving() {
            let field = Rect::from_min_size(
                pos2(middle.left(), middle.center().y - 22.0),
                vec2(middle.width(), 44.0),
            );
            let response = w::text_field(ui, field, "name", &mut self.name, "File name…");
            if response.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter)) {
                self.accept();
            }
        } else {
            let status = self.error.clone().unwrap_or_else(|| {
                let shown = self.shown().len();
                let selected = self.selection().len();
                format!("{} · {selected} selected", items(shown))
            });
            ui.painter().galley(
                pos2(middle.left(), middle.center().y - 9.0),
                ui.painter().layout(
                    status,
                    w::sans(13.0),
                    if self.error.is_some() {
                        t.destructive
                    } else {
                        t.muted_foreground
                    },
                    middle.width(),
                ),
                t.muted_foreground,
            );
        }
        let cancel = Rect::from_min_size(
            pos2(inner.right() - buttons, inner.center().y - 24.0),
            vec2(160.0, 48.0),
        );
        let open = Rect::from_min_size(pos2(cancel.right() + 16.0, cancel.top()), cancel.size());
        if w::button(ui, cancel, "cancel", "Cancel", Variant::Outline, true).clicked() {
            self.reply = Some(Reply::Cancelled);
        }
        let label = self.accept_label();
        let enabled = self.can_accept();
        if w::button(ui, open, "accept", &label, Variant::Primary, enabled).clicked() {
            self.accept();
        }
    }
}

/// `n items`, with the singular for one.
fn items(n: usize) -> String {
    if n == 1 {
        "1 item".into()
    } else {
        format!("{n} items")
    }
}

/// The Name, Size and Modified columns of a row: the name takes what the
/// fixed 120 and 180 px columns leave, with 16 px gaps.
fn columns(rect: Rect) -> [Rect; 3] {
    let date = Rect::from_min_max(pos2(rect.right() - 180.0, rect.top()), rect.max);
    let size = Rect::from_min_max(
        pos2(date.left() - 16.0 - 120.0, rect.top()),
        pos2(date.left() - 16.0, rect.bottom()),
    );
    let name = Rect::from_min_max(rect.min, pos2(size.left() - 16.0, rect.bottom()));
    [name, size, date]
}

/// Breadcrumb items for `dir`: from "Home" when inside `home`, else from
/// the root. Deep paths keep the first item and the last two.
pub fn crumbs(dir: &Path, home: &Path) -> Vec<(String, PathBuf)> {
    let mut out = Vec::new();
    let (mut path, rest) = match dir.strip_prefix(home) {
        Ok(rest) => {
            out.push(("Home".to_owned(), home.to_owned()));
            (home.to_owned(), rest)
        }
        Err(_) => {
            out.push(("/".to_owned(), PathBuf::from("/")));
            (PathBuf::from("/"), dir.strip_prefix("/").unwrap_or(dir))
        }
    };
    for part in rest.components() {
        path.push(part);
        out.push((
            part.as_os_str().to_string_lossy().into_owned(),
            path.clone(),
        ));
    }
    if out.len() > 4 {
        let tail = out.split_off(out.len() - 2);
        out.truncate(1);
        let hidden = tail[0].1.parent().map(Path::to_owned).unwrap_or_default();
        out.push(("…".into(), hidden));
        out.extend(tail);
    }
    out
}

const MONTHS: [&str; 12] = [
    "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
];

/// "Today, 09:14", "Yesterday" or "Sep 30, 2026" for a modification time.
pub fn when(time: SystemTime, now: SystemTime) -> String {
    let stamp = fs_ops::timestamp(time);
    let day = |t: SystemTime| {
        t.duration_since(SystemTime::UNIX_EPOCH)
            .map_or(0, |d| d.as_secs() / 86_400)
    };
    let (then, today) = (day(time), day(now));
    if then == today {
        return format!("Today, {}", &stamp[11..16]);
    }
    if then + 1 == today {
        return "Yesterday".into();
    }
    let year = &stamp[..4];
    let month: usize = stamp[5..7].parse().unwrap_or(1);
    let dom: u32 = stamp[8..10].parse().unwrap_or(1);
    format!("{} {dom}, {year}", MONTHS[month.clamp(1, 12) - 1])
}

/// Whether `name` matches the shell glob `glob` (`*`, `?` and `[...]`
/// sets), ignoring ASCII case as file dialogs do.
pub fn glob_matches(glob: &str, name: &str) -> bool {
    fn set(pattern: &[char], c: char) -> Option<(bool, usize)> {
        // pattern starts after '['; returns (matched, length including ']').
        let mut i = 0;
        let negate = matches!(pattern.first(), Some('!' | '^'));
        if negate {
            i += 1;
        }
        let mut matched = false;
        let start = i;
        while i < pattern.len() {
            if pattern[i] == ']' && i > start {
                return Some((matched != negate, i + 1));
            }
            if i + 2 < pattern.len() && pattern[i + 1] == '-' && pattern[i + 2] != ']' {
                if (pattern[i]..=pattern[i + 2]).contains(&c) {
                    matched = true;
                }
                i += 3;
            } else {
                if pattern[i] == c {
                    matched = true;
                }
                i += 1;
            }
        }
        None
    }
    fn go(p: &[char], n: &[char]) -> bool {
        match p.first() {
            None => n.is_empty(),
            Some('*') => (0..=n.len()).any(|k| go(&p[1..], &n[k..])),
            Some('?') => !n.is_empty() && go(&p[1..], &n[1..]),
            Some('[') => match (n.first(), set(&p[1..], n.first().copied().unwrap_or('\0'))) {
                (Some(_), Some((true, len))) => go(&p[1 + len..], &n[1..]),
                (Some(_), Some((false, _))) | (None, _) => false,
                (Some(&c), None) => c == '[' && go(&p[1..], &n[1..]),
            },
            Some(&c) => n.first() == Some(&c) && go(&p[1..], &n[1..]),
        }
    }
    let p: Vec<char> = glob.to_lowercase().chars().collect();
    let n: Vec<char> = name.to_lowercase().chars().collect();
    go(&p, &n)
}

/// Whether `name` looks like MIME type `mime` (`image/png`, `image/*`),
/// going by its extension. Unknown types match nothing.
pub fn mime_matches(mime: &str, name: &str) -> bool {
    const TABLE: &[(&str, &[&str])] = &[
        ("application/pdf", &["pdf"]),
        ("application/json", &["json"]),
        ("application/zip", &["zip"]),
        ("application/vnd.oasis.opendocument.text", &["odt"]),
        ("application/vnd.oasis.opendocument.spreadsheet", &["ods"]),
        ("application/vnd.oasis.opendocument.presentation", &["odp"]),
        (
            "application/vnd.openxmlformats-officedocument.wordprocessingml.document",
            &["docx"],
        ),
        (
            "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet",
            &["xlsx"],
        ),
        ("application/msword", &["doc"]),
        ("image/png", &["png"]),
        ("image/jpeg", &["jpg", "jpeg"]),
        ("image/gif", &["gif"]),
        ("image/webp", &["webp"]),
        ("image/svg+xml", &["svg"]),
        ("image/bmp", &["bmp"]),
        ("image/avif", &["avif"]),
        ("video/mp4", &["mp4"]),
        ("video/webm", &["webm"]),
        ("video/x-matroska", &["mkv"]),
        ("audio/mpeg", &["mp3"]),
        ("audio/ogg", &["ogg", "oga"]),
        ("audio/flac", &["flac"]),
        ("text/plain", &["txt", "text", "log"]),
        ("text/markdown", &["md", "markdown"]),
        ("text/html", &["html", "htm"]),
        ("text/csv", &["csv"]),
        ("text/x-rust", &["rs"]),
    ];
    let Some(ext) = name.rsplit_once('.').map(|(_, e)| e.to_ascii_lowercase()) else {
        return false;
    };
    let mime = mime.to_ascii_lowercase();
    TABLE.iter().any(|(m, exts)| {
        let fits = match mime.strip_suffix("/*") {
            Some(group) => m.split('/').next() == Some(group),
            None => *m == mime,
        };
        fits && exts.contains(&ext.as_str())
    })
}

impl crate::Dialog for FileChooser {
    fn reply(&self) -> Option<&Reply> {
        self.reply.as_ref()
    }

    fn size(&self) -> [f32; 2] {
        [1040.0, 636.0]
    }
}

impl App for FileChooser {
    fn title(&self) -> &str {
        &self.request.title
    }

    fn ui(&mut self, ui: &mut Ui, _theme: &Theme) {
        if crate::escape(ui) {
            self.reply = Some(Reply::Cancelled);
            return;
        }
        if ui.input_mut(|i| i.consume_key(Modifiers::ALT, Key::ArrowLeft))
            && let Ok(browser) = &mut self.browser
        {
            let _ = browser.go_back();
            self.count_folders();
        }
        if ui.input_mut(|i| i.consume_key(Modifiers::NONE, Key::Enter)) && self.can_accept() {
            self.accept();
        }
        let rect = ui.max_rect();
        let sidebar_width = 240.0_f32.min(rect.width() * 0.3);
        let (sidebar, main) = rect.split_left_right_at_x(rect.left() + sidebar_width);
        self.sidebar(ui, sidebar);
        let toolbar = Rect::from_min_size(main.min, vec2(main.width(), 76.0));
        let header = Rect::from_min_size(toolbar.left_bottom(), vec2(main.width(), 36.0));
        let footer = Rect::from_min_max(pos2(main.left(), main.bottom() - 88.0), main.max);
        let list = Rect::from_min_max(header.left_bottom(), footer.right_top());
        self.toolbar(ui, toolbar);
        Self::header(ui, header);
        self.list(ui, list);
        self.footer(ui, footer);
    }
}
