//! The spacious control sizes every portal dialog uses.
//!
//! The mcsapi components are sized for compact desktop density (36 px
//! controls). Portal dialogs are short, one-decision windows that may be
//! answered by touch, so their controls are the touch sizes of the derisk
//! design system instead: 44 px fields, switches and sliders, 48 px buttons,
//! 48–52 px rows. Colors still come from the installed [`Tokens`].
//!
//! Every function here lays out into a rectangle the caller computed, so the
//! dialogs can stretch lists and grids to fill the window.

use std::ops::RangeInclusive;

use egui::{
    Align, Align2, Color32, CornerRadius, FontId, Frame, Galley, Layout, Margin, Painter, Pos2,
    Rect, Response, RichText, Sense, Shadow, Stroke, StrokeKind, TextEdit, Ui, UiBuilder, Vec2,
    WidgetInfo, WidgetType, pos2, text::LayoutJob, vec2,
};
use mcsapi_components::Tokens;
use mcsapi_ui::egui;
use std::sync::Arc;

/// Height of fields, selects, switches, checkboxes and sliders.
pub const CONTROL: f32 = 44.0;
/// Height of dialog buttons.
pub const BUTTON: f32 = 48.0;
/// Control corner radius.
pub const RADIUS: u8 = 6;
/// Row and list-item corner radius.
pub const ROW_RADIUS: u8 = 8;
/// Card and tile corner radius.
pub const CARD_RADIUS: u8 = 12;

/// The proportional font at `size` points.
pub fn sans(size: f32) -> FontId {
    FontId::proportional(size)
}

/// The monospace font at `size` points.
pub fn mono(size: f32) -> FontId {
    FontId::monospace(size)
}

/// The tokens installed for this frame.
pub fn tokens(ui: &Ui) -> Tokens {
    Tokens::current(ui.ctx())
}

/// Fill of a selected row: the accent at 22 %, as in the command palette.
pub fn selected_fill(t: &Tokens) -> Color32 {
    t.primary.gamma_multiply(0.22)
}

/// Fill of a hovered row: the accent at 10 %.
pub fn hover_fill(t: &Tokens) -> Color32 {
    t.primary.gamma_multiply(0.10)
}

/// Fill of a snap or capture preview: the accent at 18 %.
pub fn preview_fill(t: &Tokens) -> Color32 {
    t.primary.gamma_multiply(0.18)
}

/// Runs `add` in a child `Ui` confined to `rect`, laid out top-down.
pub fn region<R>(ui: &mut Ui, rect: Rect, add: impl FnOnce(&mut Ui) -> R) -> R {
    ui.scope_builder(
        UiBuilder::new()
            .max_rect(rect)
            .layout(Layout::top_down(Align::Min)),
        add,
    )
    .inner
}

/// Lays out `text` on one line.
pub fn galley(painter: &Painter, text: &str, font: FontId, color: Color32) -> Arc<Galley> {
    painter.layout_no_wrap(text.to_owned(), font, color)
}

/// Paints `text` on one line, placed in `rect` by `align`, and returns where
/// it went.
pub fn text(
    painter: &Painter,
    rect: Rect,
    align: Align2,
    text: &str,
    font: FontId,
    color: Color32,
) -> Rect {
    let galley = galley(painter, text, font, color);
    let at = align.align_size_within_rect(galley.size(), rect);
    painter.galley(at.min, galley, color);
    at
}

/// Paints `text` on one line, ending in an ellipsis where it is wider than
/// `rect`.
pub fn text_truncated(
    painter: &Painter,
    rect: Rect,
    align: Align2,
    text: &str,
    font: FontId,
    color: Color32,
) -> Rect {
    let mut job = LayoutJob::single_section(text.to_owned(), egui::TextFormat::simple(font, color));
    job.wrap.max_width = rect.width().max(1.0);
    job.wrap.max_rows = 1;
    job.wrap.break_anywhere = true;
    let galley = painter.layout_job(job);
    let at = align.align_size_within_rect(galley.size(), rect);
    painter.galley(at.min, galley, color);
    at
}

