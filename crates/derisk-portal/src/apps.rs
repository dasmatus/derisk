//! Installed apps: names and icons for the dialogs, default handlers and
//! autostart entries.

use std::{
    io,
    path::{Path, PathBuf},
};

use derisk::desktop::{self, DesktopEntry};

/// Makes `app_id` the default app for `content_type` (`xdg-mime
/// default`), logging failure: the app opens this time either way.
pub async fn make_default(app_id: &str, content_type: &str) {
    let status = tokio::process::Command::new("xdg-mime")
        .args(["default", &format!("{app_id}.desktop"), content_type])
        .status()
        .await;
    if !status.is_ok_and(|s| s.success()) {
        eprintln!(
            "xdg-desktop-portal-derisk: could not make {app_id} the default for {content_type}"
        );
    }
}

/// Every installed app's desktop entry.
pub fn entries() -> Vec<DesktopEntry> {
    desktop::scan(&desktop::application_dirs())
}

/// The desktop entry of portal app ID `app_id` (no `.desktop`).
pub fn find<'a>(entries: &'a [DesktopEntry], app_id: &str) -> Option<&'a DesktopEntry> {
    let id = format!("{app_id}.desktop");
    entries.iter().find(|e| e.id == id)
}

/// A name to show for `app_id`: its desktop entry's, else the last part of
/// the ID, else "An app" for an unsandboxed caller with no ID.
pub fn display_name(entries: &[DesktopEntry], app_id: &str) -> String {
    if let Some(entry) = find(entries, app_id) {
        return entry.name.clone();
    }
    match app_id.rsplit('.').next() {
        Some(last) if !last.is_empty() => last.to_owned(),
        _ => "An app".to_owned(),
    }
}

/// Whether `app_id` is an installed Flatpak app (per user or system wide).
pub fn is_flatpak(app_id: &str) -> bool {
    if !desktop::is_action_id(app_id) || app_id.is_empty() {
        return false;
    }
    let mut dirs = vec![PathBuf::from("/var/lib/flatpak/app")];
    if let Some(home) = std::env::var_os("HOME") {
        dirs.push(PathBuf::from(home).join(".local/share/flatpak/app"));
    }
    dirs.iter().any(|d| d.join(app_id).is_dir())
}

/// A content type's singular and plural names: "PDF document" / "PDF
/// documents". Unknown types are named after their subtype.
pub fn type_names(content_type: &str) -> (String, String) {
    const KNOWN: &[(&str, &str, &str)] = &[
        ("application/pdf", "PDF document", "PDF documents"),
        ("text/plain", "Text file", "text files"),
        ("text/markdown", "Markdown document", "Markdown documents"),
        ("text/html", "Web page", "web pages"),
        ("x-scheme-handler/http", "Web link", "web links"),
        ("x-scheme-handler/https", "Web link", "web links"),
        (
            "x-scheme-handler/mailto",
            "Email address",
            "email addresses",
        ),
        ("inode/directory", "Folder", "folders"),
        ("application/zip", "ZIP archive", "ZIP archives"),
        (
            "application/vnd.oasis.opendocument.text",
            "Text document",
            "text documents",
        ),
        (
            "application/vnd.oasis.opendocument.spreadsheet",
            "Spreadsheet",
            "spreadsheets",
        ),
    ];
    if let Some((_, one, many)) = KNOWN.iter().find(|(t, ..)| *t == content_type) {
        return ((*one).to_owned(), (*many).to_owned());
    }
    let (group, sub) = content_type.split_once('/').unwrap_or((content_type, ""));
    let sub = sub.trim_start_matches("x-").trim_start_matches("vnd.");
    let noun = match group {
        "image" => "image",
        "video" => "video",
        "audio" => "audio file",
        _ => "file",
    };
    let label = sub.to_uppercase();
    if label.is_empty() {
        ("File".to_owned(), "files".to_owned())
    } else {
        (format!("{label} {noun}"), format!("{label} {noun}s"))
    }
}

/// The `Exec` line of `entry` without field codes (`%U`, `%f`...), for
/// showing what will run.
pub fn command_line(entry: &DesktopEntry) -> String {
    entry
        .exec
        .split_whitespace()
        .filter(|part| !(part.len() == 2 && part.starts_with('%')))
        .collect::<Vec<_>>()
        .join(" ")
}

