//! Runs the derisk core apps in one desktop window, each in its own movable
//! egui window, until a compositor host can give them real surfaces.
//!
//! ```console
//! $ cargo run -p derisk-apps --features preview --bin derisk-preview -- org.derisk.files
//! ```
//!
//! Arguments are app IDs to open at start. Colors follow the Settings app and
//! update a moment after it saves.

use std::{
    fmt::Write,
    io::IsTerminal,
    path::PathBuf,
    time::{Duration, Instant, SystemTime},
};

use derisk_apps::{APPS, Session, find, visuals};
use derisk_settings::Settings;
use mcsapi_runtime::{AppId, InstanceId};
use mcsapi_ui::{Theme, egui};
use tracing_subscriber::EnvFilter;

/// An argument that is not one of the core apps' IDs. The report's help
/// lists them.
#[derive(Debug, thiserror::Error, miette::Diagnostic)]
#[error("unknown app: {app}")]
#[diagnostic(code(derisk_preview::unknown_app))]
struct UnknownApp {
    app: String,
    #[help]
    apps: String,
}

struct Preview {
    session: Session,
    open: Vec<InstanceId>,
    settings_path: Option<PathBuf>,
    settings_seen: Option<SystemTime>,
    last_check: Instant,
    theme: Theme,
    text_scale: f32,
    status: Option<String>,
}

impl Preview {
    fn new(launch: &[String]) -> Self {
        let mut preview = Self {
            session: Session::new().expect("catalog IDs are unique"),
            open: Vec::new(),
            settings_path: derisk_settings::default_path(),
            settings_seen: None,
            last_check: Instant::now(),
            theme: Theme::default(),
            text_scale: 1.0,
            status: None,
        };
        preview.reload_settings();
        for id in launch {
            preview.launch(id);
        }
        preview
    }

    fn launch(&mut self, id: &str) {
        let result = AppId::new(id)
            .ok_or_else(|| format!("invalid app ID: {id}"))
            .and_then(|id| self.session.launch(&id).map_err(|e| e.to_string()));
        match result {
            Ok(instance) => self.open.push(instance),
            Err(error) => self.status = Some(error),
        }
    }

    fn reload_settings(&mut self) {
        let Some(path) = &self.settings_path else {
            return;
        };
        let modified = std::fs::metadata(path).and_then(|m| m.modified()).ok();
        if modified == self.settings_seen && self.settings_seen.is_some() {
            return;
        }
        self.settings_seen = modified;
        if let Ok((settings, _)) = Settings::load(path) {
            self.theme = settings.theme();
            self.text_scale = settings.appearance.text_scale;
        }
    }
}

impl eframe::App for Preview {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        if self.last_check.elapsed() > Duration::from_millis(500) {
            self.last_check = Instant::now();
            self.reload_settings();
        }
        ui.ctx().request_repaint_after(Duration::from_millis(500));
        ui.ctx().set_visuals(visuals(&self.theme));
        ui.ctx().set_zoom_factor(self.text_scale);

        egui::Panel::top("preview-launcher").show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.strong("derisk");
                ui.separator();
                for app in &APPS {
                    let button = ui
                        .button(format!("{} {}", app.icon, app.name))
                        .on_hover_text(app.summary);
                    if button.clicked() {
                        self.launch(app.id);
                    }
                }
                if let Some(status) = &self.status {
                    ui.separator();
                    ui.label(status);
                }
            });
        });

        let theme = self.theme;
        let mut closed = Vec::new();
        egui::CentralPanel::default().show(ui, |ui| {
            let area = ui.max_rect();
            for (index, &instance) in self.open.iter().enumerate() {
                let Some(app) = self.session.app_mut(instance) else {
                    continue;
                };
                let mut open = true;
                let offset = 28.0 * (index % 8) as f32;
                egui::Window::new(app.title().to_owned())
                    .id(egui::Id::new(("app", instance.get())))
                    .open(&mut open)
                    .default_pos(area.min + egui::vec2(24.0 + offset, 24.0 + offset))
                    .default_size([760.0, 480.0])
                    .constrain_to(area)
                    .frame(egui::Frame::window(ui.style()).fill(theme.background))
                    .show(ui.ctx(), |ui| app.ui(ui, &theme));
                if !open {
                    closed.push(instance);
                }
            }
            if self.open.is_empty() {
                ui.centered_and_justified(|ui| ui.label("Open an app from the bar above."));
            }
        });
        for instance in closed {
            let _ = self.session.stop(instance);
            self.open.retain(|&i| i != instance);
        }
    }
}

fn main() -> miette::Result<()> {
    logging();
    let args: Vec<String> = std::env::args().skip(1).collect();
    if let Some(unknown) = args.iter().find(|id| find(id).is_none()) {
        let mut apps = String::from("apps:");
        for app in &APPS {
            let _ = write!(apps, "\n  {:24} {}", app.id, app.name);
        }
        return Err(UnknownApp {
            app: unknown.clone(),
            apps,
        }
        .into());
    }
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("derisk apps")
            .with_inner_size([1280.0, 800.0]),
        ..Default::default()
    };
    eframe::run_native(
        "derisk apps",
        options,
        Box::new(move |_| Ok(Box::new(Preview::new(&args)))),
    )
    // eframe's error is not Send + Sync, so it goes in as its message.
    .map_err(|error| miette::miette!("running the preview window: {error}"))
}

/// Logs `tracing` events, and eframe's and winit's `log` records through
/// tracing-subscriber's bridge, to stderr. `RUST_LOG` picks what is logged:
/// `info` and up when it is unset or does not parse.
fn logging() {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .with_writer(std::io::stderr)
        // No colour escapes in a pipe or a log file.
        .with_ansi(std::io::stderr().is_terminal())
        .init();
}
