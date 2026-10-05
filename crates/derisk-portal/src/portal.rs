//! The portal interfaces derisk serves, on ashpd's backend traits.

use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    sync::{Arc, RwLock},
    time::Duration,
};

use ashpd::{
    MaybeAppID, PortalError, Uri, WindowIdentifierType,
    async_trait::async_trait,
    backend::{
        Result,
        background::{
            Activity, AppState, AutoStartFlags, Background, BackgroundImpl, BackgroundSignalEmitter,
        },
        request::RequestImpl,
        screenshot::ScreenshotImpl,
        settings::{SettingsImpl, SettingsSignalEmitter},
        wallpaper::WallpaperImpl,
    },
    desktop::{
        Color, HandleToken,
        screenshot::{AvailableTargets, ColorOptions, Screenshot, ScreenshotOptions},
        settings::Namespace,
        wallpaper::{SetOn, WallpaperOptions},
    },
    enumflags2::BitFlags,
    zbus::{self, zvariant::OwnedValue},
};
use serde_json::json;

use derisk_portal_ui::{Reply, Request, background, screenshot};

use crate::{
    access::{self, Question},
    agent,
    appearance::{self, Appearance},
    apps,
    dialog::Dialogs,
    dialogs, files,
};

/// The object path every portal interface lives at.
pub const PATH: &str = "/org/freedesktop/portal/desktop";

/// The current appearance, `None` until derisk has published a theme.
pub type Shared = Arc<RwLock<Option<Appearance>>>;

fn failed(error: impl std::fmt::Display) -> PortalError {
    PortalError::Failed(error.to_string())
}

fn app_name(app_id: Option<&MaybeAppID>) -> String {
    app_id
        .map(ToString::to_string)
        .filter(|id| !id.is_empty())
        .unwrap_or_else(|| "An app".to_owned())
}

/// `org.freedesktop.impl.portal.Settings`: the appearance keys.
pub struct Settings {
    /// What [`watch_theme`] last read.
    pub appearance: Shared,
}

#[async_trait]
impl SettingsImpl for Settings {
    async fn read_all(
        &self,
        namespaces: Vec<String>,
    ) -> std::result::Result<HashMap<String, Namespace>, PortalError> {
        let mut all = HashMap::new();
        if appearance::requested(&namespaces, appearance::NAMESPACE)
            && let Some(current) = *self.appearance.read().map_err(failed)?
        {
            all.insert(appearance::NAMESPACE.to_owned(), current.namespace());
        }
        Ok(all)
    }

    async fn read(
        &self,
        namespace: &str,
        key: &str,
    ) -> std::result::Result<OwnedValue, PortalError> {
        let current = *self.appearance.read().map_err(failed)?;
        (namespace == appearance::NAMESPACE)
            .then_some(current)
            .flatten()
            .and_then(|a| a.get(key))
            .ok_or_else(|| PortalError::NotFound(format!("{namespace}.{key}")))
    }

    // ashpd 0.13 never calls this; `watch_theme` emits SettingChanged on the
    // connection itself.
    fn set_signal_emitter(&mut self, _: Arc<dyn SettingsSignalEmitter>) {}
}

/// Re-reads `theme.json` once a second, the same cadence the session polls
/// its settings at, and emits `SettingChanged` for each key that moved.
pub async fn watch_theme(connection: zbus::Connection, shared: Shared) {
    let mut ticker = tokio::time::interval(Duration::from_secs(1));
    loop {
        ticker.tick().await;
        let Some(dir) = std::env::var_os("XDG_RUNTIME_DIR") else {
            continue;
        };
        let path = Path::new(&dir).join("derisk").join("theme.json");
        let Some(now) = tokio::fs::read_to_string(&path)
            .await
            .ok()
            .and_then(|json| Appearance::parse(&json))
        else {
            continue;
        };
        let before = shared.read().ok().and_then(|a| *a);
        if before == Some(now) {
            continue;
        }
        if let Ok(mut current) = shared.write() {
            *current = Some(now);
        }
        for key in now.changed_since(before.as_ref()) {
            let Some(value) = now.get(key) else { continue };
            let _ = connection
                .emit_signal(
                    None::<()>,
                    PATH,
                    "org.freedesktop.impl.portal.Settings",
                    "SettingChanged",
                    &(appearance::NAMESPACE, key, &value),
                )
                .await;
        }
    }
}

