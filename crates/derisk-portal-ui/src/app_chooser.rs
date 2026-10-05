//! 1e Open With (`org.freedesktop.portal.OpenURI` through
//! `org.freedesktop.impl.portal.AppChooser`): a grid of apps that can open
//! the file, a search field, "Always use", and an Open button naming the
//! chosen app.

use egui::{Align2, Rect, Ui, pos2, vec2};
use mcsapi_ui::egui;
use mcsapi_ui::{App, Theme};
use serde::{Deserialize, Serialize};

use crate::{
    Reply, Update,
    icons::{self, Icon},
    widgets as w,
};

/// An app that can open the file.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct AppEntry {
    /// Desktop file ID without `.desktop`, as the portal names apps.
    pub id: String,
    /// Display name.
    pub name: String,
    /// One line under the name ("Default", "Web browser").
    #[serde(default)]
    pub detail: String,
    /// The app's `Icon` key.
    #[serde(default)]
    pub icon: String,
}

impl AppEntry {
    /// Looks up `id`'s desktop entry for its name, description and icon.
    pub fn from_desktop(id: &str, entries: &[derisk::desktop::DesktopEntry]) -> Self {
        let desktop_id = format!("{id}.desktop");
        match entries.iter().find(|e| e.id == desktop_id) {
            Some(e) => Self {
                id: id.to_owned(),
                name: e.name.clone(),
                detail: if e.generic_name.is_empty() {
                    e.comment.clone()
                } else {
                    e.generic_name.clone()
                },
                icon: e.icon.clone(),
            },
            None => Self {
                id: id.to_owned(),
                name: id.rsplit('.').next().unwrap_or(id).to_owned(),
                detail: String::new(),
                icon: String::new(),
            },
        }
    }
}

/// What is being opened and by which apps.
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
pub struct Request {
    /// File name (or link) being opened.
    pub file_name: String,
    /// One muted line: type, size and who asked.
    #[serde(default)]
    pub detail: String,
    /// The content type, for example `application/pdf`.
    #[serde(default)]
    pub content_type: String,
    /// The type's plural name for "Always use for …", for example "PDF
    /// documents". Empty hides the checkbox.
    #[serde(default)]
    pub type_name: String,
    /// Apps to choose from.
    pub choices: Vec<AppEntry>,
    /// The app chosen last time, selected at first.
    #[serde(default)]
    pub last_choice: Option<String>,
}

impl Request {
    /// The design's placeholder content.
    pub fn sample() -> Self {
        let app = |id: &str, name: &str, detail: &str, icon: &str| AppEntry {
            id: id.into(),
            name: name.into(),
            detail: detail.into(),
            icon: icon.into(),
        };
        Self {
            file_name: "Q3 report.pdf".into(),
            detail: "PDF document · 2.4 MB · requested by Files".into(),
            content_type: "application/pdf".into(),
            type_name: "PDF documents".into(),
            choices: vec![
                app(
                    "org.gnome.Evince",
                    "Document Viewer",
                    "Default",
                    "folder-documents",
                ),
                app("org.mozilla.firefox", "Firefox", "Web browser", ""),
                app(
                    "org.derisk.editor",
                    "Text Editor",
                    "derisk",
                    "accessories-text-editor",
                ),
                app("org.derisk.files", "Files", "derisk", "system-file-manager"),
                app(
                    "org.libreoffice.LibreOffice.draw",
                    "Draw",
                    "LibreOffice",
                    "",
                ),
                app("org.gimp.GIMP", "Image Editor", "GIMP", "folder-pictures"),
            ],
            last_choice: None,
        }
    }
}

/// The chosen app.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct Choice {
    /// Its ID, from [`AppEntry::id`].
    pub app_id: String,
    /// Make it the default for the content type.
    pub always: bool,
}

/// The Open With dialog.
pub struct AppChooser {
    request: Request,
    heading: String,
    search: String,
    selected: Option<String>,
    always: bool,
    reply: Option<Reply>,
}

