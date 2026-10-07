//! `xdg-desktop-portal-derisk`: serves the portal backend on the session
//! bus until the bus goes away. D-Bus activates it the first time
//! xdg-desktop-portal asks for `org.freedesktop.impl.portal.desktop.derisk`.

use std::{
    io::IsTerminal,
    sync::{Arc, RwLock},
};

use ashpd::{backend::Builder, zbus};
use derisk_portal::{
    BUS_NAME,
    portal::{self, Apps, Screenshots, Settings, Wallpaper},
};
use miette::{IntoDiagnostic, WrapErr};
use tracing_subscriber::EnvFilter;

#[tokio::main(flavor = "current_thread")]
async fn main() -> miette::Result<()> {
    logging();
    let connection = zbus::connection::Builder::session()
        .into_diagnostic()
        .wrap_err("connecting to the session bus")?
        .build()
        .await
        .into_diagnostic()
        .wrap_err("connecting to the session bus")?;
    let appearance = Arc::new(RwLock::new(None));
    tokio::spawn(portal::watch_theme(
        connection.clone(),
        Arc::clone(&appearance),
    ));
    tokio::spawn(portal::watch_apps(connection.clone()));
    Builder::new(BUS_NAME)
        .into_diagnostic()
        .wrap_err_with(|| format!("serving {BUS_NAME}"))?
        .settings(Settings { appearance })
        .screenshot(Screenshots {
            connection: connection.clone(),
        })
        .wallpaper(Wallpaper {
            connection: connection.clone(),
        })
        .background(Apps)
        // Serves until the bus connection closes.
        .build_with_connection(connection)
        .await
        .into_diagnostic()
        .wrap_err_with(|| format!("serving {BUS_NAME}"))?;
    Ok(())
}

/// Logs `tracing` events to stderr, which D-Bus activation hands to the
/// journal. `RUST_LOG` picks what is logged: `info` and up when it is unset
/// or does not parse.
fn logging() {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .with_writer(std::io::stderr)
        // No colour escapes when stderr is the journal or a pipe.
        .with_ansi(std::io::stderr().is_terminal())
        .init();
}