/// `org.freedesktop.impl.portal.Screenshot`: the screen, a window or an
/// area of it, from the session's compositor.
pub struct Screenshots {
    /// Shows the screenshot dialog for interactive requests.
    pub dialogs: Dialogs,
}

#[async_trait]
impl RequestImpl for Screenshots {
    // ashpd aborts the request's future, which kills the dialog.
    async fn close(&self, _token: HandleToken) {}
}

/// A capture of the whole screen from the session.
struct Capture {
    path: PathBuf,
    size: [u32; 2],
}

async fn capture() -> Result<Capture> {
    let result = agent::call(json!({"method": "screenshot"}))
        .await
        .map_err(PortalError::Failed)?;
    let path = result["path"]
        .as_str()
        .ok_or_else(|| failed("the session returned no screenshot"))?;
    let size = |key: &str| result[key].as_u64().and_then(|v| u32::try_from(v).ok());
    Ok(Capture {
        path: PathBuf::from(path),
        size: [size("width").unwrap_or(0), size("height").unwrap_or(0)],
    })
}

/// The focused window's frame in a `state` result, as fractions of the
/// output: left, top, right, bottom.
pub fn window_area(state: &serde_json::Value) -> Option<screenshot::Area> {
    let out = &state["output"];
    let n = |v: &serde_json::Value| v.as_f64();
    let (ox, oy, ow, oh) = (n(&out["x"])?, n(&out["y"])?, n(&out["w"])?, n(&out["h"])?);
    if ow <= 0.0 || oh <= 0.0 {
        return None;
    }
    let window = state["windows"].as_array()?.iter().find(|w| {
        w["focused"] == json!(true) && w["minimized"] != json!(true) && w["frame"].is_object()
    })?;
    let f = &window["frame"];
    let (x, y, w, h) = (n(&f["x"])?, n(&f["y"])?, n(&f["w"])?, n(&f["h"])?);
    let fx = |v: f64| ((v - ox) / ow).clamp(0.0, 1.0) as f32;
    let fy = |v: f64| ((v - oy) / oh).clamp(0.0, 1.0) as f32;
    Some([fx(x), fy(y), fx(x + w), fy(y + h)])
}

#[async_trait]
impl ScreenshotImpl for Screenshots {
    fn available_targets(&self) -> BitFlags<AvailableTargets> {
        AvailableTargets::Screen | AvailableTargets::Window | AvailableTargets::Area
    }

    async fn screenshot(
        &self,
        token: HandleToken,
        app_id: Option<MaybeAppID>,
        _window_identifier: Option<WindowIdentifierType>,
        options: ScreenshotOptions,
    ) -> Result<Screenshot> {
        // Taken before the dialog opens, so the dialog is not in it. The
        // frontend asks before a non-interactive screenshot; an interactive
        // one shows derisk's screenshot dialog, which is the consent.
        let first = capture().await?;
        let (source, area) = if options.interactive() == Some(true) {
            let state = agent::call(json!({"method": "state"})).await.ok();
            let request = screenshot::Request {
                app_name: dialogs::app_name(app_id.as_ref()).await,
                heading: String::new(),
                preview: Some(first.path.clone()),
                screen: first.size,
                window: state.as_ref().and_then(window_area),
                // The session's capture has no pointer.
                pointer: false,
            };
            match dialogs::ask(&self.dialogs, &token, Request::Screenshot(request)).await? {
                Reply::Screenshot(choice) if choice.delay > 0 => {
                    tokio::time::sleep(Duration::from_secs(u64::from(choice.delay))).await;
                    (capture().await?.path, choice.area)
                }
                Reply::Screenshot(choice) => (first.path, choice.area),
                _ => return Err(failed("the screenshot dialog gave an unexpected answer")),
            }
        } else {
            (first.path, screenshot::FULL)
        };
        let (home, config) = files::home_and_config().ok_or_else(|| failed("HOME is not set"))?;
        let pictures = files::pictures_dir(&home, &config);
        let kept = tokio::task::spawn_blocking(move || {
            files::keep_screenshot_area(&source, area, &pictures)
        })
        .await
        .map_err(failed)?
        .map_err(failed)?;
        let uri = Uri::parse(&files::uri_from_path(&kept)).map_err(failed)?;
        Ok(Screenshot::new(uri))
    }

