//! The WebAssembly runtime derisk's plugins are confined to: palette
//! plugins and overview widget plugins alike.
//!
//! Built the way pm confines its plugins (losos-project/pm,
//! `src/plugin/engine.rs`), with the same limits. A plugin runs inside
//! derisk's own process, the desktop shell, so the WebAssembly sandbox is
//! the whole boundary:
//!
//! - **Imports:** `host.log`, one-way, and for a palette plugin
//!   `assistant.interpret`, a pure function. No WASI is linked, so a
//!   component importing anything else fails to instantiate rather than
//!   being handed it.
//! - **Time:** every call is metered with [`FUEL`]; running out is a trap,
//!   deterministically, so a plugin spinning in a loop costs one frame, not
//!   the session.
//! - **Memory:** [`MEMORY_BYTES`] of linear memory and [`TABLE_ELEMENTS`]
//!   of tables, through [`StoreLimits`]; growing past them fails inside the
//!   guest as an allocation failure.
//! - **Stack:** [`STACK_BYTES`]; past it the call traps.
//! - **State:** a fresh [`Store`], and so a fresh instance, for every call.
//!   Nothing a plugin saw in one view carries into the next; the palette
//!   and the overview are what remember.
//!
//! The compiled [`Component`] is the expensive part. It is built once per
//! plugin at load and shared by every call on every thread.

use miette::{Result, miette};
use tracing::{debug, error, info, trace, warn};
use wasmtime::{
    Config, Engine, Store, StoreLimits, StoreLimitsBuilder, Trap,
    component::{Component, Linker},
};

use crate::{
    Interpreter, Kind,
    wit::{palette, widget},
};

/// Instructions one call may execute. The bundled plugins build a catalog
/// of tens of thousands of files in well under a tenth of it.
const FUEL: u64 = 2_000_000_000;

/// Largest linear memory one instance may grow to: room for a view holding
/// the files index and the rows built from it.
const MEMORY_BYTES: usize = 256 << 20;

/// Largest table one instance may grow to, in elements.
const TABLE_ELEMENTS: usize = 10_000;

/// Core instances, memories and tables one component may be built from: a
/// backstop against an absurd instance graph, not a budget.
const MAX_CORE_INSTANCES: usize = 32;
const MAX_MEMORIES: usize = 4;
const MAX_TABLES: usize = 8;

/// Native stack one call may use.
const STACK_BYTES: usize = 512 << 10;

/// Longest log line a plugin may write; the cut is marked.
const MAX_LOG_BYTES: usize = 4 << 10;
const TRUNCATION_MARK: &str = " ... [truncated]";

/// The engine and linker every plugin of kind `K` shares. Both are `Sync`;
/// the mutable state of a call lives in the [`Store`] [`Runtime::enter`]
/// creates.
pub(crate) struct Runtime<K: Kind> {
    engine: Engine,
    linker: Linker<State>,
    interpreter: Option<Interpreter>,
    kind: std::marker::PhantomData<fn() -> K>,
}

impl<K: Kind> Runtime<K> {
    /// Builds the engine and defines the imports a plugin of kind `K` gets.
    /// `interpreter` answers `assistant.interpret`, which only palette
    /// plugins import.
    pub(crate) fn new(interpreter: Option<Interpreter>) -> Result<Self> {
        let mut config = Config::new();
        config
            .wasm_component_model(true)
            .consume_fuel(true)
            // A plugin's rows should not depend on which machine draws them.
            .relaxed_simd_deterministic(true)
            .max_wasm_stack(STACK_BYTES);
        let engine = Engine::new(&config).map_err(|error| {
            miette!(
                "cannot start the WebAssembly engine {}s run in: {error:?}",
                K::NOUN
            )
        })?;
        let mut linker: Linker<State> = Linker::new(&engine);
        // Each kind's world, and nothing else. Adding WASI here would hand
        // every installed plugin the person's files, in the shell.
        K::add_to_linker(&mut linker)
            .map_err(|error| miette!("cannot define the {} interface: {error:?}", K::NOUN))?;
        Ok(Self {
            engine,
            linker,
            interpreter,
            kind: std::marker::PhantomData,
        })
    }

    /// Compiles `bytes` into a component.
    pub(crate) fn compile(&self, bytes: &[u8]) -> Result<Component> {
        Component::new(&self.engine, bytes).map_err(|error| {
            let (noun, wit) = (K::NOUN, K::WIT);
            miette!(
                help = format!(
                    "A {noun} is a WebAssembly *component* built against derisk's `{wit}`; \
                     a core module has to go through `wasm-tools component new` first."
                ),
                "not a usable WebAssembly component: {error:?}"
            )
        })
    }

