//! How the installer and the first-boot setup look: one card of pages over
//! the wallpaper, drawn with mcsapi's components.
//!
//! Both are sequences of pages with a heading, a body and Back and Next, so
//! everything about their look is decided here and nowhere else. The pages
//! themselves only lay out components (`mcsapi_components`: buttons, inputs,
//! alerts, progress) and the [`list`] rows below, which take their colors
//! and radii from the theme's design tokens. So restyling the components, or
//! the theme, restyles the installer and setup with the rest of the desktop.
//!
//! On a phone the card takes the whole screen, controls are finger-sized
//! (see [`crate::ui::set_touch_style`]) and the on-screen keyboard comes up
//! under it while a field has focus; [`screen_layout`] says where each goes.

use egui::{
    Align, Align2, Color32, CornerRadius, FontId, Id, Layout, Rect, RichText, ScrollArea, Sense,
    Stroke, StrokeKind, Ui, UiBuilder, vec2,
};
use mcsapi::{Geometry, toolkit::egui, widgets::Theme};
use mcsapi_components::{Alert, AlertVariant, Button, ButtonSize, ButtonVariant, Spinner, Tokens};

use crate::{
    geom::rect,
    keyboard::{self, Keyboard, Output as OskOutput},
    ui::{keyboard_keys, paint_wallpaper},
};

/// The card's width on screens wider than a phone.
const CARD_WIDTH: f32 = 600.0;
/// The card's height on screens taller than it needs.
const CARD_HEIGHT: f32 = 640.0;
/// Space around the card on screens wider than a phone.
const MARGIN: f32 = 24.0;
/// A list row's height, with a pointer and on a phone.
const ROW: f32 = 40.0;
const ROW_TOUCH: f32 = 52.0;

/// Where the card and the on-screen keyboard go on an output.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ScreenLayout {
    /// The card.
    pub card: Rect,
    /// The on-screen keyboard, when it shows.
    pub keyboard: Option<Geometry>,
    /// Whether this is a phone: full-screen card, finger-sized controls.
    pub phone: bool,
}

/// Lays out an output of `size` logical pixels. A short side under 600 is a
/// phone, as derisk decides everywhere (see [`crate::adaptive`]). The
/// keyboard, when `keyboard` is set, takes the bottom of the screen and the
/// card shrinks to sit above it.
pub fn screen_layout(size: (i32, i32), keyboard: bool) -> ScreenLayout {
    let (w, h) = size;
    let phone = w.min(h) < 600;
    let keyboard = keyboard.then(|| {
        let height = keyboard::HEIGHT.min(h);
        let width = if phone { w } else { w.min(900) };
        rect((w - width) / 2, h - height, width, height)
    });
    let above = keyboard.map_or(h, |k| k.loc.y) as f32;
    let card = if phone {
        Rect::from_min_size(egui::Pos2::ZERO, vec2(w as f32, above))
    } else {
        let width = CARD_WIDTH.min(w as f32 - 2.0 * MARGIN);
        let height = CARD_HEIGHT.min(above - 2.0 * MARGIN).max(200.0);
        // Centered on the whole screen, unless the keyboard pushes it up.
        let top = ((h as f32 - height) / 2.0).min(above - MARGIN - height);
        Rect::from_min_size(
            egui::pos2((w as f32 - width) / 2.0, top.max(0.0)),
            vec2(width, height),
        )
    };
    ScreenLayout {
        card,
        keyboard,
        phone,
    }
}

/// One page's frame: what the header and footer say.
#[derive(Clone, Debug)]
pub struct Page<'a> {
    /// The heading.
    pub title: &'a str,
    /// The line under it.
    pub subtitle: &'a str,
    /// Which of `count` pages this is, for the dots; `None` hides them.
    pub index: Option<usize>,
    /// How many pages there are.
    pub count: usize,
    /// Whether Back shows.
    pub back: bool,
    /// The forward button, if any.
    pub next: Option<Next<'a>>,
}

/// The forward button.
#[derive(Clone, Copy, Debug)]
pub struct Next<'a> {
    /// Its label: Next, Install, Restart.
    pub label: &'a str,
    /// Whether it can be pressed yet.
    pub enabled: bool,
    /// Whether it does something that cannot be undone, such as erasing a
    /// disk, and so is drawn in the destructive color.
    pub destructive: bool,
}

impl<'a> Next<'a> {
    /// An ordinary forward button.
    pub fn new(label: &'a str, enabled: bool) -> Self {
        Self {
            label,
            enabled,
            destructive: false,
        }
    }
}

/// What the footer's buttons were asked to do this frame.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Nav {
    /// Nothing.
    None,
    /// Back.
    Back,
    /// The forward button, or Enter when it is enabled.
    Next,
}

