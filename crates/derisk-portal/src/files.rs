//! Files the portals hand out and take in: `file://` URIs, where
//! screenshots are kept, and the copy of a picture an app makes the
//! wallpaper.

use std::{
    fs, io,
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

/// The local path a `file://` URI names. Other schemes are refused: the
/// portal frontend only ever passes files, and fetching anything else for an
/// app is not this backend's job.
pub fn path_from_uri(uri: &str) -> Option<PathBuf> {
    let rest = uri.strip_prefix("file://")?;
    // `file://host/path` names another machine; only an empty or
    // `localhost` authority is this one.
    let path = rest.strip_prefix("localhost").unwrap_or(rest);
    if !path.starts_with('/') {
        return None;
    }
    let bytes = path.as_bytes();
    let mut decoded = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' {
            let hex = std::str::from_utf8(bytes.get(i + 1..i + 3)?).ok()?;
            decoded.push(u8::from_str_radix(hex, 16).ok()?);
            i += 3;
        } else {
            decoded.push(bytes[i]);
            i += 1;
        }
    }
    use std::os::unix::ffi::OsStringExt;
    let path = PathBuf::from(std::ffi::OsString::from_vec(decoded));
    // A NUL cannot be in a path, and `..` has no business in a URI the
    // frontend built from a real file.
    (!path.as_os_str().as_encoded_bytes().contains(&0)
        && !path
            .components()
            .any(|c| c == std::path::Component::ParentDir))
    .then_some(path)
}

/// A `file://` URI for `path`, escaping everything outside RFC 3986's
/// unreserved set and `/`.
pub fn uri_from_path(path: &Path) -> String {
    let mut uri = String::from("file://");
    for &byte in path.as_os_str().as_encoded_bytes() {
        if byte.is_ascii_alphanumeric() || b"/-._~".contains(&byte) {
            uri.push(byte as char);
        } else {
            uri.push_str(&format!("%{byte:02X}"));
        }
    }
    uri
}

/// The extension for a picture the wallpaper can show, from its first
/// bytes rather than its name: a file from an app's sandbox can be called
/// anything.
pub fn image_extension(head: &[u8]) -> Option<&'static str> {
    if head.starts_with(b"\x89PNG\r\n\x1a\n") {
        Some("png")
    } else if head.starts_with(&[0xff, 0xd8, 0xff]) {
        Some("jpg")
    } else if head.len() >= 12 && &head[..4] == b"RIFF" && &head[8..12] == b"WEBP" {
        Some("webp")
    } else {
        None
    }
}

/// The user's pictures folder: `XDG_PICTURES_DIR` from
/// `user-dirs.dirs` under `config`, else `~/Pictures`.
pub fn pictures_dir(home: &Path, config: &Path) -> PathBuf {
    let dirs = fs::read_to_string(config.join("user-dirs.dirs")).unwrap_or_default();
    for line in dirs.lines() {
        let Some(value) = line.trim().strip_prefix("XDG_PICTURES_DIR=") else {
            continue;
        };
        let value = value.trim().trim_matches('"');
        let path = match value.strip_prefix("$HOME") {
            Some(rest) => home.join(rest.trim_start_matches('/')),
            None => PathBuf::from(value),
        };
        // xdg-user-dirs sets an unused folder to $HOME itself; screenshots
        // do not belong loose in there.
        if path.is_absolute() && path != home {
            return path;
        }
    }
    home.join("Pictures")
}

/// `$HOME` and `$XDG_CONFIG_HOME` (default `~/.config`).
pub fn home_and_config() -> Option<(PathBuf, PathBuf)> {
    let home = PathBuf::from(std::env::var_os("HOME")?);
    let config = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .unwrap_or_else(|| home.join(".config"));
    Some((home, config))
}

/// `$XDG_DATA_HOME` (default `~/.local/share`).
pub fn data_home() -> Option<PathBuf> {
    std::env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .or_else(|| Some(PathBuf::from(std::env::var_os("HOME")?).join(".local/share")))
}

