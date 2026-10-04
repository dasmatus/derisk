//! Monochrome icons, looked up in freedesktop icon-theme order.
//!
//! Every icon the shell draws takes the text color. A name resolves to the
//! first of:
//!
//! 1. **Papirus symbolic**: `Papirus/symbolic/<context>/<name>-symbolic.svg`
//!    or `Papirus/16x16/<context>/<name>.svg`, already single-color.
//! 2. **hicolor**: the fallback theme where third-party apps install their
//!    own icons, scalable SVG first, then the largest PNG. derisk recolors
//!    these to the text color, as it does tray icons ([`crate::tray`]).
//! 3. **A Nerd Font glyph** ([`glyph`]), drawn as text.
//!
//! An `Icon=` value that is an absolute path is used as is (tier 2).
//!
//! ```
//! use derisk::icons::{Source, lookup};
//!
//! // Nothing is installed under an empty search path: the glyph remains.
//! assert!(matches!(lookup("system-file-manager", &[]), Source::Glyph(_)));
//! ```

use std::path::{Path, PathBuf};

use mcsapi_ui::fonts::icon;
use resvg::{tiny_skia, usvg};

use crate::tray::{Pixmap, monochrome};

/// Where an icon comes from.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Source {
    /// A Papirus symbolic SVG: draw its alpha in the text color.
    Papirus(PathBuf),
    /// A hicolor (or absolute-path) SVG or PNG: recolor it to the text color.
    Hicolor(PathBuf),
    /// A Nerd Font glyph to draw as text.
    Glyph(&'static str),
}

/// `icons` directories in XDG precedence order, plus the legacy `~/.icons`.
pub fn icon_dirs() -> Vec<PathBuf> {
    let home = std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".icons"));
    home.into_iter()
        .chain(
            crate::desktop::data_dirs()
                .into_iter()
                .map(|d| d.join("icons")),
        )
        .collect()
}

/// Papirus contexts, most specific to the shell first.
const CONTEXTS: [&str; 9] = [
    "apps",
    "places",
    "devices",
    "status",
    "actions",
    "categories",
    "mimetypes",
    "panel",
    "emblems",
];

/// hicolor PNG sizes, largest first, so downscaling stays crisp.
const HICOLOR_SIZES: [&str; 9] = [
    "256x256", "128x128", "96x96", "64x64", "48x48", "32x32", "24x24", "22x22", "16x16",
];

/// Resolves `name` (a theme icon name or an absolute path) under `dirs`.
pub fn lookup(name: &str, dirs: &[PathBuf]) -> Source {
    let path = Path::new(name);
    if path.is_absolute() {
        return if path.is_file() {
            Source::Hicolor(path.to_owned())
        } else {
            Source::Glyph(glyph(name))
        };
    }
    if name.is_empty() || name.contains('/') {
        return Source::Glyph(glyph(name));
    }
    let base = name.strip_suffix("-symbolic").unwrap_or(name);
    for dir in dirs {
        for theme in ["Papirus", "Papirus-Dark"] {
            let theme = dir.join(theme);
            for context in CONTEXTS {
                for candidate in [
                    theme
                        .join("symbolic")
                        .join(context)
                        .join(format!("{base}-symbolic.svg")),
                    theme
                        .join("16x16")
                        .join(context)
                        .join(format!("{base}.svg")),
                ] {
                    if candidate.is_file() {
                        return Source::Papirus(candidate);
                    }
                }
            }
        }
    }
    for dir in dirs {
        let hicolor = dir.join("hicolor");
        let scalable = hicolor.join("scalable/apps").join(format!("{name}.svg"));
        if scalable.is_file() {
            return Source::Hicolor(scalable);
        }
        for size in HICOLOR_SIZES {
            let png = hicolor.join(size).join("apps").join(format!("{name}.png"));
            if png.is_file() {
                return Source::Hicolor(png);
            }
        }
    }
    Source::Glyph(glyph(name))
}

/// The Nerd Font glyph standing in for an icon name: the last fallback.
pub fn glyph(name: &str) -> &'static str {
    let name = name.rsplit('/').next().unwrap_or(name);
    let has = |words: &[&str]| words.iter().any(|w| name.contains(w));
    if has(&["folder", "file-manager", "user-home", "directory"]) {
        icon::FOLDER
    } else if has(&["text", "document", "editor", "file"]) {
        icon::FILE
    } else if has(&["settings", "preferences", "configur"]) {
        icon::SETTINGS
    } else if has(&["terminal", "console"]) {
        icon::COMMAND
    } else if has(&["search", "find"]) {
        icon::SEARCH
    } else if has(&["lock"]) {
        icon::LOCK
    } else if has(&["power", "shutdown", "log-out"]) {
        icon::POWER
    } else if has(&["error", "warning", "dialog-"]) {
        icon::WARNING
    } else if has(&["magic", "assistant"]) {
        icon::ASSISTANT
    } else {
        icon::APP
    }
}