/// Paints wrapped `text` from `top_left`, at most `width` wide, and returns
/// its height.
pub fn paragraph(
    painter: &Painter,
    top_left: Pos2,
    width: f32,
    text: &str,
    font: FontId,
    color: Color32,
) -> f32 {
    let galley = painter.layout(text.to_owned(), font, color, width.max(1.0));
    let height = galley.size().y;
    painter.galley(top_left, galley, color);
    height
}

/// The height of `text` wrapped at `width`.
pub fn paragraph_height(painter: &Painter, width: f32, text: &str, font: FontId) -> f32 {
    painter
        .layout(text.to_owned(), font, Color32::PLACEHOLDER, width.max(1.0))
        .size()
        .y
}

/// A heading (24 px) and a muted description (15 px) stacked from
/// `top_left`, as every dialog opens. Returns the height used.
pub fn title_block(
    ui: &Ui,
    top_left: Pos2,
    width: f32,
    title: &str,
    title_size: f32,
    gap: f32,
    description: &str,
) -> f32 {
    let t = tokens(ui);
    let painter = ui.painter();
    let mut y = top_left.y;
    y += paragraph(
        painter,
        pos2(top_left.x, y),
        width,
        title,
        sans(title_size),
        t.foreground,
    );
    if !description.is_empty() {
        y += gap;
        y += paragraph(
            painter,
            pos2(top_left.x, y),
            width,
            description,
            sans(15.0),
            t.muted_foreground,
        );
    }
    y - top_left.y
}

/// The height [`title_block`] would use.
pub fn title_block_height(
    ui: &Ui,
    width: f32,
    title: &str,
    title_size: f32,
    gap: f32,
    description: &str,
) -> f32 {
    let painter = ui.painter();
    let mut h = paragraph_height(painter, width, title, sans(title_size));
    if !description.is_empty() {
        h += gap + paragraph_height(painter, width, description, sans(15.0));
    }
    h
}

/// Visual style of a [`button`].
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Variant {
    /// Accent fill: the confirming choice.
    Primary,
    /// A border and no fill: the dismissing choice.
    Outline,
    /// No border or fill until hovered: toolbar icons.
    Ghost,
}

fn focus_ring(ui: &Ui, response: &Response, rect: Rect, radius: u8) {
    if response.has_focus() {
        ui.painter()
            .rect_stroke(rect, radius, tokens(ui).ring_stroke(), StrokeKind::Outside);
    }
}

/// A 48 px dialog button filling `rect`.
pub fn button(
    ui: &mut Ui,
    rect: Rect,
    id_salt: &str,
    label: &str,
    variant: Variant,
    enabled: bool,
) -> Response {
    let t = tokens(ui);
    let sense = if enabled {
        Sense::click()
    } else {
        Sense::hover()
    };
    let response = ui.interact(rect, ui.id().with(("button", id_salt)), sense);
    response.widget_info(|| WidgetInfo::labeled(WidgetType::Button, enabled, label));
    let hovered = enabled && response.hovered();
    let (mut fill, mut color) = match variant {
        Variant::Primary => (
            if hovered {
                t.primary.gamma_multiply(0.9)
            } else {
                t.primary
            },
            t.primary_foreground,
        ),
        Variant::Outline | Variant::Ghost => (
            if hovered {
                t.hover
            } else {
                Color32::TRANSPARENT
            },
            t.foreground,
        ),
    };
    if !enabled {
        fill = fill.gamma_multiply(0.5);
        color = color.gamma_multiply(0.5);
    }
    let painter = ui.painter();
    painter.rect_filled(rect, RADIUS, fill);
    if variant == Variant::Outline {
        painter.rect_stroke(rect, RADIUS, t.border_stroke(), StrokeKind::Inside);
    }
    text_truncated(
        painter,
        rect.shrink2(vec2(12.0, 0.0)),
        Align2::CENTER_CENTER,
        label,
        sans(15.0),
        color,
    );
    focus_ring(ui, &response, rect, RADIUS);
    response
}

