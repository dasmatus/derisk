//! What a derisk overview widget plugin is built with.
//!
//! The bindings for `crates/derisk-plugin/wit/widget/widget.wit`, generated
//! once here so every plugin shares them, and builders for the cards a
//! plugin returns. A plugin implements [`Guest`] and exports it with
//! [`export!`]:
//!
//! ```ignore
//! use derisk_widget_sdk::{Card, Guest, Manifest, Text, View, export};
//!
//! struct Hello;
//!
//! impl Guest for Hello {
//!     fn describe() -> Manifest { Manifest::new("hello", "Says hello") }
//!     fn render(_: View) -> Vec<Card> {
//!         vec![Card::new("hello").text(Text::plain("Hello"))]
//!     }
//! }
//!
//! export!(Hello with_types_in derisk_widget_sdk::bindings);
//! ```
//!
//! A card says what it holds, never how it looks: text in a [`Tone`] and an
//! [`Emphasis`], progress bars, rows of buttons, grids, a text field.
//! derisk draws them with its theme.
//!
//! The plugins compile to `wasm32-unknown-unknown` and `derisk-plugin`'s
//! build script wraps them in components. They also build for the host, so
//! `cargo test` runs their logic natively; there the imports are stubs that
//! must not be called, which is why [`log`] is only reached from code the
//! tests do not run.

pub use serde_json::{self, Value, json};

#[allow(missing_docs, clippy::all, clippy::pedantic)]
pub mod bindings {
    wit_bindgen::generate!({
        path: "../../../crates/derisk-plugin/wit/widget",
        world: "widget-plugin",
        pub_export_macro: true,
        export_macro_name: "export",
        default_bindings_module: "derisk_widget_sdk::bindings",
    });
}

pub use bindings::{
    Guest,
    derisk::widget::{
        host::Level,
        types::{
            AppButton, Battery, Button, Card, Clock, Emphasis, Field, Grid, Inline, Input,
            Manifest, Part, Progress, Registration, Text, Tone, View,
        },
    },
    export,
};

const WEEKDAYS: [&str; 7] = ["Mon", "Tue", "Wed", "Thu", "Fri", "Sat", "Sun"];
const MONTHS: [&str; 12] = [
    "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
];

/// Writes a line to derisk's log.
pub fn log(level: Level, message: &str) {
    bindings::derisk::widget::host::log(level, message);
}

/// `actions` as the JSON text a button carries.
pub fn actions(actions: &[Value]) -> Vec<String> {
    actions.iter().map(Value::to_string).collect()
}

impl Default for Clock {
    /// Noon on Saturday 1 January 2000, what derisk shows a plugin that may
    /// not see the clock.
    fn default() -> Self {
        Self {
            year: 2000,
            month: 1,
            day: 1,
            hour: 12,
            minute: 0,
            weekday: 5,
            twenty_four_hour: true,
        }
    }
}

impl Clock {
    /// `HH:MM` on a 24-hour clock, e.g. `3:07 PM` on a 12-hour one.
    pub fn time_label(&self) -> String {
        if self.twenty_four_hour {
            return format!("{:02}:{:02}", self.hour, self.minute);
        }
        let hour = match self.hour % 12 {
            0 => 12,
            h => h,
        };
        let half = if self.hour < 12 { "AM" } else { "PM" };
        format!("{hour}:{:02} {half}", self.minute)
    }

    /// e.g. `Sat 3 Oct`.
    pub fn date_label(&self) -> String {
        format!(
            "{} {} {}",
            WEEKDAYS[usize::from(self.weekday % 7)],
            self.day,
            MONTHS[usize::from(self.month.clamp(1, 12) - 1)]
        )
    }

    /// Number of days in this clock's month.
    pub fn days_in_month(&self) -> u8 {
        match self.month {
            2 if (self.year % 4 == 0 && self.year % 100 != 0) || self.year % 400 == 0 => 29,
            2 => 28,
            4 | 6 | 9 | 11 => 30,
            _ => 31,
        }
    }

    /// Weekday (0 = Monday) of the first day of this clock's month.
    pub fn first_weekday(&self) -> u8 {
        let first = (i32::from(self.weekday) - (i32::from(self.day) - 1)).rem_euclid(7);
        u8::try_from(first).unwrap_or(0)
    }
}

