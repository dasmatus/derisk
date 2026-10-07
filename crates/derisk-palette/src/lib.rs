//! The derisk command palette's plugins: WebAssembly components that supply
//! every row the palette shows.
//!
//! A plugin answers two questions from a [`View`] of the desktop: its
//! catalog ([`Plugins::entries`]), which the palette searches as the person
//! types, and the rows for one typed query ([`Plugins::query`]). The palette
//! core in `derisk` ranks, remembers and draws; this crate loads plugins,
//! runs them in a sandbox (see [`engine`]) and holds each to its
//! [`Manifest`]:
//!
//! - a plugin sees only the parts of the view its manifest lists
//!   ([`Plugin::view`]): one that never asked for files never sees a file
//!   name, and one that asked for registered data sees only its sources;
//! - a row is dropped when its category is not in the manifest, or when one
//!   of its actions is not a JSON object whose `action` tag the manifest
//!   lists ([`Plugin::check`]).
//!
//! # Where plugins come from
//!
//! - **Bundled** ([`Plugins::bundled`]): built from `plugins/` by this
//!   crate's build script and embedded. Apps, windows, menus, commands,
//!   workspaces, the session, the tray and failed units, the browser, files,
//!   the assistant, Sonne's agent and web search are all bundled plugins.
//! - **System** ([`Plugins::load_system`]): `derisk/palette-plugins/*.wasm`
//!   under `$XDG_DATA_DIRS`, from files and folders only root can write, as
//!   an OS image installs them.
//! - **Installed by the person** ([`Plugins::load_signed`]):
//!   `$XDG_DATA_HOME/derisk/palette-plugins/*.wasm`, each with a detached
//!   `.sig` from a key in `$XDG_CONFIG_HOME/derisk/trusted/`. The signature
//!   format and trust store are pm's (`pm-signing`), so `pm sign` signs a
//!   palette plugin, and the same key can be trusted in both.
//!
//! A plugin may not take a name already loaded, so nothing installed later
//! can stand in for a bundled plugin.

#![deny(missing_docs)]

mod bundled {
    include!(concat!(env!("OUT_DIR"), "/bundled.rs"));
}
pub mod engine;
mod wit;

use std::{
    collections::BTreeSet,
    fmt, fs,
    os::unix::fs::MetadataExt as _,
    path::{Path, PathBuf},
};

use miette::{IntoDiagnostic as _, Result, WrapErr as _, miette};
use pm_signing::Signature;
pub use pm_signing::TrustStore;
use serde_json::Value;
use tracing::{debug, info, warn};
use wasmtime::component::Component;

use crate::engine::Runtime;
pub use crate::wit::derisk::palette::types::{
    App, AppAction, Category, Desk, Entry, File, Hook, Input, Manifest, MenuCommand, Position,
    Registration, TrayItem, View, Window, Workspace,
};

/// The built-in assistant a plugin can ask (`assistant.interpret`): the
/// actions a request asks for as JSON objects, or why it is not understood.
pub type Interpreter = fn(&str) -> std::result::Result<Vec<String>, String>;

/// Where the system's palette plugins go under each `$XDG_DATA_DIRS` entry,
/// and the person's under `$XDG_DATA_HOME`.
pub const PLUGIN_DIR: &str = "derisk/palette-plugins";

/// Where the keys trusted to sign the person's plugins are, under
/// `$XDG_CONFIG_HOME`.
pub const TRUST_DIR: &str = "derisk/trusted";

/// Largest plugin file loaded, so a stray file cannot stall the session
/// compiling it.
const MAX_PLUGIN_BYTES: u64 = 32 << 20;

/// Most rows one call may return; past it the rest are dropped. A files
/// index is capped well below this.
const MAX_ROWS: usize = 100_000;

impl Category {
    /// Group heading shown above rows of this kind.
    pub fn heading(self) -> &'static str {
        match self {
            Self::App => "Apps",
            Self::AppAction => "App actions",
            Self::Window => "Windows",
            Self::AppCommand => "App commands",
            Self::Command => "Commands",
            Self::Workspace => "Workspaces",
            Self::Setting => "Settings",
            Self::Session => "Session",
            Self::System => "System",
            Self::File => "Files",
            Self::Ask => "Assistant",
            Self::Web => "Web",
            Self::Agent => "Agent",
            Self::Tab => "Tabs",
            Self::Extension => "Extensions",
        }
    }
}

