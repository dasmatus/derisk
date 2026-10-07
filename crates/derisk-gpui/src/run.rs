//! Opening an app's window and keeping it on the desktop theme.

use std::time::Duration;

use gpui::{
    App, AppContext, Application, Bounds, Global, TitlebarOptions, WindowBounds, WindowOptions, px,
    size,
};
use mcsapi_components_gpui::Tokens;
use mcsapi_theme::Theme;

use crate::{CALCULATOR, ThemeWatch, calculator::Calculator};

/// The desktop theme the windows draw with, beside the component [`Tokens`]
/// derived from it: views read fonts from here.
pub struct DesktopTheme(pub Theme);

impl Global for DesktopTheme {}

fn install(theme: Theme, cx: &mut App) {
    Tokens::from_spec(&theme).install(cx);
    cx.set_global(DesktopTheme(theme));
    cx.refresh_windows();
}

/// Runs the app `id` (one of [`crate::APPS`]) until its window closes.
///
/// Returns an error for an unknown ID without opening anything.
pub fn run(id: &str) -> Result<(), String> {
    if !crate::APPS.contains(&id) {
        return Err(format!(
            "no GPUI app {id}; known: {}",
            crate::APPS.join(", ")
        ));
    }
    let id = id.to_owned();
    Application::new().run(move |cx: &mut App| {
        mcsapi_components_gpui::bind_text_input_keys(cx);
        let mut watch = ThemeWatch::default();
        install(watch.poll().unwrap_or_default(), cx);
        // Settings are a file; re-read it about once a second, like the
        // session does, so a theme change reaches running apps too.
        cx.spawn(async move |cx| {
            loop {
                cx.background_executor().timer(Duration::from_secs(1)).await;
                if let Some(theme) = watch.poll()
                    && cx.update(|cx| install(theme, cx)).is_err()
                {
                    break;
                }
            }
        })
        .detach();
        let (title, width, height) = match id.as_str() {
            CALCULATOR => ("Calculator", 640.0, 480.0),
            _ => unreachable!("checked against APPS above"),
        };
        let bounds = Bounds::centered(None, size(px(width), px(height)), cx);
        let options = WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(bounds)),
            titlebar: Some(TitlebarOptions {
                title: Some(title.into()),
                ..Default::default()
            }),
            // The compositor matches windows to apps (and their .desktop
            // files) by this.
            app_id: Some(id.clone()),
            ..Default::default()
        };
        let opened = cx.open_window(options, |window, cx| {
            cx.new(|cx| Calculator::new(window, cx))
        });
        if let Err(error) = opened {
            tracing::error!("opening the {title} window: {error}");
            cx.quit();
            return;
        }
        cx.on_window_closed(|cx| cx.quit()).detach();
        cx.activate(true);
    });
    Ok(())
}
