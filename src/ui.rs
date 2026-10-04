//! egui rendering of the shell surfaces.
//!
//! The compositor renders three passes per output:
//!
//! 1. [`ShellUi::paint_decorations`] below client surfaces (title bars).
//! 2. Client surfaces, at [`crate::shell::WindowPlacement::client`].
//! 3. [`ShellUi::show`] above them: top bar with global menu and tray, snap
//!    preview and Snap Assist, the overview with widgets, the command
//!    palette, and the startup animation.
//!
//! Logical compositor pixels map 1:1 to egui points; set
//! `pixels_per_point` to the output scale.

use std::{cell::RefCell, collections::HashMap, path::PathBuf, sync::Arc};

use egui::{
    Align, Align2, Color32, CornerRadius, FontId, Id, Key, Layout, Modifiers, Order, Painter, Pos2,
    Rect, RichText, Sense, Stroke, StrokeKind, TextureHandle, TextureOptions, Ui, UiBuilder, pos2,
    vec2,
};
use mcsapi::{Geometry, WindowId, toolkit::egui, widgets::Theme};

use crate::{
    action::Action,
    animation::{StartupAnimation, StartupFrame},
    apps::AppLook,
    conversation::{Source, StepStatus, Turn},
    decorations::Button,
    effects::{BlurArea, Look},
    geom::inset,
    icons,
    menu::{Menu, MenuEntry},
    overview::{OverviewLayout, Widget, fit, grid, row},
    palette::{self, Category, Entry, History},
    shell::{DropTarget, Shell, WindowPlacement},
};

/// Converts a logical geometry to an egui rectangle.
pub fn to_rect(g: Geometry) -> Rect {
    Rect::from_min_size(
        pos2(g.loc.x as f32, g.loc.y as f32),
        vec2(g.size.w as f32, g.size.h as f32),
    )
}

fn button_color(button: Button) -> Color32 {
    match button {
        Button::Close => Color32::from_rgb(239, 68, 68),
        Button::Minimize => Color32::from_rgb(245, 158, 11),
        Button::Maximize => Color32::from_rgb(34, 197, 94),
    }
}

fn elide(text: &str, width: f32, size: f32) -> String {
    let max = ((width / (size * 0.55)) as usize).max(1);
    if text.chars().count() <= max {
        text.to_owned()
    } else {
        let mut s: String = text.chars().take(max.saturating_sub(1)).collect();
        s.push('…');
        s
    }
}

fn card<R>(ui: &mut Ui, theme: &Theme, add: impl FnOnce(&mut Ui) -> R) -> R {
    egui::Frame::new()
        .fill(theme.surface)
        .stroke(Stroke::new(1.0, theme.border))
        .corner_radius(12)
        .inner_margin(12)
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            add(ui)
        })
        .inner
}

/// Persistent UI state for the shell chrome.
pub struct ShellUi {
    /// Shared colors (from mcsapi).
    pub theme: Theme,
    /// The startup animation.
    pub startup: StartupAnimation,
    notes: String,
    overview_open: bool,
    /// Window being dragged in the overview, toward a workspace.
    overview_drag: Option<WindowId>,
    tray: Option<(u64, Vec<TextureHandle>)>,
    /// The command palette.
    pub palette: PaletteUi,
    reply: Option<String>,
    reduced_motion: bool,
    look: Look,
    blurs: Vec<BlurArea>,
    /// Decoded app icons by theme name or path, `None` when the theme has
    /// none. Behind a `RefCell` because title bars paint through `&self`.
    icons: RefCell<HashMap<String, Option<Arc<egui::ColorImage>>>>,
}

/// Pixels app icons are decoded at: crisp up to 32 points at 2x, the largest
/// the shell draws them on a HiDPI screen.
const ICON_PX: u32 = 64;

/// Command palette state. The host fills [`PaletteUi::extra`] and
/// [`PaletteUi::files`]; the rest is kept between openings.
#[derive(Default)]
pub struct PaletteUi {
    /// Host-provided entries: apps, settings pages.
    pub extra: Vec<Entry>,
    /// Indexed files (see [`palette::index_files`]).
    pub files: Vec<PathBuf>,
    /// What was chosen before, for ranking.
    pub history: History,
    /// Text to start the next opening with, instead of an empty query.
    pub preset: Option<String>,
    /// Open on the conversation next time instead of the search.
    pub chat_next: bool,
    chat: bool,
    seen: u64,
    cleared: u64,
    asks: Vec<Ask>,
    query: String,
    shown_query: String,
    selected: usize,
    armed: Option<String>,
    message: Option<String>,
    open: bool,
}

/// Rows the palette shows at once before scrolling.
const PALETTE_ROWS: usize = 9;
const PALETTE_ROW_HEIGHT: f32 = 40.0;

fn limit_palette_hits(entries: &[Entry], hits: &[usize], limit: usize) -> Vec<usize> {
    if hits.len() <= limit {
        return hits.to_vec();
    }

    let mut categories: Vec<(Category, Vec<usize>)> = Vec::new();
    for &hit in hits {
        let category = entries[hit].category;
        if let Some((_, category_hits)) = categories
            .iter_mut()
            .find(|(existing, _)| *existing == category)
        {
            category_hits.push(hit);
        } else {
            categories.push((category, vec![hit]));
        }
    }

    let base_quota = limit / categories.len();
    let extra_quota = limit % categories.len();
    let mut included = vec![false; entries.len()];
    let mut included_count = 0;
    for (index, (_, category_hits)) in categories.iter().enumerate() {
        let quota = base_quota + usize::from(index < extra_quota);
        for &hit in category_hits.iter().take(quota) {
            included[hit] = true;
            included_count += 1;
        }
    }
    for &hit in hits {
        if included_count == limit {
            break;
        }
        if !included[hit] {
            included[hit] = true;
            included_count += 1;
        }
    }

    categories
        .into_iter()
        .flat_map(|(_, category_hits)| category_hits.into_iter().filter(|&hit| included[hit]))
        .collect()
}

impl ShellUi {
    /// Creates UI state; `reduced_motion` shortens the startup animation, as
    /// do the reduce motion setting and low power mode.
    pub fn new(shell: &Shell, reduced_motion: bool) -> Self {
        Self {
            theme: Theme::default(),
            startup: StartupAnimation {
                reduced_motion,
                bar_height: shell.profile().top_bar as f32,
            },
            notes: String::new(),
            overview_open: false,
            overview_drag: None,
            tray: None,
            palette: PaletteUi::default(),
            reply: None,
            reduced_motion,
            look: shell.look(),
            blurs: Vec::new(),
            icons: RefCell::default(),
        }
    }