impl AppChooser {
    /// Opens with the last choice, else the first app, selected.
    pub fn new(request: Request) -> Self {
        let selected = request
            .last_choice
            .clone()
            .filter(|id| request.choices.iter().any(|a| a.id == *id))
            .or_else(|| request.choices.first().map(|a| a.id.clone()));
        Self {
            heading: format!("Open “{}” with…", request.file_name),
            request,
            search: String::new(),
            selected,
            always: false,
            reply: None,
        }
    }

    fn shown(&self) -> Vec<AppEntry> {
        let query = self.search.trim().to_lowercase();
        self.request
            .choices
            .iter()
            .filter(|a| {
                query.is_empty()
                    || a.name.to_lowercase().contains(&query)
                    || a.detail.to_lowercase().contains(&query)
            })
            .cloned()
            .collect()
    }

    fn open(&mut self) {
        if let Some(app_id) = self.selected.clone() {
            self.reply = Some(Reply::App(Choice {
                app_id,
                always: self.always,
            }));
        }
    }

    fn grid(&mut self, ui: &mut Ui, rect: Rect) {
        let t = w::tokens(ui);
        let apps = self.shown();
        if apps.is_empty() {
            w::text(
                ui.painter(),
                rect,
                Align2::CENTER_CENTER,
                "No apps match the search",
                w::sans(15.0),
                t.muted_foreground,
            );
            return;
        }
        let (gap, cols) = (12.0, 3);
        let rows = apps.len().div_ceil(cols);
        let cell_w = (rect.width() - gap * (cols - 1) as f32) / cols as f32;
        let cell_h = ((rect.height() - gap * (rows - 1) as f32) / rows as f32).max(72.0);
        let total = rows as f32 * cell_h + gap * (rows - 1) as f32;
        let mut clicked = None;
        w::region(ui, rect, |ui| {
            egui::ScrollArea::vertical()
                .auto_shrink(false)
                .show(ui, |ui| {
                    let (area, _) =
                        ui.allocate_exact_size(vec2(rect.width(), total), egui::Sense::hover());
                    for (index, app) in apps.iter().enumerate() {
                        let cell = Rect::from_min_size(
                            pos2(
                                area.left() + (index % cols) as f32 * (cell_w + gap),
                                area.top() + (index / cols) as f32 * (cell_h + gap),
                            ),
                            vec2(cell_w, cell_h),
                        );
                        let on = self.selected.as_deref() == Some(app.id.as_str());
                        let fill = if on { w::selected_fill(&t) } else { t.card };
                        let response = w::tile(
                            ui,
                            cell,
                            ui.id().with(("app", &app.id)),
                            on,
                            fill,
                            &app.name,
                        );
                        let tile = Rect::from_min_size(
                            pos2(cell.left() + 20.0, cell.center().y - 22.0),
                            vec2(44.0, 44.0),
                        );
                        ui.painter().rect_filled(tile, 10, t.muted);
                        let icon = if app.icon.is_empty() {
                            icons::APP
                        } else {
                            Icon::App(&app.icon, "⊞")
                        };
                        icons::paint(
                            ui,
                            Rect::from_center_size(tile.center(), vec2(24.0, 24.0)),
                            icon,
                            t.foreground,
                        );
                        let text_left = tile.right() + 16.0;
                        let width = (cell.right() - 20.0 - text_left).max(0.0);
                        let lines = if app.detail.is_empty() {
                            18.0
                        } else {
                            18.0 + 4.0 + 16.0
                        };
                        let top = cell.center().y - lines / 2.0;
                        w::text_truncated(
                            ui.painter(),
                            Rect::from_min_size(pos2(text_left, top), vec2(width, 18.0)),
                            Align2::LEFT_CENTER,
                            &app.name,
                            w::sans(15.0),
                            t.foreground,
                        );
                        if !app.detail.is_empty() {
                            w::text_truncated(
                                ui.painter(),
                                Rect::from_min_size(pos2(text_left, top + 22.0), vec2(width, 16.0)),
                                Align2::LEFT_CENTER,
                                &app.detail,
                                w::sans(13.0),
                                t.muted_foreground,
                            );
                        }
                        if response.double_clicked() {
                            clicked = Some((app.id.clone(), true));
                        } else if response.clicked() {
                            clicked = Some((app.id.clone(), false));
                        }
                    }
                });
        });
        if let Some((id, open)) = clicked {
            self.selected = Some(id);
            if open {
                self.open();
            }
        }
    }
}