/// Rasterizes an SVG or PNG icon file to a `size`×`size` (at most) RGBA
/// pixmap, recolored to `color`: Papirus by its alpha, hicolor through
/// [`monochrome`].
pub fn rasterize(source: &Source, size: u32, color: [u8; 3]) -> Option<Pixmap> {
    let (path, symbolic) = match source {
        Source::Papirus(path) => (path, true),
        Source::Hicolor(path) => (path, false),
        Source::Glyph(_) => return None,
    };
    let data = std::fs::read(path).ok()?;
    let pixmap = if path
        .extension()
        .is_some_and(|e| e.eq_ignore_ascii_case("png"))
    {
        let png = tiny_skia::Pixmap::decode_png(&data).ok()?;
        scale(&png, size)?
    } else {
        let tree = usvg::Tree::from_data(&data, &usvg::Options::default()).ok()?;
        let tree_size = tree.size();
        let k = size as f32 / tree_size.width().max(tree_size.height());
        let mut out = tiny_skia::Pixmap::new(
            (tree_size.width() * k).ceil().max(1.0) as u32,
            (tree_size.height() * k).ceil().max(1.0) as u32,
        )?;
        resvg::render(
            &tree,
            tiny_skia::Transform::from_scale(k, k),
            &mut out.as_mut(),
        );
        out
    };
    let rgba = unpremultiply(&pixmap);
    let pixmap = Pixmap::from_rgba(pixmap.width(), pixmap.height(), rgba)?;
    Some(if symbolic {
        tint(&pixmap, color)
    } else {
        monochrome(&pixmap, color)
    })
}

/// Downscales a decoded PNG to fit `size`, keeping its aspect ratio.
fn scale(png: &tiny_skia::Pixmap, size: u32) -> Option<tiny_skia::Pixmap> {
    let k = (size as f32 / png.width().max(png.height()) as f32).min(1.0);
    let mut out = tiny_skia::Pixmap::new(
        ((png.width() as f32 * k).round() as u32).max(1),
        ((png.height() as f32 * k).round() as u32).max(1),
    )?;
    out.draw_pixmap(
        0,
        0,
        png.as_ref(),
        &tiny_skia::PixmapPaint {
            quality: tiny_skia::FilterQuality::Bicubic,
            ..Default::default()
        },
        tiny_skia::Transform::from_scale(k, k),
        None,
    );
    Some(out)
}

/// Straight-alpha RGBA bytes from tiny-skia's premultiplied pixels.
fn unpremultiply(pixmap: &tiny_skia::Pixmap) -> Vec<u8> {
    pixmap
        .pixels()
        .iter()
        .flat_map(|p| {
            let c = p.demultiply();
            [c.red(), c.green(), c.blue(), c.alpha()]
        })
        .collect()
}

/// Fills every pixel with `color`, keeping its alpha: symbolic icons are
/// single-color shapes already.
fn tint(icon: &Pixmap, [r, g, b]: [u8; 3]) -> Pixmap {
    let rgba = icon
        .rgba
        .as_chunks::<4>()
        .0
        .iter()
        .flat_map(|p| [r, g, b, p[3]])
        .collect();
    Pixmap {
        width: icon.width,
        height: icon.height,
        rgba,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SQUARE: &str = r##"<svg xmlns="http://www.w3.org/2000/svg" width="16" height="16"><rect x="4" y="4" width="8" height="8" fill="#e91e63"/></svg>"##;

    fn theme_dir(files: &[&str]) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "derisk-icons-{}-{}",
            std::process::id(),
            files.join("+").replace('/', "_")
        ));
        let _ = std::fs::remove_dir_all(&dir);
        for file in files {
            let path = dir.join(file);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, SQUARE).unwrap();
        }
        dir
    }

    #[test]
    fn papirus_comes_before_hicolor() {
        let dir = theme_dir(&[
            "Papirus/symbolic/apps/org.example.app-symbolic.svg",
            "hicolor/scalable/apps/org.example.app.svg",
        ]);
        assert_eq!(
            lookup("org.example.app", std::slice::from_ref(&dir)),
            Source::Papirus(dir.join("Papirus/symbolic/apps/org.example.app-symbolic.svg"))
        );
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn hicolor_then_glyph() {
        let dir = theme_dir(&["hicolor/scalable/apps/org.example.app.svg"]);
        let dirs = std::slice::from_ref(&dir);
        assert_eq!(
            lookup("org.example.app", dirs),
            Source::Hicolor(dir.join("hicolor/scalable/apps/org.example.app.svg"))
        );
        assert_eq!(
            lookup("system-file-manager", dirs),
            Source::Glyph(icon::FOLDER)
        );
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn symbolic_icons_take_the_text_color() {
        let dir = theme_dir(&["Papirus/16x16/places/folder.svg"]);
        let source = lookup("folder", std::slice::from_ref(&dir));
        let pixmap = rasterize(&source, 32, [248, 250, 252]).expect("renders");
        assert_eq!((pixmap.width, pixmap.height), (32, 32));
        let inside = &pixmap.rgba[(16 * 32 + 16) * 4..][..4];
        assert_eq!(inside, [248, 250, 252, 255]);
        let outside = &pixmap.rgba[..4];
        assert_eq!(outside[3], 0);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn glyphs_cover_common_names() {
        assert_eq!(glyph("user-home"), icon::FOLDER);
        assert_eq!(glyph("accessories-text-editor"), icon::FILE);
        assert_eq!(glyph("preferences-system"), icon::SETTINGS);
        assert_eq!(glyph("utilities-terminal"), icon::COMMAND);
        assert_eq!(glyph("org.mozilla.firefox"), icon::APP);
    }
}
