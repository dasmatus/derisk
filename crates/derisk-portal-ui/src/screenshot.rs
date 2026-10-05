//! 1c Screenshot (`org.freedesktop.portal.Screenshot`): a large preview with
//! the capture outlined, the mode (screen, window, area), an include-pointer
//! switch, a delay slider, and Cancel / Capture.

use std::path::PathBuf;

use egui::{Align2, Pos2, Rect, Shape, Stroke, TextureHandle, Ui, pos2, vec2};
use mcsapi_ui::egui;
use mcsapi_ui::{App, Theme};
use serde::{Deserialize, Serialize};

use crate::{Reply, widgets as w};

/// A rectangle in fractions of the screen: left, top, right, bottom.
pub type Area = [f32; 4];

/// The whole screen.
pub const FULL: Area = [0.0, 0.0, 1.0, 1.0];
/// Where a window is drawn when the shell doesn't say.
pub const SAMPLE_WINDOW: Area = [0.10, 0.12, 0.70, 0.82];
/// The area selected when the dialog opens.
pub const SAMPLE_AREA: Area = [0.24, 0.22, 0.64, 0.74];

/// What the app asked for and what the screen looks like.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct Request {
    /// The requesting app's name.
    pub app_name: String,
    /// The heading; "<app> wants a screenshot" when empty.
    #[serde(default)]
    pub heading: String,
    /// A PNG of the screen as it was when the request came in.
    #[serde(default)]
    pub preview: Option<PathBuf>,
    /// The screen's size in pixels.
    #[serde(default = "default_screen")]
    pub screen: [u32; 2],
    /// The focused window, if the shell said where it is.
    #[serde(default)]
    pub window: Option<Area>,
    /// Whether the capture can include the pointer.
    #[serde(default)]
    pub pointer: bool,
}

fn default_screen() -> [u32; 2] {
    [2560, 1440]
}

impl Default for Request {
    fn default() -> Self {
        Self {
            app_name: String::new(),
            heading: String::new(),
            preview: None,
            screen: default_screen(),
            window: None,
            pointer: true,
        }
    }
}

/// What to capture.
#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Mode {
    /// The whole screen.
    Screen,
    /// The focused window.
    Window,
    /// A rectangle the person drew.
    #[default]
    Area,
}

/// What the person chose.
#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Serialize)]
pub struct Choice {
    /// The mode.
    pub mode: Mode,
    /// The part of the screen to keep, in fractions.
    pub area: Area,
    /// Include the pointer.
    pub include_pointer: bool,
    /// Seconds to wait before capturing.
    pub delay: u32,
}

/// The Screenshot dialog.
pub struct Screenshot {
    request: Request,
    heading: String,
    mode: Mode,
    area: Area,
    drag_from: Option<Pos2>,
    include_pointer: bool,
    delay: f32,
    preview: Option<Result<TextureHandle, ()>>,
    reply: Option<Reply>,
}

const MODES: [(Mode, &str, &str); 3] = [
    (Mode::Screen, "Entire screen", "S"),
    (Mode::Window, "Window", "W"),
    (Mode::Area, "Selected area", "A"),
];

impl Screenshot {
    /// Opens with an area selected.
    pub fn new(request: Request) -> Self {
        let heading = if request.heading.is_empty() {
            format!("{} wants a screenshot", request.app_name)
        } else {
            request.heading.clone()
        };
        Self {
            heading,
            request,
            mode: Mode::Area,
            area: SAMPLE_AREA,
            drag_from: None,
            include_pointer: false,
            delay: 0.0,
            preview: None,
            reply: None,
        }
    }

    /// The outlined part of the screen in the current mode.
    pub fn selection(&self) -> Area {
        match self.mode {
            Mode::Screen => FULL,
            Mode::Window => self.request.window.unwrap_or(SAMPLE_WINDOW),
            Mode::Area => self.area,
        }
    }

    fn capture(&mut self) {
        self.reply = Some(Reply::Screenshot(Choice {
            mode: self.mode,
            area: self.selection(),
            include_pointer: self.include_pointer && self.request.pointer,
            delay: self.delay.round() as u32,
        }));
    }