    /// Paints an app's icon into `r`: the icon theme's, else the app's
    /// glyph, else its initial on an accent tile, so every window has one.
    fn paint_app_icon(&self, painter: &Painter, r: Rect, look: &AppLook) {
        let image = self
            .icons
            .borrow_mut()
            .entry(look.icon.to_owned())
            .or_insert_with(|| {
                icons::load(&icons::find(look.icon, ICON_PX)?, ICON_PX).map(Arc::new)
            })
            .clone();
        // Title bars and the chrome are separate egui contexts with their own
        // textures, so each context uploads the icon once and keeps it.
        let texture = image.map(|image| {
            let ctx = painter.ctx();
            let id = Id::new(("derisk-app-icon", look.icon));
            ctx.data(|d| d.get_temp::<TextureHandle>(id))
                .unwrap_or_else(|| {
                    let texture = ctx.load_texture(
                        format!("derisk-app-icon:{}", look.icon),
                        image,
                        TextureOptions::LINEAR,
                    );
                    ctx.data_mut(|d| d.insert_temp(id, texture.clone()));
                    texture
                })
        });
        if let Some(texture) = texture {
            let [w, h] = texture.size().map(|n| n.max(1) as f32);
            let scale = r.width().min(r.height()) / w.max(h);
            painter.image(
                texture.id(),
                Rect::from_center_size(r.center(), vec2(w, h) * scale),
                Rect::from_min_max(Pos2::ZERO, pos2(1.0, 1.0)),
                Color32::WHITE,
            );
        } else if !look.glyph.is_empty() {
            painter.text(
                r.center(),
                Align2::CENTER_CENTER,
                look.glyph,
                FontId::proportional(r.height() * 0.8),
                self.theme.foreground,
            );
        } else {
            painter.rect_filled(r, r.height() * 0.25, self.theme.accent);
            let initial: String = look
                .name
                .chars()
                .take(1)
                .flat_map(char::to_uppercase)
                .collect();
            painter.text(
                r.center(),
                Align2::CENTER_CENTER,
                initial,
                FontId::proportional(r.height() * 0.6),
                self.theme.background,
            );
        }
    }

    /// Paints an app icon and `text` on one line, the pair centered on
    /// `center` and elided to `width`.
    #[allow(clippy::too_many_arguments)]
    fn paint_icon_label(
        &self,
        painter: &Painter,
        center: Pos2,
        width: f32,
        look: &AppLook,
        text: &str,
        size: f32,
        color: Color32,
    ) {
        let icon = (size * 1.3).round();
        let gap = (size * 0.45).round();
        let galley = painter.layout_no_wrap(
            elide(text, width - icon - gap, size),
            FontId::proportional(size),
            color,
        );
        let left = center.x - (icon + gap + galley.size().x) / 2.0;
        self.paint_app_icon(
            painter,
            Rect::from_min_size(pos2(left, center.y - icon / 2.0), vec2(icon, icon)),
            look,
        );
        painter.galley(
            pos2(left + icon + gap, center.y - galley.size().y / 2.0),
            galley,
            color,
        );
    }

    /// A window card's label (overview, Snap Assist): the app icon, the app
    /// name under it, and the window title under that when there is one.
    fn paint_window_card(&self, painter: &Painter, r: Rect, look: &AppLook, title: &str) {
        let icon = (r.height() * 0.3).clamp(16.0, 48.0);
        let name = (icon * 0.4).clamp(12.0, 18.0);
        let top = r.center().y - (icon + 6.0 + name + 4.0 + name * 0.75) / 2.0;
        self.paint_app_icon(
            painter,
            Rect::from_center_size(pos2(r.center().x, top + icon / 2.0), vec2(icon, icon)),
            look,
        );
        let name_y = top + icon + 6.0 + name / 2.0;
        painter.text(
            pos2(r.center().x, name_y),
            Align2::CENTER_CENTER,
            elide(&look.name, r.width() - 16.0, name),
            FontId::proportional(name),
            self.theme.foreground,
        );
        // The title only adds something when it is not just the name again.
        if !title.is_empty() && !title.eq_ignore_ascii_case(&look.name) {
            painter.text(
                pos2(r.center().x, name_y + name / 2.0 + 4.0 + name * 0.375),
                Align2::CENTER_CENTER,
                elide(title, r.width() - 16.0, name * 0.75),
                FontId::proportional(name * 0.75),
                self.theme.border,
            );
        }
    }

    /// Areas to blur under the translucent panels shown by the last
    /// [`ShellUi::show`], bottom to top. Empty when blur is off.
    pub fn blur_regions(&self) -> &[BlurArea] {
        &self.blurs
    }

    /// The startup animation's frame `elapsed_ms` after start. Low power mode
    /// skips the animation; reduced motion turns it into a cross-fade.
    pub fn startup_frame(&self, elapsed_ms: u32) -> StartupFrame {
        if self.look.low_power {
            StartupFrame::DONE
        } else {
            self.startup.frame(elapsed_ms)
        }
    }

    /// Records a panel filled with the background at `opacity` so the
    /// compositor blurs behind it. Nothing shows through an opaque or
    /// invisible panel, so those aren't blurred.
    fn frost(&mut self, area: Rect, corner_radius: u8, opacity: f32) {
        let opacity = opacity * f32::from(self.theme.background.a()) / 255.0;
        if self.look.blur == 0 || opacity <= 0.0 || opacity >= 1.0 {
            return;
        }
        let area = Geometry::new(
            (area.min.x.round() as i32, area.min.y.round() as i32).into(),
            (area.width().round() as i32, area.height().round() as i32).into(),
        );
        self.blurs.push(BlurArea {
            area,
            corner_radius,
            strength: self.look.blur,
        });
    }

    /// The assistant's last reply, if any.
    pub fn assistant_reply(&self) -> Option<&str> {
        self.reply.as_deref()
    }

    /// Paints server-side title bars, buttons on the left, bottom to top.
    pub fn paint_decorations(&self, painter: &Painter, shell: &Shell) {
        for p in shell.placements() {
            self.paint_decoration(painter, shell, &p);
        }
    }

    /// Paints one window's title bar and border.
    ///
    /// Hosts that interleave decorations with client surfaces (so a window
    /// above covers the title bar of one below) call this per placement.
    pub fn paint_decoration(&self, painter: &Painter, shell: &Shell, p: &WindowPlacement) {
        let bar = shell.profile().title_bar;
        let theme = &self.theme;
        let radius = CornerRadius {
            nw: 10,
            ne: 10,
            sw: 0,
            se: 0,
        };
        // A soft shadow separates overlapping windows.
        if shell.look().shadows {
            painter.add(
                egui::epaint::Shadow {
                    offset: [0, 6],
                    blur: if p.focused { 28 } else { 16 },
                    spread: 0,
                    color: Color32::from_black_alpha(if p.focused { 150 } else { 90 }),
                }
                .as_shape(to_rect(p.frame), radius),
            );
        }
        painter.rect_filled(
            to_rect(bar.bar(p.frame)),
            radius,
            if p.focused {
                theme.surface
            } else {
                theme.background
            },
        );
        painter.rect_stroke(
            to_rect(p.frame),
            radius,
            Stroke::new(
                1.0,
                if p.focused {
                    theme.accent
                } else {
                    theme.border
                },
            ),
            StrokeKind::Inside,
        );
        for (button, area) in bar.buttons(p.frame) {
            let color = if p.focused {
                button_color(button)
            } else {
                theme.border
            };
            let r = to_rect(area);
            painter.circle_filled(r.center(), r.width() / 2.0, color);
        }
        let title_area = to_rect(bar.title(p.frame));
        let (app, title) = shell.window_label(p.window).unwrap_or_default();
        let look = shell.apps.look(app);
        let size = (bar.height as f32 * 0.42).max(11.0);
        self.paint_icon_label(
            painter,
            title_area.center(),
            title_area.width(),
            &look,
            if title.is_empty() { &look.name } else { title },
            size,
            theme.foreground,
        );
    }

