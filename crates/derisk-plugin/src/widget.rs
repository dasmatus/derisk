//! Overview widget plugins: every card the overview shows.
//!
//! A plugin answers one question, [`Plugins::render`]: its cards for a
//! [`View`] of the desktop. A card is semantic parts (text in a [`Tone`]
//! and an [`Emphasis`], progress bars, rows of buttons, grids, a text
//! field), never pixels: derisk draws every part with its theme, so a theme
//! restyles every widget, and no plugin can paint over the desktop. This
//! module holds each plugin to its [`Manifest`]:
//!
//! - a plugin sees only the parts of the view its manifest lists
//!   ([`Plugin::view`]);
//! - a card is dropped when it is larger than the overview draws, or when a
//!   button's action is not a JSON object whose `action` tag the manifest
//!   lists ([`Plugin::check`]).
//!
//! The clock, the calendar, the battery, suggested apps, failed services,
//! the notes, and the cards programs register over the agent socket are
//! all bundled plugins, under `plugins/widgets/`.

use miette::Result;
use tracing::warn;
use wasmtime::{
    Store,
    component::{Component, HasSelf, Linker},
};

pub use crate::wit::widget::derisk::widget::types::{
    AppButton, Battery, Button, Card, Clock, Emphasis, Field, Grid, Inline, Input, Manifest, Part,
    Progress, Registration, Text, Tone, View,
};
use crate::{Kind, bundled, check_actions, engine::Runtime, engine::State, wit};

/// Most cards one plugin may show.
pub const MAX_CARDS: usize = 16;
/// Most parts in one card.
pub const MAX_PARTS: usize = 32;
/// Most inline parts in one row or flow, and most cells in one grid.
pub const MAX_ITEMS: usize = 64;
/// Longest text, label, key, icon name or action, in characters.
pub const MAX_TEXT: usize = 200;
/// Most columns in a grid, and most lines in a text field.
pub const MAX_SPAN: u8 = 12;

/// The widget plugin kind.
#[derive(Debug)]
pub struct Widget;

impl Kind for Widget {
    const NOUN: &'static str = "widget plugin";
    const WIT: &'static str = "widget.wit";
    const DIR: &'static str = PLUGIN_DIR;
    const BUNDLED: &'static [(&'static str, &'static [u8])] = bundled::WIDGETS;
    type Bindings = wit::widget::WidgetPlugin;
    type Manifest = Manifest;

    fn add_to_linker(linker: &mut Linker<State>) -> wasmtime::Result<()> {
        wit::widget::WidgetPlugin::add_to_linker::<State, HasSelf<State>>(linker, |state| state)
    }

    fn instantiate(
        store: &mut Store<State>,
        component: &Component,
        linker: &Linker<State>,
    ) -> wasmtime::Result<Self::Bindings> {
        wit::widget::WidgetPlugin::instantiate(store, component, linker)
    }

    fn describe(bindings: &Self::Bindings, store: &mut Store<State>) -> wasmtime::Result<Manifest> {
        bindings.call_describe(store)
    }

    fn name(manifest: &Manifest) -> &str {
        &manifest.name
    }
}

/// Where the system's widget plugins go under each `$XDG_DATA_DIRS` entry,
/// and the person's under `$XDG_DATA_HOME`.
pub const PLUGIN_DIR: &str = "derisk/widget-plugins";

/// A loaded widget plugin.
pub type Plugin = crate::Plugin<Widget>;

/// The loaded widget plugins.
pub type Plugins = crate::Plugins<Widget>;

impl Default for Clock {
    /// Noon on Saturday 1 January 2000: what a plugin that may not see the
    /// clock is told.
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
}

impl Plugin {
    /// Whether it may see `input`.
    pub fn reads(&self, input: Input) -> bool {
        self.manifest.inputs.contains(&input)
    }

    /// `view` with only what this plugin may see; everything else empty.
    pub fn view(&self, view: &View) -> View {
        let mut out = View::default();
        if self.reads(Input::Clock) {
            out.clock = view.clock;
        }
        if self.reads(Input::Battery) {
            out.battery = view.battery;
        }
        if self.reads(Input::Apps) {
            out.suggested.clone_from(&view.suggested);
        }
        if self.reads(Input::Units) {
            out.failed_units.clone_from(&view.failed_units);
        }
        if self.reads(Input::Registered) {
            out.registered.clone_from(&view.registered);
        }
        out
    }