/// Cancel and confirm side by side, splitting `rect` with `gap` between.
/// Returns whether each was clicked.
pub fn button_pair(
    ui: &mut Ui,
    rect: Rect,
    gap: f32,
    cancel: &str,
    confirm: &str,
    confirm_enabled: bool,
) -> (bool, bool) {
    let half = (rect.width() - gap) / 2.0;
    let left = Rect::from_min_size(rect.min, vec2(half, rect.height()));
    let right = Rect::from_min_size(pos2(rect.right() - half, rect.top()), left.size());
    let cancelled = button(ui, left, "cancel", cancel, Variant::Outline, true).clicked();
    let confirmed = button(
        ui,
        right,
        "confirm",
        confirm,
        Variant::Primary,
        confirm_enabled,
    )
    .clicked();
    (cancelled, confirmed)
}

/// A square ghost button with a centered icon.
pub fn icon_button(
    ui: &mut Ui,
    rect: Rect,
    id_salt: &str,
    icon: crate::icons::Icon,
    label: &str,
    enabled: bool,
) -> Response {
    let response = button(ui, rect, id_salt, "", Variant::Ghost, enabled);
    response.widget_info(|| WidgetInfo::labeled(WidgetType::Button, enabled, label));
    let t = tokens(ui);
    let color = if enabled {
        t.foreground
    } else {
        t.foreground.gamma_multiply(0.5)
    };
    crate::icons::paint(
        ui,
        Rect::from_center_size(rect.center(), Vec2::splat(18.0)),
        icon,
        color,
    );
    response
}

/// A clickable row or tile in `rect`: accent-tinted when `selected`, a
/// fainter tint when hovered.
pub fn row(ui: &mut Ui, rect: Rect, id: egui::Id, selected: bool, label: &str) -> Response {
    let t = tokens(ui);
    let response = ui.interact(rect, id, Sense::click());
    response
        .widget_info(|| WidgetInfo::selected(WidgetType::SelectableLabel, true, selected, label));
    let fill = if selected {
        selected_fill(&t)
    } else if response.hovered() {
        hover_fill(&t)
    } else {
        Color32::TRANSPARENT
    };
    ui.painter().rect_filled(rect, ROW_RADIUS, fill);
    focus_ring(ui, &response, rect, ROW_RADIUS);
    response
}

/// A bordered tile in `rect` (screen sources, apps): the accent border when
/// selected, the theme border when hovered, else the component border.
pub fn tile(
    ui: &mut Ui,
    rect: Rect,
    id: egui::Id,
    selected: bool,
    fill: Color32,
    label: &str,
) -> Response {
    let t = tokens(ui);
    let response = ui.interact(rect, id, Sense::click());
    response
        .widget_info(|| WidgetInfo::selected(WidgetType::SelectableLabel, true, selected, label));
    let border = if selected {
        t.primary
    } else if response.hovered() {
        t.muted_foreground
    } else {
        t.border
    };
    let painter = ui.painter();
    painter.rect_filled(rect, CARD_RADIUS, fill);
    painter.rect_stroke(
        rect,
        CARD_RADIUS,
        Stroke::new(2.0, border),
        StrokeKind::Inside,
    );
    focus_ring(ui, &response, rect, CARD_RADIUS);
    response
}

/// A flat card: fill plus a 1 px border.
pub fn card(painter: &Painter, rect: Rect, t: &Tokens) {
    painter.rect_filled(rect, CARD_RADIUS, t.card);
    painter.rect_stroke(rect, CARD_RADIUS, t.border_stroke(), StrokeKind::Inside);
}

/// Width of a [`switch`] or [`checkbox`] with `label`.
pub fn toggle_width(ui: &Ui, control: f32, label: Option<&str>) -> f32 {
    label.map_or(control, |label| {
        control
            + 12.0
            + galley(ui.painter(), label, sans(16.0), Color32::PLACEHOLDER)
                .size()
                .x
    })
}

fn toggle_response(
    ui: &mut Ui,
    left_center: Pos2,
    id_salt: &str,
    control: f32,
    label: Option<&str>,
    enabled: bool,
) -> (Response, Rect) {
    let width = toggle_width(ui, control, label);
    let rect = Rect::from_min_size(
        pos2(left_center.x, left_center.y - CONTROL / 2.0),
        vec2(width, CONTROL),
    );
    let sense = if enabled {
        Sense::click()
    } else {
        Sense::hover()
    };
    let response = ui.interact(rect, ui.id().with(("toggle", id_salt)), sense);
    if let Some(label) = label {
        let t = tokens(ui);
        let color = if enabled {
            t.foreground
        } else {
            t.foreground.gamma_multiply(0.5)
        };
        text(
            ui.painter(),
            Rect::from_min_max(pos2(rect.left() + control + 12.0, rect.top()), rect.max),
            Align2::LEFT_CENTER,
            label,
            sans(16.0),
            color,
        );
    }
    (response, rect)
}