    /// Shows the chrome above client surfaces and returns requested actions.
    ///
    /// `elapsed_ms` is time since the session started (drives the startup
    /// animation). Pass the root `Ui` from `Context::run_ui`.
    pub fn show(&mut self, ui: &mut Ui, shell: &Shell, elapsed_ms: u32) -> Vec<Action> {
        let mut actions = Vec::new();
        self.look = shell.look();
        self.blurs.clear();
        self.startup.reduced_motion = self.reduced_motion || !self.look.animate;
        let frame = self.startup_frame(elapsed_ms);
        self.drag_preview(ui, shell);
        self.snap_assist(ui, shell, &mut actions);
        if shell.overview_visible() {
            self.overview(ui, shell, &mut actions);
        }
        self.overview_open = shell.overview_visible();
        if !self.overview_open {
            self.overview_drag = None;
        }
        self.top_bar(ui, shell, frame, &mut actions);
        if shell.palette_visible() {
            self.palette(ui, shell, &mut actions);
        }
        self.palette.open = shell.palette_visible();
        if !frame.done {
            let painter = ui.ctx().layer_painter(egui::LayerId::new(
                Order::Foreground,
                Id::new("derisk-startup"),
            ));
            paint_startup(&painter, to_rect(shell.output()), frame, &self.theme);
            ui.ctx().request_repaint();
        }
        actions
    }

    fn drag_preview(&self, ui: &Ui, shell: &Shell) {
        let preview = match shell.drag_target() {
            Some(DropTarget::Snap { preview, .. } | DropTarget::Tile { preview, .. }) => preview,
            _ => return,
        };
        let r = to_rect(preview);
        let painter = ui.painter();
        painter.rect_filled(r, 12, self.theme.accent.gamma_multiply(0.18));
        painter.rect_stroke(
            r,
            12,
            Stroke::new(2.0, self.theme.accent),
            StrokeKind::Inside,
        );
    }

    fn snap_assist(&mut self, ui: &mut Ui, shell: &Shell, actions: &mut Vec<Action>) {
        let Some(assist) = shell.snap_assist() else {
            return;
        };
        self.frost(to_rect(assist.frame), 12, self.look.snap_assist);
        ui.painter().rect_filled(
            to_rect(assist.frame),
            12,
            self.theme.background.gamma_multiply(self.look.snap_assist),
        );
        let cells = grid(assist.candidates.len(), assist.frame, 16);
        for (window, cell) in assist.candidates.iter().zip(cells) {
            let r = to_rect(inset(cell, 8));
            let response = ui.interact(r, Id::new(("derisk-assist", window.get())), Sense::click());
            let stroke = if response.hovered() {
                self.theme.accent
            } else {
                self.theme.border
            };
            ui.painter().rect_filled(r, 10, self.theme.surface);
            ui.painter()
                .rect_stroke(r, 10, Stroke::new(1.5, stroke), StrokeKind::Inside);
            let (app, title) = shell.window_label(*window).unwrap_or_default();
            self.paint_window_card(ui.painter(), r, &shell.apps.look(app), title);
            if response.clicked() {
                actions.push(Action::Snap {
                    window: Some(window.get()),
                    zone: assist.zone,
                });
            }
        }
    }

