//! 1f Background & Autostart (`org.freedesktop.portal.Background`):
//! switches for running in the background and starting at login, the
//! command that will run, and Don't Allow / Allow.

use egui::{Align2, Rect, Ui, pos2, vec2};
use mcsapi_ui::egui;
use mcsapi_ui::{App, Theme};
use serde::{Deserialize, Serialize};

use crate::{Reply, widgets as w};

/// The app asking to keep running.
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
pub struct Request {
    /// The app's ID.
    pub app_id: String,
    /// The app's name.
    pub app_name: String,
    /// A muted line under the heading.
    #[serde(default)]
    pub description: String,
    /// The command it runs; empty hides the Command box.
    #[serde(default)]
    pub command: String,
    /// Whether "Start at login" is on at first.
    #[serde(default)]
    pub autostart: bool,
}

/// Background activity allowed.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct Choice {
    /// Keep running after the window closes; off allows it this once.
    pub background: bool,
    /// Start at login.
    pub autostart: bool,
}

/// The Background Activity dialog.
pub struct Background {
    request: Request,
    heading: String,
    background: bool,
    autostart: bool,
    reply: Option<Reply>,
}

impl Background {
    /// Opens with running in the background on.
    pub fn new(request: Request) -> Self {
        Self {
            heading: format!("Let {} run in the background?", request.app_name),
            background: true,
            autostart: request.autostart,
            request,
            reply: None,
        }
    }

    fn setting(ui: &mut Ui, row: Rect, id: &str, label: &str, on: &mut bool) {
        let t = w::tokens(ui);
        let inner = row.shrink2(vec2(20.0, 12.0));
        w::text(
            ui.painter(),
            inner,
            Align2::LEFT_CENTER,
            label,
            w::sans(15.0),
            t.foreground,
        );
        let response = w::switch(
            ui,
            pos2(inner.right() - 44.0, inner.center().y),
            id,
            on,
            None,
            true,
        );
        response.widget_info(|| {
            egui::WidgetInfo::selected(egui::WidgetType::Checkbox, true, *on, label)
        });
    }
}

impl crate::Dialog for Background {
    fn reply(&self) -> Option<&Reply> {
        self.reply.as_ref()
    }

    fn size(&self) -> [f32; 2] {
        [720.0, 476.0]
    }
}

impl App for Background {
    fn title(&self) -> &str {
        "Background Activity"
    }

    fn ui(&mut self, ui: &mut Ui, _theme: &Theme) {
        if crate::escape(ui) {
            self.reply = Some(Reply::Cancelled);
            return;
        }
        let t = w::tokens(ui);
        let body = ui.max_rect().shrink(32.0);
        let mut y = body.top();
        y += w::title_block(
            ui,
            pos2(body.left(), y),
            body.width(),
            &self.heading,
            24.0,
            8.0,
            &self.request.description,
        );
        y += 24.0;
        let card = Rect::from_min_size(pos2(body.left(), y), vec2(body.width(), 52.0 * 2.0 + 1.0));
        w::card(ui.painter(), card, &t);
        let first = Rect::from_min_size(card.min, vec2(card.width(), 52.0));
        ui.painter()
            .hline(card.x_range(), first.bottom() + 0.5, t.border_stroke());
        Self::setting(
            ui,
            first,
            "background",
            "Run in the background",
            &mut self.background,
        );
        let second = Rect::from_min_size(pos2(card.left(), first.bottom() + 1.0), first.size());
        Self::setting(
            ui,
            second,
            "autostart",
            "Start at login",
            &mut self.autostart,
        );
        y = card.bottom() + 24.0;

        if !self.request.command.is_empty() {
            w::text(
                ui.painter(),
                Rect::from_min_size(pos2(body.left(), y), vec2(body.width(), 13.0)),
                Align2::LEFT_CENTER,
                "Command",
                w::sans(13.0),
                t.muted_foreground,
            );
            y += 13.0 + 8.0;
            let text_h = w::paragraph_height(
                ui.painter(),
                body.width() - 32.0,
                &self.request.command,
                w::mono(13.0),
            );
            let code = Rect::from_min_size(pos2(body.left(), y), vec2(body.width(), text_h + 28.0));
            ui.painter().rect_filled(code, w::ROW_RADIUS, t.muted);
            w::paragraph(
                ui.painter(),
                code.min + vec2(16.0, 14.0),
                body.width() - 32.0,
                &self.request.command,
                w::mono(13.0),
                t.foreground,
            );
        }

        let buttons = Rect::from_min_max(pos2(body.left(), body.bottom() - w::BUTTON), body.max);
        let (deny, allow) = w::button_pair(ui, buttons, 16.0, "Don’t Allow", "Allow", true);
        if deny {
            self.reply = Some(Reply::Cancelled);
        }
        if allow {
            self.reply = Some(Reply::Background(Choice {
                background: self.background,
                autostart: self.autostart,
            }));
        }
    }
}
