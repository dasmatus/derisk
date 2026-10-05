//! 1b Screen Share (`org.freedesktop.portal.ScreenCast`): "Entire screen"
//! and "Window" tabs over a grid of sources, switches for the pointer and
//! remote control, and Cancel / Share.

use egui::{Align2, Color32, Rect, Ui, pos2, vec2};
use mcsapi_ui::egui;
use mcsapi_ui::{App, Theme};
use serde::{Deserialize, Serialize};

use crate::{
    Reply,
    icons::{self, Icon},
    widgets as w,
};

/// A screen or window that can be shared.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct Source {
    /// Stable ID the backend understands (an output name or window ID).
    pub id: String,
    /// Display name, for example "Built-in display" or a window title.
    pub name: String,
    /// One line under the name, for example "eDP-1 · 1920×1200".
    pub detail: String,
    /// For windows, the app's `Icon` key; empty for screens.
    #[serde(default)]
    pub icon: String,
}

/// What the app asked for and what can be shared.
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
pub struct Request {
    /// The requesting app's name.
    pub app_name: String,
    /// One muted line under the heading.
    #[serde(default)]
    pub detail: String,
    /// Screens on offer; empty hides the tab.
    #[serde(default)]
    pub screens: Vec<Source>,
    /// Windows on offer; empty hides the tab.
    #[serde(default)]
    pub windows: Vec<Source>,
    /// Allow picking several sources.
    #[serde(default)]
    pub multiple: bool,
    /// Offer the pointer switch.
    #[serde(default = "yes")]
    pub pointer: bool,
    /// Offer the remote control switch (a remote desktop session).
    #[serde(default)]
    pub remote_control: bool,
}

fn yes() -> bool {
    true
}

impl Request {
    /// The design's placeholder content.
    pub fn sample() -> Self {
        let source = |id: &str, name: &str, detail: &str, icon: &str| Source {
            id: id.into(),
            name: name.into(),
            detail: detail.into(),
            icon: icon.into(),
        };
        Self {
            app_name: "Firefox".into(),
            detail: "meet.example.org · choose what others in the call will see".into(),
            screens: vec![
                source("eDP-1", "Built-in display", "eDP-1 · 1920×1200", ""),
                source("DP-1", "External monitor", "DP-1 · 2560×1440", ""),
            ],
            windows: vec![
                source(
                    "1",
                    "Files — Documents",
                    "Workspace 1",
                    "system-file-manager",
                ),
                source(
                    "2",
                    "notes.md — Text Editor",
                    "Workspace 1",
                    "accessories-text-editor",
                ),
                source("3", "Terminal", "Workspace 2", "utilities-terminal"),
                source(
                    "4",
                    "System Monitor",
                    "Workspace 3",
                    "utilities-system-monitor",
                ),
            ],
            multiple: false,
            pointer: true,
            remote_control: true,
        }
    }
}

/// Whether a source is a screen or a window.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Kind {
    /// A whole output.
    Screen,
    /// One window.
    Window,
}

/// What the person chose to share.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct Choice {
    /// Screens or windows.
    pub kind: Kind,
    /// IDs of the chosen sources.
    pub sources: Vec<String>,
    /// Show the pointer in the stream.
    pub show_pointer: bool,
    /// Let the other side control the pointer and keyboard.
    pub remote_control: bool,
}

/// The Screen Share dialog.
pub struct ScreenCast {
    request: Request,
    title: String,
    tab: Kind,
    picked: Vec<usize>,
    show_pointer: bool,
    remote_control: bool,
    reply: Option<Reply>,
}

impl ScreenCast {
    /// Opens on the screens tab, or on windows when no screen is offered.
    pub fn new(request: Request) -> Self {
        let tab = if request.screens.is_empty() && !request.windows.is_empty() {
            Kind::Window
        } else {
            Kind::Screen
        };
        Self {
            title: format!("{} wants to share your screen", request.app_name),
            request,
            tab,
            picked: vec![0],
            show_pointer: true,
            remote_control: false,
            reply: None,
        }
    }

    fn sources(&self) -> &[Source] {
        match self.tab {
            Kind::Screen => &self.request.screens,
            Kind::Window => &self.request.windows,
        }
    }

