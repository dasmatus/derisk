//! The derisk core apps in one catalog.
//!
//! Each app lives in its own crate and implements [`mcsapi_ui::App`]. This
//! crate lists them ([`APPS`]), registers them with an
//! [`mcsapi_runtime::Runtime`], and keeps the running [`App`] objects next
//! to their runtime instances ([`Session`]), so a compositor host only has to
//! give each instance a surface and call [`mcsapi_ui::run_frame`].
//!
//! ```
//! use derisk_apps::{SETTINGS, Session};
//! use mcsapi_runtime::AppId;
//!
//! let mut session = Session::new()?;
//! assert_eq!(session.runtime().apps().count(), derisk_apps::APPS.len());
//! let instance = session.launch(&AppId::new(SETTINGS).unwrap())?;
//! assert_eq!(session.app_mut(instance).unwrap().title(), "Settings");
//! session.stop(instance)?;
//! # Ok::<(), mcsapi_runtime::Error>(())
//! ```

#![forbid(unsafe_code)]
#![deny(missing_docs)]

use std::collections::BTreeMap;

use mcsapi_runtime::{AppId, Error, InstanceId, Manifest, Runtime};
use mcsapi_ui::{App, Theme, egui};

/// App ID of Settings.
pub const SETTINGS: &str = "org.derisk.settings";
/// App ID of Files.
pub const FILES: &str = "org.derisk.files";
/// App ID of the Text Editor.
pub const EDITOR: &str = "org.derisk.editor";
/// App ID of the System Monitor.
pub const MONITOR: &str = "org.derisk.monitor";
/// App ID of the Calculator.
pub const CALCULATOR: &str = "org.derisk.calculator";

/// A catalog entry for one core app.
#[derive(Clone, Copy, Debug)]
pub struct AppInfo {
    /// Reverse-DNS app ID.
    pub id: &'static str,
    /// Display name.
    pub name: &'static str,
    /// One-line description for launchers and search.
    pub summary: &'static str,
    /// An emoji icon from egui's built-in font.
    pub icon: &'static str,
    /// Search keywords.
    pub keywords: &'static [&'static str],
    create: fn() -> Box<dyn App>,
}

impl AppInfo {
    /// The validated app ID.
    pub fn app_id(&self) -> AppId {
        AppId::new(self.id).expect("catalog IDs are valid")
    }

    /// The runtime manifest.
    pub fn manifest(&self) -> Manifest {
        Manifest::new(self.app_id(), self.name)
    }

    /// Creates a fresh app object with its default state.
    pub fn create(&self) -> Box<dyn App> {
        (self.create)()
    }

    /// Whether the app matches a case-insensitive search.
    pub fn matches(&self, query: &str) -> bool {
        let query = query.trim().to_lowercase();
        query.is_empty()
            || self.name.to_lowercase().contains(&query)
            || self.summary.to_lowercase().contains(&query)
            || self.keywords.iter().any(|k| k.contains(&query))
    }
}

/// Every core app, in launcher order.
pub const APPS: [AppInfo; 5] = [
    AppInfo {
        id: FILES,
        name: "Files",
        summary: "Browse, copy, rename, and trash files",
        icon: "🗀",
        keywords: &["file manager", "folders", "browse", "trash"],
        create: || Box::new(derisk_files::FilesApp::default()),
    },
    AppInfo {
        id: SETTINGS,
        name: "Settings",
        summary: "Appearance, desktop, input, notifications, and power",
        icon: "⚙",
        keywords: &["preferences", "theme", "accent", "keyboard", "power"],
        create: || Box::new(derisk_settings::SettingsApp::default()),
    },
    AppInfo {
        id: EDITOR,
        name: "Text Editor",
        summary: "Edit plain-text files",
        icon: "📝",
        keywords: &["notepad", "text", "code", "write"],
        create: || Box::new(derisk_editor::EditorApp::new()),
    },
    AppInfo {
        id: MONITOR,
        name: "System Monitor",
        summary: "CPU, memory, and running processes",
        icon: "📈",
        keywords: &["task manager", "processes", "cpu", "memory", "kill"],
        create: || Box::new(derisk_monitor::MonitorApp::default()),
    },
    AppInfo {
        id: CALCULATOR,
        name: "Calculator",
        summary: "Arithmetic and scientific functions",
        icon: "🖩",
        keywords: &["math", "calc", "numbers"],
        create: || Box::new(derisk_calculator::CalculatorApp::default()),
    },
];

