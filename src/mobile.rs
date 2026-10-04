//! Touch navigation for phone-sized screens.
//!
//! A phone has no Super key and no pointer to hover with, and the top of a
//! tall screen is out of reach of the hand holding it. So below the phone
//! breakpoint (see [`crate::adaptive::Profile::detect`]) the shell adds a
//! navigation bar along the bottom edge: Back, Home and Apps buttons that
//! each fill a third of the width, and swipes that start on the bar.
//!
//! This module only does geometry and decides what a tap or swipe means, so
//! it can be tested without a display and drawn by any toolkit; [`crate::ui`]
//! draws it with egui.

use mcsapi::Geometry;

use crate::{
    action::Action,
    geom::{Point, contains, rect},
};

/// The smallest touch target, in logical pixels: about 9 mm on a phone,
/// what Android and GNOME's mobile guidelines ask for.
pub const TOUCH_TARGET: i32 = 48;

/// How far a finger must travel on the bar before it counts as a swipe
/// rather than a tap.
pub const SWIPE_DISTANCE: f32 = 48.0;

/// A button on the navigation bar.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum NavButton {
    /// Close the topmost overlay, or go back to the previous app.
    Back,
    /// The overview: widgets, open apps and workspaces.
    Home,
    /// The command palette, which lists every app and also searches.
    Apps,
}

impl NavButton {
    /// Left to right.
    pub const ORDER: [Self; 3] = [Self::Back, Self::Home, Self::Apps];

    /// The glyph drawn on the button.
    pub fn icon(self) -> &'static str {
        match self {
            Self::Back => "◀",
            Self::Home => "🏠",
            Self::Apps => "🔍",
        }
    }

    /// The accessible name.
    pub fn label(self) -> &'static str {
        match self {
            Self::Back => "Back",
            Self::Home => "Home",
            Self::Apps => "Apps and search",
        }
    }
}

/// What the shell is showing, as far as navigation cares.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct NavState {
    /// The overview is open.
    pub overview: bool,
    /// The command palette is open.
    pub palette: bool,
}

/// The navigation bar along the bottom of the output.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct NavBar {
    /// Height in logical pixels; 0 means no bar.
    pub height: i32,
}

impl NavBar {
    /// The bar's strip at the bottom of `output`.
    pub fn area(&self, output: Geometry) -> Geometry {
        let h = self.height.clamp(0, output.size.h);
        rect(
            output.loc.x,
            output.loc.y + output.size.h - h,
            output.size.w,
            h,
        )
    }

    /// Whether `(x, y)` is on the bar.
    pub fn contains(&self, output: Geometry, point: Point) -> bool {
        self.height > 0 && contains(self.area(output), point)
    }

    /// Each button's tap area: equal thirds of the bar, so every target is
    /// as wide as it can be and there are no dead gaps between them.
    pub fn buttons(&self, output: Geometry) -> [(NavButton, Geometry); 3] {
        let a = self.area(output);
        let third = a.size.w / 3;
        let mut i = 0;
        NavButton::ORDER.map(|b| {
            // The last third takes the remainder, so the bar is covered edge
            // to edge whatever the width.
            let w = if i == 2 { a.size.w - 2 * third } else { third };
            let area = rect(a.loc.x + i * third, a.loc.y, w, a.size.h);
            i += 1;
            (b, area)
        })
    }
}

/// What tapping `button` does in `state`.
pub fn tap(button: NavButton, state: NavState) -> Vec<Action> {
    let hide_palette = Action::Palette {
        visible: Some(false),
    };
    match button {
        // Back peels overlays off one at a time, like Escape does, and with
        // none open returns to the app used before this one.
        NavButton::Back if state.palette => vec![hide_palette],
        NavButton::Back if state.overview => vec![Action::Overview {
            visible: Some(false),
        }],
        NavButton::Back => vec![Action::FocusPrevious],
        // Home always lands on the overview; pressing it there goes back to
        // the app, so the same thumb toggles between the two.
        NavButton::Home if state.palette => vec![
            hide_palette,
            Action::Overview {
                visible: Some(true),
            },
        ],
        NavButton::Home => vec![Action::Overview { visible: None }],
        NavButton::Apps => vec![Action::Palette { visible: None }],
    }
}

/// A swipe that started on the navigation bar.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Swipe {
    /// Up, away from the edge: go home.
    Up,
    /// Along the bar to the left: the previous app.
    Left,
    /// Along the bar to the right: the next app.
    Right,
}

