//! derisk's own dialogs ([`derisk_portal_ui`]), each in a
//! `derisk-portal-dialog` process.
//!
//! The backend itself draws nothing: a dialog runs as a separate Wayland
//! client per request, so a dialog that crashes ends only its request, and
//! ashpd aborting a request (the app called `Request.Close`) drops the child,
//! which kills it.

use std::{
    collections::HashMap,
    path::PathBuf,
    process::Stdio,
    sync::{Arc, Mutex},
};

use derisk_portal_ui::{Reply, Request, Update};
use tokio::{
    io::{AsyncBufReadExt, AsyncWriteExt, BufReader},
    process::{ChildStdin, Command},
};

/// The dialog program: `$DERISK_PORTAL_DIALOG`, else
/// `derisk-portal-dialog` next to this backend, else on `PATH`.
pub fn program() -> PathBuf {
    if let Some(path) = std::env::var_os("DERISK_PORTAL_DIALOG").filter(|p| !p.is_empty()) {
        return path.into();
    }
    std::env::current_exe()
        .ok()
        .and_then(|exe| Some(exe.parent()?.join("derisk-portal-dialog")))
        .filter(|p| p.is_file())
        .unwrap_or_else(|| PathBuf::from("derisk-portal-dialog"))
}

type Inputs = Arc<Mutex<HashMap<String, Arc<tokio::sync::Mutex<ChildStdin>>>>>;

/// Open dialogs, by request token, so `UpdateChoices` can reach them.
#[derive(Clone, Default)]
pub struct Dialogs {
    open: Inputs,
}

/// Forgets a dialog's stdin when its request ends, aborted or not.
struct Forget {
    open: Inputs,
    token: String,
}

impl Drop for Forget {
    fn drop(&mut self) {
        if let Ok(mut open) = self.open.lock() {
            open.remove(&self.token);
        }
    }
}

impl Dialogs {
    /// Shows `request` and waits for the person's answer.
    pub async fn ask(&self, token: &str, request: &Request) -> Result<Reply, String> {
        let line = serde_json::to_string(request).map_err(|e| e.to_string())?;
        let program = program();
        let mut child = Command::new(&program)
            .arg("--dialog")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .kill_on_drop(true)
            .spawn()
            .map_err(|e| format!("cannot start {}: {e}", program.display()))?;
        let (Some(mut stdin), Some(stdout)) = (child.stdin.take(), child.stdout.take()) else {
            return Err("the dialog has no pipes".into());
        };
        stdin
            .write_all(format!("{line}\n").as_bytes())
            .await
            .map_err(|e| format!("cannot send the request: {e}"))?;
        let _forget = Forget {
            open: Arc::clone(&self.open),
            token: token.to_owned(),
        };
        if let Ok(mut open) = self.open.lock() {
            open.insert(token.to_owned(), Arc::new(tokio::sync::Mutex::new(stdin)));
        }
        let reply = BufReader::new(stdout)
            .lines()
            .next_line()
            .await
            .map_err(|e| e.to_string())?
            .ok_or("the dialog closed without answering")?;
        let _ = child.wait().await;
        serde_json::from_str(&reply).map_err(|e| format!("bad reply from the dialog: {e}"))
    }

    /// Sends `update` to the dialog showing request `token`, if any.
    pub async fn update(&self, token: &str, update: &Update) {
        let Some(stdin) = self.open.lock().ok().and_then(|o| o.get(token).cloned()) else {
            return;
        };
        let Ok(line) = serde_json::to_string(update) else {
            return;
        };
        if let Err(e) = stdin
            .lock()
            .await
            .write_all(format!("{line}\n").as_bytes())
            .await
        {
            eprintln!("xdg-desktop-portal-derisk: cannot update the dialog: {e}");
        }
    }
}
