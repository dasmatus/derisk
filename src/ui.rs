//! egui rendering of the shell surfaces.
//!
//! The compositor renders three passes per output:
//!
//! 1. [`ShellUi::paint_decorations`] below client surfaces (title bars).
//! 2. Client surfaces, at [`crate::shell::WindowPlacement::client`].
//! 3. [`ShellUi::show`] above them: top bar with global menu and tray, snap
//!    preview and Snap Assist, the overview with widgets, and the startup
//!    animation.
//!
//! Logical compositor pixels map 1:1 to egui points; set
//! `pixels_per_point` to the output scale.

use egui::{
    Align, Align2, Color32, CornerRadius, FontId, Id, Layout, Order, Painter, Pos2, Rect, RichText,
    Sense, Stroke, StrokeKind, TextureHandle, TextureOptions, Ui, UiBuilder, pos2, vec2,
};
use mcsapi::{Geometry, WindowId, toolkit::egui, widgets::Theme};

use crate::{
    action::Action,
    animation::{StartupAnimation, StartupFrame},
    assistant,
    decorations::Button,
    effects::{BlurArea, Look},
    geom::inset,
    menu::{Menu, MenuEntry},
    overview::{OverviewLayout, Widget, fit, grid, row},
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
    /// Window being dragged in the overview, toward a workspace.
    overview_drag: Option<WindowId>,
    tray: Option<(u64, Vec<TextureHandle>)>,
    reduced_motion: bool,
    look: Look,
    blurs: Vec<BlurArea>,
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
            assistant: String::new(),
            reply: None,
            notes: String::new(),
            overview_open: false,
            overview_drag: None,
            tray: None,
            reduced_motion,
            look: shell.look(),
            blurs: Vec::new(),
        }
    }

    /// Areas to blur under the translucent panels shown by the last
    /// [`ShellUi::show`], bottom to top. Empty when blur is off.
    pub fn blur_regions(&self) -> &[BlurArea] {
        &self.blurs
    }

    /// Records a translucent panel so the compositor blurs behind it.
    fn frost(&mut self, area: Rect, corner_radius: u8) {
        if self.look.blur == 0 {
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
        self.look = shell.look();
        self.blurs.clear();
        self.startup.reduced_motion = self.reduced_motion || !self.look.animate;
        let frame = self.startup.frame(elapsed_ms);
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
        self.frost(to_rect(assist.frame), 12);
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
        let opacity = frame.shell_opacity.max(0.0);
        if opacity > 0.0 {
            self.frost(bar, 0);
        }
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
                ui.menu_button("derisk", |ui| system_menu(ui, actions));
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
        self.frost(to_rect(area), 0);
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
            painter.text(
                r.center(),
                Align2::CENTER_CENTER,
                elide(app, r.width() - 12.0, 14.0),
                FontId::proportional(14.0),
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
