//! The overview's widgets: cards from widget plugins (see `derisk-plugin`),
//! and the cards programs register over the agent socket, which the bundled
//! `programs` plugin turns into cards like the others.
//!
//! Every card comes from a plugin: the clock, the calendar, the battery,
//! suggested apps, failed services and the notes are bundled plugins under
//! `plugins/widgets/`, and more can be installed the way palette plugins
//! are. This module builds the [`View`] they are shown ([`view`]) and keeps
//! their cards until what a plugin reads changes ([`Board`]); the overview
//! draws the cards with the theme.
//!
//! A program's card:
//!
//! ```text
//! {"method":"register_widget","widget":{"id":"weather","title":"Weather",
//!   "icon":"weather-clear","rows":[{"type":"text","text":"18 °C, clear"},
//!   {"type":"progress","value":0.4,"label":"Rain 40%"},
//!   {"type":"button","label":"Refresh","item":"refresh"}]}}
//! {"method":"remove_widget","id":"weather"}
//! ```
//!
//! A widget is only data: text, progress bars and buttons, drawn in the
//! shell's theme like the built-in cards, so a program cannot paint over the
//! desktop or style itself out of the theme. Registering the same `id` again
//! replaces the widget, which is how a program keeps it current. On a live
//! session's socket the registering connection owns the widget, hears of
//! button presses as `{"event":"widget","id":"weather","item":"refresh"}`
//! ([`widget_event`]), and takes the widget with it when it closes.

use std::{collections::BTreeMap, sync::OnceLock};

use derisk_plugin::widget::{Battery, Card, Clock, Plugins, Registration, View};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::shell::Shell;

/// Most custom widgets the overview shows, so a runaway program cannot push
/// the built-in ones out of reach.
pub const MAX_WIDGETS: usize = 8;
/// Most rows in one widget.
pub const MAX_ROWS: usize = 12;
/// Longest ID, title, label or line of text, in characters.
pub const MAX_TEXT: usize = 200;

/// One custom card.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CustomWidget {
    /// Stable name, unique on the desktop; also what events carry.
    pub id: String,
    /// Heading, drawn like the built-in cards' ("Notes", "Services").
    pub title: String,
    /// Symbolic icon by freedesktop name, drawn beside the title.
    #[serde(default)]
    pub icon: Option<String>,
    /// Content, top to bottom.
    #[serde(default)]
    pub rows: Vec<WidgetRow>,
}

/// One line of a [`CustomWidget`].
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum WidgetRow {
    /// A line of text.
    Text {
        /// What it says.
        text: String,
        /// Drawn in the theme's secondary color, for hints and captions.
        #[serde(default)]
        dim: bool,
    },
    /// A bar filled from 0.0 to 1.0.
    Progress {
        /// How full.
        value: f32,
        /// Text on the bar.
        #[serde(default)]
        label: Option<String>,
    },
    /// A button; pressing it sends `item` to the widget's owner.
    Button {
        /// Its text.
        label: String,
        /// What the owner is told was pressed.
        item: String,
    },
}

impl CustomWidget {
    /// Refuses a widget the overview would draw badly or that could crowd
    /// it: an empty or overlong ID, too many rows, overlong text.
    pub fn check(&self) -> Result<(), String> {
        let long = |s: &str| s.chars().count() > MAX_TEXT;
        if self.id.is_empty() || long(&self.id) {
            return Err("widget id must be 1 to 200 characters".into());
        }
        if long(&self.title) || self.icon.as_deref().is_some_and(long) {
            return Err("widget title or icon too long".into());
        }
        if self.rows.len() > MAX_ROWS {
            return Err(format!("a widget has at most {MAX_ROWS} rows"));
        }
        for row in &self.rows {
            let too_long = match row {
                WidgetRow::Text { text, .. } => long(text),
                WidgetRow::Progress { label, .. } => label.as_deref().is_some_and(long),
                WidgetRow::Button { label, item } => long(label) || long(item),
            };
            if too_long {
                return Err(format!("widget text is at most {MAX_TEXT} characters"));
            }
        }
        Ok(())
    }

    fn has_button(&self, item: &str) -> bool {
        self.rows
            .iter()
            .any(|r| matches!(r, WidgetRow::Button { item: i, .. } if i == item))
    }
}

/// The registered custom widgets, in the order they were first registered,
/// and which agent connection, if any, owns each.
#[derive(Clone, Debug, Default)]
pub struct CustomWidgets {
    widgets: Vec<CustomWidget>,
    owners: BTreeMap<String, u64>,
}

impl CustomWidgets {
    /// Registers or replaces `widget` on behalf of `owner` (none for a
    /// headless agent). A widget another connection owns is refused, so one
    /// program cannot take over a card another keeps current.
    pub fn register(&mut self, widget: CustomWidget, owner: Option<u64>) -> Result<(), String> {
        widget.check()?;
        if let Some(&other) = self.owners.get(&widget.id)
            && Some(other) != owner
        {
            return Err(format!(
                "another connection registered widget {}",
                widget.id
            ));
        }
        match self.widgets.iter().position(|w| w.id == widget.id) {
            Some(i) => self.widgets[i] = widget.clone(),
            None if self.widgets.len() >= MAX_WIDGETS => {
                return Err(format!(
                    "the overview shows at most {MAX_WIDGETS} custom widgets"
                ));
            }
            None => self.widgets.push(widget.clone()),
        }
        match owner {
            Some(owner) => self.owners.insert(widget.id, owner),
            None => self.owners.remove(&widget.id),
        };
        Ok(())
    }