/// A 44×24 switch whose left edge is at `left_center`, with an optional
/// label right of it. The 44 px tall hit area covers the label too.
pub fn switch(
    ui: &mut Ui,
    left_center: Pos2,
    id_salt: &str,
    on: &mut bool,
    label: Option<&str>,
    enabled: bool,
) -> Response {
    let (mut response, _) = toggle_response(ui, left_center, id_salt, 44.0, label, enabled);
    if response.clicked() {
        *on = !*on;
        response.mark_changed();
    }
    let value = *on;
    response.widget_info(|| {
        WidgetInfo::selected(
            WidgetType::Checkbox,
            enabled,
            value,
            label.unwrap_or_default(),
        )
    });
    let t = tokens(ui);
    let track = Rect::from_min_size(pos2(left_center.x, left_center.y - 12.0), vec2(44.0, 24.0));
    let anim = ui.ctx().animate_bool_responsive(response.id, value);
    let dim = |c: Color32| if enabled { c } else { c.gamma_multiply(0.5) };
    let painter = ui.painter();
    painter.rect_filled(
        track,
        CornerRadius::same(u8::MAX),
        dim(t.hover.lerp_to_gamma(t.primary, anim)),
    );
    let x = egui::lerp((track.left() + 12.0)..=(track.right() - 12.0), anim);
    painter.circle_filled(pos2(x, track.center().y), 10.0, dim(t.background));
    focus_ring(ui, &response, track, u8::MAX);
    response
}

/// A 20 px checkbox whose left edge is at `left_center`, with a label.
pub fn checkbox(
    ui: &mut Ui,
    left_center: Pos2,
    id_salt: &str,
    checked: &mut bool,
    label: &str,
) -> Response {
    let (mut response, _) = toggle_response(ui, left_center, id_salt, 20.0, Some(label), true);
    if response.clicked() {
        *checked = !*checked;
        response.mark_changed();
    }
    let value = *checked;
    response.widget_info(|| WidgetInfo::selected(WidgetType::Checkbox, true, value, label));
    let t = tokens(ui);
    let rect = Rect::from_min_size(pos2(left_center.x, left_center.y - 10.0), Vec2::splat(20.0));
    let painter = ui.painter();
    if value {
        painter.rect_filled(rect, 4, t.primary);
        let c = rect.center();
        painter.line(
            vec![
                c + vec2(-4.5, 0.0),
                c + vec2(-1.5, 3.0),
                c + vec2(4.5, -3.5),
            ],
            Stroke::new(2.0, t.primary_foreground),
        );
    } else {
        painter.rect_stroke(
            rect,
            4,
            Stroke::new(1.0, t.muted_foreground),
            StrokeKind::Inside,
        );
    }
    focus_ring(ui, &response, rect, 4);
    response
}

