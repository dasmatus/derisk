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

use std::path::PathBuf;

use egui::{
    Align, Align2, Color32, CornerRadius, FontId, Id, Key, Layout, Modifiers, Order, Painter, Pos2,
    Rect, RichText, Sense, Stroke, StrokeKind, TextureHandle, TextureOptions, Ui, UiBuilder, pos2,
    vec2,
};
use mcsapi::{Geometry, toolkit::egui, widgets::Theme};

use crate::{
    action::Action,
    animation::{StartupAnimation, StartupFrame},
    assistant,
    decorations::Button,
    geom::{inset, rect},
    menu::{Menu, MenuEntry},
    overview::{OverviewLayout, Widget, fit, grid},
    palette::{self, Category, Entry, History},
    shell::{DropTarget, Shell, WindowPlacement},
    systemd::SessionOp,
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
    assistant: String,
    reply: Option<String>,
    notes: String,
    overview_open: bool,
    tray: Option<(u64, Vec<TextureHandle>)>,
    /// The command palette.
    pub palette: PaletteUi,
}

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

impl ShellUi {
    /// Creates UI state; `reduced_motion` shortens the startup animation.
    pub fn new(shell: &Shell, reduced_motion: bool) -> Self {
        Self {
            theme: Theme::default(),
            startup: StartupAnimation {
                reduced_motion,
                bar_height: shell.profile().top_bar as f32,
            },
            assistant: String::new(),
            reply: None,
            notes: String::new(),
            overview_open: false,
            tray: None,
            palette: PaletteUi::default(),
        }
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
        painter.add(
            egui::epaint::Shadow {
                offset: [0, 6],
                blur: if p.focused { 28 } else { 16 },
                spread: 0,
                color: Color32::from_black_alpha(if p.focused { 150 } else { 90 }),
            }
            .as_shape(to_rect(p.frame), radius),
        );
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
        let label = shell
            .window_label(p.window)
            .map(|(app, title)| if title.is_empty() { app } else { title })
            .unwrap_or_default();
        let size = (bar.height as f32 * 0.42).max(11.0);
        painter.text(
            title_area.center(),
            Align2::CENTER_CENTER,
            elide(label, title_area.width(), size),
            FontId::proportional(size),
            theme.foreground,
        );
    }