/// The UTC date and time `seconds` after the epoch, as
/// `2026-10-05 07:48:09`. Howard Hinnant's `civil_from_days`.
pub fn utc_timestamp(seconds: u64) -> String {
    let days = (seconds / 86_400) as i64;
    let rem = seconds % 86_400;
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    format!(
        "{year:04}-{month:02}-{day:02} {:02}:{:02}:{:02}",
        rem / 3600,
        rem / 60 % 60,
        rem % 60
    )
}

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

/// Copies the session's screenshot into `Screenshots` under the pictures
/// folder, where it outlives the few the session keeps in the runtime
/// directory, and returns the copy.
pub fn keep_screenshot(capture: &Path, pictures: &Path) -> io::Result<PathBuf> {
    let path = new_screenshot(pictures)?;
    fs::copy(capture, &path)?;
    Ok(path)
}

/// A free `Screenshots/Screenshot from <time>.png` under `pictures`.
fn new_screenshot(pictures: &Path) -> io::Result<PathBuf> {
    let dir = pictures.join("Screenshots");
    fs::create_dir_all(&dir)?;
    let stamp = utc_timestamp(now());
    let mut path = dir.join(format!("Screenshot from {stamp}.png"));
    let mut n = 1;
    while path.exists() {
        n += 1;
        path = dir.join(format!("Screenshot from {stamp} ({n}).png"));
    }
    Ok(path)
}

/// [`keep_screenshot`], cropped to `area` (left, top, right, bottom as
/// fractions of the capture). The whole screen is copied as it is.
pub fn keep_screenshot_area(
    capture: &Path,
    area: [f32; 4],
    pictures: &Path,
) -> io::Result<PathBuf> {
    if area == [0.0, 0.0, 1.0, 1.0] {
        return keep_screenshot(capture, pictures);
    }
    let image = image::open(capture).map_err(io::Error::other)?;
    let (w, h) = (image.width() as f32, image.height() as f32);
    let [l, t, r, b] = area.map(|v| v.clamp(0.0, 1.0));
    let x = (l * w).round() as u32;
    let y = (t * h).round() as u32;
    let cw = (((r - l) * w).round() as u32).max(1);
    let ch = (((b - t) * h).round() as u32).max(1);
    let path = new_screenshot(pictures)?;
    image
        .crop_imm(x, y, cw, ch)
        .save_with_format(&path, image::ImageFormat::Png)
        .map_err(io::Error::other)?;
    Ok(path)
}

