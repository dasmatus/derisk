//! `xdg-desktop-portal-derisk`: serves the portal backend on the session
//! bus until the bus goes away. D-Bus activates it the first time
//! xdg-desktop-portal asks for `org.freedesktop.impl.portal.desktop.derisk`.

use std::{
    process::ExitCode,
    sync::{Arc, RwLock},
};

use ashpd::{backend::Builder, zbus};
use derisk_portal::{
    BUS_NAME,
    portal::{self, Apps, Screenshots, Settings, Wallpaper},
};

#[tokio::main(flavor = "current_thread")]
async fn main() -> ExitCode {
    match run().await {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("xdg-desktop-portal-derisk: {error}");
            ExitCode::FAILURE
        }
    }
}

async fn run() -> Result<(), Box<dyn std::error::Error>> {
    let connection = zbus::connection::Builder::session()?.build().await?;
    let appearance = Arc::new(RwLock::new(None));
    tokio::spawn(portal::watch_theme(
        connection.clone(),
        Arc::clone(&appearance),
    ));
    tokio::spawn(portal::watch_apps(connection.clone()));
    Builder::new(BUS_NAME)?
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
        .await?;
    Ok(())
}