impl Default for View {
    fn default() -> Self {
        Self {
            hour: 12,
            apps: Vec::new(),
            suggested: Vec::new(),
            windows: Vec::new(),
            menus: Vec::new(),
            desk: Desk {
                workspaces: Vec::new(),
                window_focused: false,
                can_add_workspace: false,
            },
            tray: Vec::new(),
            failed_units: Vec::new(),
            files: Vec::new(),
            search_engine: None,
            registered: Vec::new(),
        }
    }
}

/// How a plugin got in.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Origin {
    /// Built into derisk.
    Bundled,
    /// From a system directory only root can write.
    System(PathBuf),
    /// Installed by the person, with a signature from a trusted key.
    Signed(PathBuf),
}

impl fmt::Display for Origin {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Bundled => f.write_str("bundled"),
            Self::System(path) => write!(f, "system, {}", path.display()),
            Self::Signed(path) => write!(f, "signed, {}", path.display()),
        }
    }
}

/// One loaded plugin.
pub struct Plugin {
    manifest: Manifest,
    component: Component,
    origin: Origin,
    inputs: BTreeSet<Input>,
    categories: BTreeSet<Category>,
    actions: BTreeSet<String>,
}

impl fmt::Debug for Plugin {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Plugin")
            .field("manifest", &self.manifest)
            .field("origin", &self.origin)
            .finish_non_exhaustive()
    }
}

impl Plugin {
    /// What the plugin says it is and may do.
    pub fn manifest(&self) -> &Manifest {
        &self.manifest
    }

    /// Its name.
    pub fn name(&self) -> &str {
        &self.manifest.name
    }

    /// How it got in.
    pub fn origin(&self) -> &Origin {
        &self.origin
    }

    /// Whether it implements `hook`.
    pub fn has(&self, hook: Hook) -> bool {
        self.manifest.hooks.contains(&hook)
    }

    /// Whether it may see `input`.
    pub fn reads(&self, input: Input) -> bool {
        self.inputs.contains(&input)
    }

    /// `view` with only what this plugin may see; everything else empty.
    pub fn view(&self, view: &View) -> View {
        let mut out = View {
            hour: view.hour,
            ..View::default()
        };
        if self.reads(Input::Apps) {
            out.apps.clone_from(&view.apps);
            out.suggested.clone_from(&view.suggested);
        }
        if self.reads(Input::Windows) {
            out.windows.clone_from(&view.windows);
        }
        if self.reads(Input::Menus) {
            out.menus.clone_from(&view.menus);
        }
        if self.reads(Input::Workspaces) {
            out.desk.clone_from(&view.desk);
        }
        if self.reads(Input::Tray) {
            out.tray.clone_from(&view.tray);
        }
        if self.reads(Input::Units) {
            out.failed_units.clone_from(&view.failed_units);
        }
        if self.reads(Input::Files) {
            out.files.clone_from(&view.files);
        }
        if self.reads(Input::SearchEngine) {
            out.search_engine.clone_from(&view.search_engine);
        }
        if self.reads(Input::Registered) {
            out.registered = view
                .registered
                .iter()
                .filter(|r| self.manifest.sources.contains(&r.source))
                .cloned()
                .collect();
        }
        out
    }

    /// Whether `entry` stays within the manifest: its category is listed,
    /// and every action is a JSON object whose `action` tag is listed.
    pub fn check(&self, entry: &Entry) -> std::result::Result<(), String> {
        if !self.categories.contains(&entry.category) {
            return Err(format!(
                "category {:?} is not in its manifest",
                entry.category
            ));
        }
        for action in &entry.actions {
            let kind = serde_json::from_str::<Value>(action)
                .ok()
                .and_then(|v| v.get("action")?.as_str().map(str::to_owned))
                .ok_or_else(|| format!("{action:?} is not an action"))?;
            if !self.actions.contains(&kind) {
                return Err(format!("action {kind:?} is not in its manifest"));
            }
        }
        Ok(())
    }

    /// The rows of an answer that pass [`Plugin::check`], at most
    /// [`MAX_ROWS`]; the rest are logged and dropped.
    fn keep(&self, rows: Vec<Entry>) -> Vec<Entry> {
        let total = rows.len();
        if total > MAX_ROWS {
            warn!(
                plugin = self.name(),
                total, "palette plugin returned too many rows"
            );
        }
        rows.into_iter()
            .take(MAX_ROWS)
            .filter(|entry| match self.check(entry) {
                Ok(()) => true,
                Err(reason) => {
                    warn!(
                        plugin = self.name(),
                        title = entry.title,
                        "row dropped: {reason}"
                    );
                    false
                }
            })
            .collect()
    }
}

