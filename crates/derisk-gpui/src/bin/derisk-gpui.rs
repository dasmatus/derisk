//! Opens one derisk core app in a GPUI window.

use std::io::IsTerminal;

use tracing_subscriber::EnvFilter;

/// `derisk-gpui` takes exactly one app ID.
#[derive(Debug, thiserror::Error, miette::Diagnostic)]
#[error("usage: derisk-gpui <app-id>")]
#[diagnostic(code(derisk_gpui::usage))]
struct Usage {
    #[help]
    apps: String,
}

fn main() -> miette::Result<()> {
    logging();
    let mut args = std::env::args().skip(1);
    let id = match (args.next(), args.next()) {
        (Some(id), None) if !id.starts_with('-') => id,
        _ => {
            return Err(Usage {
                apps: format!("apps: {}", derisk_gpui::APPS.join(", ")),
            }
            .into());
        }
    };
    // Accept the .desktop file name too, as `derisk launch` does.
    let id = id.strip_suffix(".desktop").unwrap_or(&id);
    derisk_gpui::run(id).map_err(|error| miette::miette!("{error}"))
}

/// Logs `tracing` events to stderr, which the session's unit or the
/// compositor that spawned this hands to the journal. `RUST_LOG` picks what
/// is logged: `info` and up when it is unset or does not parse.
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