    fn top_bar(
        &mut self,
        ui: &mut Ui,
        shell: &Shell,
        frame: StartupFrame,
        actions: &mut Vec<Action>,
    ) {
        let output = to_rect(shell.output());
        let height = shell.profile().top_bar as f32;
        let bar = Rect::from_min_size(
            output.min + vec2(0.0, frame.bar_offset),
            vec2(output.width(), height),
        );
        let opacity = frame.shell_opacity.max(0.0);
        self.frost(bar, 0, self.look.top_bar * opacity);
        ui.painter().rect_filled(
            bar,
            0,
            self.theme
                .background
                .gamma_multiply(self.look.top_bar * opacity),
        );
        let focused = shell.focused();
        ui.scope_builder(
            UiBuilder::new()
                .id_salt("derisk-top-bar")
                .max_rect(bar.shrink2(vec2(10.0, 2.0)))
                .layout(Layout::left_to_right(Align::Center)),
            |ui| {
                ui.visuals_mut().override_text_color = Some(self.theme.foreground);
                ui.spacing_mut().item_spacing.x = 12.0;
                if ui
                    .add(
                        egui::Button::new(RichText::new("◆").color(self.theme.accent)).frame(false),
                    )
                    .on_hover_text("Overview")
                    .clicked()
                {
                    actions.push(Action::Overview { visible: None });
                }
                if ui
                    .add(
                        egui::Button::new(
                            RichText::new("🔍  Search or ask…").color(self.theme.border),
                        )
                        .fill(self.theme.surface)
                        .stroke(Stroke::new(1.0, self.theme.border))
                        .corner_radius(8),
                    )
                    .on_hover_text("Command palette (Super+Space)")
                    .clicked()
                {
                    actions.push(Action::Palette { visible: None });
                }
                if let Some(w) = focused {
                    let (app, _) = shell.window_label(w).unwrap_or_default();
                    let look = shell.apps.look(app);
                    let side = (height * 0.6).round();
                    let (r, _) = ui.allocate_exact_size(vec2(side, side), Sense::hover());
                    ui.spacing_mut().item_spacing.x = 6.0;
                    self.paint_app_icon(ui.painter(), r, &look);
                    ui.spacing_mut().item_spacing.x = 12.0;
                    ui.label(RichText::new(look.name.as_ref()).strong());
                }
                for menu in shell.menus.bar(focused.map(|w| w.get())) {
                    menu_button(ui, &menu, focused.map(|w| w.get()), actions);
                }
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    ui.label(format!(
                        "{}  {}",
                        shell.clock.date_label(),
                        shell.clock.time_label()
                    ));
                    if let Some(b) = shell.battery {
                        ui.label(format!(
                            "{}{}%",
                            if b.charging { "⚡" } else { "▮" },
                            b.percent
                        ));
                    }
                    if !shell.failed_units.is_empty()
                        && ui
                            .add(
                                egui::Button::new(format!("⚠ {}", shell.failed_units.len()))
                                    .frame(false),
                            )
                            .on_hover_text("Failed user services: restart or dismiss them")
                            .clicked()
                    {
                        self.palette.preset = Some("> failed".to_owned());
                        actions.push(Action::Palette {
                            visible: Some(true),
                        });
                    }
                    // Agent activity the person has not looked at yet.
                    let unseen = shell
                        .conversation
                        .turns()
                        .filter(|t| t.id > self.palette.seen && t.source == Source::Agent)
                        .count();
                    let working = shell.conversation.turns().any(|t| !t.is_settled());
                    if (unseen > 0 || working)
                        && ui
                            .add(
                                egui::Button::new(
                                    RichText::new(if unseen > 0 {
                                        format!("✨ {unseen}")
                                    } else {
                                        "✨".to_owned()
                                    })
                                    .color(self.theme.accent),
                                )
                                .frame(false),
                            )
                            .on_hover_text(if working {
                                "derisk is working on a request"
                            } else {
                                "Agent activity: open the conversation"
                            })
                            .clicked()
                    {
                        self.palette.chat_next = true;
                        actions.push(Action::Palette {
                            visible: Some(true),
                        });
                    }
                    self.tray_icons(ui, shell, height, actions);
                });
            },
        );
    }

    fn tray_icons(&mut self, ui: &mut Ui, shell: &Shell, bar: f32, actions: &mut Vec<Action>) {
        let generation = shell.tray.generation();
        if self.tray.as_ref().is_none_or(|(g, _)| *g != generation) {
            let textures = shell
                .tray
                .items()
                .map(|item| {
                    let image = egui::ColorImage::from_rgba_unmultiplied(
                        [item.icon.width as usize, item.icon.height as usize],
                        &item.icon.rgba,
                    );
                    ui.ctx().load_texture(
                        format!("derisk-tray-{}", item.id),
                        image,
                        TextureOptions::LINEAR,
                    )
                })
                .collect();
            self.tray = Some((generation, textures));
        }
        let size = (bar * 0.6).round();
        let textures = self
            .tray
            .as_ref()
            .map(|(_, t)| t.as_slice())
            .unwrap_or_default();
        // Right-to-left layout: reverse so items read left to right.
        for (item, tex) in shell.tray.items().zip(textures).rev() {
            let response = ui
                .add(
                    egui::Button::image(egui::Image::new((tex.id(), vec2(size, size))))
                        .frame(false),
                )
                .on_hover_text(&item.title);
            if response.clicked() {
                actions.push(Action::ActivateTray {
                    id: item.id.clone(),
                    item: None,
                });
            }
            if !item.menu.is_empty() {
                response.context_menu(|ui| {
                    entries(
                        ui,
                        &item.menu,
                        &mut |id| Action::ActivateTray {
                            id: item.id.clone(),
                            item: Some(id),
                        },
                        actions,
                    );
                });
            }
        }
    }

    fn overview(&mut self, ui: &mut Ui, shell: &Shell, actions: &mut Vec<Action>) {
        // Typing on the overview (outside the notes) asks or searches in the
        // palette, which is where the assistant lives.
        if !shell.palette_visible() && ui.ctx().memory(|m| m.focused().is_none()) {
            let typed: String = ui.input(|i| {
                i.events
                    .iter()
                    .filter_map(|e| match e {
                        egui::Event::Text(t) => Some(t.as_str()),
                        _ => None,
                    })
                    .collect()
            });
            if !typed.trim().is_empty() {
                self.palette.preset = Some(typed);
                actions.push(Action::Palette {
                    visible: Some(true),
                });
            }
        }
        let area = shell.work_area();
        self.frost(to_rect(area), 0, self.look.overview);
        ui.painter().rect_filled(
            to_rect(area),
            0,
            self.theme.background.gamma_multiply(self.look.overview),
        );
        let layout = OverviewLayout::new(area, shell.profile().form_factor);

        // Workspace strip, Mission Control style: one cell per open workspace,
        // then a "+" slot that opens a new one. Windows dragged from the grid
        // drop onto either.
        let open = shell.workspaces();
        let active = shell.active_workspace();
        let slots = open.len() + usize::from(shell.can_add_workspace());
        let cells = row(slots, layout.workspaces, 10);
        let pointer = ui.input(|i| i.pointer.latest_pos());
        self.overview_drag = self
            .overview_drag
            .filter(|w| shell.workspace_of(*w) == Some(open[active as usize - 1]));
        let dragging = self.overview_drag;
        let drop_on = pointer.and_then(|p| cells.iter().position(|c| to_rect(*c).contains(p)));
        for (i, cell) in cells.iter().enumerate() {
            let n = i as u64 + 1;
            let r = to_rect(*cell);
            let response = ui.interact(r, Id::new(("derisk-ws", n)), Sense::click());
            let hot = if dragging.is_some() {
                drop_on == Some(i)
            } else {
                response.hovered()
            };
            let Some(&ws) = open.get(i) else {
                // The "+" slot.
                ui.painter().rect_filled(
                    r,
                    10,
                    if hot {
                        self.theme.surface
                    } else {
                        self.theme.background
                    },
                );
                ui.painter().rect_stroke(
                    r,
                    10,
                    Stroke::new(
                        if hot { 2.0 } else { 1.0 },
                        if hot {
                            self.theme.accent
                        } else {
                            self.theme.border
                        },
                    ),
                    StrokeKind::Inside,
                );
                ui.painter().text(
                    r.center(),
                    Align2::CENTER_CENTER,
                    "+",
                    FontId::proportional(24.0),
                    if hot {
                        self.theme.accent
                    } else {
                        self.theme.foreground
                    },
                );
                if response.on_hover_text("New workspace").clicked() {
                    actions.push(Action::SwitchWorkspace { workspace: n });
                }
                continue;
            };
            let windows = shell.windows_on(ws);
            ui.painter().rect_filled(r, 10, self.theme.surface);
            ui.painter().rect_stroke(
                r,
                10,
                Stroke::new(
                    if n == active || hot { 2.0 } else { 1.0 },
                    if n == active || hot {
                        self.theme.accent
                    } else {
                        self.theme.border
                    },
                ),
                StrokeKind::Inside,
            );
            // A miniature of each window, so workspaces read as thumbnails.
            let minis = grid(windows.len(), inset(*cell, 12), 4);
            for mini in &minis {
                ui.painter()
                    .rect_filled(to_rect(*mini), 3, self.theme.border.gamma_multiply(0.35));
            }
            ui.painter().text(
                r.center(),
                Align2::CENTER_CENTER,
                format!("{n}"),
                FontId::proportional(15.0),
                self.theme.foreground,
            );
            if response.clicked() {
                actions.push(Action::SwitchWorkspace { workspace: n });
            }
        }

        // Window grid (exposé), including minimized windows. Drag a window
        // onto a workspace to move it there.
        let windows = shell.windows_on(open[active as usize - 1]);
        let placements = shell.placements();
        let mut ghost = None;
        for (w, cell) in windows.iter().zip(grid(windows.len(), layout.windows, 24)) {
            let frame = placements
                .iter()
                .find(|p| p.window == *w)
                .map_or(cell, |p| p.frame);
            let r = to_rect(fit(frame, cell));
            let response =
                ui.interact(r, Id::new(("derisk-win", w.get())), Sense::click_and_drag());
            if response.drag_started_by(egui::PointerButton::Primary) {
                self.overview_drag = Some(*w);
            }
            let lifted = self.overview_drag == Some(*w);
            if lifted {
                ghost = Some((*w, r.size()));
            }
            let minimized = shell.is_minimized(*w);
            let hovered = response.hovered() && dragging.is_none();
            let fill = if minimized {
                self.theme.background
            } else {
                self.theme.surface
            };
            ui.painter().rect_filled(
                r,
                12,
                if lifted {
                    fill.gamma_multiply(0.4)
                } else {
                    fill
                },
            );
            ui.painter().rect_stroke(
                r,
                12,
                Stroke::new(
                    if hovered { 2.0 } else { 1.0 },
                    if hovered {
                        self.theme.accent
                    } else {
                        self.theme.border
                    },
                ),
                StrokeKind::Inside,
            );
            let (app, title) = shell.window_label(*w).unwrap_or_default();
            self.paint_window_card(ui.painter(), r, &shell.apps.look(app), title);
            if response.clicked() {
                actions.push(if minimized {
                    Action::Restore { window: w.get() }
                } else {
                    Action::Focus { window: w.get() }
                });
                actions.push(Action::Overview {
                    visible: Some(false),
                });
            }
        }

        // The lifted window follows the pointer, shrinking like a thumbnail.
        if let (Some((w, size)), Some(p)) = (ghost, pointer) {
            let scale = (180.0 / size.x.max(1.0)).min(1.0);
            let r = Rect::from_center_size(p, size * scale);
            let painter = ui.ctx().layer_painter(egui::LayerId::new(
                Order::Foreground,
                Id::new("derisk-overview-drag"),
            ));
            painter.rect_filled(r, 8, self.theme.surface.gamma_multiply(0.9));
            painter.rect_stroke(
                r,
                8,
                Stroke::new(2.0, self.theme.accent),
                StrokeKind::Inside,
            );
            let (app, _) = shell.window_label(w).unwrap_or_default();
            let look = shell.apps.look(app);
            self.paint_icon_label(
                &painter,
                r.center(),
                r.width() - 12.0,
                &look,
                &look.name,
                14.0,
                self.theme.foreground,
            );
        }
        if let Some(w) = self.overview_drag
            && ui.input(|i| i.pointer.primary_released())
        {
            self.overview_drag = None;
            if let Some(i) = drop_on {
                let n = i as u64 + 1;
                if n != active {
                    actions.push(Action::MoveToWorkspace {
                        window: Some(w.get()),
                        workspace: n,
                    });
                }
            }
        }

        // Widgets.
        ui.scope_builder(
            UiBuilder::new()
                .id_salt("derisk-widgets")
                .max_rect(to_rect(layout.widgets))
                .layout(Layout::top_down(Align::Min)),
            |ui| {
                ui.visuals_mut().override_text_color = Some(self.theme.foreground);
                egui::ScrollArea::vertical().show(ui, |ui| {
                    for widget in Widget::DEFAULT {
                        self.widget(ui, shell, widget, actions);
                    }
                });
            },
        );
    }

    fn widget(&mut self, ui: &mut Ui, shell: &Shell, widget: Widget, actions: &mut Vec<Action>) {
        let theme = self.theme;
        match widget {
            Widget::Clock => card(ui, &theme, |ui| {
                ui.label(RichText::new(shell.clock.time_label()).size(44.0).strong());
                ui.label(shell.clock.date_label());
            }),
            Widget::Calendar => card(ui, &theme, |ui| calendar(ui, shell, &theme)),
            Widget::Battery => {
                let Some(b) = shell.battery else { return };
                card(ui, &theme, |ui| {
                    ui.label(if b.charging {
                        "Battery · charging"
                    } else {
                        "Battery"
                    });
                    ui.add(
                        egui::ProgressBar::new(f32::from(b.percent) / 100.0)
                            .text(format!("{}%", b.percent)),
                    );
                });
            }
            Widget::Suggestions => card(ui, &theme, |ui| {
                ui.label(RichText::new("Suggested").strong());
                let suggestions = shell.habits.suggestions(shell.clock.hour, 5);
                if suggestions.is_empty() {
                    ui.label(RichText::new("Apps you use will appear here.").color(theme.border));
                }
                ui.horizontal_wrapped(|ui| {
                    for app in suggestions {
                        if ui.button(&app).clicked() {
                            actions.push(Action::Launch { app });
                            actions.push(Action::Overview {
                                visible: Some(false),
                            });
                        }
                    }
                });
            }),
            Widget::Units => card(ui, &theme, |ui| {
                ui.label(RichText::new("Services").strong());
                if shell.failed_units.is_empty() {
                    ui.label(RichText::new("All user services running ✓").color(theme.border));
                }
                for unit in &shell.failed_units {
                    ui.horizontal(|ui| {
                        ui.label(RichText::new(unit).color(button_color(Button::Close)));
                        if ui.small_button("Restart").clicked() {
                            actions.push(Action::RestartUnit { unit: unit.clone() });
                        }
                        if ui.small_button("Dismiss").clicked() {
                            actions.push(Action::ResetFailed { unit: unit.clone() });
                        }
                    });
                }
            }),
            Widget::Notes => card(ui, &theme, |ui| {
                ui.label(RichText::new("Notes").strong());
                ui.add(
                    egui::TextEdit::multiline(&mut self.notes)
                        .desired_rows(4)
                        .desired_width(f32::INFINITY),
                );
            }),
        }
        ui.add_space(10.0);
    }

    /// The command palette: a search box over [`palette::entries`] with the
    /// assistant as fallback, and the agent conversation (chat view).
    fn palette(&mut self, ui: &mut Ui, shell: &Shell, actions: &mut Vec<Action>) {
        let theme = self.theme;
        let state = &mut self.palette;
        let mut cursor_to_end = false;
        if !state.open {
            state.query = state.preset.take().unwrap_or_default();
            // The field only gets focus next frame; keep what is typed now.
            let typed: String = ui.input_mut(|i| {
                let mut typed = String::new();
                i.events.retain(|e| match e {
                    egui::Event::Text(t) => {
                        typed.push_str(t);
                        false
                    }
                    _ => true,
                });
                typed
            });
            state.query.push_str(&typed);
            cursor_to_end = !state.query.is_empty();
            state.chat = std::mem::take(&mut state.chat_next);
            state.shown_query.clear();
            state.selected = 0;
            state.armed = None;
            state.message = None;
        }
        // `?` switches to the conversation; the rest is the request.
        if !state.chat
            && let Some(rest) = state.query.trim_start().strip_prefix('?')
        {
            state.query = rest.trim_start().to_owned();
            state.chat = true;
        }

        let (entries, hits, ask) = if state.chat {
            (Vec::new(), Vec::new(), None)
        } else {
            let entries = palette::entries(shell, &state.extra, &state.files);
            let hits = palette::search(&entries, &state.query, &state.history);
            let (scope, text) = palette::scope(&state.query);
            // The assistant joins everything-searches; a request it does not
            // understand only shows when nothing else matched, to say why.
            let ask = (!text.is_empty() && scope == palette::Scope::All)
                .then(|| palette::ask(&state.query))
                .filter(|ask| !ask.actions.is_empty() || hits.is_empty());
            (entries, hits, ask)
        };
        let mut rows: Vec<&Entry> = limit_palette_hits(&entries, &hits, 60)
            .iter()
            .map(|&i| &entries[i])
            .collect();
        if let Some(ask) = &ask {
            if palette::prefer_assistant(&entries, &hits, &state.query) {
                rows.insert(0, ask);
            } else {
                rows.push(ask);
            }
        }
        if state.query != state.shown_query {
            state.shown_query = state.query.clone();
            state.selected = 0;
            state.armed = None;
            if !state.chat {
                state.message = None;
            }
        }
        state.selected = state.selected.min(rows.len().saturating_sub(1));

        // Keys the text field would otherwise eat.
        let empty = state.query.is_empty();
        let (down, up, enter, escape, back) = ui.input_mut(|i| {
            (
                i.consume_key(Modifiers::NONE, Key::ArrowDown)
                    || i.consume_key(Modifiers::NONE, Key::Tab)
                    || i.consume_key(Modifiers::CTRL, Key::N),
                i.consume_key(Modifiers::NONE, Key::ArrowUp)
                    || i.consume_key(Modifiers::SHIFT, Key::Tab)
                    || i.consume_key(Modifiers::CTRL, Key::P),
                i.consume_key(Modifiers::NONE, Key::Enter),
                i.consume_key(Modifiers::NONE, Key::Escape),
                state.chat && empty && i.consume_key(Modifiers::NONE, Key::Backspace),
            )
        });
        if back {
            state.chat = false;
            state.message = None;
        }
        if !rows.is_empty() {
            if down {
                state.selected = (state.selected + 1) % rows.len();
            }
            if up {
                state.selected = (state.selected + rows.len() - 1) % rows.len();
            }
        }
        if escape {
            actions.push(Action::Palette {
                visible: Some(false),
            });
            return;
        }

        let screen = to_rect(shell.output());
        // Dim the desktop; clicking it closes the palette.
        let backdrop = egui::Area::new(Id::new("derisk-palette-backdrop"))
            .order(Order::Middle)
            .fixed_pos(screen.min)
            .show(ui.ctx(), |ui| {
                let (r, response) = ui.allocate_exact_size(screen.size(), Sense::click());
                ui.painter()
                    .rect_filled(r, 0, Color32::from_black_alpha(110));
                response.clicked()
            })
            .inner;
        if backdrop {
            actions.push(Action::Palette {
                visible: Some(false),
            });
            return;
        }

        let width = (screen.width() - 32.0).clamp(240.0, 680.0);
        let top =
            screen.top() + (screen.height() * 0.14).max(shell.profile().top_bar as f32 + 16.0);
        let mut chosen = (enter && !state.chat).then_some(state.selected);
        let mut new_conversation = false;
        egui::Area::new(Id::new("derisk-palette"))
            .order(Order::Foreground)
            .fixed_pos(pos2(screen.center().x - width / 2.0, top))
            .show(ui.ctx(), |ui| {
                egui::Frame::new()
                    .fill(theme.surface)
                    .stroke(Stroke::new(1.0, theme.border))
                    .corner_radius(14)
                    .inner_margin(10)
                    .shadow(egui::epaint::Shadow {
                        offset: [0, 12],
                        blur: 40,
                        spread: 0,
                        color: Color32::from_black_alpha(140),
                    })
                    .show(ui, |ui| {
                        ui.set_width(width - 20.0);
                        ui.visuals_mut().override_text_color = Some(theme.foreground);
                        ui.horizontal(|ui| {
                            if state.chat {
                                ui.label(RichText::new("✨").size(18.0).color(theme.accent));
                            }
                            let input = ui.add(
                                egui::TextEdit::singleline(&mut state.query)
                                    .hint_text(if state.chat {
                                        "Ask derisk to do something…"
                                    } else {
                                        "Search apps, windows, commands, files… or ask derisk"
                                    })
                                    .font(FontId::proportional(18.0))
                                    .frame(egui::Frame::NONE)
                                    .margin(vec2(6.0, 8.0))
                                    .desired_width(f32::INFINITY),
                            );
                            input.request_focus();
                            // Preset text (typed on the overview, or a
                            // filter) continues where it ends.
                            if cursor_to_end
                                && let Some(mut edit) =
                                    egui::text_edit::TextEditState::load(ui.ctx(), input.id)
                            {
                                let end = egui::text::CCursor::new(state.query.chars().count());
                                edit.cursor.set_char_range(Some(
                                    egui::text_selection::CCursorRange::one(end),
                                ));
                                edit.store(ui.ctx(), input.id);
                            }
                        });
                        ui.add_space(4.0);
                        ui.painter().hline(
                            ui.min_rect().x_range(),
                            ui.cursor().top(),
                            Stroke::new(1.0, theme.border),
                        );
                        ui.add_space(6.0);
                        if state.chat {
                            new_conversation = conversation(ui, &theme, shell, state.cleared);
                        } else {
                            if rows.is_empty() {
                                ui.label(
                                    RichText::new("Type to search, or ask in plain words.")
                                        .color(theme.border),
                                );
                            }
                            let selected = state.selected;
                            egui::ScrollArea::vertical()
                                .max_height(PALETTE_ROWS as f32 * PALETTE_ROW_HEIGHT + 40.0)
                                .auto_shrink([false, true])
                                .show(ui, |ui| {
                                    let mut heading = None;
                                    for (i, entry) in rows.iter().enumerate() {
                                        if heading != Some(entry.category) {
                                            heading = Some(entry.category);
                                            ui.label(
                                                RichText::new(entry.category.heading())
                                                    .size(11.0)
                                                    .color(theme.border),
                                            );
                                        }
                                        let response =
                                            palette_row(ui, &theme, entry, i == selected);
                                        if i == selected && (up || down) {
                                            response.scroll_to_me(None);
                                        }
                                        if response.clicked() {
                                            chosen = Some(i);
                                        }
                                    }
                                });
                        }
                        if let Some(message) = &state.message {
                            ui.add_space(6.0);
                            ui.label(RichText::new(message).color(theme.accent));
                        }
                        ui.add_space(4.0);
                        ui.label(
                            RichText::new(if state.chat {
                                "Enter ask   Backspace back to search   Esc close"
                            } else {
                                "⬆⬇ select   Enter run   Esc close   > commands   @ windows   / files   ? ask"
                            })
                            .size(11.0)
                            .color(theme.border),
                        );
                    });
            });

        if state.chat {
            state.seen = shell.conversation.last_id().unwrap_or(0).max(state.seen);
            if new_conversation {
                state.cleared = state.seen;
            }
            if enter && !state.query.trim().is_empty() {
                let text = state.query.trim().to_owned();
                let preview = palette::ask(&text);
                if preview.confirm && state.armed.as_deref() != Some(text.as_str()) {
                    state.armed = Some(text);
                    state.message =
                        Some("This ends your session. Press Enter again to go ahead.".into());
                    return;
                }
                let confirmed = preview.confirm;
                Self::send_ask(state, shell, &preview, text, confirmed, actions);
            }
            return;
        }

        let Some(entry) = chosen.and_then(|i| rows.get(i)).map(|e| (*e).clone()) else {
            return;
        };
        if entry.actions.is_empty() {
            // The assistant did not understand; say why and keep the text.
            state.message = Some(entry.detail.clone());
            return;
        }
        if entry.confirm && state.armed.as_deref() != Some(&entry.key()) {
            state.armed = Some(entry.key());
            state.message = Some(format!(
                "Press Enter again to {}.",
                entry.title.to_lowercase()
            ));
            return;
        }
        if entry.category == Category::Ask {
            // Requests run in the conversation, which stays open to show
            // their progress.
            let text = palette::scope(&state.query).1.to_owned();
            Self::send_ask(state, shell, &entry, text, entry.confirm, actions);
            state.chat = true;
            return;
        }
        state.history.record(&entry);
        let touches_overview = entry
            .actions
            .iter()
            .any(|a| matches!(a, Action::Overview { .. }));
        actions.extend(entry.actions);
        actions.push(Action::Palette {
            visible: Some(false),
        });
        if shell.overview_visible() && !touches_overview {
            actions.push(Action::Overview {
                visible: Some(false),
            });
        }
    }

    /// Queues a request for the assistant and clears the field.
    fn send_ask(
        state: &mut PaletteUi,
        shell: &Shell,
        preview: &Entry,
        text: String,
        confirmed: bool,
        actions: &mut Vec<Action>,
    ) {
        // Get the overview out of the way so results are visible, unless
        // the request is about the overview itself.
        if shell.overview_visible()
            && !preview
                .actions
                .iter()
                .any(|a| matches!(a, Action::Overview { .. }))
        {
            actions.push(Action::Overview {
                visible: Some(false),
            });
        }
        state.asks.push(Ask { text, confirmed });
        state.query.clear();
        state.shown_query.clear();
        state.armed = None;
        state.message = None;
    }

    /// Requests typed in the palette since the last call, for the host to
    /// run with [`Shell::ask`] as [`Source::User`].
    pub fn take_asks(&mut self) -> Vec<Ask> {
        std::mem::take(&mut self.palette.asks)
    }
}

