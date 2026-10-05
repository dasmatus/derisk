//! The running session's agent socket, for what only the compositor knows:
//! the pixels on screen and which apps have windows.
//!
//! The portal runs as the session user, the only one the socket admits, and
//! speaks the same JSON-lines protocol agents do (`derisk::ipc`): one
//! request line, one `{"ok":...}` line back.

use std::{collections::BTreeMap, path::PathBuf};

use serde_json::{Value, json};
use tokio::{
    io::{AsyncBufReadExt, AsyncWriteExt, BufReader},
    net::UnixStream,
};

/// `$DERISK_AGENT_SOCKET`, else `$XDG_RUNTIME_DIR/derisk/agent.sock`: the
/// same lookup `derisk` itself makes.
pub fn socket_path() -> Option<PathBuf> {
    if let Some(path) = std::env::var_os("DERISK_AGENT_SOCKET") {
        return Some(path.into());
    }
    let dir = std::env::var_os("XDG_RUNTIME_DIR")?;
    Some(PathBuf::from(dir).join("derisk").join("agent.sock"))
}

/// Sends one request and returns its `result`, or the session's error.
pub async fn call(request: Value) -> Result<Value, String> {
    let path = socket_path().ok_or("XDG_RUNTIME_DIR is not set")?;
    let stream = UnixStream::connect(&path)
        .await
        .map_err(|e| format!("cannot reach the derisk session at {}: {e}", path.display()))?;
    let (read, mut write) = stream.into_split();
    let mut line = request.to_string();
    line.push('\n');
    write
        .write_all(line.as_bytes())
        .await
        .map_err(|e| e.to_string())?;
    let mut reply = String::new();
    BufReader::new(read)
        .read_line(&mut reply)
        .await
        .map_err(|e| e.to_string())?;
    result(&reply)
}

/// Unwraps a response line.
pub fn result(line: &str) -> Result<Value, String> {
    let mut reply: Value =
        serde_json::from_str(line).map_err(|e| format!("bad reply from derisk: {e}"))?;
    if reply["ok"] == json!(true) {
        Ok(reply["result"].take())
    } else {
        Err(reply["error"]
            .as_str()
            .unwrap_or("derisk refused the request")
            .to_owned())
    }
}

/// How visible an app is, as the Background portal numbers it.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd)]
pub enum Visibility {
    /// At least one window.
    Running = 1,
    /// Has the keyboard focus.
    Active = 2,
}

/// Each app with a window in a `state` result, at its most visible.
pub fn apps(state: &Value) -> BTreeMap<String, Visibility> {
    let mut apps = BTreeMap::new();
    for window in state["windows"].as_array().into_iter().flatten() {
        let Some(app) = window["app_id"].as_str().filter(|a| !a.is_empty()) else {
            continue;
        };
        let seen = if window["focused"] == json!(true) {
            Visibility::Active
        } else {
            Visibility::Running
        };
        let entry = apps.entry(app.to_owned()).or_insert(seen);
        *entry = (*entry).max(seen);
    }
    apps
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unwraps_results_and_errors() {
        assert_eq!(
            result(r#"{"ok":true,"result":{"path":"/x.png"}}"#).unwrap()["path"],
            "/x.png"
        );
        assert_eq!(
            result(r#"{"ok":false,"error":"no output"}"#).unwrap_err(),
            "no output"
        );
        assert!(result("garbage").is_err());
    }

    #[test]
    fn ranks_each_app_by_its_most_visible_window() {
        let state = json!({"windows": [
            {"app_id": "org.gnome.Maps", "focused": false},
            {"app_id": "org.gnome.Maps", "focused": true},
            {"app_id": "foot", "focused": false},
            {"app_id": "", "focused": false},
        ]});
        let apps = apps(&state);
        assert_eq!(apps["org.gnome.Maps"], Visibility::Active);
        assert_eq!(apps["foot"], Visibility::Running);
        assert_eq!(apps.len(), 2);
    }
}