    /// Shows the chrome above client surfaces and returns requested actions.
    ///
    /// `elapsed_ms` is time since the session started (drives the startup
    /// animation). Pass the root `Ui` from `Context::run_ui`.
    pub fn show(&mut self, ui: &mut Ui, shell: &Shell, elapsed_ms: u32) -> Vec<Action> {
        let mut actions = Vec::new();
        let frame = self.startup.frame(elapsed_ms);
        self.drag_preview(ui, shell);
        self.snap_assist(ui, shell, &mut actions);
        if shell.overview_visible() {
            self.overview(ui, shell, &mut actions);
        }
        self.overview_open = shell.overview_visible();
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

    fn snap_assist(&self, ui: &mut Ui, shell: &Shell, actions: &mut Vec<Action>) {
        let Some(assist) = shell.snap_assist() else {
            return;
        };
        ui.painter().rect_filled(
            to_rect(assist.frame),
            12,
            self.theme.background.gamma_multiply(0.85),
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
            ui.painter().text(
                r.center(),
                Align2::CENTER_CENTER,
                elide(if title.is_empty() { app } else { title }, r.width(), 14.0),
                FontId::proportional(14.0),
                self.theme.foreground,
            );
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
        ui.painter().rect_filled(
            bar,
            0,
            self.theme
                .background
                .gamma_multiply(frame.shell_opacity.max(0.0)),
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
                ui.menu_button("derisk", |ui| system_menu(ui, actions));
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
                    let (app, title) = shell.window_label(w).unwrap_or_default();
                    // Reverse-DNS app IDs (org.derisk.files) read better as the title.
                    let name = if app.contains('.') && !title.is_empty() {
                        title
                    } else {
                        app
                    };
                    ui.label(RichText::new(name).strong());
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
                    if !shell.failed_units.is_empty() {
                        ui.label(format!("⚠ {}", shell.failed_units.len()))
                            .on_hover_text("Failed user services, see the overview");
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
        let area = shell.work_area();
        ui.painter()
            .rect_filled(to_rect(area), 0, self.theme.background.gamma_multiply(0.92));
        let layout = OverviewLayout::new(area, shell.profile().form_factor);

        // Workspace strip.
        let ids: Vec<_> = shell.desktop().workspaces().map(|w| w.id()).collect();
        let active = shell.desktop().active().id();
        for (ws, cell) in ids.iter().zip(row(ids.len(), layout.workspaces, 10)) {
            let r = to_rect(cell);
            let response = ui.interact(r, Id::new(("derisk-ws", ws.get())), Sense::click());
            let count = shell.windows_on(*ws).len();
            let highlight = *ws == active || response.hovered();
            ui.painter().rect_filled(r, 10, self.theme.surface);
            ui.painter().rect_stroke(
                r,
                10,
                Stroke::new(
                    if *ws == active { 2.0 } else { 1.0 },
                    if highlight {
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
                if count == 0 {
                    format!("{ws}")
                } else {
                    format!("{ws} · {count}")
                },
                FontId::proportional(15.0),
                self.theme.foreground,
            );
            if response.clicked() {
                actions.push(Action::SwitchWorkspace {
                    workspace: ws.get(),
                });
            }
        }

        // Window grid (exposé), including minimized windows.
        let windows = shell.windows_on(active);
        let placements = shell.placements();
        for (w, cell) in windows.iter().zip(grid(windows.len(), layout.windows, 24)) {
            let frame = placements
                .iter()
                .find(|p| p.window == *w)
                .map_or(cell, |p| p.frame);
            let r = to_rect(fit(frame, cell));
            let response = ui.interact(r, Id::new(("derisk-win", w.get())), Sense::click());
            let minimized = shell.is_minimized(*w);
            let hovered = response.hovered();
            ui.painter().rect_filled(
                r,
                12,
                if minimized {
                    self.theme.background
                } else {
                    self.theme.surface
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
            // Reverse-DNS app IDs (org.derisk.files) read better as the title.
            let (app, title) = if app.contains('.') && !title.is_empty() {
                (title, app)
            } else {
                (app, title)
            };
            ui.painter().text(
                r.center() - vec2(0.0, 10.0),
                Align2::CENTER_CENTER,
                elide(app, r.width() - 16.0, 18.0),
                FontId::proportional(18.0),
                self.theme.foreground,
            );
            ui.painter().text(
                r.center() + vec2(0.0, 14.0),
                Align2::CENTER_CENTER,
                elide(title, r.width() - 16.0, 13.0),
                FontId::proportional(13.0),
                self.theme.border,
            );
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
            Widget::Assistant => card(ui, &theme, |ui| {
                let response = ui.add(
                    egui::TextEdit::singleline(&mut self.assistant)
                        .hint_text("Ask derisk… e.g. \"open firefox and snap it left\"")
                        .desired_width(f32::INFINITY),
                );
                // Opening the overview focuses the assistant, so typing just works.
                if !self.overview_open {
                    response.request_focus();
                }
                if response.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                    self.submit(actions);
                }
                if let Some(reply) = &self.reply {
                    ui.label(RichText::new(reply).color(theme.border));
                }
            }),
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
    /// assistant as fallback.
    fn palette(&mut self, ui: &mut Ui, shell: &Shell, actions: &mut Vec<Action>) {
        let theme = self.theme;
        let state = &mut self.palette;
        if !state.open {
            state.query.clear();
            state.shown_query.clear();
            state.selected = 0;
            state.armed = None;
            state.message = None;
        }

        let entries = palette::entries(shell, &state.extra, &state.files);
        let hits = palette::search(&entries, &state.query, &state.history);
        let (scope, text) = palette::scope(&state.query);
        let mut rows: Vec<&Entry> = if scope == palette::Scope::Ask {
            Vec::new()
        } else {
            hits.iter().take(60).map(|&i| &entries[i]).collect()
        };
        // The assistant joins everything-searches and `?`; a request it does
        // not understand only shows when nothing else matched, to say why.
        let ask = (!text.is_empty() && matches!(scope, palette::Scope::All | palette::Scope::Ask))
            .then(|| palette::ask(&state.query))
            .filter(|ask| !ask.actions.is_empty() || rows.is_empty());
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
            state.message = None;
        }
        state.selected = state.selected.min(rows.len().saturating_sub(1));

        // Keys the text field would otherwise eat.
        let (down, up, enter, escape) = ui.input_mut(|i| {
            (
                i.consume_key(Modifiers::NONE, Key::ArrowDown)
                    || i.consume_key(Modifiers::NONE, Key::Tab)
                    || i.consume_key(Modifiers::CTRL, Key::N),
                i.consume_key(Modifiers::NONE, Key::ArrowUp)
                    || i.consume_key(Modifiers::SHIFT, Key::Tab)
                    || i.consume_key(Modifiers::CTRL, Key::P),
                i.consume_key(Modifiers::NONE, Key::Enter),
                i.consume_key(Modifiers::NONE, Key::Escape),
            )
        });
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
        let mut chosen = enter.then_some(state.selected);
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
                        let input = ui.add(
                            egui::TextEdit::singleline(&mut state.query)
                                .hint_text("Search apps, windows, commands, files… or ask derisk")
                                .font(FontId::proportional(18.0))
                                .frame(egui::Frame::NONE)
                                .margin(vec2(6.0, 8.0))
                                .desired_width(f32::INFINITY),
                        );
                        input.request_focus();
                        ui.add_space(4.0);
                        ui.painter().hline(
                            ui.min_rect().x_range(),
                            ui.cursor().top(),
                            Stroke::new(1.0, theme.border),
                        );
                        ui.add_space(6.0);
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
                        if let Some(message) = &state.message {
                            ui.add_space(6.0);
                            ui.label(RichText::new(message).color(theme.accent));
                        }
                        ui.add_space(4.0);
                        ui.label(
                            RichText::new("⬆⬇ select   Enter run   Esc close   > commands   @ windows   / files   ? ask")
                                .size(11.0)
                                .color(theme.border),
                        );
                    });
            });

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
        if entry.category != Category::Ask {
            state.history.record(&entry);
        }
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

    /// Interprets the assistant prompt typed at the console.
    fn submit(&mut self, actions: &mut Vec<Action>) {
        match assistant::interpret(&self.assistant) {
            Ok(parsed) => {
                self.reply = Some(format!("On it ({} step(s)).", parsed.len()));
                // Get out of the way so the result is visible, unless the
                // request was about the overview itself.
                let close = !parsed.iter().any(|a| matches!(a, Action::Overview { .. }));
                // Typed by the user at the console, so session operations count
                // as confirmed. Agents over IPC must confirm explicitly.
                actions.extend(parsed.into_iter().map(|a| match a {
                    Action::Session { op, .. } => Action::Session {
                        op,
                        confirmed: true,
                    },
                    other => other,
                }));
                if close {
                    actions.push(Action::Overview {
                        visible: Some(false),
                    });
                }
                self.assistant.clear();
            }
            Err(e) => self.reply = Some(e.to_string()),
        }
    }
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

fn row(count: usize, area: Geometry, gap: i32) -> Vec<Geometry> {
    if count == 0 {
        return Vec::new();
    }
    let n = count as i32;
    let w = ((area.size.w - gap * (n - 1)) / n).max(1);
    (0..n)
        .map(|i| rect(area.loc.x + i * (w + gap), area.loc.y, w, area.size.h))
        .collect()
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

fn system_menu(ui: &mut Ui, actions: &mut Vec<Action>) {
    for (label, op) in [
        ("Lock Screen", SessionOp::Lock),
        ("Suspend", SessionOp::Suspend),
        ("Hibernate", SessionOp::Hibernate),
    ] {
        if ui.button(label).clicked() {
            actions.push(Action::Session {
                op,
                confirmed: false,
            });
            ui.close();
        }
    }
    ui.separator();
    for (label, op) in [
        ("Log Out", SessionOp::Logout),
        ("Restart", SessionOp::Reboot),
        ("Shut Down", SessionOp::PowerOff),
    ] {
        // A nested confirmation keeps one stray click from ending the session.
        ui.menu_button(label, |ui| {
            if ui.button(format!("{label} now")).clicked() {
                actions.push(Action::Session {
                    op,
                    confirmed: true,
                });
                ui.close();
            }
        });
    }
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