/// Paints the screen behind the card: the desktop's wallpaper.
/// Call before [`page`]: both paint in `ui`'s layer, in order.
pub fn background(ui: &Ui, screen: Rect, theme: &Theme) {
    let painter = ui.painter();
    painter.rect_filled(screen, 0, theme.background);
    paint_wallpaper(painter, screen, theme);
}

/// Draws one page in `layout.card`: dots, heading and subtitle at the top,
/// Back and the forward button at the bottom, and `body` in between, given
/// the height that is left. Installs the theme's tokens first.
pub fn page(
    ui: &mut Ui,
    layout: &ScreenLayout,
    theme: &Theme,
    page: Page<'_>,
    body: impl FnOnce(&mut Ui),
) -> Nav {
    let tokens = Tokens::from_theme(theme);
    tokens.install(ui.ctx());
    let card = layout.card;
    let radius = if layout.phone {
        CornerRadius::ZERO
    } else {
        tokens.card_radius()
    };
    ui.painter().rect_filled(card, radius, tokens.card);
    if !layout.phone {
        ui.painter()
            .rect_stroke(card, radius, tokens.border_stroke(), StrokeKind::Inside);
    }
    let margin = if layout.phone { 16.0 } else { 32.0 };
    let inner = card.shrink(margin);
    let footer_height = if layout.phone { ROW_TOUCH } else { 40.0 };

    let mut nav = Nav::None;
    let mut child = ui.new_child(
        UiBuilder::new()
            .id_salt("derisk-wizard-card")
            .max_rect(inner)
            .layout(Layout::top_down(Align::Min)),
    );
    let ui = &mut child;
    ui.visuals_mut().override_text_color = Some(tokens.foreground);

    // Header.
    if let Some(index) = page.index {
        dots(ui, index, page.count, &tokens);
        ui.add_space(16.0);
    }
    ui.label(
        RichText::new(page.title)
            .font(FontId::proportional(if layout.phone { 24.0 } else { 28.0 }))
            .strong()
            .color(tokens.foreground),
    );
    ui.add_space(4.0);
    ui.label(
        RichText::new(page.subtitle)
            .font(tokens.body_font())
            .color(tokens.muted_foreground),
    );
    ui.add_space(20.0);

    // Body: everything down to the footer.
    let top = ui.cursor().top();
    let body_rect = Rect::from_min_max(
        egui::pos2(inner.left(), top),
        egui::pos2(inner.right(), inner.bottom() - footer_height - 16.0),
    );
    let mut body_ui = ui.new_child(
        UiBuilder::new()
            .id_salt("derisk-wizard-body")
            .max_rect(body_rect)
            .layout(Layout::top_down(Align::Min)),
    );
    body_ui.set_clip_rect(body_rect.intersect(ui.clip_rect()));
    body(&mut body_ui);

    // Footer.
    let footer = Rect::from_min_max(
        egui::pos2(inner.left(), inner.bottom() - footer_height),
        inner.max,
    );
    let mut footer_ui = ui.new_child(
        UiBuilder::new()
            .id_salt("derisk-wizard-footer")
            .max_rect(footer)
            .layout(Layout::right_to_left(Align::Center)),
    );
    let size = if layout.phone {
        ButtonSize::Lg
    } else {
        ButtonSize::Default
    };
    if let Some(next) = page.next {
        let variant = if next.destructive {
            ButtonVariant::Destructive
        } else {
            ButtonVariant::Default
        };
        let button = footer_ui.add(
            Button::new(next.label)
                .variant(variant)
                .size(size)
                .enabled(next.enabled),
        );
        if button.clicked() {
            nav = Nav::Next;
        }
        // Enter moves on, as in any dialog, unless a list or button has
        // focus and Enter means something there. A page that wants Enter in
        // a field to do something else can check before calling this.
        if next.enabled && !next.destructive && footer_ui.input(|i| i.key_pressed(egui::Key::Enter))
        {
            nav = Nav::Next;
        }
    }
    if page.back {
        footer_ui.with_layout(Layout::left_to_right(Align::Center), |ui| {
            if ui
                .add(Button::new("Back").variant(ButtonVariant::Ghost).size(size))
                .clicked()
            {
                nav = Nav::Back;
            }
        });
    }
    nav
}

/// The page dots: done, current and to come.
fn dots(ui: &mut Ui, index: usize, count: usize, tokens: &Tokens) {
    let (rect, _) = ui.allocate_exact_size(vec2(count as f32 * 20.0, 8.0), Sense::hover());
    for i in 0..count {
        let x = rect.left() + i as f32 * 20.0;
        let r = Rect::from_min_size(
            egui::pos2(x, rect.top()),
            vec2(if i == index { 16.0 } else { 8.0 }, 8.0),
        );
        let fill = if i <= index {
            tokens.primary
        } else {
            tokens.muted
        };
        ui.painter().rect_filled(r, CornerRadius::same(4), fill);
    }
}