/// The loaded plugins, and the engine they run in.
pub struct Plugins {
    runtime: Runtime,
    plugins: Vec<Plugin>,
}

impl fmt::Debug for Plugins {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_list().entries(&self.plugins).finish()
    }
}

impl Plugins {
    /// derisk's own plugins, compiled in parallel.
    ///
    /// # Errors
    ///
    /// Fails if the engine cannot start or a bundled plugin does not load,
    /// both bugs in derisk.
    pub fn bundled(interpreter: Interpreter) -> Result<Self> {
        let runtime = Runtime::new(interpreter)?;
        let compiled = parallel(bundled::BUNDLED, |&(name, bytes)| {
            load(&runtime, name, bytes, Origin::Bundled)
        });
        let plugins = compiled.into_iter().collect::<Result<Vec<_>>>()?;
        Ok(Self { runtime, plugins })
    }

    /// The loaded plugins, in the order they are asked.
    pub fn iter(&self) -> impl Iterator<Item = &Plugin> {
        self.plugins.iter()
    }

    /// How many are loaded.
    pub fn len(&self) -> usize {
        self.plugins.len()
    }

    /// Whether none are.
    pub fn is_empty(&self) -> bool {
        self.plugins.is_empty()
    }

    /// Loads the system's plugins from `derisk/palette-plugins` under each
    /// of `data_dirs` (`$XDG_DATA_DIRS`): only files, in folders, that root
    /// owns and nobody else can write, since such a file is as trusted as
    /// derisk itself. Returns why each refused file was refused.
    pub fn load_system<'a>(
        &mut self,
        data_dirs: impl IntoIterator<Item = &'a Path>,
    ) -> Vec<miette::Report> {
        let mut errors = Vec::new();
        for dir in data_dirs.into_iter().map(|d| d.join(PLUGIN_DIR)) {
            for path in plugin_files(&dir) {
                let result = root_only(&dir)
                    .and_then(|()| root_only(&path))
                    .and_then(|()| read_plugin(&path))
                    .and_then(|bytes| self.add(&bytes, Origin::System(path.clone())));
                if let Err(error) = result {
                    errors.push(miette!(
                        "not loading {}: {}",
                        path.display(),
                        reasons(&error)
                    ));
                }
            }
        }
        errors
    }

    /// Loads the person's plugins from `dir`, each with a `.sig` beside it
    /// by a key in `trust`. Returns why each refused file was refused.
    pub fn load_signed(&mut self, dir: &Path, trust: &TrustStore) -> Vec<miette::Report> {
        let mut errors = Vec::new();
        for path in plugin_files(dir) {
            let result = read_plugin(&path)
                .and_then(|bytes| verify(&path, &bytes, trust).map(|()| bytes))
                .and_then(|bytes| self.add(&bytes, Origin::Signed(path.clone())));
            if let Err(error) = result {
                errors.push(miette!(
                    "not loading {}: {}",
                    path.display(),
                    reasons(&error)
                ));
            }
        }
        errors
    }

    /// Compiles `bytes`, asks for its manifest and adds it after the others.
    fn add(&mut self, bytes: &[u8], origin: Origin) -> Result<()> {
        let label = origin.to_string();
        let plugin = load(&self.runtime, &label, bytes, origin)?;
        if self.plugins.iter().any(|p| p.name() == plugin.name()) {
            return Err(miette!(
                "a palette plugin named {:?} is already loaded",
                plugin.name()
            ));
        }
        info!(plugin = plugin.name(), origin = %plugin.origin, "loaded palette plugin");
        self.plugins.push(plugin);
        Ok(())
    }

    /// `plugin`'s catalog for `view`, checked against its manifest.
    ///
    /// # Errors
    ///
    /// Fails if the plugin traps, runs out of fuel or memory, or does not
    /// implement `entries`; the palette shows its other rows without it.
    pub fn entries(&self, plugin: &Plugin, view: &View) -> Result<Vec<Entry>> {
        if !plugin.has(Hook::Entries) {
            return Ok(Vec::new());
        }
        let view = plugin.view(view);
        let rows = self
            .runtime
            .enter(plugin.name(), &plugin.component, |b, store| {
                b.call_entries(store, &view)
            })?;
        Ok(plugin.keep(rows))
    }

    /// `plugin`'s rows for typed `text`, checked against its manifest.
    ///
    /// # Errors
    ///
    /// As [`Plugins::entries`].
    pub fn query(&self, plugin: &Plugin, view: &View, text: &str) -> Result<Vec<Entry>> {
        if !plugin.has(Hook::Query) {
            return Ok(Vec::new());
        }
        let view = plugin.view(view);
        let rows = self
            .runtime
            .enter(plugin.name(), &plugin.component, |b, store| {
                b.call_query(store, &view, text)
            })?;
        Ok(plugin.keep(rows))
    }

    /// Runs `ask` for each plugin `which` picks, on scoped threads, and
    /// returns the answers by plugin index. A plugin that fails is logged
    /// and answers nothing.
    pub fn each(
        &self,
        which: impl Fn(usize, &Plugin) -> bool,
        ask: impl Fn(&Plugin) -> Result<Vec<Entry>> + Sync,
    ) -> Vec<(usize, Vec<Entry>)> {
        let picked: Vec<(usize, &Plugin)> = self
            .plugins
            .iter()
            .enumerate()
            .filter(|&(i, p)| which(i, p))
            .collect();
        parallel(&picked, |&(index, plugin)| {
            let rows = ask(plugin).unwrap_or_else(|error| {
                warn!(
                    plugin = plugin.name(),
                    "palette plugin failed: {}",
                    reasons(&error)
                );
                Vec::new()
            });
            (index, rows)
        })
    }
}