// The bindings generate `View`, so the derive this would be cannot be
// added to it.
#[allow(clippy::derivable_impls)]
impl Default for View {
    fn default() -> Self {
        Self {
            clock: Clock::default(),
            battery: None,
            suggested: Vec::new(),
            failed_units: Vec::new(),
            registered: Vec::new(),
        }
    }
}

impl Text {
    /// Body text in the theme's text color.
    pub fn plain(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            tone: Tone::Normal,
            emphasis: Emphasis::Normal,
        }
    }

    /// A card's heading.
    pub fn heading(text: impl Into<String>) -> Self {
        Self::plain(text).emphasis(Emphasis::Strong)
    }

    /// Sets its color.
    pub fn tone(mut self, tone: Tone) -> Self {
        self.tone = tone;
        self
    }

    /// Sets its prominence.
    pub fn emphasis(mut self, emphasis: Emphasis) -> Self {
        self.emphasis = emphasis;
        self
    }
}

impl Inline {
    /// A button running `actions`.
    pub fn button(label: impl Into<String>, actions: &[Value]) -> Self {
        Self::Button(Button {
            label: label.into(),
            actions: crate::actions(actions),
        })
    }

    /// App `app`'s icon and name, running `actions`.
    pub fn app(app: impl Into<String>, actions: &[Value]) -> Self {
        Self::App(AppButton {
            app: app.into(),
            actions: crate::actions(actions),
        })
    }
}

impl Card {
    /// An empty card named `key`.
    pub fn new(key: impl Into<String>) -> Self {
        Self {
            key: key.into(),
            parts: Vec::new(),
        }
    }

    /// Adds `part` below the others.
    pub fn part(mut self, part: Part) -> Self {
        self.parts.push(part);
        self
    }

    /// Adds a paragraph.
    pub fn text(self, text: Text) -> Self {
        self.part(Part::Text(text))
    }

    /// Adds a row of inline parts.
    pub fn row(self, items: impl IntoIterator<Item = Inline>) -> Self {
        self.part(Part::Row(items.into_iter().collect()))
    }

    /// Adds inline parts that wrap.
    pub fn flow(self, items: impl IntoIterator<Item = Inline>) -> Self {
        self.part(Part::Flow(items.into_iter().collect()))
    }

    /// Adds a progress bar filled to `value`, 0.0 to 1.0.
    pub fn progress(self, value: f32, label: Option<String>) -> Self {
        self.part(Part::Progress(Progress { value, label }))
    }

    /// Every action the card's buttons run, parsed back, for tests.
    pub fn parsed_actions(&self) -> Vec<Value> {
        let inline = |item: &Inline| match item {
            Inline::Button(b) => b.actions.clone(),
            Inline::App(a) => a.actions.clone(),
            Inline::Text(_) | Inline::Icon(_) => Vec::new(),
        };
        self.parts
            .iter()
            .flat_map(|part| match part {
                Part::Row(items) | Part::Flow(items) => items.iter().flat_map(inline).collect(),
                _ => Vec::new(),
            })
            .filter_map(|a| serde_json::from_str(&a).ok())
            .collect()
    }

    /// Every piece of text the card shows, for tests.
    pub fn texts(&self) -> Vec<String> {
        self.parts
            .iter()
            .flat_map(|part| match part {
                Part::Text(t) => vec![t.text.clone()],
                Part::Progress(p) => p.label.iter().cloned().collect(),
                Part::Row(items) | Part::Flow(items) => items
                    .iter()
                    .filter_map(|item| match item {
                        Inline::Text(t) => Some(t.text.clone()),
                        Inline::Button(b) => Some(b.label.clone()),
                        _ => None,
                    })
                    .collect(),
                Part::Grid(g) => g.cells.iter().map(|c| c.text.clone()).collect(),
                Part::Field(f) => vec![f.hint.clone()],
            })
            .collect()
    }
}

impl Manifest {
    /// A manifest for `name`, at this crate's version, asking for nothing.
    pub fn new(name: &str, summary: &str) -> Self {
        Self {
            name: name.to_owned(),
            version: env!("CARGO_PKG_VERSION").to_owned(),
            summary: summary.to_owned(),
            inputs: Vec::new(),
            actions: Vec::new(),
        }
    }

    /// Sets the parts of the view to fill.
    pub fn inputs(mut self, inputs: &[Input]) -> Self {
        self.inputs = inputs.to_vec();
        self
    }

    /// Sets the action kinds its buttons may run.
    pub fn actions(mut self, actions: &[&str]) -> Self {
        self.actions = actions.iter().map(|&a| a.to_owned()).collect();
        self
    }
}
