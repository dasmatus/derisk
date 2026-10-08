//! derisk's plugins: WebAssembly components that supply every row the
//! command palette shows ([`palette`]) and every card on the overview
//! ([`widget`]).
//!
//! The two kinds share one runtime and one way in. Each is a component
//! built against its own WIT world, run in the same sandbox (see
//! [`engine`]), loaded from the same three places, and held to a manifest it
//! declares at load: a plugin sees only the parts of the desktop its
//! manifest lists, and what it returns is dropped when it names a category
//! or an action kind the manifest does not.
//!
//! # Where plugins come from
//!
//! - **Bundled** ([`palette::Plugins::bundled`],
//!   [`widget::Plugins::bundled`]): built from `plugins/palette/` and
//!   `plugins/widgets/` by this crate's build script and embedded.
//! - **System** ([`Plugins::load_system`]): `derisk/palette-plugins/*.wasm`
//!   and `derisk/widget-plugins/*.wasm` under `$XDG_DATA_DIRS`, from files
//!   and folders only root can write, as an OS image installs them.
//! - **Installed by the person** ([`Plugins::load_signed`]): the same
//!   folders under `$XDG_DATA_HOME`, each file with a detached `.sig` from a
//!   key in `$XDG_CONFIG_HOME/derisk/trusted/`. The signature format and
//!   trust store are pm's (`pm-signing`), so `pm sign` signs a derisk
//!   plugin, and the same key can be trusted in both.
//!
//! A plugin may not take a name already loaded for its kind, so nothing
//! installed later can stand in for a bundled plugin.

#![deny(missing_docs)]

mod bundled {
    include!(concat!(env!("OUT_DIR"), "/bundled.rs"));
}
pub mod engine;
pub mod palette;
pub mod widget;
mod wit;

use std::{
    fmt, fs,
    os::unix::fs::MetadataExt as _,
    path::{Path, PathBuf},
};

use miette::{IntoDiagnostic as _, Result, WrapErr as _, miette};
use pm_signing::Signature;
pub use pm_signing::TrustStore;
use serde_json::Value;
use tracing::{debug, info, warn};
use wasmtime::{
    Store,
    component::{Component, Linker},
};

use crate::engine::{Runtime, State};

/// The built-in assistant a palette plugin can ask (`assistant.interpret`):
/// the actions a request asks for as JSON objects, or why it is not
/// understood.
pub type Interpreter = fn(&str) -> std::result::Result<Vec<String>, String>;

/// Where the keys trusted to sign the person's plugins are, under
/// `$XDG_CONFIG_HOME`.
pub const TRUST_DIR: &str = "derisk/trusted";

/// Largest plugin file loaded, so a stray file cannot stall the session
/// compiling it.
const MAX_PLUGIN_BYTES: u64 = 32 << 20;

/// A kind of plugin: the WIT world it is built against and where it is
/// installed. [`palette::Palette`] and [`widget::Widget`] are the two.
pub trait Kind: Sized + 'static {
    /// What the log calls one, such as "palette plugin".
    const NOUN: &'static str;
    /// The WIT file it is built against, for error messages.
    const WIT: &'static str;
    /// Where its plugins go under each `$XDG_DATA_DIRS` entry, and the
    /// person's under `$XDG_DATA_HOME`.
    const DIR: &'static str;
    /// The bundled plugins, by name, in the order they are asked.
    const BUNDLED: &'static [(&'static str, &'static [u8])];
    /// The world's generated bindings.
    type Bindings;
    /// What `describe` returns.
    type Manifest: fmt::Debug + Send + Sync;

    /// Defines the world's imports, and nothing else.
    ///
    /// # Errors
    ///
    /// As wasmtime's `add_to_linker`.
    fn add_to_linker(linker: &mut Linker<State>) -> wasmtime::Result<()>;

    /// Instantiates `component` as this world.
    ///
    /// # Errors
    ///
    /// If it does not implement the world, or imports anything else.
    fn instantiate(
        store: &mut Store<State>,
        component: &Component,
        linker: &Linker<State>,
    ) -> wasmtime::Result<Self::Bindings>;

    /// Calls `describe`.
    ///
    /// # Errors
    ///
    /// If the plugin traps.
    fn describe(
        bindings: &Self::Bindings,
        store: &mut Store<State>,
    ) -> wasmtime::Result<Self::Manifest>;

    /// The manifest's name.
    fn name(manifest: &Self::Manifest) -> &str;
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
pub struct Plugin<K: Kind> {
    manifest: K::Manifest,
    component: Component,
    origin: Origin,
}

