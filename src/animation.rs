//! The startup animation timeline and easing curves.
//!
//! The timeline is a pure function of elapsed time, so any renderer (egui,
//! a GLES pass in the compositor, a test) can sample it.

/// Cubic ease-out.
pub fn ease_out_cubic(t: f32) -> f32 {
    1.0 - (1.0 - t.clamp(0.0, 1.0)).powi(3)
}

/// Cubic ease-in-out.
pub fn ease_in_out_cubic(t: f32) -> f32 {
    let t = t.clamp(0.0, 1.0);
    if t < 0.5 {
        4.0 * t * t * t
    } else {
        1.0 - (-2.0 * t + 2.0).powi(3) / 2.0
    }
}

/// Ease-out with a slight overshoot.
pub fn ease_out_back(t: f32) -> f32 {
    let t = t.clamp(0.0, 1.0);
    let (c1, c3) = (1.70158, 2.70158);
    1.0 + c3 * (t - 1.0).powi(3) + c1 * (t - 1.0).powi(2)
}

/// Progress of `now` through the window `[start, end)` in ms, clamped to 0–1.
fn phase(now: f32, start: f32, end: f32) -> f32 {
    ((now - start) / (end - start)).clamp(0.0, 1.0)
}

/// What to draw at one instant of the startup animation.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct StartupFrame {
    /// Opacity of the cover over the desktop (1 = fully hidden desktop).
    pub cover: f32,
    /// Opacity of the logo.
    pub logo_opacity: f32,
    /// Scale of the logo (1 = rest size).
    pub logo_scale: f32,
    /// Progress (0–1) of the ring sweeping around the logo.
    pub ring: f32,
    /// Opacity of the shell chrome (top bar, windows).
    pub shell_opacity: f32,
    /// Vertical offset, in logical pixels, of the top bar sliding in.
    pub bar_offset: f32,
    /// The animation has finished; the renderer can stop sampling.
    pub done: bool,
}

/// The startup animation: the logo pops in, a ring sweeps around it, then it
/// blooms away as the desktop fades up and the top bar slides into place.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct StartupAnimation {
    /// Honor the user's reduced-motion preference with a short cross-fade.
    pub reduced_motion: bool,
    /// Top bar height, used for the slide distance.
    pub bar_height: f32,
}

impl StartupAnimation {
    /// Total length in milliseconds.
    pub fn duration_ms(&self) -> u32 {
        if self.reduced_motion { 300 } else { 1800 }
    }

    /// Samples the animation `elapsed_ms` after start.
    pub fn frame(&self, elapsed_ms: u32) -> StartupFrame {
        let now = elapsed_ms as f32;
        if self.reduced_motion {
            let t = phase(now, 0.0, 300.0);
            return StartupFrame {
                cover: 1.0 - t,
                logo_opacity: 0.0,
                logo_scale: 1.0,
                ring: 1.0,
                shell_opacity: t,
                bar_offset: 0.0,
                done: elapsed_ms >= self.duration_ms(),
            };
        }
        let appear = phase(now, 0.0, 600.0);
        let sweep = phase(now, 250.0, 1150.0);
        let bloom = phase(now, 1100.0, 1500.0);
        let reveal = phase(now, 1150.0, 1800.0);
        let slide = phase(now, 1250.0, 1800.0);
        StartupFrame {
            cover: 1.0 - ease_in_out_cubic(reveal),
            logo_opacity: ease_out_cubic(appear) * (1.0 - bloom),
            logo_scale: 0.6 + 0.4 * ease_out_back(appear) + 0.5 * ease_in_out_cubic(bloom),
            ring: ease_in_out_cubic(sweep),
            shell_opacity: ease_out_cubic(reveal),
            bar_offset: -self.bar_height * (1.0 - ease_out_cubic(slide)),
            done: elapsed_ms >= self.duration_ms(),
        }
    }
}