    fn texture(&mut self, ui: &Ui) -> Option<TextureHandle> {
        let path = self.request.preview.as_ref()?;
        let loaded = self.preview.get_or_insert_with(|| {
            let image = image::open(path).map_err(|_| ())?.into_rgba8();
            let size = [image.width() as usize, image.height() as usize];
            let image = egui::ColorImage::from_rgba_unmultiplied(size, image.as_raw());
            Ok(ui
                .ctx()
                .load_texture("screenshot-preview", image, egui::TextureOptions::LINEAR))
        });
        loaded.as_ref().ok().cloned()
    }

    fn preview(&mut self, ui: &mut Ui, pane: Rect) {
        let t = w::tokens(ui);
        let texture = self.texture(ui);
        // A real capture keeps the screen's shape; the placeholder fills.
        let screen = if texture.is_some() {
            let [sw, sh] = self.request.screen.map(|v| v.max(1) as f32);
            let scale = (pane.width() / sw).min(pane.height() / sh);
            Rect::from_center_size(pane.center(), vec2(sw * scale, sh * scale))
        } else {
            pane
        };
        if let Some(texture) = &texture {
            ui.painter().rect_filled(pane, w::CARD_RADIUS, t.muted);
            ui.painter().image(
                texture.id(),
                screen,
                Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)),
                egui::Color32::WHITE,
            );
        } else {
            w::wallpaper(ui, pane, w::CARD_RADIUS);
        }
        ui.painter().rect_stroke(
            pane,
            w::CARD_RADIUS,
            t.border_stroke(),
            egui::StrokeKind::Inside,
        );

        // Drawing a rectangle on the preview selects that area.
        let response = ui.interact(
            screen,
            ui.id().with("preview"),
            egui::Sense::click_and_drag(),
        );
        let to_fraction = |p: Pos2| {
            [
                ((p.x - screen.left()) / screen.width()).clamp(0.0, 1.0),
                ((p.y - screen.top()) / screen.height()).clamp(0.0, 1.0),
            ]
        };
        if response.drag_started() {
            self.drag_from = response.interact_pointer_pos();
        }
        if let (Some(from), Some(to)) = (self.drag_from, response.interact_pointer_pos())
            && response.dragged()
        {
            let ([x0, y0], [x1, y1]) = (to_fraction(from), to_fraction(to));
            if (x1 - x0).abs() > 0.01 && (y1 - y0).abs() > 0.01 {
                self.mode = Mode::Area;
                self.area = [x0.min(x1), y0.min(y1), x0.max(x1), y0.max(y1)];
            }
        }
        if response.drag_stopped() {
            self.drag_from = None;
        }

        let [l, top, r, b] = self.selection();
        let sel = Rect::from_min_max(
            pos2(
                screen.left() + l * screen.width(),
                screen.top() + top * screen.height(),
            ),
            pos2(
                screen.left() + r * screen.width(),
                screen.top() + b * screen.height(),
            ),
        )
        .shrink(1.0);
        ui.painter()
            .rect_filled(sel, w::ROW_RADIUS, w::preview_fill(&t));
        let corners = [
            sel.left_top(),
            sel.right_top(),
            sel.right_bottom(),
            sel.left_bottom(),
            sel.left_top(),
        ];
        ui.painter().extend(Shape::dashed_line(
            &corners,
            Stroke::new(2.0, t.primary),
            6.0,
            4.0,
        ));

        let [sw, sh] = self.request.screen;
        let size = format!(
            "{}×{}",
            ((r - l) * sw as f32).round() as u32,
            ((b - top) * sh as f32).round() as u32
        );
        let g = w::galley(ui.painter(), &size, w::mono(12.0), t.muted_foreground);
        let chip = Rect::from_min_size(
            pos2(
                pane.left() + 16.0,
                pane.bottom() - 16.0 - (g.size().y + 12.0),
            ),
            g.size() + vec2(20.0, 12.0),
        );
        ui.painter().rect_filled(chip, 6, t.background);
        ui.painter()
            .galley(chip.min + vec2(10.0, 6.0), g, t.muted_foreground);
    }

    fn controls(&mut self, ui: &mut Ui, column: Rect) {
        let t = w::tokens(ui);
        let mut y = column.top();
        y += w::title_block(
            ui,
            pos2(column.left(), y),
            column.width(),
            &self.heading,
            22.0,
            6.0,
            "Choose what to capture. Nothing is sent until you confirm.",
        );
        y += 24.0;
        for (index, (mode, label, key)) in MODES.into_iter().enumerate() {
            let row = Rect::from_min_size(pos2(column.left(), y), vec2(column.width(), 52.0));
            let on = self.mode == mode;
            let response = w::row(ui, row, ui.id().with(("mode", index)), on, label);
            ui.painter().rect_stroke(
                row,
                w::ROW_RADIUS,
                Stroke::new(1.0, if on { t.primary } else { t.border }),
                egui::StrokeKind::Inside,
            );
            w::text(
                ui.painter(),
                row.shrink2(vec2(18.0, 0.0)),
                Align2::LEFT_CENTER,
                label,
                w::sans(15.0),
                t.foreground,
            );
            w::kbd(ui.painter(), row.shrink2(vec2(18.0, 0.0)), key, &t);
            if response.clicked() {
                self.mode = mode;
            }
            y += 52.0 + 8.0;
        }
        y += 24.0 - 8.0;
        w::switch(
            ui,
            pos2(column.left(), y + 22.0),
            "pointer",
            &mut self.include_pointer,
            Some("Include pointer"),
            self.request.pointer,
        );
        y += 44.0 + 24.0;
        let label = Rect::from_min_size(pos2(column.left(), y), vec2(column.width(), 15.0));
        w::text(
            ui.painter(),
            label,
            Align2::LEFT_CENTER,
            "Delay",
            w::sans(15.0),
            t.foreground,
        );
        let value = if self.delay < 0.5 {
            "None".to_owned()
        } else {
            format!("{} s", self.delay.round())
        };
        w::text(
            ui.painter(),
            label,
            Align2::RIGHT_CENTER,
            &value,
            w::mono(15.0),
            t.muted_foreground,
        );
        y += 15.0 + 8.0;
        w::slider(
            ui,
            Rect::from_min_size(pos2(column.left(), y), vec2(column.width(), 44.0)),
            "delay",
            &mut self.delay,
            0.0..=10.0,
            1.0,
        );

        let buttons =
            Rect::from_min_max(pos2(column.left(), column.bottom() - w::BUTTON), column.max);
        let (cancel, capture) = w::button_pair(ui, buttons, 12.0, "Cancel", "Capture", true);
        if cancel {
            self.reply = Some(Reply::Cancelled);
        }
        if capture {
            self.capture();
        }
    }
}