impl<K: Kind> fmt::Debug for Plugin<K> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Plugin")
            .field("manifest", &self.manifest)
            .field("origin", &self.origin)
            .finish_non_exhaustive()
    }
}

impl<K: Kind> Plugin<K> {
    /// What the plugin says it is and may do.
    pub fn manifest(&self) -> &K::Manifest {
        &self.manifest
    }

    /// Its name.
    pub fn name(&self) -> &str {
        K::name(&self.manifest)
    }

    /// How it got in.
    pub fn origin(&self) -> &Origin {
        &self.origin
    }
}

/// The loaded plugins of one kind, and the engine they run in.
pub struct Plugins<K: Kind> {
    runtime: Runtime<K>,
    plugins: Vec<Plugin<K>>,
}

impl<K: Kind> fmt::Debug for Plugins<K> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_list().entries(&self.plugins).finish()
    }
}

impl<K: Kind> Plugins<K> {
    /// The bundled plugins of kind `K`, compiled in parallel, in `runtime`.
    fn with_bundled(runtime: Runtime<K>) -> Result<Self> {
        let compiled = parallel(K::BUNDLED, |&(name, bytes)| {
            load(&runtime, name, bytes, Origin::Bundled)
        });
        let plugins = compiled.into_iter().collect::<Result<Vec<_>>>()?;
        Ok(Self { runtime, plugins })
    }