/// A natural-language request typed in the palette.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Ask {
    /// The request.
    pub text: String,
    /// The person confirmed a destructive session operation in it.
    pub confirmed: bool,
}

/// The conversation transcript, newest at the bottom. Returns whether
/// "New conversation" was clicked.
fn conversation(ui: &mut Ui, theme: &Theme, shell: &Shell, cleared: u64) -> bool {
    let turns: Vec<&Turn> = shell
        .conversation
        .turns()
        .filter(|t| t.id > cleared)
        .collect();
    let mut clear = false;
    ui.horizontal(|ui| {
        ui.label(RichText::new("Conversation").size(11.0).color(theme.border));
        if !turns.is_empty() {
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                clear = ui
                    .add(egui::Button::new(RichText::new("New").size(11.0)).frame(false))
                    .on_hover_text("Start a new conversation")
                    .clicked();
            });
        }
    });
    if turns.is_empty() {
        ui.label(
            RichText::new(
                "Ask in plain words, e.g. \"open calculator and snap it right, then go to workspace 2\". \
                 Requests from other agents show up here too.",
            )
            .color(theme.border),
        );
        return clear;
    }
    egui::ScrollArea::vertical()
        .max_height(PALETTE_ROWS as f32 * PALETTE_ROW_HEIGHT)
        .auto_shrink([false, true])
        .stick_to_bottom(true)
        .show(ui, |ui| {
            for turn in turns {
                ui.add_space(4.0);
                ui.horizontal_wrapped(|ui| {
                    let who = match turn.source {
                        Source::User => "You",
                        Source::Agent => "Agent",
                    };
                    ui.label(RichText::new(who).size(11.0).color(theme.accent));
                    ui.label(RichText::new(&turn.request).strong());
                });
                for step in &turn.steps {
                    let (icon, color) = match &step.status {
                        StepStatus::Done => ("✔", theme.accent),
                        StepStatus::Waiting => ("⟳", theme.border),
                        StepStatus::Failed(_) => ("✖", button_color(Button::Close)),
                        StepStatus::Skipped => ("·", theme.border),
                    };
                    ui.horizontal(|ui| {
                        ui.add_space(12.0);
                        ui.label(RichText::new(icon).color(color));
                        ui.label(RichText::new(&step.label).color(
                            if step.status == StepStatus::Skipped {
                                theme.border
                            } else {
                                theme.foreground
                            },
                        ));
                    });
                }
                ui.horizontal_wrapped(|ui| {
                    ui.add_space(12.0);
                    let failed = turn.error.is_some()
                        || turn
                            .steps
                            .iter()
                            .any(|s| matches!(s.status, StepStatus::Failed(_)));
                    ui.label(RichText::new(turn.reply()).color(if failed {
                        button_color(Button::Close)
                    } else {
                        theme.border
                    }));
                });
            }
        });
    clear
}