impl crate::Dialog for Screenshot {
    fn reply(&self) -> Option<&Reply> {
        self.reply.as_ref()
    }

    fn size(&self) -> [f32; 2] {
        [1040.0, 616.0]
    }
}

impl App for Screenshot {
    fn title(&self) -> &str {
        "Take Screenshot"
    }

    fn ui(&mut self, ui: &mut Ui, _theme: &Theme) {
        if crate::escape(ui) {
            self.reply = Some(Reply::Cancelled);
            return;
        }
        let (enter, s, wk, a) = ui.input_mut(|i| {
            let none = egui::Modifiers::NONE;
            (
                i.consume_key(none, egui::Key::Enter),
                i.consume_key(none, egui::Key::S),
                i.consume_key(none, egui::Key::W),
                i.consume_key(none, egui::Key::A),
            )
        });
        for (pressed, mode) in [(s, Mode::Screen), (wk, Mode::Window), (a, Mode::Area)] {
            if pressed {
                self.mode = mode;
            }
        }
        if enter {
            self.capture();
            return;
        }
        let rect = ui.max_rect();
        let column_width = 380.0_f32.min(rect.width() * 0.45);
        let (left, right) = rect.split_left_right_at_x(rect.right() - column_width);
        self.preview(ui, left.shrink(32.0));
        let column = Rect::from_min_max(
            pos2(right.left(), right.top() + 32.0),
            pos2(right.right() - 32.0, right.bottom() - 28.0),
        );
        self.controls(ui, column);
    }
}
