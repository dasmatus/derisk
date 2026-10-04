//! App icons from freedesktop icon themes, as egui images.
//!
//! [`find`] follows the [icon theme spec]'s layout without reading every
//! `index.theme`: apps install their own icons into `hicolor`, so that theme
//! is searched first, then every other installed theme (which is where
//! generic names such as `system-file-manager` live), then `pixmaps`.
//!
//! [icon theme spec]: https://specifications.freedesktop.org/icon-theme-spec/latest/

use std::{
    fs,
    path::{Path, PathBuf},
};

use mcsapi::toolkit::egui::ColorImage;

/// Icons bigger than this are not decoded, so a stray wallpaper-sized file
/// named like an icon cannot stall a frame.
const MAX_BYTES: u64 = 4 << 20;

/// `icons` and `pixmaps` roots in XDG precedence order.
fn roots() -> (Vec<PathBuf>, Vec<PathBuf>) {
    let home = std::env::var_os("HOME").map(PathBuf::from);
    let data_home = std::env::var_os("XDG_DATA_HOME")
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
        .or_else(|| home.as_ref().map(|h| h.join(".local/share")));
    let data_dirs = std::env::var("XDG_DATA_DIRS")
        .ok()
        .filter(|v| !v.is_empty())
        .unwrap_or_else(|| "/usr/local/share:/usr/share".to_owned());
    let data: Vec<PathBuf> = data_home
        .into_iter()
        .chain(
            data_dirs
                .split(':')
                .filter(|d| !d.is_empty())
                .map(PathBuf::from),
        )
        .collect();
    let icons = home
        .map(|h| h.join(".icons"))
        .into_iter()
        .chain(data.iter().map(|d| d.join("icons")))
        .collect();
    let pixmaps = data.iter().map(|d| d.join("pixmaps")).collect();
    (icons, pixmaps)
}

/// The nominal size of a theme directory such as `48x48`, `48x48@2` or
/// `48`, or `None` for `scalable` and anything else.
fn dir_size(name: &str) -> Option<u32> {
    let name = name.split('@').next().unwrap_or(name);
    name.split('x').next()?.parse().ok()
}

/// How well a file fits `want` pixels; lower is better. A PNG at least as
/// big scales down cleanly, an SVG renders at any size, and a smaller PNG
/// would be blurry, so it is the last resort.
fn fit(path: &Path, size: Option<u32>, want: u32) -> (u8, u32) {
    match (path.extension().and_then(|e| e.to_str()), size) {
        (Some("svg"), _) => (1, 0),
        (_, Some(n)) if n >= want => (0, n - want),
        (_, Some(n)) => (2, want - n),
        (_, None) => (3, 0),
    }
}

/// Every `<theme>/<a>/<b>/<name>.{png,svg}` under `theme`, which covers both
/// `48x48/apps` and `apps/48` layouts, scored by [`fit`].
fn in_theme(theme: &Path, name: &str, want: u32, best: &mut Option<((u8, u32), PathBuf)>) {
    let Ok(outer) = fs::read_dir(theme) else {
        return;
    };
    for a in outer.filter_map(Result::ok) {
        let Ok(inner) = fs::read_dir(a.path()) else {
            continue;
        };
        let a_size = dir_size(&a.file_name().to_string_lossy());
        for b in inner.filter_map(Result::ok) {
            let size = a_size.or_else(|| dir_size(&b.file_name().to_string_lossy()));
            for ext in ["png", "svg"] {
                let path = b.path().join(format!("{name}.{ext}"));
                if !path.is_file() {
                    continue;
                }
                let score = fit(&path, size, want);
                if best.as_ref().is_none_or(|(s, _)| score < *s) {
                    *best = Some((score, path));
                }
            }
        }
    }
}

/// The file for icon `name` (a theme name or an absolute path) closest to
/// `want` pixels square, from the XDG icon directories.
pub fn find(name: &str, want: u32) -> Option<PathBuf> {
    let (icons, pixmaps) = roots();
    find_in(name, want, &icons, &pixmaps)
}

/// [`find`] in the given `icons` and `pixmaps` roots, in precedence order.
pub fn find_in(name: &str, want: u32, icons: &[PathBuf], pixmaps: &[PathBuf]) -> Option<PathBuf> {
    if name.starts_with('/') {
        return Some(PathBuf::from(name)).filter(|p| p.is_file());
    }
    // A name with a path separator or a leading dot would escape the theme.
    if name.is_empty() || name.contains('/') || name.starts_with('.') {
        return None;
    }
    let mut best = None;
    for root in icons {
        in_theme(&root.join("hicolor"), name, want, &mut best);
    }
    if best.is_some() {
        return best.map(|(_, p)| p);
    }
    for root in icons {
        let Ok(themes) = fs::read_dir(root) else {
            continue;
        };
        let mut themes: Vec<_> = themes
            .filter_map(Result::ok)
            .map(|t| t.path())
            .filter(|t| !t.ends_with("hicolor"))
            .collect();
        themes.sort();
        for theme in themes {
            in_theme(&theme, name, want, &mut best);
        }
        if best.is_some() {
            return best.map(|(_, p)| p);
        }
    }
    pixmaps.iter().find_map(|dir| {
        ["png", "svg"]
            .iter()
            .map(|ext| dir.join(format!("{name}.{ext}")))
            .find(|p| p.is_file())
    })
}

