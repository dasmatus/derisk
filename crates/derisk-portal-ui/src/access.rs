//! 1d Device Access (`org.freedesktop.portal.Camera` and every other
//! `org.freedesktop.impl.portal.Access` prompt): who is asking on the left,
//! the question, the app's choices and Deny / Allow on the right.

use egui::{Align2, Rect, Ui, pos2, vec2};
use mcsapi_ui::egui;
use mcsapi_ui::{App, Theme};
use serde::{Deserialize, Serialize};

use crate::{
    Reply,
    icons::{self, Icon},
    widgets as w,
};

/// One extra choice the app offers with the question.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct AccessChoice {
    /// Choice ID, returned with the answer.
    pub id: String,
    /// Label.
    pub label: String,
    /// `(id, label)` options for a list; empty makes it a checkbox whose
    /// value is `"true"` or `"false"`.
    #[serde(default)]
    pub options: Vec<(String, String)>,
    /// The option ID (or `"true"`) selected at first.
    #[serde(default)]
    pub initial: String,
}

/// The question.
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
pub struct Request {
    /// Window title, for example "Camera Access".
    #[serde(default)]
    pub window_title: String,
    /// The requesting app's ID.
    pub app_id: String,
    /// The requesting app's name.
    pub app_name: String,
    /// The app's `Icon` key.
    #[serde(default)]
    pub app_icon: String,
    /// Whether the app runs in a Flatpak sandbox.
    #[serde(default)]
    pub sandboxed: bool,
    /// The question, for example "Allow Video Calls to use your camera?".
    pub title: String,
    /// What granting means.
    #[serde(default)]
    pub subtitle: String,
    /// A footnote under the choices.
    #[serde(default)]
    pub body: String,
    /// Label of the deny button; "Deny" when unset.
    #[serde(default)]
    pub deny_label: Option<String>,
    /// Label of the grant button; "Allow" when unset.
    #[serde(default)]
    pub grant_label: Option<String>,
    /// Extra choices.
    #[serde(default)]
    pub choices: Vec<AccessChoice>,
}

impl Request {
    /// The design's placeholder content.
    pub fn sample() -> Self {
        Self {
            window_title: "Camera Access".into(),
            app_id: "org.example.Calls".into(),
            app_name: "Video Calls".into(),
            app_icon: String::new(),
            sandboxed: true,
            title: "Allow Video Calls to use your camera?".into(),
            subtitle: "The app will see a live picture while it’s open. A camera indicator stays in the top bar.".into(),
            body: "Change this later in Settings › Privacy › Camera.".into(),
            deny_label: None,
            grant_label: None,
            choices: vec![
                AccessChoice {
                    id: "camera".into(),
                    label: "Camera".into(),
                    options: vec![
                        ("integrated".into(), "Integrated Camera (USB 2.0)".into()),
                        ("c920".into(), "Logitech C920 HD".into()),
                    ],
                    initial: "integrated".into(),
                },
                AccessChoice {
                    id: "remember".into(),
                    label: "Remember for this app".into(),
                    options: Vec::new(),
                    initial: "true".into(),
                },
            ],
        }
    }
}

/// Access granted, with the value of every choice.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct Choice {
    /// `(choice id, option id)` pairs; checkboxes give `"true"`/`"false"`.
    pub choices: Vec<(String, String)>,
}

enum Value {
    List(usize),
    Check(bool),
}

/// The Device Access dialog.
pub struct Access {
    request: Request,
    values: Vec<Value>,
    reply: Option<Reply>,
}

impl Access {
    /// Opens with each choice at its initial value.
    pub fn new(request: Request) -> Self {
        let values = request
            .choices
            .iter()
            .map(|c| {
                if c.options.is_empty() {
                    Value::Check(c.initial == "true")
                } else {
                    Value::List(
                        c.options
                            .iter()
                            .position(|(id, _)| *id == c.initial)
                            .unwrap_or(0),
                    )
                }
            })
            .collect();
        Self {
            request,
            values,
            reply: None,
        }
    }

    fn grant(&mut self) {
        let choices = self
            .request
            .choices
            .iter()
            .zip(&self.values)
            .map(|(c, v)| {
                let value = match v {
                    Value::Check(on) => on.to_string(),
                    Value::List(i) => c
                        .options
                        .get(*i)
                        .map(|(id, _)| id.clone())
                        .unwrap_or_default(),
                };
                (c.id.clone(), value)
            })
            .collect();
        self.reply = Some(Reply::Access(Choice { choices }));
    }

