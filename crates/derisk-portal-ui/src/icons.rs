//! Icons painted in the text color, as the shell paints them.
//!
//! Places and file types use Papirus symbolic icons; apps use their own icon
//! from the icon theme. Both are decoded as a mask and tinted, so they follow
//! the theme. Where nothing is installed, a glyph from egui's built-in fonts
//! stands in, like the core apps' catalog icons.

use egui::{Align2, Color32, Rect, TextureHandle, TextureOptions, Ui, pos2};
use mcsapi_ui::egui;

/// An icon to paint.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Icon<'a> {
    /// A Papirus symbolic icon by name, with a glyph to fall back on.
    Symbolic(&'a str, &'a str),
    /// An app's `Icon` key (a theme name or an absolute path), with a glyph
    /// to fall back on.
    App(&'a str, &'a str),
}

/// The user's home folder.
pub const HOME: Icon<'static> = Icon::Symbolic("user-home", "🏠");
/// A folder.
pub const FOLDER: Icon<'static> = Icon::Symbolic("folder", "🗀");
/// The Documents folder, also used for document files.
pub const DOCUMENTS: Icon<'static> = Icon::Symbolic("folder-documents", "🗐");
/// The Downloads folder.
pub const DOWNLOADS: Icon<'static> = Icon::Symbolic("folder-download", "⮋");
/// The Pictures folder, also used for image files.
pub const PICTURES: Icon<'static> = Icon::Symbolic("folder-pictures", "🖼");
/// The Videos folder, also used for video files.
pub const VIDEOS: Icon<'static> = Icon::Symbolic("folder-videos", "🎞");
/// The trash.
pub const TRASH: Icon<'static> = Icon::Symbolic("user-trash", "🗑");
/// A plain text file.
pub const TEXT: Icon<'static> = Icon::Symbolic("accessories-text-editor", "🗋");
/// A display.
pub const COMPUTER: Icon<'static> = Icon::Symbolic("computer", "🖵");
/// Back.
pub const BACK: Icon<'static> = Icon::Symbolic("go-history-previous", "⏴");
/// A generic app.
pub const APP: Icon<'static> = Icon::Symbolic("view-app-grid", "⊞");

fn load(icon: Icon<'_>, px: u32) -> Option<egui::ColorImage> {
    let path = match icon {
        Icon::Symbolic(name, _) => derisk::icons::find_action(name),
        Icon::App(name, _) => derisk::icons::find(name, px),
    }?;
    derisk::icons::load_mask(&path, px)
}

fn texture(ui: &Ui, icon: Icon<'_>, px: u32) -> Option<TextureHandle> {
    let (kind, name) = match icon {
        Icon::Symbolic(name, _) => ("symbolic", name),
        Icon::App(name, _) => ("app", name),
    };
    let id = egui::Id::new(("derisk-portal-icon", kind, name, px));
    if let Some(cached) = ui.ctx().data(|d| d.get_temp::<Option<TextureHandle>>(id)) {
        return cached;
    }
    let handle = load(icon, px).map(|image| {
        ui.ctx().load_texture(
            format!("icon-{kind}-{name}-{px}"),
            image,
            TextureOptions::LINEAR,
        )
    });
    ui.ctx().data_mut(|d| d.insert_temp(id, handle.clone()));
    handle
}

/// Paints `icon` in `rect`, tinted `color`.
pub fn paint(ui: &Ui, rect: Rect, icon: Icon<'_>, color: Color32) {
    let px = (rect.height() * ui.ctx().pixels_per_point())
        .round()
        .max(1.0) as u32;
    if let Some(texture) = texture(ui, icon, px) {
        let size = texture.size_vec2() / ui.ctx().pixels_per_point();
        let fit = Align2::CENTER_CENTER.align_size_within_rect(size, rect);
        ui.painter().image(
            texture.id(),
            fit,
            Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)),
            color,
        );
    } else {
        let glyph = match icon {
            Icon::Symbolic(_, glyph) | Icon::App(_, glyph) => glyph,
        };
        ui.painter().text(
            rect.center(),
            Align2::CENTER_CENTER,
            glyph,
            egui::FontId::proportional(rect.height()),
            color,
        );
    }
}

/// The icon for a file named `name`: a folder, or a guess from its
/// extension.
pub fn for_file(name: &str, is_dir: bool) -> Icon<'static> {
    if is_dir {
        return FOLDER;
    }
    let ext = name.rsplit_once('.').map(|(_, e)| e.to_ascii_lowercase());
    match ext.as_deref() {
        Some("png" | "jpg" | "jpeg" | "gif" | "webp" | "svg" | "bmp" | "avif" | "heic") => PICTURES,
        Some("mp4" | "mkv" | "webm" | "mov" | "avi") => VIDEOS,
        Some("txt" | "md" | "rs" | "toml" | "json" | "conf" | "ini" | "log" | "odt" | "sh") => TEXT,
        _ => DOCUMENTS,
    }
}