/// A label over a text field, the field taking the width it is given.
pub fn field(
    ui: &mut Ui,
    label: &str,
    text: &mut String,
    password: bool,
    placeholder: &str,
) -> egui::Response {
    let tokens = Tokens::current(ui.ctx());
    ui.label(
        RichText::new(label)
            .font(tokens.small_font())
            .strong()
            .color(tokens.foreground),
    );
    ui.add_space(2.0);
    let response = ui.add(
        mcsapi_components::Input::new(text)
            .password(password)
            .placeholder(placeholder),
    );
    ui.add_space(10.0);
    response
}

/// An error, or a note, under the controls.
pub fn notice(ui: &mut Ui, text: &str, error: bool) {
    if error {
        ui.add(Alert::new(text).variant(AlertVariant::Destructive));
    } else {
        let tokens = Tokens::current(ui.ctx());
        ui.label(
            RichText::new(text)
                .font(tokens.body_font())
                .color(tokens.muted_foreground),
        );
    }
}

/// A spinner with a line beside it, for work in progress.
pub fn busy(ui: &mut Ui, text: &str) {
    ui.horizontal(|ui| {
        ui.add(Spinner::new().size(18.0));
        ui.add_space(6.0);
        ui.label(text);
    });
}

/// One row of a [`list`].
#[derive(Clone, Debug, Default)]
pub struct Row {
    /// The main text.
    pub title: String,
    /// Smaller text after it, right-aligned: a code, a size, a signal.
    pub detail: String,
}

/// A scrolling list of rows that fills the height it is given, drawing only
/// the rows in view (time zones are hundreds). Returns the row clicked.
/// `scroll_to` brings a row into view, for showing the current choice when a
/// page opens.
pub fn list(
    ui: &mut Ui,
    id: &str,
    count: usize,
    selected: Option<usize>,
    scroll_to: Option<usize>,
    row: impl Fn(usize) -> Row,
) -> Option<usize> {
    let tokens = Tokens::current(ui.ctx());
    let touch = ui.spacing().interact_size.y >= 40.0;
    let height = if touch { ROW_TOUCH } else { ROW };
    let frame = ui.available_rect_before_wrap();
    ui.painter().rect_stroke(
        frame,
        tokens.control_radius(),
        tokens.border_stroke(),
        StrokeKind::Inside,
    );
    let mut clicked = None;
    let mut area = ScrollArea::vertical()
        .id_salt(id)
        .auto_shrink([false, false])
        .max_height(frame.height());
    if let Some(i) = scroll_to {
        area = area.vertical_scroll_offset((i as f32 * height - frame.height() / 2.0).max(0.0));
    }
    // show_rows counts the spacing between rows in.
    ui.spacing_mut().item_spacing.y = 0.0;
    area.show_rows(ui, height, count, |ui, range| {
        for i in range {
            let item = row(i);
            let (r, response) =
                ui.allocate_exact_size(vec2(ui.available_width(), height), Sense::click());
            response.widget_info(|| {
                egui::WidgetInfo::selected(
                    egui::WidgetType::SelectableLabel,
                    true,
                    selected == Some(i),
                    &item.title,
                )
            });
            let fill = if selected == Some(i) {
                tokens.primary.gamma_multiply(0.22)
            } else if response.hovered() {
                tokens.hover
            } else {
                Color32::TRANSPARENT
            };
            let inset = r.shrink2(vec2(4.0, 2.0));
            ui.painter()
                .rect_filled(inset, tokens.control_radius(), fill);
            let text = inset.shrink2(vec2(12.0, 0.0));
            ui.painter().text(
                text.left_center(),
                Align2::LEFT_CENTER,
                &item.title,
                tokens.body_font(),
                tokens.foreground,
            );
            if !item.detail.is_empty() {
                ui.painter().text(
                    text.right_center(),
                    Align2::RIGHT_CENTER,
                    &item.detail,
                    tokens.small_font(),
                    tokens.muted_foreground,
                );
            }
            if selected == Some(i) {
                ui.painter().rect_stroke(
                    inset,
                    tokens.control_radius(),
                    Stroke::new(1.0, tokens.primary),
                    StrokeKind::Inside,
                );
            }
            if response.clicked() {
                clicked = Some(i);
            }
        }
    });
    clicked
}