    fn who(&self, ui: &mut Ui, rect: Rect) {
        let t = w::tokens(ui);
        let painter = ui.painter();
        painter.rect_filled(rect, 0, t.card);
        painter.vline(rect.right() - 0.5, rect.y_range(), t.border_stroke());
        let mut h = 96.0 + 16.0 + 22.0 + 16.0 + 14.0;
        if self.request.sandboxed {
            h += 16.0 + 22.0;
        }
        let x = rect.center().x;
        let mut y = rect.center().y - h / 2.0;
        let tile = Rect::from_min_size(pos2(x - 48.0, y), vec2(96.0, 96.0));
        painter.rect_filled(tile, 24, t.muted);
        painter.rect_stroke(tile, 24, t.border_stroke(), egui::StrokeKind::Inside);
        let icon = if self.request.app_icon.is_empty() {
            icons::APP
        } else {
            Icon::App(&self.request.app_icon, "⊞")
        };
        icons::paint(
            ui,
            Rect::from_center_size(tile.center(), vec2(44.0, 44.0)),
            icon,
            t.foreground,
        );
        y += 96.0 + 16.0;
        let width = rect.width() - 64.0;
        let painter = ui.painter();
        w::text_truncated(
            painter,
            Rect::from_center_size(pos2(x, y + 11.0), vec2(width, 22.0)),
            Align2::CENTER_CENTER,
            &self.request.app_name,
            w::sans(18.0),
            t.foreground,
        );
        y += 22.0 + 16.0;
        w::text_truncated(
            painter,
            Rect::from_center_size(pos2(x, y + 7.0), vec2(width, 14.0)),
            Align2::CENTER_CENTER,
            &self.request.app_id,
            w::mono(12.0),
            t.muted_foreground,
        );
        y += 14.0 + 16.0;
        if self.request.sandboxed {
            w::badge(painter, pos2(x, y), "Flatpak · sandboxed", &t);
        }
    }

    fn question(&mut self, ui: &mut Ui, rect: Rect) {
        let t = w::tokens(ui);
        let mut y = rect.top();
        y += w::title_block(
            ui,
            pos2(rect.left(), y),
            rect.width(),
            &self.request.title,
            24.0,
            8.0,
            &self.request.subtitle,
        );
        for (index, choice) in self.request.choices.iter().enumerate() {
            y += 24.0;
            match &mut self.values[index] {
                Value::List(selected) => {
                    w::text(
                        ui.painter(),
                        Rect::from_min_size(pos2(rect.left(), y), vec2(rect.width(), 13.0)),
                        Align2::LEFT_CENTER,
                        &choice.label,
                        w::sans(13.0),
                        t.muted_foreground,
                    );
                    y += 13.0 + 8.0;
                    let labels: Vec<&str> =
                        choice.options.iter().map(|(_, l)| l.as_str()).collect();
                    w::select(
                        ui,
                        Rect::from_min_size(pos2(rect.left(), y), vec2(rect.width(), w::CONTROL)),
                        &choice.id,
                        selected,
                        &labels,
                    );
                    y += w::CONTROL;
                }
                Value::Check(on) => {
                    w::checkbox(
                        ui,
                        pos2(rect.left(), y + 22.0),
                        &choice.id,
                        on,
                        &choice.label,
                    );
                    y += w::CONTROL;
                }
            }
        }
        if !self.request.body.is_empty() {
            y += 24.0;
            w::paragraph(
                ui.painter(),
                pos2(rect.left(), y),
                rect.width(),
                &self.request.body,
                w::sans(13.0),
                t.muted_foreground,
            );
        }
        let buttons = Rect::from_min_max(pos2(rect.left(), rect.bottom() - w::BUTTON), rect.max);
        let deny = self
            .request
            .deny_label
            .clone()
            .unwrap_or_else(|| "Deny".into());
        let grant = self
            .request
            .grant_label
            .clone()
            .unwrap_or_else(|| "Allow".into());
        let (denied, granted) = w::button_pair(ui, buttons, 12.0, &deny, &grant, true);
        if denied {
            self.reply = Some(Reply::Cancelled);
        }
        if granted {
            self.grant();
        }
    }
}

impl crate::Dialog for Access {
    fn reply(&self) -> Option<&Reply> {
        self.reply.as_ref()
    }

    fn size(&self) -> [f32; 2] {
        let extra = self.request.choices.len().saturating_sub(2) as f32 * 68.0;
        [880.0, 476.0 + extra]
    }
}

impl App for Access {
    fn title(&self) -> &str {
        if self.request.window_title.is_empty() {
            "Access"
        } else {
            &self.request.window_title
        }
    }

    fn ui(&mut self, ui: &mut Ui, _theme: &Theme) {
        if crate::escape(ui) {
            self.reply = Some(Reply::Cancelled);
            return;
        }
        let rect = ui.max_rect();
        let side = 280.0_f32.min(rect.width() * 0.35);
        let (left, right) = rect.split_left_right_at_x(rect.left() + side);
        self.who(ui, left);
        let inner = Rect::from_min_max(
            pos2(right.left() + 36.0, right.top() + 36.0),
            pos2(right.right() - 36.0, right.bottom() - 28.0),
        );
        self.question(ui, inner);
    }
}