    /// Removes widget `id`, unless another connection than `owner` owns it.
    pub fn remove(&mut self, id: &str, owner: Option<u64>) -> Result<(), String> {
        if let Some(&other) = self.owners.get(id)
            && Some(other) != owner
        {
            return Err(format!("another connection registered widget {id}"));
        }
        let before = self.widgets.len();
        self.widgets.retain(|w| w.id != id);
        self.owners.remove(id);
        if self.widgets.len() == before {
            return Err(format!("no widget {id}"));
        }
        Ok(())
    }

    /// Removes the widgets connection `owner` registered, once it has closed:
    /// nobody is left to keep them current or answer their buttons.
    pub fn disown(&mut self, owner: u64) {
        let gone: Vec<String> = self
            .owners
            .iter()
            .filter(|(_, o)| **o == owner)
            .map(|(id, _)| id.clone())
            .collect();
        for id in gone {
            self.widgets.retain(|w| w.id != id);
            self.owners.remove(&id);
        }
    }

    /// The widgets, in the order the overview draws them.
    pub fn list(&self) -> &[CustomWidget] {
        &self.widgets
    }

    /// Whether widget `id` has a button that sends `item`.
    pub fn has_button(&self, id: &str, item: &str) -> bool {
        self.widgets
            .iter()
            .any(|w| w.id == id && w.has_button(item))
    }

    /// The connection to tell that `item` was pressed on widget `id`: its
    /// owner, when the widget has that button.
    pub fn recipient(&self, id: &str, item: &str) -> Option<u64> {
        self.has_button(id, item)
            .then(|| self.owners.get(id).copied())
            .flatten()
    }
}

/// The event line telling a widget's owner that a button was pressed.
pub fn widget_event(id: &str, item: &str) -> Value {
    json!({"event": "widget", "id": id, "item": item})
}

static PLUGINS: OnceLock<Plugins> = OnceLock::new();

/// The overview's widget plugins: the bundled ones, then the system's, then
/// the person's signed ones. Loaded once, on first use; [`preload`] starts
/// that early.
///
/// # Panics
///
/// If the bundled plugins do not load, which is a bug in derisk's build.
pub fn plugins() -> &'static Plugins {
    PLUGINS.get_or_init(|| {
        crate::plugins::with_installed(Plugins::bundled().expect("derisk's bundled widget plugins"))
    })
}

/// Loads [`plugins`] now, so the overview's first opening does not wait
/// for them to compile. For a thread of its own at startup.
pub fn preload() {
    plugins();
}

/// How many suggested apps the view carries.
const SUGGESTED: usize = 5;

/// The desktop as widget plugins see it; each sees only the parts its
/// manifest asks for.
pub fn view(shell: &Shell) -> View {
    let clock = shell.clock;
    View {
        clock: Clock {
            year: clock.year,
            month: clock.month,
            day: clock.day,
            hour: clock.hour,
            minute: clock.minute,
            weekday: clock.weekday,
            twenty_four_hour: shell.effects.top_bar.clock_24h,
        },
        battery: shell.battery.map(|b| Battery {
            percent: b.percent,
            charging: b.charging,
        }),
        suggested: shell.habits.suggestions(clock.hour, SUGGESTED),
        failed_units: shell.failed_units.clone(),
        registered: shell
            .widgets
            .list()
            .iter()
            .map(|w| Registration {
                id: w.id.clone(),
                data: serde_json::to_string(w).unwrap_or_default(),
            })
            .collect(),
    }
}

/// What one plugin last showed, and for what.
#[derive(Debug)]
struct Answer {
    /// The view it was rendered for, as the plugin saw it.
    view: View,
    cards: Vec<Card>,
}

/// The overview's cards, kept between frames: each plugin renders again
/// only when a part of the desktop it reads has changed, which for most of
/// them is once a minute at most.
#[derive(Debug, Default)]
pub struct Board {
    answers: Vec<Option<Answer>>,
}

impl Board {
    /// Renders, in parallel, every plugin whose part of `view` changed
    /// since it last rendered.
    pub fn refresh(&mut self, plugins: &Plugins, view: &View) {
        self.answers.resize_with(plugins.len(), || None);
        let seen: Vec<View> = plugins.iter().map(|p| p.view(view)).collect();
        let answers = &self.answers;
        let fresh = plugins.each(
            |i, _| answers[i].as_ref().is_none_or(|a| a.view != seen[i]),
            |plugin| plugins.render(plugin, view),
        );
        for (i, cards) in fresh {
            self.answers[i] = Some(Answer {
                view: seen[i].clone(),
                cards,
            });
        }
    }

    /// The cards, plugin by plugin in load order, each with the index of
    /// the plugin that made it.
    pub fn cards(&self) -> impl Iterator<Item = (usize, &Card)> {
        self.answers.iter().enumerate().flat_map(|(i, answer)| {
            answer
                .iter()
                .flat_map(move |a| a.cards.iter().map(move |card| (i, card)))
        })
    }
}
