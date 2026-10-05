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
//! [`environment`]: the cursor theme, and `XDG_CONFIG_DIRS` with the GTK
//! settings first, so they apply unless the user's own `~/.config/gtk-*`
//! says otherwise.
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
    }
}
