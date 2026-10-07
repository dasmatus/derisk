//! Publishes the desktop theme to everything that is not drawn by derisk.
//!
//! derisk's own chrome and core apps take the theme directly. Other software
//! reads it from files under `$XDG_RUNTIME_DIR/derisk`, rewritten whenever
//! the theme changes:
//!
//! | File | Read by |
//! | --- | --- |
//! | `theme.json` | anything: the Android translation layer, scripts, agents |
//! | `android/values/colors.xml` | the Android translation layer's theme bridge |
//! | `config/gtk-3.0/settings.ini`, `config/gtk-4.0/settings.ini` | GTK, through `XDG_CONFIG_DIRS` |
//!
//! Each file is written to a temporary name and renamed into place, so a
//! reader never sees half a theme; watch the directory for renames
//! (`IN_MOVED_TO`) to follow changes. Apps launched from the shell also get
//! [`environment`]: the cursor theme, the icon theme for Qt, and
//! `XDG_CONFIG_DIRS` with the GTK settings first, so they apply unless the
//! user's own `~/.config/gtk-*` says otherwise.
//!
//! The icon theme reaches two places files cannot, through
//! [`icon_theme_argv`]: GSettings, which GTK prefers over `settings.ini`
//! whenever the GNOME schemas are installed and which the GTK portal backend
//! serves to Flatpak apps, and the environment of the user manager and D-Bus
//! activation, for Qt apps the shell did not launch.
//!
//! ```
//! let dir = std::env::temp_dir().join(format!("derisk-theme-doc-{}", std::process::id()));
//! let theme = mcsapi_theme::Theme::light();
//! derisk::theme::publish(&dir, "derisk-light", &theme).unwrap();
//! let json = std::fs::read_to_string(dir.join("theme.json")).unwrap();
//! assert!(json.contains("\"scheme\":\"light\""));
//! # std::fs::remove_dir_all(&dir).unwrap();
//! ```

use std::{
    ffi::OsString,
    fs, io,
    path::{Path, PathBuf},
};

use mcsapi_theme::{Theme, export};

/// `$XDG_RUNTIME_DIR/derisk`, where the theme is published. `None` when
/// `XDG_RUNTIME_DIR` is unset or relative.
pub fn runtime_dir() -> Option<PathBuf> {
    std::env::var_os("XDG_RUNTIME_DIR")
        .map(PathBuf::from)
        .filter(|dir| dir.is_absolute())
        .map(|dir| dir.join("derisk"))
}

/// Writes `contents` to `path` atomically, creating parent directories.
fn replace(path: &Path, contents: &str) -> io::Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let mut temporary = path.as_os_str().to_owned();
    temporary.push(".tmp");
    fs::write(&temporary, contents)?;
    fs::rename(&temporary, path)
}

/// Writes every published file for `theme`, named `id`, under `dir`.
pub fn publish(dir: &Path, id: &str, theme: &Theme) -> io::Result<()> {
    let gtk = export::gtk_settings(theme);
    replace(
        &dir.join("android/values/colors.xml"),
        &export::android_colors(theme),
    )?;
    replace(&dir.join("config/gtk-3.0/settings.ini"), &gtk)?;
    replace(&dir.join("config/gtk-4.0/settings.ini"), &gtk)?;
    // Last, so a reader that sees the new JSON finds the other files current.
    replace(&dir.join("theme.json"), &export::json(id, theme))
}

/// The variable Qt 5.15 and 6 read the icon theme from before asking their
/// platform theme, which in a derisk session (neither GNOME nor Plasma)
/// names none, leaving Qt apps with hicolor's few icons.
pub const QT_ICON_THEME: &str = "QT_QPA_SYSTEM_ICON_THEME";

/// `icons` if it can be handed on as a theme name: not empty, and without
/// control characters, which no theme directory has and which would end a
/// `settings.ini` line or an environment assignment early.
fn icon_theme_name(icons: &str) -> Option<&str> {
    (!icons.is_empty() && !icons.chars().any(char::is_control)).then_some(icons)
}

