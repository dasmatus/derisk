//! Where derisk finds the plugins beyond its bundled ones, for the palette
//! and the overview alike.

use std::path::{Path, PathBuf};

use derisk_plugin::{Kind, Plugins, TRUST_DIR, TrustStore};
use tracing::{info, warn};

/// `plugins`, the bundled ones, with the system's from `$XDG_DATA_DIRS` and
/// then the person's signed ones from `$XDG_DATA_HOME` added after them.
/// Each refused file is logged.
pub fn with_installed<K: Kind>(mut plugins: Plugins<K>) -> Plugins<K> {
    let started = std::time::Instant::now();
    let data_dirs = std::env::var_os("XDG_DATA_DIRS")
        .filter(|d| !d.is_empty())
        .unwrap_or_else(|| "/usr/local/share:/usr/share".into());
    let data_dirs: Vec<PathBuf> = std::env::split_paths(&data_dirs).collect();
    let mut refused = plugins.load_system(data_dirs.iter().map(PathBuf::as_path));
    let home = std::env::var_os("HOME").map(PathBuf::from);
    let xdg = |var: &str, fallback: &str| {
        std::env::var_os(var)
            .filter(|d| !d.is_empty())
            .map(PathBuf::from)
            .or_else(|| home.as_ref().map(|h| h.join(fallback)))
    };
    if let (Some(data), Some(config)) = (
        xdg("XDG_DATA_HOME", ".local/share"),
        xdg("XDG_CONFIG_HOME", ".config"),
    ) {
        let dir = data.join(K::DIR);
        if dir.is_dir() {
            match trust(&config) {
                Ok(trust) => refused.extend(plugins.load_signed(&dir, &trust)),
                Err(e) => refused.push(e),
            }
        }
    }
    for error in refused {
        warn!("{} refused: {error}", K::NOUN);
    }
    info!(
        plugins = plugins.len(),
        ms = started.elapsed().as_millis() as u64,
        "{}s loaded",
        K::NOUN
    );
    plugins
}

/// The keys trusted to sign the person's plugins.
fn trust(config: &Path) -> miette::Result<TrustStore> {
    TrustStore::load(&config.join(TRUST_DIR))
}
