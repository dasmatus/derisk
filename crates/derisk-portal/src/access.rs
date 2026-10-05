//! Asking the person before a portal acts.
//!
//! The frontend leaves some consent to the backend: a wallpaper with a
//! preview is supposed to show the person something before it happens.
//! Doing it silently would let any app change the background by setting one
//! flag, so this backend asks with derisk's own access dialog
//! ([`derisk_portal_ui::access`]), the same one it serves as
//! `org.freedesktop.impl.portal.Access`. Setting `$DERISK_PORTAL_ACCESS` to
//! another backend's bus name (the GTK backend's, say) asks through that
//! backend's `Access` dialog instead.

use std::collections::HashMap;

use ashpd::zbus::{
    self,
    zvariant::{ObjectPath, OwnedValue, Value},
};
use derisk_portal_ui::{Reply, Request};

use crate::dialog::Dialogs;

/// Another backend whose access dialog to use instead of derisk's:
/// `$DERISK_PORTAL_ACCESS`, if set.
pub fn backend() -> Option<String> {
    std::env::var("DERISK_PORTAL_ACCESS")
        .ok()
        .filter(|b| !b.is_empty())
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
    dialogs: &Dialogs,
    token: &str,
    q: &Question<'_>,
) -> Result<bool, String> {
    match backend() {
        Some(backend) => ask_backend(connection, &backend, token, q)
            .await
            .map_err(|e| e.to_string()),
        None => {
            let mut request = crate::dialogs::access_request(
                q.app_id,
                q.title.to_owned(),
                q.subtitle.to_owned(),
                q.body.to_owned(),
                None,
            )
            .await;
            request.grant_label = Some(q.grant_label.to_owned());
            let reply = dialogs.ask(token, &Request::Access(request)).await?;
            Ok(matches!(reply, Reply::Access(_)))
        }
    }
}

/// [`ask`] through `backend`'s `org.freedesktop.impl.portal.Access`.
async fn ask_backend(
    connection: &zbus::Connection,
    backend: &str,
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
            Some(backend),
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

/// Closes another backend's dialog [`ask`] is still showing for `token`,
/// when the app gives up on its request. derisk's own dialog closes when
/// ashpd drops the request.
pub async fn close(connection: &zbus::Connection, token: &str) {
    let Some(backend) = backend() else {
        return;
    };
    let _ = connection
        .call_method(
            Some(backend.as_str()),
            handle(token).as_str(),
            Some("org.freedesktop.impl.portal.Request"),
            "Close",
            &(),
        )
        .await;
}
