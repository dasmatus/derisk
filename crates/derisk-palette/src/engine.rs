//! The WebAssembly runtime a palette plugin is confined to.
//!
//! Built the way pm confines its plugins (losos-project/pm,
//! `src/plugin/engine.rs`), with the same limits. A plugin runs inside
//! derisk's own process, the desktop shell, so the WebAssembly sandbox is
//! the whole boundary:
//!
//! - **Imports:** `host.log`, one-way, and `assistant.interpret`, a pure
//!   function. No WASI is linked, so a component importing anything else
//!   fails to instantiate rather than being handed it.
//! - **Time:** every call is metered with [`FUEL`]; running out is a trap,
//!   deterministically, so a plugin spinning in a loop costs one frame, not
//!   the session.
//! - **Memory:** [`MEMORY_BYTES`] of linear memory and [`TABLE_ELEMENTS`]
//!   of tables, through [`StoreLimits`]; growing past them fails inside the
//!   guest as an allocation failure.
//! - **Stack:** [`STACK_BYTES`]; past it the call traps.
//! - **State:** a fresh [`Store`], and so a fresh instance, for every call.
//!   Nothing a plugin saw in one view carries into the next; the palette
//!   core is what remembers.
//!
//! The compiled [`Component`] is the expensive part. It is built once per
//! plugin at load and shared by every call on every thread.

use miette::{Result, miette};
use tracing::{debug, error, info, trace, warn};
use wasmtime::{
    Config, Engine, Store, StoreLimits, StoreLimitsBuilder, Trap,
    component::{Component, HasSelf, Linker},
};

use crate::{
    Interpreter,
    wit::{
        PalettePlugin,
        derisk::palette::{assistant, host, types},
    },
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

/// The engine and linker every plugin shares. Both are `Sync`; the mutable
/// state of a call lives in the [`Store`] [`Runtime::enter`] creates.
pub(crate) struct Runtime {
    engine: Engine,
    linker: Linker<State>,
    interpreter: Interpreter,
}

impl Runtime {
    /// Builds the engine and defines the two imports a plugin gets.
    pub(crate) fn new(interpreter: Interpreter) -> Result<Self> {
        let mut config = Config::new();
        config
            .wasm_component_model(true)
            .consume_fuel(true)
            // A plugin's rows should not depend on which machine draws them.
            .relaxed_simd_deterministic(true)
            .max_wasm_stack(STACK_BYTES);
        let engine = Engine::new(&config).map_err(|error| {
            miette!("cannot start the WebAssembly engine palette plugins run in: {error:?}")
        })?;
        let mut linker: Linker<State> = Linker::new(&engine);
        // The only `add_to_linker` call in the crate. Adding WASI here would
        // hand every installed plugin the person's files, in the shell.
        PalettePlugin::add_to_linker::<State, HasSelf<State>>(&mut linker, |state| state)
            .map_err(|error| miette!("cannot define the palette plugin interface: {error:?}"))?;
        Ok(Self {
            engine,
            linker,
            interpreter,
        })
    }

    /// Compiles `bytes` into a component.
    pub(crate) fn compile(&self, bytes: &[u8]) -> Result<Component> {
        Component::new(&self.engine, bytes).map_err(|error| {
            miette!(
                help = "A palette plugin is a WebAssembly *component* built against \
                        derisk's `palette.wit`; a core module has to go through \
                        `wasm-tools component new` first.",
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
        call: impl FnOnce(&PalettePlugin, &mut Store<State>) -> wasmtime::Result<T>,
    ) -> Result<T> {
        let mut store = Store::new(&self.engine, State::new(plugin, self.interpreter));
        store.limiter(|state| &mut state.limits);
        store
            .set_fuel(FUEL)
            .map_err(|error| miette!("cannot meter the palette plugin {plugin}: {error:?}"))?;
        let bindings = PalettePlugin::instantiate(&mut store, component, &self.linker)
            .map_err(|error| describe(plugin, "instantiate", &mut store, &error))?;
        let outcome = call(&bindings, &mut store);
        let spent = FUEL.saturating_sub(store.get_fuel().unwrap_or(0));
        trace!(plugin, fuel = spent, "palette plugin call finished");
        outcome.map_err(|error| describe(plugin, "answer", &mut store, &error))
    }
}

/// Everything one call may touch: its limits, its name for the log, and
/// the assistant.
pub(crate) struct State {
    plugin: String,
    limits: StoreLimits,
    interpreter: Interpreter,
}

impl State {
    fn new(plugin: &str, interpreter: Interpreter) -> Self {
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

// `types` declares no functions, but the world uses it, so the (empty)
// trait still needs an implementation.
impl types::Host for State {}

impl host::Host for State {
    /// Writes a plugin's line into derisk's log, tagged with its name.
    fn log(&mut self, level: host::Level, message: String) {
        let message = truncate(message);
        let plugin = self.plugin.as_str();
        match level {
            host::Level::Error => error!(plugin, "{message}"),
            host::Level::Warn => warn!(plugin, "{message}"),
            host::Level::Info => info!(plugin, "{message}"),
            host::Level::Debug => debug!(plugin, "{message}"),
            host::Level::Trace => trace!(plugin, "{message}"),
        }
    }
}

impl assistant::Host for State {
    fn interpret(&mut self, text: String) -> Result<Vec<String>, String> {
        (self.interpreter)(&text)
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
    plugin: &str,
    what: &str,
    store: &mut Store<State>,
    error: &wasmtime::Error,
) -> miette::Report {
    match error.downcast_ref::<Trap>() {
        Some(Trap::OutOfFuel) => miette!(
            help = "It ran past the per-call instruction budget, which is a runaway \
                    loop far more often than a plugin that needs more room.",
            "the palette plugin {plugin} used all {FUEL} units of fuel trying to {what}"
        ),
        Some(Trap::StackOverflow) => miette!(
            "the palette plugin {plugin} overflowed its {STACK_BYTES}-byte stack trying to {what}"
        ),
        _ => {
            debug!(
                plugin,
                fuel_left = store.get_fuel().unwrap_or(0),
                "palette plugin failed"
            );
            miette!("the palette plugin {plugin} failed to {what}: {error:?}")
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
