//! Server-side window decorations with the window buttons on the left.

use mcsapi::Geometry;
use serde::{Deserialize, Serialize};

use crate::geom::{Point, contains, rect};

/// A title bar button.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Button {
    /// Close the window.
    Close,
    /// Minimize the window.
    Minimize,
    /// Toggle maximization.
    Maximize,
}

impl Button {
    /// Left-to-right order of the buttons, starting at the left edge.
    pub const ORDER: [Self; 3] = [Self::Close, Self::Minimize, Self::Maximize];
}

/// What a point on a window frame hits.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Hit {
    /// One of the title bar buttons.
    Button(Button),
    /// The draggable title area.
    Title,
    /// The client surface below the title bar.
    Client,
}

/// Title bar metrics, in logical pixels.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TitleBar {
    /// Height of the bar.
    pub height: i32,
    /// Diameter of each round button.
    pub button: i32,
    /// Space between buttons.
    pub spacing: i32,
    /// Space between the left frame edge and the first button.
    pub padding: i32,
}

impl Default for TitleBar {
    fn default() -> Self {
        Self {
            height: 32,
            button: 14,
            spacing: 8,
            padding: 12,
        }
    }
}

impl TitleBar {
    /// The title bar strip of a window frame.
    pub fn bar(&self, frame: Geometry) -> Geometry {
        rect(
            frame.loc.x,
            frame.loc.y,
            frame.size.w,
            self.height.min(frame.size.h),
        )
    }

    /// The area left for the client surface.
    pub fn client(&self, frame: Geometry) -> Geometry {
        let h = self.height.min(frame.size.h - 1).max(0);
        rect(
            frame.loc.x,
            frame.loc.y + h,
            frame.size.w,
            (frame.size.h - h).max(1),
        )
    }

    /// Button rectangles, laid out from the left edge in [`Button::ORDER`].
    pub fn buttons(&self, frame: Geometry) -> impl Iterator<Item = (Button, Geometry)> + '_ {
        let y = frame.loc.y + (self.height - self.button) / 2;
        Button::ORDER.into_iter().enumerate().map(move |(i, b)| {
            let x = frame.loc.x + self.padding + i as i32 * (self.button + self.spacing);
            (b, rect(x, y, self.button, self.button))
        })
    }

    /// Area used for the centered title text, right of the buttons.
    pub fn title(&self, frame: Geometry) -> Geometry {
        let start = self.padding + 3 * self.button + 3 * self.spacing;
        rect(
            frame.loc.x + start,
            frame.loc.y,
            (frame.size.w - 2 * start).max(1),
            self.height.min(frame.size.h),
        )
    }

    /// Hit-tests a point against a frame. Buttons get a slightly larger target.
    pub fn hit(&self, frame: Geometry, point: Point) -> Option<Hit> {
        if !contains(frame, point) {
            return None;
        }
        if !contains(self.bar(frame), point) {
            return Some(Hit::Client);
        }
        let slop = self.spacing / 2;
        for (button, area) in self.buttons(frame) {
            let target = rect(
                area.loc.x - slop,
                frame.loc.y,
                area.size.w + 2 * slop,
                self.height,
            );
            if contains(target, point) {
                return Some(Hit::Button(button));
            }
        }
        Some(Hit::Title)
    }
}

/// Detects double clicks on title bars (toggle maximize, like Windows).
#[derive(Clone, Copy, Debug, Default)]
pub struct ClickTracker {
    last: Option<(u64, u64)>,
}

impl ClickTracker {
    /// Maximum delay between clicks, in milliseconds.
    pub const THRESHOLD_MS: u64 = 400;

    /// Records a click on `target`; returns true when it completes a double click.
    pub fn click(&mut self, target: u64, time_ms: u64) -> bool {
        let double = matches!(self.last, Some((t, at)) if t == target && time_ms.saturating_sub(at) <= Self::THRESHOLD_MS);
        self.last = if double {
            None
        } else {
            Some((target, time_ms))
        };
        double
    }
}