    async fn pick_color(
        &self,
        _token: HandleToken,
        _app_id: Option<MaybeAppID>,
        _window_identifier: Option<WindowIdentifierType>,
        _options: ColorOptions,
    ) -> Result<Color> {
        // Needs a pointer the person moves over the screen, which the
        // compositor does not offer the portal yet.
        Err(PortalError::NotAllowed(
            "derisk cannot pick a color from the screen yet".into(),
        ))
    }
}

/// `org.freedesktop.impl.portal.Wallpaper`: a picture as the desktop
/// background, through derisk's own settings file.
pub struct Wallpaper {
    /// For another backend's access dialog (`$DERISK_PORTAL_ACCESS`).
    pub connection: zbus::Connection,
    /// derisk's own access dialog.
    pub dialogs: Dialogs,
}

#[async_trait]
impl RequestImpl for Wallpaper {
    async fn close(&self, token: HandleToken) {
        access::close(&self.connection, &token.to_string()).await;
    }
}

#[async_trait]
impl WallpaperImpl for Wallpaper {
    async fn with_uri(
        &self,
        token: HandleToken,
        app_id: Option<MaybeAppID>,
        window_identifier: Option<WindowIdentifierType>,
        uri: Uri,
        options: WallpaperOptions,
    ) -> Result<()> {
        // The lock screen draws derisk's own gradient, never a picture.
        if options.set_on() == Some(SetOn::Lockscreen) {
            return Err(PortalError::NotAllowed(
                "derisk's lock screen does not show a picture".into(),
            ));
        }
        let picture = files::path_from_uri(uri.as_str())
            .ok_or_else(|| PortalError::InvalidArgument(format!("not a local file: {uri}")))?;
        // Without a preview the frontend has already asked; with one, the
        // backend is meant to show the picture first, so ask here.
        if options.show_preview() == Some(true) {
            let app = app_name(app_id.as_ref());
            let parent = window_identifier.map(|w| w.to_string()).unwrap_or_default();
            let id = app_id.as_ref().map(ToString::to_string).unwrap_or_default();
            let name = picture
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default();
            let granted = access::ask(
                &self.connection,
                &self.dialogs,
                &token.to_string(),
                &Question {
                    app_id: &id,
                    parent_window: &parent,
                    title: "Change the background?",
                    subtitle: &format!("{app} wants to set {name} as the desktop background."),
                    body: "You can change it back in Settings, under Wallpaper.",
                    grant_label: "Set Background",
                },
            )
            .await
            .map_err(failed)?;
            if !granted {
                return Err(PortalError::Cancelled("the wallpaper was declined".into()));
            }
        }
        let data = files::data_home().ok_or_else(|| failed("HOME is not set"))?;
        let installed = files::install_wallpaper(&picture, &data.join("derisk").join("wallpapers"))
            .map_err(|e| PortalError::InvalidArgument(e.to_string()))?;
        let path = derisk_settings::default_path().ok_or_else(|| failed("HOME is not set"))?;
        let (mut settings, _warnings) = derisk_settings::Settings::load(&path).map_err(failed)?;
        settings.wallpaper.kind = derisk_settings::WallpaperKind::Image;
        settings.wallpaper.path = installed;
        // The session polls the file and repaints within a second.
        settings.save(&path).map_err(failed)
    }
}

/// `org.freedesktop.impl.portal.Background`: which apps have windows, from
/// the session, and the background dialog for apps without a stored choice.
pub struct Apps {
    /// Shows the background dialog.
    pub dialogs: Dialogs,
}

#[async_trait]
impl RequestImpl for Apps {
    async fn close(&self, _token: HandleToken) {}
}

#[async_trait]
impl BackgroundImpl for Apps {
    async fn get_app_state(
        &self,
    ) -> std::result::Result<HashMap<MaybeAppID, AppState>, PortalError> {
        let state = agent::call(json!({"method": "state"}))
            .await
            .map_err(PortalError::Failed)?;
        Ok(agent::apps(&state)
            .into_iter()
            .map(|(app, seen)| {
                let state = match seen {
                    agent::Visibility::Active => AppState::Active,
                    agent::Visibility::Running => AppState::Running,
                };
                (MaybeAppID::from(app), state)
            })
            .collect())
    }