/// One palette row: icon, title, detail and shortcut hint.
fn palette_row(ui: &mut Ui, theme: &Theme, entry: &Entry, selected: bool) -> egui::Response {
    let (r, response) = ui.allocate_exact_size(
        vec2(ui.available_width(), PALETTE_ROW_HEIGHT),
        Sense::click(),
    );
    let painter = ui.painter_at(r);
    if selected || response.hovered() {
        painter.rect_filled(
            r,
            8,
            theme
                .accent
                .gamma_multiply(if selected { 0.22 } else { 0.1 }),
        );
    }
    let mid = r.center().y;
    painter.text(
        pos2(r.left() + 18.0, mid),
        Align2::CENTER_CENTER,
        &entry.icon,
        FontId::proportional(16.0),
        theme.foreground,
    );
    let shortcut_width = entry
        .shortcut
        .as_ref()
        .map_or(0.0, |s| s.len() as f32 * 7.0 + 16.0);
    let text_left = r.left() + 40.0;
    let title_galley = painter.layout_no_wrap(
        elide(
            &entry.title,
            (r.width() - 56.0 - shortcut_width) * 0.6,
            15.0,
        ),
        FontId::proportional(15.0),
        theme.foreground,
    );
    let title_width = title_galley.size().x;
    painter.galley(
        pos2(text_left, mid - title_galley.size().y / 2.0),
        title_galley,
        theme.foreground,
    );
    if !entry.detail.is_empty() {
        let room = r.right() - shortcut_width - (text_left + title_width + 12.0);
        painter.text(
            pos2(text_left + title_width + 12.0, mid),
            Align2::LEFT_CENTER,
            elide(&entry.detail, room, 13.0),
            FontId::proportional(13.0),
            theme.border,
        );
    }
    if let Some(shortcut) = &entry.shortcut {
        painter.text(
            pos2(r.right() - 10.0, mid),
            Align2::RIGHT_CENTER,
            shortcut,
            FontId::monospace(12.0),
            theme.border,
        );
    }
    response
}

