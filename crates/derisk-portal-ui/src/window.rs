//! One dialog in its own window, as a Wayland (or X11) client of the
//! session. The service runs this in a child process per request, so a
//! dialog that crashes or hangs only ends that request.

use std::{
    io::BufRead,
    sync::{Arc, Mutex, mpsc},
    time::Duration,
};

use mcsapi_ui::{Theme, egui};

use crate::{Dialog, Reply, Request, Update};

/// The shell's colors and text scale from the Settings app, or the
/// defaults.
pub fn appearance() -> (Theme, f32) {
    derisk_settings::default_path()
        .and_then(|path| derisk_settings::Settings::load(&path).ok())
        .map_or((Theme::default(), 1.0), |(settings, _)| {
            (settings.theme(), settings.appearance.text_scale)
        })
}

/// egui's built-in visuals in the theme's colors, for the few egui widgets
/// the dialogs use as they are (text fields, scroll bars, popups).
pub fn visuals(theme: &Theme) -> egui::Visuals {
    let [r, g, b, _] = theme.background.to_array();
    let dark = u16::from(r) + u16::from(g) + u16::from(b) < 384;
    let mut visuals = if dark {
        egui::Visuals::dark()
    } else {
        egui::Visuals::light()
    };
    visuals.override_text_color = Some(theme.foreground);
    visuals.panel_fill = theme.background;
    visuals.window_fill = theme.background;
    visuals.extreme_bg_color = theme.background;
    visuals.faint_bg_color = theme.surface;
    visuals.selection.bg_fill = theme.accent.gamma_multiply(0.45);
    visuals.selection.stroke.color = theme.accent;
    visuals.text_cursor.stroke.color = theme.accent;
    visuals.widgets.noninteractive.bg_stroke.color = theme.border;
    visuals
}

struct Window {
    dialog: Box<dyn Dialog>,
    theme: Theme,
    text_scale: f32,
    updates: Option<mpsc::Receiver<Update>>,
    reply: Arc<Mutex<Option<Reply>>>,
}

impl eframe::App for Window {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        if let Some(updates) = &self.updates {
            while let Ok(update) = updates.try_recv() {
                self.dialog.update(update);
            }
            ui.ctx().request_repaint_after(Duration::from_millis(250));
        }
        ui.ctx().set_visuals(visuals(&self.theme));
        ui.ctx().set_zoom_factor(self.text_scale);
        crate::show(&mut *self.dialog, ui, &self.theme);
        if let Some(reply) = self.dialog.reply() {
            *self.reply.lock().unwrap_or_else(|e| e.into_inner()) = Some(reply.clone());
            ui.ctx().send_viewport_cmd(egui::ViewportCommand::Close);
        }
    }
}

/// Shows the dialog for `request` until it ends, applying `updates` as
/// they come. Closing the window counts as [`Reply::Cancelled`].
pub fn run(request: Request, updates: Option<mpsc::Receiver<Update>>) -> Result<Reply, String> {
    let (theme, text_scale) = appearance();
    let dialog = crate::open(request);
    let [w, h] = dialog.size();
    let title = dialog.title().to_owned();
    let reply = Arc::new(Mutex::new(None));
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title(&title)
            .with_app_id("org.derisk.Portal")
            .with_inner_size([w, h])
            .with_min_inner_size([w * 0.6, h * 0.8]),
        ..Default::default()
    };
    let window = Window {
        dialog,
        theme,
        text_scale,
        updates,
        reply: reply.clone(),
    };
    eframe::run_native(&title, options, Box::new(move |_| Ok(Box::new(window))))
        .map_err(|e| format!("cannot open the dialog window: {e}"))?;
    let reply = reply.lock().unwrap_or_else(|e| e.into_inner()).take();
    Ok(reply.unwrap_or(Reply::Cancelled))
}

/// The child process side: reads a [`Request`] line from stdin, shows it,
/// takes later lines as [`Update`]s, and prints the [`Reply`] as one JSON
/// line on stdout.
pub fn serve_stdio() -> Result<(), String> {
    let stdin = std::io::stdin();
    let mut first = String::new();
    stdin
        .lock()
        .read_line(&mut first)
        .map_err(|e| format!("cannot read the request: {e}"))?;
    let request: Request = serde_json::from_str(&first).map_err(|e| format!("bad request: {e}"))?;
    let (send, receive) = mpsc::channel();
    std::thread::spawn(move || {
        for line in std::io::stdin().lock().lines() {
            let Ok(line) = line else { break };
            match serde_json::from_str::<Update>(&line) {
                Ok(update) => {
                    if send.send(update).is_err() {
                        break;
                    }
                }
                Err(e) => eprintln!("ignoring a bad update: {e}"),
            }
        }
    });
    let reply = run(request, Some(receive))?;
    let line = serde_json::to_string(&reply).map_err(|e| e.to_string())?;
    println!("{line}");
    Ok(())
}
