//! The overview screen: workspace strip, window grid and widgets.

use std::path::Path;

use mcsapi::Geometry;
use serde::{Deserialize, Serialize};

use crate::{
    adaptive::FormFactor,
    geom::{centered, inset, rect},
};

/// A widget shown on the overview.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Widget {
    /// Large clock and date.
    Clock,
    /// Month calendar.
    Calendar,
    /// Battery level, if the device has one.
    Battery,
    /// Apps the user usually opens around now.
    Suggestions,
    /// A scratch pad.
    Notes,
    /// Natural-language prompt for the built-in assistant.
    Assistant,
    /// Failed systemd user units, with restart/reset buttons.
    Units,
}

impl Widget {
    /// The default widget board.
    pub const DEFAULT: [Self; 7] = [
        Self::Assistant,
        Self::Clock,
        Self::Suggestions,
        Self::Units,
        Self::Calendar,
        Self::Battery,
        Self::Notes,
    ];
}

/// Regions of the overview.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct OverviewLayout {
    /// Workspace thumbnails.
    pub workspaces: Geometry,
    /// Window grid.
    pub windows: Geometry,
    /// Widget column (or stack on phones).
    pub widgets: Geometry,
}

impl OverviewLayout {
    /// Splits `area` for the form factor.
    pub fn new(area: Geometry, form_factor: FormFactor) -> Self {
        let area = inset(area, 16);
        let (x, y, w, h) = (area.loc.x, area.loc.y, area.size.w, area.size.h);
        match form_factor {
            FormFactor::Phone => {
                let widgets_h = h * 2 / 5;
                let strip = 56.min(h / 6);
                Self {
                    widgets: rect(x, y, w, widgets_h),
                    windows: rect(
                        x,
                        y + widgets_h + 12,
                        w,
                        (h - widgets_h - strip - 24).max(1),
                    ),
                    workspaces: rect(x, y + h - strip, w, strip),
                }
            }
            FormFactor::Tablet | FormFactor::Desktop => {
                let widgets_w = 320.min(w / 3);
                let strip = 110.min(h / 6);
                let main_w = w - widgets_w - 16;
                Self {
                    workspaces: rect(x, y, main_w, strip),
                    windows: rect(x, y + strip + 16, main_w, (h - strip - 16).max(1)),
                    widgets: rect(x + main_w + 16, y, widgets_w, h),
                }
            }
        }
    }
}

/// Grid cells for `count` windows inside `area`, filled row by row.
///
/// Columns are chosen so cells stay close to the area's aspect ratio.
pub fn grid(count: usize, area: Geometry, gap: i32) -> Vec<Geometry> {
    if count == 0 {
        return Vec::new();
    }
    let aspect = f64::from(area.size.w) / f64::from(area.size.h.max(1));
    let cols = ((count as f64 * aspect).sqrt().ceil() as usize).clamp(1, count);
    let rows = count.div_ceil(cols);
    let cw = (area.size.w - gap * (cols as i32 - 1)) / cols as i32;
    let ch = (area.size.h - gap * (rows as i32 - 1)) / rows as i32;
    (0..count)
        .map(|i| {
            let (r, c) = ((i / cols) as i32, (i % cols) as i32);
            // Center a short last row.
            let in_row = if r as usize == rows - 1 {
                count - (rows - 1) * cols
            } else {
                cols
            } as i32;
            let offset = (cols as i32 - in_row) * (cw + gap) / 2;
            rect(
                area.loc.x + offset + c * (cw + gap),
                area.loc.y + r * (ch + gap),
                cw.max(1),
                ch.max(1),
            )
        })
        .collect()
}

/// Scales a window frame to fit a cell, preserving its aspect ratio.
pub fn fit(window: Geometry, cell: Geometry) -> Geometry {
    let scale = (f64::from(cell.size.w) / f64::from(window.size.w.max(1)))
        .min(f64::from(cell.size.h) / f64::from(window.size.h.max(1)))
        .min(1.0);
    centered(
        cell,
        (f64::from(window.size.w) * scale) as i32,
        (f64::from(window.size.h) * scale) as i32,
    )
}

/// Battery state for the battery widget.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct Battery {
    /// Charge, 0–100.
    pub percent: u8,
    /// Whether it is charging.
    pub charging: bool,
}

impl Battery {
    /// Reads the first battery under a `/sys/class/power_supply`-like directory.
    pub fn read(power_supply: &Path) -> Option<Self> {
        let mut entries: Vec<_> = std::fs::read_dir(power_supply)
            .ok()?
            .filter_map(Result::ok)
            .map(|e| e.path())
            .collect();
        entries.sort();
        entries.into_iter().find_map(|dir| {
            let kind = std::fs::read_to_string(dir.join("type")).ok()?;
            if kind.trim() != "Battery" {
                return None;
            }
            let percent = std::fs::read_to_string(dir.join("capacity"))
                .ok()?
                .trim()
                .parse::<u8>()
                .ok()?
                .min(100);
            let status = std::fs::read_to_string(dir.join("status")).unwrap_or_default();
            Some(Self {
                percent,
                charging: matches!(status.trim(), "Charging" | "Full"),
            })
        })
    }
}
