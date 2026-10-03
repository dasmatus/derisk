//! System tray items with icons recolored to a single theme color.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::menu::MenuEntry;

/// An RGBA8 (non-premultiplied) image.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct Pixmap {
    /// Width in pixels.
    pub width: u32,
    /// Height in pixels.
    pub height: u32,
    /// `width * height * 4` bytes, row-major RGBA.
    pub rgba: Vec<u8>,
}

impl Pixmap {
    /// Wraps RGBA bytes, rejecting a length that does not match the size.
    pub fn from_rgba(width: u32, height: u32, rgba: Vec<u8>) -> Option<Self> {
        (rgba.len() as u64 == u64::from(width) * u64::from(height) * 4).then_some(Self {
            width,
            height,
            rgba,
        })
    }

    /// Converts StatusNotifierItem `IconPixmap` data (ARGB32, network byte order).
    pub fn from_argb32_be(width: u32, height: u32, argb: &[u8]) -> Option<Self> {
        if argb.len() as u64 != u64::from(width) * u64::from(height) * 4 {
            return None;
        }
        let rgba = argb
            .chunks_exact(4)
            .flat_map(|p| [p[1], p[2], p[3], p[0]])
            .collect();
        Some(Self {
            width,
            height,
            rgba,
        })
    }

    fn pixels(&self) -> impl Iterator<Item = &[u8]> {
        self.rgba.chunks_exact(4)
    }
}

fn luminance(p: &[u8]) -> u32 {
    (2126 * u32::from(p[0]) + 7152 * u32::from(p[1]) + 722 * u32::from(p[2])) / 10_000
}

/// Recolors an icon to `color`, keeping only its shape.
///
/// Icons drawn on transparency keep their alpha as the mask. Icons that fill
/// most of their canvas (e.g. a glyph on an opaque badge) instead use each
/// pixel's luminance distance from the dominant background as the mask, so
/// the glyph survives and the badge disappears.
pub fn monochrome(icon: &Pixmap, color: [u8; 3]) -> Pixmap {
    let total = (icon.rgba.len() / 4).max(1);
    let opaque = icon.pixels().filter(|p| p[3] >= 224).count();
    let filled = opaque * 100 / total >= 85;

    let mask: Vec<u8> = if filled {
        // Background luminance = median of opaque pixels' luminance.
        let mut lum: Vec<u32> = icon
            .pixels()
            .filter(|p| p[3] >= 224)
            .map(luminance)
            .collect();
        lum.sort_unstable();
        let background = lum[lum.len() / 2];
        let distance: Vec<u32> = icon
            .pixels()
            .map(|p| luminance(p).abs_diff(background) * u32::from(p[3]) / 255)
            .collect();
        let max = distance.iter().copied().max().unwrap_or(0).max(1);
        distance.iter().map(|d| (d * 255 / max) as u8).collect()
    } else {
        icon.pixels().map(|p| p[3]).collect()
    };

    Pixmap {
        width: icon.width,
        height: icon.height,
        rgba: mask
            .into_iter()
            .flat_map(|a| [color[0], color[1], color[2], a])
            .collect(),
    }
}

/// A StatusNotifierItem-like tray entry.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct TrayItem {
    /// Unique ID (e.g. the SNI service name).
    pub id: String,
    /// Tooltip text.
    pub title: String,
    /// Monochrome icon, already recolored for the theme.
    pub icon: Pixmap,
    /// Context menu.
    #[serde(default)]
    pub menu: Vec<MenuEntry>,
}

/// All tray items, ordered by ID.
#[derive(Clone, Debug, Default)]
pub struct Tray {
    items: BTreeMap<String, TrayItem>,
    generation: u64,
}

impl Tray {
    /// Adds or replaces an item, recoloring its icon to `color`.
    pub fn insert(
        &mut self,
        id: impl Into<String>,
        title: impl Into<String>,
        icon: &Pixmap,
        menu: Vec<MenuEntry>,
        color: [u8; 3],
    ) {
        let id = id.into();
        self.items.insert(
            id.clone(),
            TrayItem {
                id,
                title: title.into(),
                icon: monochrome(icon, color),
                menu,
            },
        );
        self.generation += 1;
    }

    /// Removes an item.
    pub fn remove(&mut self, id: &str) -> Option<TrayItem> {
        self.generation += 1;
        self.items.remove(id)
    }

    /// Items in ID order.
    pub fn items(&self) -> impl ExactSizeIterator<Item = &TrayItem> {
        self.items.values()
    }

    /// Increments whenever items change, so renderers can refresh textures.
    pub fn generation(&self) -> u64 {
        self.generation
    }
}