/// Copies `picture` into `dir` as the wallpaper and removes the previous
/// portal wallpaper. The copy is what the settings name: a file an app
/// passed may be in its sandbox or the document portal and gone later.
///
/// Each copy gets a new name so the session, which reloads the wallpaper
/// when the setting changes, sees a change.
pub fn install_wallpaper(picture: &Path, dir: &Path) -> io::Result<PathBuf> {
    let bytes = fs::read(picture)?;
    let ext = image_extension(&bytes).ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            "not a PNG, JPEG or WebP picture",
        )
    })?;
    fs::create_dir_all(dir)?;
    let path = dir.join(format!("portal-{}.{ext}", now()));
    let mut temporary = path.as_os_str().to_owned();
    temporary.push(".tmp");
    fs::write(&temporary, &bytes)?;
    fs::rename(&temporary, &path)?;
    for entry in fs::read_dir(dir)?.flatten() {
        let old = entry.path();
        let ours = old
            .file_name()
            .and_then(|n| n.to_str())
            .is_some_and(|n| n.starts_with("portal-"));
        if ours && old != path {
            let _ = fs::remove_file(old);
        }
    }
    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("derisk-portal-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn crops_a_screenshot_to_the_chosen_area() {
        let dir = scratch("crop");
        let capture = dir.join("screen.png");
        image::RgbaImage::from_pixel(200, 100, image::Rgba([10, 20, 30, 255]))
            .save(&capture)
            .unwrap();
        let kept = keep_screenshot_area(&capture, [0.25, 0.5, 0.75, 1.0], &dir).unwrap();
        assert!(kept.starts_with(dir.join("Screenshots")));
        let out = image::open(&kept).unwrap();
        assert_eq!((out.width(), out.height()), (100, 50));
        // The whole screen is copied byte for byte.
        let whole = keep_screenshot_area(&capture, [0.0, 0.0, 1.0, 1.0], &dir).unwrap();
        assert_eq!(fs::read(&whole).unwrap(), fs::read(&capture).unwrap());
    }

    #[test]
    fn round_trips_uris() {
        let path = Path::new("/home/a b/Pictures/ü%.png");
        let uri = uri_from_path(path);
        assert_eq!(uri, "file:///home/a%20b/Pictures/%C3%BC%25.png");
        assert_eq!(path_from_uri(&uri).unwrap(), path);
        assert_eq!(
            path_from_uri("file://localhost/tmp/x").unwrap(),
            Path::new("/tmp/x")
        );
    }

    #[test]
    fn refuses_uris_that_are_not_local_files() {
        assert_eq!(path_from_uri("https://example.com/x.png"), None);
        assert_eq!(path_from_uri("file://host/x.png"), None);
        assert_eq!(path_from_uri("file:///a/%00b"), None);
        assert_eq!(path_from_uri("file:///a/../etc/shadow"), None);
        assert_eq!(path_from_uri("file:///a/%zz"), None);
    }

    #[test]
    fn sniffs_pictures() {
        assert_eq!(image_extension(b"\x89PNG\r\n\x1a\nrest"), Some("png"));
        assert_eq!(image_extension(&[0xff, 0xd8, 0xff, 0xe0]), Some("jpg"));
        assert_eq!(image_extension(b"RIFF\0\0\0\0WEBPVP8 "), Some("webp"));
        assert_eq!(image_extension(b"GIF89a"), None);
    }

    #[test]
    fn finds_the_pictures_folder() {
        let dir = scratch("dirs");
        let home = Path::new("/home/u");
        assert_eq!(pictures_dir(home, &dir), home.join("Pictures"));
        fs::write(
            dir.join("user-dirs.dirs"),
            "# comment\nXDG_PICTURES_DIR=\"$HOME/Bilder\"\n",
        )
        .unwrap();
        assert_eq!(pictures_dir(home, &dir), home.join("Bilder"));
        fs::write(dir.join("user-dirs.dirs"), "XDG_PICTURES_DIR=\"$HOME/\"\n").unwrap();
        assert_eq!(pictures_dir(home, &dir), home.join("Pictures"));
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn formats_utc() {
        assert_eq!(utc_timestamp(0), "1970-01-01 00:00:00");
        assert_eq!(utc_timestamp(1_791_186_489), "2026-10-05 07:48:09");
        assert_eq!(utc_timestamp(951_782_400), "2000-02-29 00:00:00");
    }

    #[test]
    fn keeps_screenshots_without_overwriting() {
        let dir = scratch("shots");
        let capture = dir.join("screen-1.png");
        fs::write(&capture, b"png").unwrap();
        let first = keep_screenshot(&capture, &dir).unwrap();
        let second = keep_screenshot(&capture, &dir).unwrap();
        assert_ne!(first, second);
        assert_eq!(fs::read(second).unwrap(), b"png");
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn installs_one_wallpaper_at_a_time() {
        let dir = scratch("walls");
        let picture = dir.join("from-app");
        fs::write(&picture, b"\x89PNG\r\n\x1a\npixels").unwrap();
        let walls = dir.join("wallpapers");
        fs::create_dir_all(&walls).unwrap();
        fs::write(walls.join("portal-1.jpg"), b"old").unwrap();
        fs::write(walls.join("mine.png"), b"keep").unwrap();
        let installed = install_wallpaper(&picture, &walls).unwrap();
        assert_eq!(installed.extension().unwrap(), "png");
        assert!(!walls.join("portal-1.jpg").exists());
        assert!(walls.join("mine.png").exists());
        fs::write(&picture, b"GIF89a").unwrap();
        assert!(install_wallpaper(&picture, &walls).is_err());
        fs::remove_dir_all(&dir).unwrap();
    }
}
