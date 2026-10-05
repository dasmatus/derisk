//! Shows one portal dialog in its own window.
//!
//! ```console
//! $ derisk-portal-dialog --preview NAME   a dialog with sample content
//! $ derisk-portal-dialog --dialog         the request as a JSON line on stdin
//! ```
//!
//! `xdg-desktop-portal-derisk` runs `--dialog` once per request: the request
//! is the first line on stdin, later lines update it (`UpdateChoices`), and
//! the reply is printed as one JSON line on stdout.

use std::process::ExitCode;

use derisk_portal_ui::window;

const USAGE: &str = "usage: derisk-portal-dialog --dialog | --preview NAME";

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let result = match args
        .iter()
        .map(String::as_str)
        .collect::<Vec<_>>()
        .as_slice()
    {
        ["--dialog"] => window::serve_stdio(),
        ["--preview", name] => preview(name),
        ["--help" | "-h"] => {
            println!("{USAGE}");
            println!("dialogs: {}", derisk_portal_ui::SAMPLES.join(", "));
            Ok(())
        }
        _ => Err(USAGE.to_owned()),
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("derisk-portal-dialog: {e}");
            ExitCode::FAILURE
        }
    }
}

fn preview(name: &str) -> Result<(), String> {
    let request = derisk_portal_ui::sample(name).ok_or_else(|| {
        format!(
            "unknown dialog {name}; try one of: {}",
            derisk_portal_ui::SAMPLES.join(", ")
        )
    })?;
    let reply = window::run(request, None)?;
    println!(
        "{}",
        serde_json::to_string(&reply).map_err(|e| e.to_string())?
    );
    Ok(())
}