/// Commands that make `icons` the icon theme where [`publish`]'s files do not
/// reach: GSettings' `org.gnome.desktop.interface icon-theme`, written with
/// `dconf` so no schema has to be installed, and the user manager's and
/// D-Bus activation's [`QT_ICON_THEME`]. Empty for a name that cannot be
/// handed on.
pub fn icon_theme_argv(icons: &str) -> impl Iterator<Item = Vec<String>> + use<> {
    icon_theme_name(icons)
        .map(|name| {
            // A GVariant string literal: single quotes, with quotes and
            // backslashes inside escaped.
            let quoted = format!("'{}'", name.replace('\\', "\\\\").replace('\'', "\\'"));
            [
                vec![
                    "dconf".to_owned(),
                    "write".to_owned(),
                    "/org/gnome/desktop/interface/icon-theme".to_owned(),
                    quoted,
                ],
                vec![
                    "dbus-update-activation-environment".to_owned(),
                    "--systemd".to_owned(),
                    format!("{QT_ICON_THEME}={name}"),
                ],
            ]
        })
        .into_iter()
        .flatten()
}

/// Environment for apps launched under `theme` published in `dir`.
///
/// `system_config_dirs` is the session's `XDG_CONFIG_DIRS`; the published
/// GTK settings go in front of it (default `/etc/xdg`).
pub fn environment(
    dir: &Path,
    theme: &Theme,
    system_config_dirs: Option<OsString>,
) -> Vec<(String, String)> {
    let mut env: Vec<(String, String)> = export::environment(theme)
        .into_iter()
        .map(|(key, value)| (key.to_owned(), value))
        .collect();
    if let Some(icons) = icon_theme_name(&theme.icons.theme) {
        env.push((QT_ICON_THEME.to_owned(), icons.to_owned()));
    }
    let system = system_config_dirs
        .filter(|dirs| !dirs.is_empty())
        .unwrap_or_else(|| "/etc/xdg".into());
    let ours = dir.join("config");
    let dirs =
        std::iter::once(ours.clone()).chain(std::env::split_paths(&system).filter(|d| *d != ours));
    if let Ok(joined) = std::env::join_paths(dirs) {
        env.push((
            "XDG_CONFIG_DIRS".to_owned(),
            joined.to_string_lossy().into_owned(),
        ));
    }
    env
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn publish_replaces_every_file() {
        let dir = std::env::temp_dir().join(format!("derisk-theme-{}", std::process::id()));
        publish(&dir, "derisk-dark", &Theme::dark()).unwrap();
        publish(&dir, "derisk-light", &Theme::light()).unwrap();
        let json = fs::read_to_string(dir.join("theme.json")).unwrap();
        assert!(json.contains("\"id\":\"derisk-light\""));
        let ini = fs::read_to_string(dir.join("config/gtk-4.0/settings.ini")).unwrap();
        assert!(ini.contains("gtk-application-prefer-dark-theme=0"));
        let xml = fs::read_to_string(dir.join("android/values/colors.xml")).unwrap();
        assert!(xml.contains("mcsapi_primary"));
        // No temporary files are left behind.
        assert!(!dir.join("theme.json.tmp").exists());
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn environment_puts_the_gtk_settings_first_once() {
        let dir = Path::new("/run/user/1000/derisk");
        let env = environment(
            dir,
            &Theme::dark(),
            Some("/run/user/1000/derisk/config:/etc/xdg".into()),
        );
        let dirs = env.iter().find(|(k, _)| k == "XDG_CONFIG_DIRS").unwrap();
        assert_eq!(dirs.1, "/run/user/1000/derisk/config:/etc/xdg");
        let env = environment(dir, &Theme::dark(), None);
        assert!(env.contains(&(
            "XDG_CONFIG_DIRS".into(),
            "/run/user/1000/derisk/config:/etc/xdg".into()
        )));
        assert!(env.contains(&("XCURSOR_SIZE".into(), "24".into())));
        assert!(env.contains(&(QT_ICON_THEME.into(), Theme::dark().icons.theme)));
    }

    #[test]
    fn icon_theme_argv_quotes_the_name_for_dconf() {
        let argv: Vec<_> = icon_theme_argv("Papirus-Dark").collect();
        assert_eq!(
            argv[0],
            [
                "dconf",
                "write",
                "/org/gnome/desktop/interface/icon-theme",
                "'Papirus-Dark'"
            ]
        );
        assert_eq!(
            argv[1],
            [
                "dbus-update-activation-environment",
                "--systemd",
                "QT_QPA_SYSTEM_ICON_THEME=Papirus-Dark"
            ]
        );
        assert_eq!(
            icon_theme_argv(r"It's\Odd").next().unwrap()[3],
            r"'It\'s\\Odd'"
        );
        assert!(icon_theme_argv("").next().is_none());
        assert!(icon_theme_argv("Bad\nName").next().is_none());
    }
}