/// The steps of work in progress: done ones ticked, the current one with a
/// spinner (or a cross when it failed), the rest dimmed.
pub fn steps(ui: &mut Ui, labels: &[&str], current: usize, failed: bool) {
    let tokens = Tokens::current(ui.ctx());
    for (i, label) in labels.iter().enumerate() {
        ui.horizontal(|ui| {
            let (mark, color) = if i < current {
                ("✔", tokens.primary)
            } else if i == current && failed {
                ("✖", tokens.destructive)
            } else if i == current {
                ("", tokens.foreground)
            } else {
                ("○", tokens.muted_foreground)
            };
            if i == current && !failed {
                ui.add(Spinner::new().size(16.0));
            } else {
                ui.add_sized(
                    [16.0, 16.0],
                    egui::Label::new(RichText::new(mark).color(color)),
                );
            }
            ui.add_space(8.0);
            let text = RichText::new(*label).font(tokens.body_font());
            ui.label(if i > current {
                text.color(tokens.muted_foreground)
            } else {
                text.color(tokens.foreground)
            });
        });
        ui.add_space(6.0);
    }
}

/// The on-screen keyboard for the setup screens: drawn in `area` above
/// everything, typing into the field that has focus. Holds the keyboard's
/// state and a held Backspace's repeat between frames.
#[derive(Debug, Default)]
pub struct ScreenKeyboard {
    keyboard: Keyboard,
    repeat: Option<f64>,
    /// What was typed last frame, delivered at the start of the next.
    pending: Vec<egui::Event>,
    /// The text field the keys type into. A tap on a key would otherwise
    /// take egui's focus away from it, and with it the keyboard.
    target: Option<Id>,
    /// Whether the keyboard was touched in the last frame.
    touched: bool,
}

impl ScreenKeyboard {
    /// A keyboard that never learns words: everything typed during setup is
    /// a name, a password or a network's passphrase.
    pub fn new() -> Self {
        let mut keyboard = Keyboard::new();
        keyboard.set_learning(false);
        Self {
            keyboard,
            ..Self::default()
        }
    }

    /// Follows egui's focus: the field with it is the one to type into, and
    /// stays so while the keyboard itself is being tapped. Returns whether
    /// a field wants the keyboard. Call before drawing the page.
    pub fn follow_focus(&mut self, ctx: &egui::Context) -> bool {
        if ctx.egui_wants_keyboard_input() {
            self.target = ctx.memory(|m| m.focused());
        } else if self.touched {
            if let Some(target) = self.target {
                ctx.memory_mut(|m| m.request_focus(target));
            }
        } else {
            self.target = None;
        }
        self.target.is_some()
    }

    /// Hands last frame's keys to egui. Call before drawing the page.
    pub fn deliver(&mut self, ctx: &egui::Context) {
        if !self.pending.is_empty() {
            let events = std::mem::take(&mut self.pending);
            ctx.input_mut(|i| i.events.extend(events));
            ctx.request_repaint();
        }
    }

    /// Draws the keyboard in `area`. Call after drawing the page.
    pub fn show(&mut self, ui: &mut Ui, area: Geometry, theme: &Theme) {
        let r = Rect::from_min_size(
            egui::pos2(area.loc.x as f32, area.loc.y as f32),
            vec2(area.size.w as f32, area.size.h as f32),
        );
        let id = Id::new("derisk-wizard-osk");
        let mut keys = Ui::new(
            ui.ctx().clone(),
            id,
            UiBuilder::new()
                .layer_id(egui::LayerId::new(egui::Order::Tooltip, id))
                .max_rect(r),
        );
        keys.interact(r, id.with("backdrop"), Sense::click());
        self.touched = keys.ctx().input(|i| {
            let on = i.pointer.interact_pos().is_some_and(|p| r.contains(p));
            on && (i.pointer.any_down() || i.pointer.any_released())
        });
        if self.touched
            && let Some(target) = self.target
        {
            keys.ctx().memory_mut(|m| m.request_focus(target));
        }
        let typed = keyboard_keys(&mut keys, &mut self.keyboard, area, theme, &mut self.repeat);
        self.pending.extend(typed.into_iter().map(|o| match o {
            OskOutput::Text(text) => egui::Event::Text(text),
            OskOutput::Backspace => key(egui::Key::Backspace),
            OskOutput::Enter => key(egui::Key::Enter),
        }));
        if !self.pending.is_empty() {
            ui.ctx().request_repaint();
        }
    }

    /// Forgets the word in progress, when the keyboard goes away.
    pub fn reset(&mut self) {
        self.keyboard.reset();
        self.touched = false;
    }
}

fn key(key: egui::Key) -> egui::Event {
    egui::Event::Key {
        key,
        physical_key: None,
        pressed: true,
        repeat: false,
        modifiers: egui::Modifiers::NONE,
    }
}
