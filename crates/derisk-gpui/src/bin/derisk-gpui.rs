//! Opens one derisk core app in a GPUI window.

use std::process::ExitCode;

fn main() -> ExitCode {
    let mut args = std::env::args().skip(1);
    let id = match (args.next(), args.next()) {
        (Some(id), None) if !id.starts_with('-') => id,
        _ => {
            eprintln!("usage: derisk-gpui <app-id>");
            eprintln!("apps: {}", derisk_gpui::APPS.join(", "));
            return ExitCode::from(2);
        }
    };
    // Accept the .desktop file name too, as `derisk launch` does.
    let id = id.strip_suffix(".desktop").unwrap_or(&id);
    match derisk_gpui::run(id) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("derisk-gpui: {error}");
            ExitCode::from(2)
        }
    }
}