    fn share(&mut self) {
        let sources: Vec<String> = self
            .picked
            .iter()
            .filter_map(|&i| self.sources().get(i))
            .map(|s| s.id.clone())
            .collect();
        if sources.is_empty() {
            return;
        }
        self.reply = Some(Reply::ScreenCast(Choice {
            kind: self.tab,
            sources,
            show_pointer: self.show_pointer && self.request.pointer,
            remote_control: self.remote_control && self.request.remote_control,
        }));
    }

    fn tabs(&mut self, ui: &mut Ui, rect: Rect) {
        let t = w::tokens(ui);
        ui.painter().rect_filled(rect, w::ROW_RADIUS, t.muted);
        let inner = rect.shrink(4.0);
        let tabs = [(Kind::Screen, "Entire screen"), (Kind::Window, "Window")];
        let width = (inner.width() - 4.0) / 2.0;
        for (index, (kind, label)) in tabs.into_iter().enumerate() {
            let r = Rect::from_min_size(
                pos2(inner.left() + index as f32 * (width + 4.0), inner.top()),
                vec2(width, inner.height()),
            );
            let on = self.tab == kind;
            let response = ui.interact(r, ui.id().with(("tab", index)), egui::Sense::click());
            response.widget_info(|| {
                egui::WidgetInfo::selected(egui::WidgetType::SelectableLabel, true, on, label)
            });
            if on {
                ui.painter().rect_filled(r, 6, t.background);
            }
            w::text(
                ui.painter(),
                r,
                Align2::CENTER_CENTER,
                label,
                w::sans(15.0),
                if on { t.foreground } else { t.muted_foreground },
            );
            if response.clicked() && !on {
                self.tab = kind;
                self.picked = vec![0];
            }
        }
    }

    fn grid(&mut self, ui: &mut Ui, rect: Rect) {
        let t = w::tokens(ui);
        let count = self.sources().len();
        if count == 0 {
            w::text(
                ui.painter(),
                rect,
                Align2::CENTER_CENTER,
                "Nothing to share",
                w::sans(15.0),
                t.muted_foreground,
            );
            return;
        }
        let gap = 16.0;
        // `repeat(auto-fill, minmax(240px, 1fr))` with rows sharing the height.
        let cols = (((rect.width() + gap) / (240.0 + gap)).floor() as usize).max(1);
        let rows = count.div_ceil(cols);
        let cell_w = (rect.width() - gap * (cols - 1) as f32) / cols as f32;
        let min_h = 100.0 + 66.0;
        let cell_h = ((rect.height() - gap * (rows - 1) as f32) / rows as f32).max(min_h);
        let total_h = rows as f32 * cell_h + gap * (rows - 1) as f32;
        let tab = self.tab;
        let sources = self.sources().to_vec();
        let mut clicked = None;
        w::region(ui, rect, |ui| {
            egui::ScrollArea::vertical()
                .auto_shrink(false)
                .show(ui, |ui| {
                    let (area, _) =
                        ui.allocate_exact_size(vec2(rect.width(), total_h), egui::Sense::hover());
                    for (index, source) in sources.iter().enumerate() {
                        let (row, col) = (index / cols, index % cols);
                        let cell = Rect::from_min_size(
                            pos2(
                                area.left() + col as f32 * (cell_w + gap),
                                area.top() + row as f32 * (cell_h + gap),
                            ),
                            vec2(cell_w, cell_h),
                        );
                        let selected = self.picked.contains(&index);
                        let response = w::tile(
                            ui,
                            cell,
                            ui.id().with(("source", index)),
                            selected,
                            t.card,
                            &source.name,
                        );
                        let inner = cell.shrink(2.0);
                        let preview = Rect::from_min_max(
                            inner.min,
                            pos2(inner.right(), inner.bottom() - 66.0),
                        );
                        w::wallpaper(ui, preview, 0);
                        let icon = match tab {
                            Kind::Screen => icons::COMPUTER,
                            Kind::Window if source.icon.is_empty() => icons::APP,
                            Kind::Window => Icon::App(&source.icon, "⊞"),
                        };
                        icons::paint(
                            ui,
                            Rect::from_center_size(preview.center(), vec2(40.0, 40.0)),
                            icon,
                            Color32::from_rgb(0xf8, 0xfa, 0xfc),
                        );
                        let text_left = inner.left() + 16.0;
                        let width = inner.width() - 32.0;
                        w::text_truncated(
                            ui.painter(),
                            Rect::from_min_size(
                                pos2(text_left, preview.bottom() + 14.0),
                                vec2(width, 18.0),
                            ),
                            Align2::LEFT_CENTER,
                            &source.name,
                            w::sans(15.0),
                            t.foreground,
                        );
                        w::text_truncated(
                            ui.painter(),
                            Rect::from_min_size(
                                pos2(text_left, preview.bottom() + 36.0),
                                vec2(width, 16.0),
                            ),
                            Align2::LEFT_CENTER,
                            &source.detail,
                            w::sans(13.0),
                            t.muted_foreground,
                        );
                        if response.double_clicked() {
                            clicked = Some((index, true));
                        } else if response.clicked() {
                            clicked = Some((index, false));
                        }
                    }
                });
        });
        if let Some((index, confirm)) = clicked {
            let extend = self.request.multiple && ui.input(|i| i.modifiers.command);
            if extend {
                if let Some(at) = self.picked.iter().position(|&i| i == index) {
                    self.picked.remove(at);
                } else {
                    self.picked.push(index);
                }
            } else {
                self.picked = vec![index];
            }
            if confirm {
                self.share();
            }
        }
    }