    async fn notify_background(
        &self,
        token: HandleToken,
        app_id: MaybeAppID,
        name: &str,
    ) -> std::result::Result<Background, PortalError> {
        // The frontend asks only about apps with no stored choice, and stores
        // Allow and Forbid; "Run in the background" off allows this instance
        // only, so nothing is stored for a choice the person didn't make.
        let id = app_id.to_string();
        let (who, command, autostart) = {
            let id = id.clone();
            tokio::task::spawn_blocking(move || {
                let entries = apps::entries();
                let entry = apps::find(&entries, &id);
                let autostart = apps::autostart_dir()
                    .is_some_and(|d| d.join(format!("{id}.desktop")).is_file());
                (
                    apps::display_name(&entries, &id),
                    entry.map(apps::command_line).unwrap_or_default(),
                    autostart,
                )
            })
            .await
            .map_err(failed)?
        };
        let app_name = if name.is_empty() {
            who
        } else {
            name.to_owned()
        };
        let request = background::Request {
            app_id: id.clone(),
            app_name: app_name.clone(),
            description: "It keeps running after its window closes.".into(),
            command: command.clone(),
            autostart,
        };
        let choice = match dialogs::ask(&self.dialogs, &token, Request::Background(request)).await {
            Ok(Reply::Background(choice)) => choice,
            Err(PortalError::Cancelled(_)) => return Ok(Background::new(Activity::Forbid)),
            Ok(_) | Err(_) => {
                // No dialog (no display, say): let it run this once.
                eprintln!(
                    "xdg-desktop-portal-derisk: {app_name} ({id}) is running in the background"
                );
                return Ok(Background::new(Activity::AllowInstance));
            }
        };
        if choice.autostart != autostart
            && let Some(dir) = apps::autostart_dir()
        {
            let argv: Vec<String> = command.split_whitespace().map(str::to_owned).collect();
            if let Err(e) =
                apps::set_autostart(&dir, &id, choice.autostart, &app_name, &argv, false)
            {
                eprintln!("xdg-desktop-portal-derisk: cannot change autostart for {id}: {e}");
            }
        }
        Ok(Background::new(if choice.background {
            Activity::Allow
        } else {
            Activity::AllowInstance
        }))
    }

    async fn enable_autostart(
        &self,
        _app_id: MaybeAppID,
        _enable: bool,
        _commandline: Vec<String>,
        _flags: BitFlags<AutoStartFlags>,
    ) -> std::result::Result<bool, PortalError> {
        // Deprecated: the frontend writes autostart files itself and no
        // longer calls this.
        Ok(false)
    }

    fn set_signal_emitter(&mut self, _: Arc<dyn BackgroundSignalEmitter>) {}
}

/// Asks the session which apps have windows every two seconds and emits
/// `RunningApplicationsChanged` when that changes, so the frontend looks
/// again.
pub async fn watch_apps(connection: zbus::Connection) {
    let mut ticker = tokio::time::interval(Duration::from_secs(2));
    let mut before = None;
    loop {
        ticker.tick().await;
        let Ok(state) = agent::call(json!({"method": "state"})).await else {
            continue;
        };
        let now = agent::apps(&state);
        if before.as_ref() == Some(&now) {
            continue;
        }
        let first = before.is_none();
        before = Some(now);
        if !first {
            let _ = connection
                .emit_signal(
                    None::<()>,
                    PATH,
                    "org.freedesktop.impl.portal.Background",
                    "RunningApplicationsChanged",
                    &(),
                )
                .await;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_focused_window_is_a_fraction_of_the_output() {
        let state = json!({
            "output": {"x": 0, "y": 0, "w": 2000, "h": 1000},
            "windows": [
                {"focused": false, "frame": {"x": 0, "y": 0, "w": 1000, "h": 1000}},
                {"focused": true, "minimized": false, "frame": {"x": 1000, "y": 250, "w": 1000, "h": 500}}
            ]
        });
        assert_eq!(window_area(&state), Some([0.5, 0.25, 1.0, 0.75]));
        let none = json!({"output": {"x": 0, "y": 0, "w": 2000, "h": 1000}, "windows": []});
        assert_eq!(window_area(&none), None);
    }
}