fn calendar(ui: &mut Ui, shell: &Shell, theme: &Theme) {
    let clock = shell.clock;
    ui.label(RichText::new(clock.date_label()).strong());
    egui::Grid::new("derisk-calendar")
        .spacing(vec2(6.0, 4.0))
        .show(ui, |ui| {
            for d in ["M", "T", "W", "T", "F", "S", "S"] {
                ui.label(RichText::new(d).color(theme.border));
            }
            ui.end_row();
            let first = usize::from(clock.first_weekday());
            for _ in 0..first {
                ui.label("");
            }
            for day in 1..=clock.days_in_month() {
                let text = RichText::new(format!("{day:>2}"));
                ui.label(if day == clock.day {
                    text.color(theme.accent).strong()
                } else {
                    text
                });
                if (first + usize::from(day)) % 7 == 0 {
                    ui.end_row();
                }
            }
        });
}

fn menu_button(ui: &mut Ui, menu: &Menu, window: Option<u64>, actions: &mut Vec<Action>) {
    ui.menu_button(menu.title.as_str(), |ui| {
        entries(
            ui,
            &menu.entries,
            &mut |item| Action::ActivateMenu { window, item },
            actions,
        );
    });
}

fn entries(
    ui: &mut Ui,
    list: &[MenuEntry],
    make: &mut dyn FnMut(String) -> Action,
    actions: &mut Vec<Action>,
) {
    for entry in list {
        match entry {
            MenuEntry::Item {
                id,
                label,
                shortcut,
                enabled,
            } => {
                let mut button = egui::Button::new(label.as_str());
                if let Some(hint) = shortcut {
                    button = button.shortcut_text(hint.as_str());
                }
                if ui.add_enabled(*enabled, button).clicked() {
                    actions.push(make(id.clone()));
                    ui.close();
                }
            }
            MenuEntry::Submenu { label, entries: e } => {
                ui.menu_button(label.as_str(), |ui| entries(ui, e, make, actions));
            }
            MenuEntry::Separator => {
                ui.separator();
            }
        }
    }
}