    fn switch_card(ui: &mut Ui, rect: Rect, id: &str, on: &mut bool, label: &str) {
        let t = w::tokens(ui);
        w::card(ui.painter(), rect, &t);
        w::switch(
            ui,
            pos2(rect.left() + 20.0, rect.center().y),
            id,
            on,
            Some(label),
            true,
        );
    }
}

impl crate::Dialog for ScreenCast {
    fn reply(&self) -> Option<&Reply> {
        self.reply.as_ref()
    }

    fn size(&self) -> [f32; 2] {
        [880.0, 636.0]
    }
}

impl App for ScreenCast {
    fn title(&self) -> &str {
        "Share Screen"
    }

    fn ui(&mut self, ui: &mut Ui, _theme: &Theme) {
        if crate::escape(ui) {
            self.reply = Some(Reply::Cancelled);
            return;
        }
        if ui.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Enter)) {
            self.share();
        }
        let rect = ui.max_rect();
        let footer = Rect::from_min_max(pos2(rect.left(), rect.bottom() - 96.0), rect.max);
        let body = Rect::from_min_max(rect.min, footer.right_top()).shrink(32.0);
        let mut y = body.top();
        y += w::title_block(
            ui,
            pos2(body.left(), y),
            body.width(),
            &self.title,
            24.0,
            8.0,
            &self.request.detail,
        );
        y += 24.0;
        if !self.request.screens.is_empty() && !self.request.windows.is_empty() {
            self.tabs(
                ui,
                Rect::from_min_size(pos2(body.left(), y), vec2(body.width(), 52.0)),
            );
            y += 52.0 + 24.0;
        }
        let switches = self.request.pointer || self.request.remote_control;
        let switches_top = if switches {
            body.bottom() - 60.0
        } else {
            body.bottom() + 24.0
        };
        self.grid(
            ui,
            Rect::from_min_max(
                pos2(body.left(), y),
                pos2(body.right(), switches_top - 24.0),
            ),
        );
        if switches {
            let row = Rect::from_min_max(pos2(body.left(), switches_top), body.max);
            let both = self.request.pointer && self.request.remote_control;
            let half = if both {
                (row.width() - 16.0) / 2.0
            } else {
                row.width()
            };
            let first = Rect::from_min_size(row.min, vec2(half, row.height()));
            if self.request.pointer {
                Self::switch_card(ui, first, "pointer", &mut self.show_pointer, "Show pointer");
            }
            if self.request.remote_control {
                let r = if both {
                    Rect::from_min_size(pos2(row.right() - half, row.top()), first.size())
                } else {
                    first
                };
                Self::switch_card(
                    ui,
                    r,
                    "remote",
                    &mut self.remote_control,
                    "Allow remote control",
                );
            }
        }
        let buttons = Rect::from_min_size(
            pos2(footer.left() + 32.0, footer.top() + 20.0),
            vec2(footer.width() - 64.0, w::BUTTON),
        );
        let (cancel, share) = w::button_pair(
            ui,
            buttons,
            16.0,
            "Cancel",
            "Share",
            !self.picked.is_empty(),
        );
        if cancel {
            self.reply = Some(Reply::Cancelled);
        }
        if share {
            self.share();
        }
    }
}