/// Runs `work` over `items` on one scoped thread each, collecting over a
/// bounded channel, and returns the results in the order of `items`.
fn parallel<I: Sync, T: Send>(items: &[I], work: impl Fn(&I) -> T + Sync) -> Vec<T> {
    if items.len() <= 1 {
        return items.iter().map(&work).collect();
    }
    let (tx, rx) = crossbeam_channel::bounded(items.len());
    std::thread::scope(|scope| {
        for (index, item) in items.iter().enumerate() {
            let tx = tx.clone();
            let work = &work;
            scope.spawn(move || {
                // The channel holds every result, so this never blocks, and
                // the receiver outlives the scope, so it cannot fail.
                let _ = tx.send((index, work(item)));
            });
        }
    });
    drop(tx);
    let mut results: Vec<(usize, T)> = rx.into_iter().collect();
    results.sort_by_key(|(index, _)| *index);
    results.into_iter().map(|(_, result)| result).collect()
}

/// Compiles one plugin and checks its manifest.
fn load(runtime: &Runtime, label: &str, bytes: &[u8], origin: Origin) -> Result<Plugin> {
    let component = runtime
        .compile(bytes)
        .wrap_err_with(|| format!("cannot compile the palette plugin {label}"))?;
    let manifest = runtime.enter(label, &component, |b, store| b.call_describe(store))?;
    if !valid_name(&manifest.name) {
        return Err(miette!(
            "the palette plugin {label} calls itself {:?}; a name is 1-32 characters of \
             a-z, 0-9 and -",
            manifest.name
        ));
    }
    debug!(plugin = manifest.name, ?manifest, "palette plugin manifest");
    Ok(Plugin {
        inputs: manifest.inputs.iter().copied().collect(),
        categories: manifest.categories.iter().copied().collect(),
        actions: manifest.actions.iter().cloned().collect(),
        manifest,
        component,
        origin,
    })
}

/// A report and everything it wraps on one line, for a log that has no
/// room for miette's graphical report.
fn reasons(report: &miette::Report) -> String {
    report
        .chain()
        .map(ToString::to_string)
        .collect::<Vec<_>>()
        .join(": ")
}

fn valid_name(name: &str) -> bool {
    (1..=32).contains(&name.len())
        && name
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
}

/// The `.wasm` files in `dir`, sorted, so plugins load in a stable order.
fn plugin_files(dir: &Path) -> Vec<PathBuf> {
    let Ok(read) = fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut files: Vec<PathBuf> = read
        .filter_map(|e| Some(e.ok()?.path()))
        .filter(|p| p.extension().is_some_and(|e| e == "wasm"))
        .collect();
    files.sort();
    files
}