/// Paints the desktop wallpaper: a vertical gradient with a soft accent glow
/// and the derisk mark in the lower right corner.
pub fn paint_wallpaper(painter: &Painter, screen: Rect, theme: &Theme) {
    let top = Color32::from_rgb(17, 24, 39);
    let bottom = Color32::from_rgb(30, 27, 75);
    let mut mesh = egui::Mesh::default();
    mesh.colored_vertex(screen.left_top(), top);
    mesh.colored_vertex(screen.right_top(), top);
    mesh.colored_vertex(screen.left_bottom(), bottom);
    mesh.colored_vertex(screen.right_bottom(), bottom);
    mesh.add_triangle(0, 1, 2);
    mesh.add_triangle(1, 2, 3);
    painter.add(egui::Shape::mesh(mesh));
    let glow = screen.center() + vec2(screen.width() * 0.22, screen.height() * 0.18);
    for i in 0..12 {
        let r = screen.height() * (0.55 - i as f32 * 0.04);
        painter.circle_filled(glow, r, theme.accent.gamma_multiply(0.012));
    }
    let mark = screen.right_bottom() - vec2(64.0, 56.0);
    painter.circle_filled(mark, 18.0, theme.accent.gamma_multiply(0.35));
    painter.text(
        mark,
        Align2::CENTER_CENTER,
        "d",
        FontId::proportional(22.0),
        theme.background.gamma_multiply(0.9),
    );
}

/// Paints one frame of the startup animation over `screen`.
pub fn paint_startup(painter: &Painter, screen: Rect, frame: StartupFrame, theme: &Theme) {
    painter.rect_filled(screen, 0, theme.background.gamma_multiply(frame.cover));
    let center: Pos2 = screen.center();
    let accent = theme.accent.gamma_multiply(frame.logo_opacity);
    let radius = 44.0 * frame.logo_scale;
    painter.circle_filled(center, radius, accent);
    painter.text(
        center,
        Align2::CENTER_CENTER,
        "d",
        FontId::proportional(radius * 1.2),
        theme.background.gamma_multiply(frame.logo_opacity),
    );
    if frame.ring > 0.0 && frame.logo_opacity > 0.0 {
        let ring = radius + 16.0;
        let steps = 96;
        let sweep = frame.ring * std::f32::consts::TAU;
        let points: Vec<Pos2> = (0..=steps)
            .map(|i| {
                let a = -std::f32::consts::FRAC_PI_2 + sweep * i as f32 / steps as f32;
                center + vec2(a.cos(), a.sin()) * ring
            })
            .collect();
        painter.line(points, Stroke::new(4.0, accent));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn palette_result_limit_keeps_each_matching_category() {
        let mut entries: Vec<_> = (0..61)
            .map(|n| Entry::new(Category::App, "", format!("App {n}"), vec![]))
            .collect();
        entries.push(Entry::new(Category::Window, "", "Window", vec![]));
        entries.push(Entry::new(Category::Command, "", "Command", vec![]));

        let hits: Vec<_> = (0..entries.len()).collect();
        let limited = limit_palette_hits(&entries, &hits, 60);

        assert_eq!(limited.len(), 60);
        for category in [Category::App, Category::Window, Category::Command] {
            assert!(
                limited.iter().any(|&hit| entries[hit].category == category),
                "{category:?} results should remain available"
            );
        }
    }
}