impl crate::Dialog for AppChooser {
    fn reply(&self) -> Option<&Reply> {
        self.reply.as_ref()
    }

    fn size(&self) -> [f32; 2] {
        [880.0, 556.0]
    }

    fn update(&mut self, update: Update) {
        let Update::Choices { choices } = update;
        if self
            .selected
            .as_ref()
            .is_none_or(|id| !choices.iter().any(|a| a.id == *id))
        {
            self.selected = choices.first().map(|a| a.id.clone());
        }
        self.request.choices = choices;
    }
}

impl App for AppChooser {
    fn title(&self) -> &str {
        "Open With"
    }

    fn ui(&mut self, ui: &mut Ui, _theme: &Theme) {
        if crate::escape(ui) {
            self.reply = Some(Reply::Cancelled);
            return;
        }
        if ui.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Enter)) {
            self.open();
            return;
        }
        let rect = ui.max_rect();
        let footer = Rect::from_min_max(pos2(rect.left(), rect.bottom() - 76.0), rect.max);
        let body = Rect::from_min_max(rect.min, footer.right_top()).shrink(32.0);

        // Heading on the left, the search field bottom-aligned on the right;
        // they stack when the window is too narrow for both.
        let search_w = 280.0;
        let side_by_side = body.width() >= 260.0 + 24.0 + search_w;
        let title_w = if side_by_side {
            body.width() - search_w - 24.0
        } else {
            body.width()
        };
        let detail = self.request.detail.clone();
        let heading = self.heading.clone();
        let title_h = w::title_block_height(ui, title_w, &heading, 24.0, 8.0, &detail);
        let header_h = if side_by_side {
            title_h.max(w::CONTROL)
        } else {
            title_h + 16.0 + w::CONTROL
        };
        let title_top = body.top()
            + if side_by_side {
                header_h - title_h
            } else {
                0.0
            };
        w::title_block(
            ui,
            pos2(body.left(), title_top),
            title_w,
            &heading,
            24.0,
            8.0,
            &detail,
        );
        let search = if side_by_side {
            Rect::from_min_size(
                pos2(body.right() - search_w, body.top() + header_h - w::CONTROL),
                vec2(search_w, w::CONTROL),
            )
        } else {
            Rect::from_min_size(
                pos2(body.left(), body.top() + header_h - w::CONTROL),
                vec2(body.width(), w::CONTROL),
            )
        };
        w::text_field(ui, search, "search", &mut self.search, "Search apps…");

        let show_always = !self.request.type_name.is_empty();
        let grid_bottom = if show_always {
            body.bottom() - w::CONTROL - 24.0
        } else {
            body.bottom()
        };
        self.grid(
            ui,
            Rect::from_min_max(
                pos2(body.left(), body.top() + header_h + 24.0),
                pos2(body.right(), grid_bottom),
            ),
        );
        if show_always {
            let label = format!("Always use for {}", self.request.type_name);
            w::checkbox(
                ui,
                pos2(body.left(), body.bottom() - w::CONTROL / 2.0),
                "always",
                &mut self.always,
                &label,
            );
        }

        let name = self
            .selected
            .as_ref()
            .and_then(|id| self.request.choices.iter().find(|a| a.id == *id))
            .map(|a| a.name.clone());
        let open_label = name
            .as_ref()
            .map_or_else(|| "Open".to_owned(), |n| format!("Open in {n}"));
        let buttons = Rect::from_min_size(
            pos2(footer.left() + 32.0, footer.top()),
            vec2(footer.width() - 64.0, w::BUTTON),
        );
        let (cancel, open) =
            w::button_pair(ui, buttons, 16.0, "Cancel", &open_label, name.is_some());
        if cancel {
            self.reply = Some(Reply::Cancelled);
        }
        if open {
            self.open();
        }
    }
}