/// `$XDG_CONFIG_HOME/autostart` (default `~/.config/autostart`).
pub fn autostart_dir() -> Option<PathBuf> {
    std::env::var_os("XDG_CONFIG_HOME")
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))
        .map(|c| c.join("autostart"))
}

/// Marks autostart entries this backend wrote, so it only removes its own.
const MARKER: &str = "X-Derisk-Portal=true";

/// Quotes one argument for an `Exec` line (the desktop entry spec's rules).
fn quote(arg: &str) -> String {
    let plain = !arg.is_empty()
        && arg
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || "-_./=:,+@".contains(c));
    if plain {
        return arg.to_owned();
    }
    let mut out = String::from("\"");
    for c in arg.chars() {
        if matches!(c, '"' | '`' | '$' | '\\') {
            out.push('\\');
        }
        out.push(c);
    }
    out.push('"');
    out.replace('%', "%%")
}

/// The autostart entry that runs `commandline` for `app_id` at login.
pub fn autostart_entry(
    app_id: &str,
    name: &str,
    commandline: &[String],
    dbus_activatable: bool,
) -> String {
    let exec: Vec<String> = commandline.iter().map(|a| quote(a)).collect();
    let mut entry = format!(
        "[Desktop Entry]\nType=Application\nName={name}\nExec={}\nX-Flatpak={app_id}\n{MARKER}\n",
        exec.join(" ")
    );
    if dbus_activatable {
        entry.push_str("DBusActivatable=true\n");
    }
    entry
}

/// Writes or removes `app_id`'s autostart entry in `dir`. Removing only
/// touches an entry this backend wrote.
pub fn set_autostart(
    dir: &Path,
    app_id: &str,
    enable: bool,
    name: &str,
    commandline: &[String],
    dbus_activatable: bool,
) -> io::Result<()> {
    if !desktop::is_action_id(app_id) || app_id.is_empty() {
        return Err(io::Error::new(io::ErrorKind::InvalidInput, "not an app ID"));
    }
    let path = dir.join(format!("{app_id}.desktop"));
    if enable {
        if commandline.is_empty() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "no command to start",
            ));
        }
        std::fs::create_dir_all(dir)?;
        std::fs::write(
            &path,
            autostart_entry(app_id, name, commandline, dbus_activatable),
        )
    } else {
        match std::fs::read_to_string(&path) {
            Ok(text) if text.lines().any(|l| l.trim() == MARKER) => std::fs::remove_file(&path),
            Ok(_) => Ok(()),
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(e),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(name: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("derisk-portal-apps-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn autostart_entries_are_written_and_only_ours_removed() {
        let dir = temp_dir("autostart");
        let argv = ["sync".to_owned(), "--home".to_owned(), "a b".to_owned()];
        set_autostart(&dir, "org.example.Sync", true, "Sync", &argv, false).unwrap();
        let text = std::fs::read_to_string(dir.join("org.example.Sync.desktop")).unwrap();
        assert!(text.contains("Exec=sync --home \"a b\"\n"), "{text}");
        assert!(text.contains("X-Flatpak=org.example.Sync\n"));
        set_autostart(&dir, "org.example.Sync", false, "Sync", &[], false).unwrap();
        assert!(!dir.join("org.example.Sync.desktop").exists());

        // Someone else's entry stays.
        std::fs::write(dir.join("org.example.Other.desktop"), "[Desktop Entry]\n").unwrap();
        set_autostart(&dir, "org.example.Other", false, "Other", &[], false).unwrap();
        assert!(dir.join("org.example.Other.desktop").exists());

        assert!(set_autostart(&dir, "../evil", true, "x", &argv, false).is_err());
        assert!(set_autostart(&dir, "org.example.Sync", true, "x", &[], false).is_err());
    }

    #[test]
    fn content_types_have_readable_names() {
        assert_eq!(
            type_names("application/pdf"),
            ("PDF document".to_owned(), "PDF documents".to_owned())
        );
        assert_eq!(
            type_names("image/png"),
            ("PNG image".to_owned(), "PNG images".to_owned())
        );
        assert_eq!(type_names("application/x-foo").1, "FOO files");
    }

    #[test]
    fn names_fall_back_to_the_app_id() {
        assert_eq!(display_name(&[], "org.example.Calls"), "Calls");
        assert_eq!(display_name(&[], ""), "An app");
    }
}