/// Reads a plugin file, refusing one too large to be a plugin.
fn read_plugin(path: &Path) -> Result<Vec<u8>> {
    let size = fs::metadata(path).into_diagnostic()?.len();
    if size > MAX_PLUGIN_BYTES {
        return Err(miette!(
            "it is {size} bytes, more than the {MAX_PLUGIN_BYTES} a palette plugin may be"
        ));
    }
    fs::read(path).into_diagnostic()
}

/// Whether `path` is root's and nobody else's to write. A symlink is
/// judged by what it points at, which is how a Nix profile installs one.
fn root_only(path: &Path) -> Result<()> {
    let meta = fs::metadata(path).into_diagnostic()?;
    if meta.uid() != 0 || meta.mode() & 0o022 != 0 {
        return Err(miette!(
            help = "A system palette plugin must be owned by root and writable by nobody \
                    else; install your own under $XDG_DATA_HOME, signed.",
            "{} can be written by someone other than root",
            path.display()
        ));
    }
    Ok(())
}

/// Checks `bytes`, already read from `path`, against `path.sig`. The bytes
/// verified are the bytes compiled: the file is not read twice.
fn verify(path: &Path, bytes: &[u8], trust: &TrustStore) -> Result<()> {
    let mut sig = path.as_os_str().to_owned();
    sig.push(".sig");
    let sig = PathBuf::from(sig);
    let text = fs::read_to_string(&sig)
        .into_diagnostic()
        .wrap_err_with(|| {
            format!(
                "it has no signature at {}; sign it with `pm sign {}` and trust the key in \
             $XDG_CONFIG_HOME/{TRUST_DIR}",
                sig.display(),
                path.display()
            )
        })?;
    Signature::from_yaml(&text)?.verify(bytes, trust)
}

#[cfg(test)]
mod tests {
    use pm_signing::SigningKey;

    use super::*;

    fn no_assistant(_: &str) -> std::result::Result<Vec<String>, String> {
        Err("no assistant here".into())
    }

    #[test]
    fn an_installed_plugin_needs_a_trusted_signature_and_a_free_name() {
        let dir =
            std::env::temp_dir().join(format!("derisk-palette-signed-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        let (plugins_dir, trust_dir) = (dir.join("plugins"), dir.join("trusted"));
        fs::create_dir_all(&plugins_dir).unwrap();
        fs::create_dir_all(&trust_dir).unwrap();
        let (_, web) = bundled::BUNDLED.iter().find(|(n, _)| *n == "web").unwrap();
        let path = plugins_dir.join("web.wasm");
        fs::write(&path, web).unwrap();
        let mut plugins = Plugins::bundled(no_assistant).unwrap();
        let refused = |plugins: &mut Plugins, trust: &TrustStore| {
            let errors = plugins.load_signed(&plugins_dir, trust);
            assert_eq!(errors.len(), 1);
            errors[0].to_string()
        };

        let mut trust = TrustStore::load(&trust_dir).unwrap();
        let unsigned = refused(&mut plugins, &trust);
        assert!(unsigned.contains("no signature"), "{unsigned}");

        let key = SigningKey::load_or_create(&dir.join("key")).unwrap();
        let signature = Signature::create(&key, web).to_yaml().unwrap();
        fs::write(plugins_dir.join("web.wasm.sig"), &signature).unwrap();
        let untrusted = refused(&mut plugins, &trust);
        assert!(!untrusted.contains("already loaded"), "{untrusted}");

        trust.add(&key.public_key_hex(), &trust_dir).unwrap();
        // Trusted, but it would stand in for the bundled web plugin.
        assert!(refused(&mut plugins, &trust).contains("already loaded"));

        // A file changed after signing is refused.
        let mut tampered = web.to_vec();
        tampered.push(0);
        fs::write(&path, tampered).unwrap();
        let tampered = refused(&mut plugins, &trust);
        assert!(!tampered.contains("already loaded"), "{tampered}");
        assert_eq!(plugins.len(), bundled::BUNDLED.len());
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn names_are_short_and_plain() {
        assert!(valid_name("apps"));
        assert!(valid_name("my-plugin-2"));
        assert!(!valid_name(""));
        assert!(!valid_name("Apps"));
        assert!(!valid_name("a/b"));
        assert!(!valid_name(&"a".repeat(33)));
    }
}
