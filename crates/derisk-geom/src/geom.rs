//! Small helpers around mcsapi's logical [`Geometry`].

use mcsapi::Geometry;
use serde::{Deserialize, Serialize};

/// A logical point, as delivered by the host's pointer handling.
pub type Point = (i32, i32);

/// Builds a logical rectangle.
pub fn rect(x: i32, y: i32, w: i32, h: i32) -> Geometry {
    Geometry::new((x, y).into(), (w, h).into())
}

/// Whether `point` lies inside `geometry` (right/bottom edges exclusive).
pub fn contains(geometry: Geometry, (x, y): Point) -> bool {
    x >= geometry.loc.x
        && y >= geometry.loc.y
        && x < geometry.loc.x + geometry.size.w
        && y < geometry.loc.y + geometry.size.h
}

/// Shrinks a rectangle by `by` on every side, never below one logical pixel.
pub fn inset(geometry: Geometry, by: i32) -> Geometry {
    let w = (geometry.size.w - 2 * by).max(1);
    let h = (geometry.size.h - 2 * by).max(1);
    let x = geometry.loc.x + (geometry.size.w - w) / 2;
    let y = geometry.loc.y + (geometry.size.h - h) / 2;
    rect(x, y, w, h)
}

/// Centers a `w`×`h` rectangle in `area`, clamping its size to the area.
pub fn centered(area: Geometry, w: i32, h: i32) -> Geometry {
    let (w, h) = (w.min(area.size.w).max(1), h.min(area.size.h).max(1));
    rect(
        area.loc.x + (area.size.w - w) / 2,
        area.loc.y + (area.size.h - h) / 2,
        w,
        h,
    )
}

/// A serializable rectangle used by IPC and snapshots.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub struct Rect {
    /// Left edge.
    pub x: i32,
    /// Top edge.
    pub y: i32,
    /// Width.
    pub w: i32,
    /// Height.
    pub h: i32,
}

impl From<Geometry> for Rect {
    fn from(g: Geometry) -> Self {
        Self {
            x: g.loc.x,
            y: g.loc.y,
            w: g.size.w,
            h: g.size.h,
        }
    }
}

impl From<Rect> for Geometry {
    fn from(r: Rect) -> Self {
        rect(r.x, r.y, r.w, r.h)
    }
}