    /// Whether `card` is one the overview draws: within the size limits,
    /// and every button's actions are JSON objects whose `action` tag the
    /// manifest lists.
    pub fn check(&self, card: &Card) -> std::result::Result<(), String> {
        let long = |s: &str| s.chars().count() > MAX_TEXT;
        let too_long = || format!("text is at most {MAX_TEXT} characters");
        if long(&card.key) {
            return Err(too_long());
        }
        if card.parts.len() > MAX_PARTS {
            return Err(format!("a card has at most {MAX_PARTS} parts"));
        }
        let actions = |actions: &[String]| -> std::result::Result<(), String> {
            if actions.iter().any(|a| long(a)) {
                return Err(too_long());
            }
            check_actions(actions, &self.manifest.actions)
        };
        for part in &card.parts {
            match part {
                Part::Text(text) if long(&text.text) => return Err(too_long()),
                Part::Text(_) => {}
                Part::Progress(progress) => {
                    if progress.label.as_deref().is_some_and(long) {
                        return Err(too_long());
                    }
                }
                Part::Row(items) | Part::Flow(items) => {
                    if items.len() > MAX_ITEMS {
                        return Err(format!("a row has at most {MAX_ITEMS} parts"));
                    }
                    for item in items {
                        match item {
                            Inline::Text(text) if long(&text.text) => return Err(too_long()),
                            Inline::Text(_) => {}
                            Inline::Icon(name) if long(name) => return Err(too_long()),
                            Inline::Icon(_) => {}
                            Inline::Button(button) if long(&button.label) => {
                                return Err(too_long());
                            }
                            Inline::Button(button) => actions(&button.actions)?,
                            Inline::App(app) if long(&app.app) => return Err(too_long()),
                            Inline::App(app) => actions(&app.actions)?,
                        }
                    }
                }
                Part::Grid(grid) => {
                    if !(1..=MAX_SPAN).contains(&grid.columns) {
                        return Err(format!("a grid has 1 to {MAX_SPAN} columns"));
                    }
                    if grid.cells.len() > MAX_ITEMS {
                        return Err(format!("a grid has at most {MAX_ITEMS} cells"));
                    }
                    if grid.cells.iter().any(|c| long(&c.text)) {
                        return Err(too_long());
                    }
                }
                Part::Field(field) => {
                    if !(1..=MAX_SPAN).contains(&field.lines) {
                        return Err(format!("a text field is 1 to {MAX_SPAN} lines"));
                    }
                    if long(&field.id) || long(&field.hint) {
                        return Err(too_long());
                    }
                }
            }
        }
        Ok(())
    }

    /// The cards of an answer that pass [`Plugin::check`], at most
    /// [`MAX_CARDS`]; the rest are logged and dropped.
    fn keep(&self, cards: Vec<Card>) -> Vec<Card> {
        if cards.len() > MAX_CARDS {
            warn!(
                plugin = self.name(),
                total = cards.len(),
                "widget plugin returned too many cards"
            );
        }
        cards
            .into_iter()
            .take(MAX_CARDS)
            .filter(|card| match self.check(card) {
                Ok(()) => true,
                Err(reason) => {
                    warn!(
                        plugin = self.name(),
                        key = card.key,
                        "card dropped: {reason}"
                    );
                    false
                }
            })
            .collect()
    }
}

impl Plugins {
    /// derisk's own widget plugins, compiled in parallel.
    ///
    /// # Errors
    ///
    /// Fails if the engine cannot start or a bundled plugin does not load,
    /// both bugs in derisk.
    pub fn bundled() -> Result<Self> {
        Self::with_bundled(Runtime::new(None)?)
    }

    /// `plugin`'s cards for `view`, checked against its manifest.
    ///
    /// # Errors
    ///
    /// Fails if the plugin traps or runs out of fuel or memory; the
    /// overview shows the other cards without it.
    pub fn render(&self, plugin: &Plugin, view: &View) -> Result<Vec<Card>> {
        let view = plugin.view(view);
        let cards = self
            .runtime
            .enter(plugin.name(), &plugin.component, |b, store| {
                b.call_render(store, &view)
            })?;
        Ok(plugin.keep(cards))
    }
}