/// Decodes a PNG or SVG icon to at most `size` pixels square.
pub fn load(path: &Path, size: u32) -> Option<ColorImage> {
    if fs::metadata(path).ok()?.len() > MAX_BYTES {
        return None;
    }
    let bytes = fs::read(path).ok()?;
    if path.extension().is_some_and(|e| e == "svg") {
        let tree = resvg::usvg::Tree::from_data(&bytes, &resvg::usvg::Options::default()).ok()?;
        let s = tree.size();
        let scale = size as f32 / s.width().max(s.height());
        let w = ((s.width() * scale).round() as u32).max(1);
        let h = ((s.height() * scale).round() as u32).max(1);
        let mut pixmap = resvg::tiny_skia::Pixmap::new(w, h)?;
        resvg::render(
            &tree,
            resvg::tiny_skia::Transform::from_scale(scale, scale),
            &mut pixmap.as_mut(),
        );
        Some(ColorImage::from_rgba_premultiplied(
            [w as usize, h as usize],
            pixmap.data(),
        ))
    } else {
        let mut image = image::load_from_memory(&bytes).ok()?.into_rgba8();
        let (w, h) = (image.width(), image.height());
        if w > size || h > size {
            let scale = size as f32 / w.max(h) as f32;
            image = image::imageops::thumbnail(
                &image,
                ((w as f32 * scale).round() as u32).max(1),
                ((h as f32 * scale).round() as u32).max(1),
            );
        }
        Some(ColorImage::from_rgba_unmultiplied(
            [image.width() as usize, image.height() as usize],
            image.as_raw(),
        ))
    }
}

/// Papirus variants, dark first because the shell's default theme is dark.
/// Each is tinted at paint time, so the variant only matters for shapes.
const PAPIRUS: [&str; 3] = ["Papirus-Dark", "Papirus", "Papirus-Light"];

/// The Papirus file for action icon `name`: the symbolic variant
/// (`16x16/symbolic/actions/window-close-symbolic.svg`), else the plain 16 px
/// one, from any context (actions, places, status, ...).
pub fn find_action(name: &str) -> Option<PathBuf> {
    let (icons, _) = roots();
    find_action_in(name, &icons)
}

/// [`find_action`] in the given `icons` roots.
pub fn find_action_in(name: &str, icons: &[PathBuf]) -> Option<PathBuf> {
    if name.is_empty() || name.contains('/') || name.starts_with('.') {
        return None;
    }
    let in_contexts = |dir: PathBuf, file: &str| {
        let mut contexts: Vec<_> = fs::read_dir(dir).ok()?.filter_map(Result::ok).collect();
        contexts.sort_by_key(|c| c.file_name());
        contexts
            .iter()
            .map(|c| c.path().join(file))
            .find(|p| p.is_file())
    };
    for theme in PAPIRUS {
        for root in icons {
            let base = root.join(theme).join("16x16");
            if let Some(found) = in_contexts(base.join("symbolic"), &format!("{name}-symbolic.svg"))
                .or_else(|| in_contexts(base.clone(), &format!("{name}.svg")))
            {
                return Some(found);
            }
        }
    }
    None
}

/// Decodes an icon as a white mask: only its shape survives, so painting it
/// with a tint gives a greyscale icon in the theme's own text color whatever
/// colors the file uses.
pub fn load_mask(path: &Path, size: u32) -> Option<ColorImage> {
    let mut image = load(path, size)?;
    for pixel in &mut image.pixels {
        *pixel = mcsapi::toolkit::egui::Color32::from_white_alpha(pixel.a());
    }
    Some(image)
}

/// The Papirus action icon standing in for a glyph the shell has always
/// drawn, or `None` for glyphs that are not actions (app emoji, ✨).
pub fn action_for_glyph(glyph: &str) -> Option<&'static str> {
    Some(match glyph {
        "◆" | "⊞" => "view-app-grid",
        "🔍" => "system-search",
        "🔄" => "go-next",
        "🔃" => "go-previous",
        "⊟" => "view-dual",
        "▣" => "view-fullscreen",
        "🖥" => "video-display",
        "⎆" => "go-jump",
        "🔒" => "system-lock-screen",
        "🌙" => "system-suspend",
        "❄" => "system-hibernate",
        "🚪" => "system-log-out",
        "⟳" => "system-reboot",
        "✖" => "system-shutdown",
        "★" => "starred",
        "⚠" => "dialog-warning",
        "🗗" => "view-restore",
        "🗖" => "window-maximize",
        "☰" => "open-menu",
        "🗀" => "folder",
        "🗋" => "text-x-generic",
        _ => return None,
    })
}
