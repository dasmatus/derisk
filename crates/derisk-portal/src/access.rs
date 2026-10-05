//! Asking the person, through another backend's access dialog.
//!
//! derisk has no dialog of its own a portal can open yet, but the frontend
//! leaves some consent to the backend: an interactive screenshot and a
//! wallpaper with a preview are both supposed to show the person something
//! before they happen. Doing them silently would let any app take the
//! screen or change the background by setting one flag, so this backend
//! asks through the `org.freedesktop.impl.portal.Access` dialog of the GTK
//! backend, the same dialog the frontend itself uses for the
//! non-interactive cases.

use std::collections::HashMap;

use ashpd::zbus::{
    self,
    zvariant::{ObjectPath, OwnedValue, Value},
};

/// The backend whose access dialog is used: `$DERISK_PORTAL_ACCESS`, else
/// the GTK backend's bus name.
pub fn backend() -> String {
    std::env::var("DERISK_PORTAL_ACCESS")
        .unwrap_or_else(|_| "org.freedesktop.impl.portal.desktop.gtk".to_owned())
}

/// The request object path the dialog is shown under, unique per portal
/// request so `close` can find it.
pub fn handle(token: &str) -> String {
    format!("/org/freedesktop/portal/desktop/request/derisk/{token}")
}

/// What to ask.
pub struct Question<'a> {
    /// The asking app's ID, empty for a host app.
    pub app_id: &'a str,
    /// The app's window, `wayland:<handle>` or empty.
    pub parent_window: &'a str,
    /// The dialog's heading.
    pub title: &'a str,
    /// The line under it.
    pub subtitle: &'a str,
    /// Smaller print.
    pub body: &'a str,
    /// The label on the button that says yes.
    pub grant_label: &'a str,
}

/// Shows the dialog and waits: `Ok(true)` only when the person grants it.
pub async fn ask(
    connection: &zbus::Connection,
    token: &str,
    q: &Question<'_>,
) -> zbus::Result<bool> {
    let handle = handle(token);
    let handle = ObjectPath::try_from(handle.as_str())?;
    let mut options: HashMap<&str, Value<'_>> = HashMap::new();
    options.insert("modal", Value::from(true));
    options.insert("grant_label", Value::from(q.grant_label));
    let reply = connection
        .call_method(
            Some(backend().as_str()),
            "/org/freedesktop/portal/desktop",
            Some("org.freedesktop.impl.portal.Access"),
            "AccessDialog",
            &(
                handle,
                q.app_id,
                q.parent_window,
                q.title,
                q.subtitle,
                q.body,
                options,
            ),
        )
        .await?;
    let (response, _results): (u32, HashMap<String, OwnedValue>) = reply.body().deserialize()?;
    Ok(response == 0)
}

/// Closes a dialog [`ask`] is still showing for `token`, when the app
/// gives up on its request.
pub async fn close(connection: &zbus::Connection, token: &str) {
    let _ = connection
        .call_method(
            Some(backend().as_str()),
            handle(token).as_str(),
            Some("org.freedesktop.impl.portal.Request"),
            "Close",
            &(),
        )
        .await;
}