/// Looks up a core app by ID.
pub fn find(id: &str) -> Option<&'static AppInfo> {
    APPS.iter().find(|app| app.id == id)
}

/// Registers every core app with `runtime`.
pub fn register_all(runtime: &mut Runtime) -> Result<(), Error> {
    APPS.iter()
        .try_for_each(|app| runtime.register(app.manifest()))
}

/// egui visuals for `theme`, so widgets match the shell colors.
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
    visuals.extreme_bg_color = theme.surface;
    visuals.faint_bg_color = theme.surface;
    visuals.selection.bg_fill = theme.accent.gamma_multiply(0.45);
    visuals.selection.stroke.color = theme.accent;
    visuals.hyperlink_color = theme.accent;
    let widgets = &mut visuals.widgets;
    widgets.noninteractive.bg_stroke.color = theme.border;
    widgets.inactive.weak_bg_fill = theme.surface;
    widgets.inactive.bg_fill = theme.surface;
    let hover = theme.surface.lerp_to_gamma(theme.border, 0.35);
    widgets.hovered.weak_bg_fill = hover;
    widgets.hovered.bg_fill = hover;
    widgets.hovered.bg_stroke.color = theme.accent;
    widgets.active.weak_bg_fill = hover;
    widgets.active.bg_fill = hover;
    // The open state also colors window title bars.
    widgets.open.weak_bg_fill = theme.surface;
    widgets.open.bg_fill = theme.surface;
    widgets.active.bg_stroke.color = theme.accent;
    visuals
}

/// Running core apps, kept in step with a [`Runtime`].
pub struct Session {
    runtime: Runtime,
    apps: BTreeMap<InstanceId, Box<dyn App>>,
}

impl std::fmt::Debug for Session {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Session")
            .field("runtime", &self.runtime)
            .field("running", &self.apps.keys().collect::<Vec<_>>())
            .finish()
    }
}

impl Session {
    /// A runtime with every core app registered and none running.
    pub fn new() -> Result<Self, Error> {
        let mut runtime = Runtime::new();
        register_all(&mut runtime)?;
        Ok(Self {
            runtime,
            apps: BTreeMap::new(),
        })
    }

    /// The underlying runtime.
    pub fn runtime(&self) -> &Runtime {
        &self.runtime
    }

    /// Starts a new instance of a core app.
    pub fn launch(&mut self, id: &AppId) -> Result<InstanceId, Error> {
        let info = find(id.as_str()).ok_or_else(|| Error::UnknownApp(id.clone()))?;
        let instance = self.runtime.launch(id)?;
        self.apps.insert(instance, info.create());
        Ok(instance)
    }

    /// Stops an instance and drops its app.
    pub fn stop(&mut self, instance: InstanceId) -> Result<AppId, Error> {
        let app = self.runtime.stop(instance)?;
        self.apps.remove(&instance);
        Ok(app)
    }

    /// A running app, to draw with [`mcsapi_ui::run_frame`].
    pub fn app_mut(&mut self, instance: InstanceId) -> Option<&mut (dyn App + 'static)> {
        self.apps.get_mut(&instance).map(Box::as_mut)
    }

    /// Running instances with their apps, ordered by launch.
    pub fn running(&mut self) -> impl Iterator<Item = (InstanceId, &mut (dyn App + 'static))> {
        self.apps.iter_mut().map(|(&id, app)| (id, app.as_mut()))
    }
}