/// A 44 px tall slider filling `rect`'s width, with a 24 px thumb on an
/// 8 px track. Arrow keys step it while focused.
pub fn slider(
    ui: &mut Ui,
    rect: Rect,
    id_salt: &str,
    value: &mut f32,
    range: RangeInclusive<f32>,
    step: f32,
) -> Response {
    let t = tokens(ui);
    let rect = Rect::from_center_size(rect.center(), vec2(rect.width(), CONTROL));
    let mut response = ui.interact(
        rect,
        ui.id().with(("slider", id_salt)),
        Sense::click_and_drag(),
    );
    let (start, end) = (*range.start(), *range.end());
    let thumb = 24.0;
    let track_x = (rect.left() + thumb / 2.0)..=(rect.right() - thumb / 2.0);
    let mut new = *value;
    if let Some(pointer) = response.interact_pointer_pos() {
        let f = egui::remap_clamp(pointer.x, track_x.clone(), 0.0..=1.0);
        new = egui::lerp(start..=end, f);
    }
    if response.has_focus() {
        ui.input(|i| {
            if i.key_pressed(egui::Key::ArrowRight) || i.key_pressed(egui::Key::ArrowUp) {
                new += step;
            }
            if i.key_pressed(egui::Key::ArrowLeft) || i.key_pressed(egui::Key::ArrowDown) {
                new -= step;
            }
        });
    }
    if step > 0.0 {
        new = start + ((new - start) / step).round() * step;
    }
    new = new.clamp(start.min(end), start.max(end));
    if new != *value {
        *value = new;
        response.mark_changed();
    }
    let v = *value;
    response.widget_info(|| WidgetInfo::slider(true, f64::from(v), id_salt));
    let f = if end == start {
        0.0
    } else {
        (v - start) / (end - start)
    };
    let painter = ui.painter();
    let track = Rect::from_x_y_ranges(
        rect.x_range(),
        (rect.center().y - 4.0)..=(rect.center().y + 4.0),
    );
    let full = CornerRadius::same(u8::MAX);
    painter.rect_filled(track, full, t.muted);
    let x = egui::lerp(track_x, f);
    let mut filled = track;
    filled.set_right(x);
    painter.rect_filled(filled, full, t.primary);
    let center = pos2(x, rect.center().y);
    painter.circle_filled(center, thumb / 2.0, t.background);
    painter.circle_stroke(center, thumb / 2.0 - 0.75, Stroke::new(1.5, t.primary));
    if response.has_focus() {
        painter.circle_stroke(center, thumb / 2.0 + 3.0, t.ring_stroke());
    }
    response
}

/// A 44 px single-line text field filling `rect`'s width.
pub fn text_field(
    ui: &mut Ui,
    rect: Rect,
    id_salt: &str,
    value: &mut String,
    placeholder: &str,
) -> Response {
    let t = tokens(ui);
    let rect = Rect::from_min_size(rect.min, vec2(rect.width(), CONTROL));
    let response = ui.put(
        rect,
        TextEdit::singleline(value)
            .id_salt(("field", id_salt))
            .hint_text(RichText::new(placeholder).color(t.muted_foreground))
            .font(sans(16.0))
            .text_color(t.foreground)
            .desired_width(rect.width())
            .min_size(rect.size())
            .vertical_align(Align::Center)
            .frame(
                Frame::new()
                    .stroke(t.border_stroke())
                    .corner_radius(RADIUS)
                    .inner_margin(Margin::symmetric(14, 11)),
            ),
    );
    focus_ring(ui, &response, response.rect, RADIUS);
    response
}

/// A 44 px select filling `rect`'s width; its list opens below it.
pub fn select<T: AsRef<str>>(
    ui: &mut Ui,
    rect: Rect,
    id_salt: &str,
    selected: &mut usize,
    options: &[T],
) -> Response {
    let t = tokens(ui);
    let rect = Rect::from_min_size(rect.min, vec2(rect.width(), CONTROL));
    let mut response = ui.interact(rect, ui.id().with(("select", id_salt)), Sense::click());
    let current = options.get(*selected).map_or("Select…", AsRef::as_ref);
    response.widget_info(|| WidgetInfo::labeled(WidgetType::ComboBox, true, current));
    let painter = ui.painter();
    if response.hovered() {
        painter.rect_filled(rect, RADIUS, t.hover.gamma_multiply(0.35));
    }
    painter.rect_stroke(rect, RADIUS, t.border_stroke(), StrokeKind::Inside);
    text_truncated(
        painter,
        Rect::from_min_max(rect.min + vec2(14.0, 0.0), rect.max - vec2(40.0, 0.0)),
        Align2::LEFT_CENTER,
        current,
        sans(16.0),
        t.foreground,
    );
    chevron(
        painter,
        pos2(rect.right() - 20.0, rect.center().y),
        t.foreground,
    );
    focus_ring(ui, &response, rect, RADIUS);

    let mut changed = false;
    egui::Popup::from_toggle_button_response(&response)
        .width(rect.width())
        .gap(4.0)
        .frame(
            Frame::new()
                .fill(t.card)
                .stroke(t.border_stroke())
                .corner_radius(RADIUS)
                .inner_margin(4)
                .shadow(Shadow {
                    offset: [0, 6],
                    blur: 16,
                    spread: 0,
                    color: Color32::from_black_alpha(90),
                }),
        )
        .show(|ui| {
            ui.spacing_mut().item_spacing.y = 0.0;
            for (index, option) in options.iter().enumerate() {
                let (row, r) =
                    ui.allocate_exact_size(vec2(ui.available_width(), CONTROL), Sense::click());
                let on = index == *selected;
                r.widget_info(|| {
                    WidgetInfo::selected(WidgetType::SelectableLabel, true, on, option.as_ref())
                });
                if on || r.hovered() {
                    ui.painter().rect_filled(row, RADIUS, t.hover);
                }
                text_truncated(
                    ui.painter(),
                    row.shrink2(vec2(12.0, 0.0)),
                    Align2::LEFT_CENTER,
                    option.as_ref(),
                    sans(16.0),
                    t.foreground,
                );
                if r.clicked() && !on {
                    *selected = index;
                    changed = true;
                }
            }
        });
    if changed {
        response.mark_changed();
    }
    response
}

