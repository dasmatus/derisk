//! derisk's xdg-desktop-portal backend, `xdg-desktop-portal-derisk`.
//!
//! xdg-desktop-portal is the frontend every Flatpak app talks to; it hands
//! each request to the backend the desktop's `derisk-portals.conf` names.
//! This backend serves what only derisk knows, and answers with derisk's
//! own dialogs ([`derisk_portal_ui`], one `derisk-portal-dialog` process per
//! request, see [`dialog`]):
//!
//! | Interface | From |
//! | --- | --- |
//! | `Settings` (`org.freedesktop.appearance`) | the theme derisk publishes, so GTK 4, Qt, Firefox and Electron apps follow dark mode and the accent |
//! | `Screenshot` | the compositor, over the agent socket; interactive requests show the screenshot dialog (screen, window or area, delay) |
//! | `Wallpaper` | derisk's settings file, which the session watches |
//! | `Background` | which apps have windows, over the agent socket; the background dialog for apps without a stored choice |
//! | `FileChooser` | the file chooser dialog |
//! | `Access` | the access dialog (Camera and other prompts), also used for this backend's own consent ([`access`]) |
//! | `AppChooser` | the "Open With" dialog |
//!
//! Everything else (printing, inhibit, ...) is left to the GTK backend,
//! which `derisk-portals.conf` lists after this one.

#![forbid(unsafe_code)]
#![deny(missing_docs)]

pub mod access;
pub mod agent;
pub mod appearance;
pub mod apps;
pub mod dialog;
pub mod dialogs;
pub mod files;
pub mod portal;

/// The bus name xdg-desktop-portal looks for, from `derisk.portal`.
pub const BUS_NAME: &str = "org.freedesktop.impl.portal.desktop.derisk";