/// Classifies a drag of `(dx, dy)` logical pixels that started on the bar.
/// Short drags are taps, and a drag must be mostly in one direction, so a
/// sloppy thumb does not switch apps when it meant to go home.
pub fn swipe(dx: f32, dy: f32) -> Option<Swipe> {
    let (ax, ay) = (dx.abs(), dy.abs());
    if ax.max(ay) < SWIPE_DISTANCE {
        return None;
    }
    if dy < 0.0 && ay >= ax * 1.5 {
        Some(Swipe::Up)
    } else if ax >= ay * 1.5 {
        Some(if dx < 0.0 { Swipe::Left } else { Swipe::Right })
    } else {
        None
    }
}

/// What a swipe does in `state`.
pub fn swiped(swipe: Swipe, state: NavState) -> Vec<Action> {
    match swipe {
        Swipe::Up => {
            let mut actions = Vec::new();
            if state.palette {
                actions.push(Action::Palette {
                    visible: Some(false),
                });
            }
            actions.push(Action::Overview {
                visible: Some(true),
            });
            actions
        }
        Swipe::Left => vec![Action::FocusPrevious],
        Swipe::Right => vec![Action::FocusNext],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn phone() -> Geometry {
        rect(0, 0, 392, 872)
    }

    #[test]
    fn bar_sits_on_the_bottom_edge() {
        let bar = NavBar { height: 56 };
        assert_eq!(bar.area(phone()), rect(0, 816, 392, 56));
        assert!(bar.contains(phone(), (10, 871)));
        assert!(!bar.contains(phone(), (10, 815)));
        assert!(!NavBar { height: 0 }.contains(phone(), (10, 871)));
    }

    #[test]
    fn buttons_cover_the_bar_and_meet_the_touch_target() {
        let bar = NavBar { height: 56 };
        let buttons = bar.buttons(phone());
        assert_eq!(buttons.map(|(b, _)| b), NavButton::ORDER);
        let width: i32 = buttons.iter().map(|(_, g)| g.size.w).sum();
        assert_eq!(width, phone().size.w);
        for (_, g) in buttons {
            assert!(g.size.w >= TOUCH_TARGET && g.size.h >= TOUCH_TARGET);
        }
        assert_eq!(buttons[2].1.loc.x + buttons[2].1.size.w, phone().size.w);
    }

    #[test]
    fn back_peels_overlays_then_switches_apps() {
        let both = NavState {
            overview: true,
            palette: true,
        };
        assert_eq!(
            tap(NavButton::Back, both),
            vec![Action::Palette {
                visible: Some(false)
            }]
        );
        let overview = NavState {
            overview: true,
            palette: false,
        };
        assert_eq!(
            tap(NavButton::Back, overview),
            vec![Action::Overview {
                visible: Some(false)
            }]
        );
        assert_eq!(
            tap(NavButton::Back, NavState::default()),
            vec![Action::FocusPrevious]
        );
    }

    #[test]
    fn home_from_the_palette_lands_on_the_overview() {
        let palette = NavState {
            overview: false,
            palette: true,
        };
        assert_eq!(
            tap(NavButton::Home, palette),
            vec![
                Action::Palette {
                    visible: Some(false)
                },
                Action::Overview {
                    visible: Some(true)
                }
            ]
        );
        assert_eq!(
            tap(NavButton::Home, NavState::default()),
            vec![Action::Overview { visible: None }]
        );
    }

    #[test]
    fn swipes_need_distance_and_a_clear_direction() {
        assert_eq!(swipe(5.0, -20.0), None);
        assert_eq!(swipe(10.0, -120.0), Some(Swipe::Up));
        assert_eq!(swipe(-90.0, -10.0), Some(Swipe::Left));
        assert_eq!(swipe(90.0, 5.0), Some(Swipe::Right));
        // Diagonal: neither.
        assert_eq!(swipe(80.0, -80.0), None);
        // Down, off the screen edge: nothing.
        assert_eq!(swipe(0.0, 120.0), None);
    }

    #[test]
    fn swipe_up_goes_home_from_anywhere() {
        let palette = NavState {
            overview: false,
            palette: true,
        };
        assert_eq!(
            swiped(Swipe::Up, palette),
            vec![
                Action::Palette {
                    visible: Some(false)
                },
                Action::Overview {
                    visible: Some(true)
                }
            ]
        );
        assert_eq!(
            swiped(Swipe::Right, NavState::default()),
            vec![Action::FocusNext]
        );
    }
}