/// A small downward chevron centered on `center`.
pub fn chevron(painter: &Painter, center: Pos2, color: Color32) {
    painter.line(
        vec![
            center + vec2(-4.0, -2.0),
            center + vec2(0.0, 2.0),
            center + vec2(4.0, -2.0),
        ],
        Stroke::new(1.5, color),
    );
}

/// A key cap, 20 px tall, right-aligned in `rect` and vertically centered.
pub fn kbd(painter: &Painter, rect: Rect, key: &str, t: &Tokens) {
    let g = galley(painter, key, mono(12.0), t.muted_foreground);
    let size = vec2((g.size().x + 8.0).max(20.0), 20.0);
    let cap = Align2::RIGHT_CENTER.align_size_within_rect(size, rect);
    painter.rect_filled(cap, 4, t.muted);
    painter.galley(
        Align2::CENTER_CENTER
            .anchor_size(cap.center(), g.size())
            .min,
        g,
        t.muted_foreground,
    );
}

/// An outline badge, 22 px tall, centered horizontally on `center_top`.
pub fn badge(painter: &Painter, center_top: Pos2, label: &str, t: &Tokens) -> Rect {
    let g = galley(painter, label, sans(12.0), t.foreground);
    let size = vec2(g.size().x + 20.0, 22.0);
    let rect = Rect::from_min_size(pos2(center_top.x - size.x / 2.0, center_top.y), size);
    painter.rect_stroke(rect, u8::MAX, t.border_stroke(), StrokeKind::Inside);
    painter.galley(
        Align2::CENTER_CENTER
            .anchor_size(rect.center(), g.size())
            .min,
        g,
        t.foreground,
    );
    rect
}

/// Top of the derisk wallpaper gradient.
pub const WALLPAPER_TOP: Color32 = Color32::from_rgb(0x11, 0x18, 0x27);
/// Bottom of the derisk wallpaper gradient.
pub const WALLPAPER_BOTTOM: Color32 = Color32::from_rgb(0x1e, 0x1b, 0x4b);

/// Fills `rect` with the wallpaper's vertical gradient, rounded by `radius`.
pub fn wallpaper(ui: &Ui, rect: Rect, radius: u8) {
    let texture = ui
        .ctx()
        .data_mut(|d| d.get_temp::<egui::TextureHandle>(egui::Id::new("derisk-portal-wallpaper")));
    let texture = texture.unwrap_or_else(|| {
        let image = egui::ColorImage::new([1, 2], vec![WALLPAPER_TOP, WALLPAPER_BOTTOM]);
        let handle = ui.ctx().load_texture(
            "derisk-portal-wallpaper",
            image,
            egui::TextureOptions::LINEAR,
        );
        ui.ctx().data_mut(|d| {
            d.insert_temp(egui::Id::new("derisk-portal-wallpaper"), handle.clone());
        });
        handle
    });
    // Texel centers sit at v = 0.25 and 0.75; sampling between them with
    // linear filtering is exactly the two-stop gradient.
    let uv = Rect::from_min_max(pos2(0.5, 0.25), pos2(0.5, 0.75));
    ui.painter().add(
        egui::epaint::RectShape::filled(rect, radius, Color32::WHITE)
            .with_texture(texture.id(), uv),
    );
}