    /// Instantiates `component` in a store of its own and hands it to
    /// `call`. The store is dropped before this returns.
    pub(crate) fn enter<T>(
        &self,
        plugin: &str,
        component: &Component,
        call: impl FnOnce(&K::Bindings, &mut Store<State>) -> wasmtime::Result<T>,
    ) -> Result<T> {
        let mut store = Store::new(&self.engine, State::new(plugin, self.interpreter));
        store.limiter(|state| &mut state.limits);
        store
            .set_fuel(FUEL)
            .map_err(|error| miette!("cannot meter the {} {plugin}: {error:?}", K::NOUN))?;
        let bindings = K::instantiate(&mut store, component, &self.linker)
            .map_err(|error| describe(K::NOUN, plugin, "instantiate", &mut store, &error))?;
        let outcome = call(&bindings, &mut store);
        let spent = FUEL.saturating_sub(store.get_fuel().unwrap_or(0));
        trace!(plugin, fuel = spent, "{} call finished", K::NOUN);
        outcome.map_err(|error| describe(K::NOUN, plugin, "answer", &mut store, &error))
    }
}

/// Everything one call may touch: its limits, its name for the log, and
/// the assistant.
pub struct State {
    plugin: String,
    limits: StoreLimits,
    interpreter: Option<Interpreter>,
}

impl State {
    fn new(plugin: &str, interpreter: Option<Interpreter>) -> Self {
        Self {
            plugin: plugin.to_owned(),
            limits: StoreLimitsBuilder::new()
                .memory_size(MEMORY_BYTES)
                .table_elements(TABLE_ELEMENTS)
                .instances(MAX_CORE_INSTANCES)
                .memories(MAX_MEMORIES)
                .tables(MAX_TABLES)
                .build(),
            interpreter,
        }
    }
}

impl State {
    /// Writes a plugin's line into derisk's log, tagged with its name.
    fn log(&self, level: tracing::Level, message: String) {
        let message = truncate(message);
        let plugin = self.plugin.as_str();
        match level {
            tracing::Level::ERROR => error!(plugin, "{message}"),
            tracing::Level::WARN => warn!(plugin, "{message}"),
            tracing::Level::INFO => info!(plugin, "{message}"),
            tracing::Level::DEBUG => debug!(plugin, "{message}"),
            tracing::Level::TRACE => trace!(plugin, "{message}"),
        }
    }
}

// `types` declares no functions, but the worlds use it, so the (empty)
// traits still need an implementation.
impl palette::derisk::palette::types::Host for State {}
impl widget::derisk::widget::types::Host for State {}

impl palette::derisk::palette::host::Host for State {
    fn log(&mut self, level: palette::derisk::palette::host::Level, message: String) {
        use palette::derisk::palette::host::Level;
        let level = match level {
            Level::Error => tracing::Level::ERROR,
            Level::Warn => tracing::Level::WARN,
            Level::Info => tracing::Level::INFO,
            Level::Debug => tracing::Level::DEBUG,
            Level::Trace => tracing::Level::TRACE,
        };
        State::log(self, level, message);
    }
}

impl widget::derisk::widget::host::Host for State {
    fn log(&mut self, level: widget::derisk::widget::host::Level, message: String) {
        use widget::derisk::widget::host::Level;
        let level = match level {
            Level::Error => tracing::Level::ERROR,
            Level::Warn => tracing::Level::WARN,
            Level::Info => tracing::Level::INFO,
            Level::Debug => tracing::Level::DEBUG,
            Level::Trace => tracing::Level::TRACE,
        };
        State::log(self, level, message);
    }
}

impl palette::derisk::palette::assistant::Host for State {
    fn interpret(&mut self, text: String) -> Result<Vec<String>, String> {
        // Only a palette runtime is built with an interpreter, and only the
        // palette world imports this.
        match self.interpreter {
            Some(interpret) => interpret(&text),
            None => Err("no assistant here".into()),
        }
    }
}

/// Cuts `message` to [`MAX_LOG_BYTES`] on a character boundary.
fn truncate(mut message: String) -> String {
    if message.len() <= MAX_LOG_BYTES {
        return message;
    }
    let mut cut = MAX_LOG_BYTES;
    while !message.is_char_boundary(cut) {
        cut -= 1;
    }
    message.truncate(cut);
    message.push_str(TRUNCATION_MARK);
    message
}

/// A wasmtime failure as a diagnostic naming the plugin and the limit hit.
fn describe(
    noun: &str,
    plugin: &str,
    what: &str,
    store: &mut Store<State>,
    error: &wasmtime::Error,
) -> miette::Report {
    match error.downcast_ref::<Trap>() {
        Some(Trap::OutOfFuel) => miette!(
            help = "It ran past the per-call instruction budget, which is a runaway \
                    loop far more often than a plugin that needs more room.",
            "the {noun} {plugin} used all {FUEL} units of fuel trying to {what}"
        ),
        Some(Trap::StackOverflow) => {
            miette!("the {noun} {plugin} overflowed its {STACK_BYTES}-byte stack trying to {what}")
        }
        _ => {
            debug!(
                plugin,
                fuel_left = store.get_fuel().unwrap_or(0),
                "{noun} failed"
            );
            miette!("the {noun} {plugin} failed to {what}: {error:?}")
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn long_log_lines_are_cut_on_a_character_boundary() {
        let line = truncate("é".repeat(MAX_LOG_BYTES));
        assert!(line.ends_with(TRUNCATION_MARK));
        assert!(line.len() <= MAX_LOG_BYTES + TRUNCATION_MARK.len());
    }
}