    /// The loaded plugins, in the order they are asked.
    pub fn iter(&self) -> impl Iterator<Item = &Plugin<K>> {
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

    /// Whether a plugin named `name` is loaded.
    pub fn has(&self, name: &str) -> bool {
        self.plugins.iter().any(|p| p.name() == name)
    }

    /// Loads the system's plugins from [`Kind::DIR`] under each of
    /// `data_dirs` (`$XDG_DATA_DIRS`): only files, in folders, that root
    /// owns and nobody else can write, since such a file is as trusted as
    /// derisk itself. Returns why each refused file was refused.
    pub fn load_system<'a>(
        &mut self,
        data_dirs: impl IntoIterator<Item = &'a Path>,
    ) -> Vec<miette::Report> {
        let mut errors = Vec::new();
        for dir in data_dirs.into_iter().map(|d| d.join(K::DIR)) {
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
        if self.has(plugin.name()) {
            return Err(miette!(
                "a {} named {:?} is already loaded",
                K::NOUN,
                plugin.name()
            ));
        }
        info!(plugin = plugin.name(), origin = %plugin.origin, "loaded {}", K::NOUN);
        self.plugins.push(plugin);
        Ok(())
    }

    /// Runs `ask` for each plugin `which` picks, on scoped threads, and
    /// returns the answers by plugin index. A plugin that fails is logged
    /// and answers `T::default()`.
    pub fn each<T: Default + Send>(
        &self,
        which: impl Fn(usize, &Plugin<K>) -> bool,
        ask: impl Fn(&Plugin<K>) -> Result<T> + Sync,
    ) -> Vec<(usize, T)> {
        let picked: Vec<(usize, &Plugin<K>)> = self
            .plugins
            .iter()
            .enumerate()
            .filter(|&(i, p)| which(i, p))
            .collect();
        parallel(&picked, |&(index, plugin)| {
            let answer = ask(plugin).unwrap_or_else(|error| {
                warn!(
                    plugin = plugin.name(),
                    "{} failed: {}",
                    K::NOUN,
                    reasons(&error)
                );
                T::default()
            });
            (index, answer)
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

/// Compiles one plugin and checks its name.
fn load<K: Kind>(
    runtime: &Runtime<K>,
    label: &str,
    bytes: &[u8],
    origin: Origin,
) -> Result<Plugin<K>> {
    let component = runtime
        .compile(bytes)
        .wrap_err_with(|| format!("cannot compile the {} {label}", K::NOUN))?;
    let manifest = runtime.enter(label, &component, |b, store| K::describe(b, store))?;
    let name = K::name(&manifest);
    if !valid_name(name) {
        return Err(miette!(
            "the {} {label} calls itself {name:?}; a name is 1-32 characters of a-z, 0-9 \
             and -",
            K::NOUN,
        ));
    }
    debug!(plugin = name, ?manifest, "{} manifest", K::NOUN);
    Ok(Plugin {
        manifest,
        component,
        origin,
    })
}

/// The `action` tag of each of `actions` when every one is a JSON object
/// whose tag `allowed` lists; otherwise why not.
fn check_actions<'a>(
    actions: impl IntoIterator<Item = &'a String>,
    allowed: &[String],
) -> std::result::Result<(), String> {
    for action in actions {
        let kind = serde_json::from_str::<Value>(action)
            .ok()
            .and_then(|v| v.get("action")?.as_str().map(str::to_owned))
            .ok_or_else(|| format!("{action:?} is not an action"))?;
        if !allowed.contains(&kind) {
            return Err(format!("action {kind:?} is not in its manifest"));
        }
    }
    Ok(())
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
            "it is {size} bytes, more than the {MAX_PLUGIN_BYTES} a plugin may be"
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
            help = "A system plugin must be owned by root and writable by nobody else; \
                    install your own under $XDG_DATA_HOME, signed.",
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
        let dir = std::env::temp_dir().join(format!("derisk-plugin-signed-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        let (plugins_dir, trust_dir) = (dir.join("plugins"), dir.join("trusted"));
        fs::create_dir_all(&plugins_dir).unwrap();
        fs::create_dir_all(&trust_dir).unwrap();
        let (_, web) = bundled::PALETTE.iter().find(|(n, _)| *n == "web").unwrap();
        let path = plugins_dir.join("web.wasm");
        fs::write(&path, web).unwrap();
        let mut plugins = palette::Plugins::bundled(no_assistant).unwrap();
        let refused = |plugins: &mut palette::Plugins, trust: &TrustStore| {
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
        assert_eq!(plugins.len(), bundled::PALETTE.len());
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_widget_plugin_is_not_a_palette_plugin() {
        let dir = std::env::temp_dir().join(format!("derisk-plugin-kind-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let key = SigningKey::load_or_create(&dir.join("key")).unwrap();
        let mut trust = TrustStore::load(&dir).unwrap();
        trust.add(&key.public_key_hex(), &dir).unwrap();
        let (_, clock) = bundled::WIDGETS
            .iter()
            .find(|(n, _)| *n == "clock")
            .unwrap();
        fs::write(dir.join("clock.wasm"), clock).unwrap();
        let signature = Signature::create(&key, clock).to_yaml().unwrap();
        fs::write(dir.join("clock.wasm.sig"), &signature).unwrap();

        let mut plugins = palette::Plugins::bundled(no_assistant).unwrap();
        let errors = plugins.load_signed(&dir, &trust);
        assert_eq!(errors.len(), 1, "{errors:?}");
        assert!(
            errors[0].to_string().contains("instantiate"),
            "{}",
            errors[0]
        );
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

    #[test]
    fn actions_must_be_tagged_objects_the_manifest_lists() {
        let allowed = ["launch".to_owned()];
        let launch = r#"{"action":"launch","app":"files"}"#.to_owned();
        let lock = r#"{"action":"session","op":"lock"}"#.to_owned();
        assert!(check_actions([&launch], &allowed).is_ok());
        assert!(check_actions([&lock], &allowed).is_err());
        assert!(check_actions([&"launch".to_owned()], &allowed).is_err());
    }
}
