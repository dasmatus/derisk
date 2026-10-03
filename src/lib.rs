//! derisk: an adaptive, agent-first Wayland desktop shell built on
//! [mcsapi](https://github.com/dasmatus/mcsapi).
//!
//! mcsapi supplies workspaces, focus and tiling policy in Smithay's logical
//! coordinates. derisk layers the rest of a desktop on top:
//!
//! - [`shell`]: tiling plus floating, snapped and minimized windows, and
//!   Windows-style dragging with edge/corner snapping and Snap Assist.
//! - [`decorations`]: server-side title bars with the buttons on the left.
//! - [`adaptive`]: phone/tablet/desktop profiles and learned app suggestions.
//! - [`overview`]: the overview grid and its widgets.
//! - [`desktop`]: installed apps and their actions from `.desktop` files.
//! - [`menu`]: the global menu.
//! - [`palette`]: the command palette (Super+Space), the center of the
//!   workflow: apps, windows, commands, settings and files in one search,
//!   with the assistant as the fallback.
//! - [`tray`]: monochrome system tray icons.
//! - [`animation`]: the startup animation.
//! - [`assistant`] and [`ipc`]: natural-language and agent control.
//! - [`keys`]: keyboard shortcuts.
//! - [`systemd`]: apps as transient units, sd_notify, socket activation,
//!   journald, logind session actions and focus-aware resource weights.
//! - [`ui`]: egui rendering of all shell surfaces.
//!
//! ```
//! use derisk::{action::Action, geom::rect, shell::Shell, snap::SnapZone};
//!
//! let mut shell = Shell::new(rect(0, 0, 1920, 1080), false);
//! let (editor, _) = shell.map_window("editor", "notes.txt");
//! shell.map_window("terminal", "~");
//! shell.apply(Action::Snap { window: Some(editor.get()), zone: SnapZone::Left })?;
//! assert_eq!(shell.placements().len(), 2);
//! # Ok::<(), derisk::shell::Error>(())
//! ```

#![forbid(unsafe_code)]
#![deny(missing_docs)]

pub mod action;
pub mod adaptive;
pub mod animation;
pub mod assistant;
pub mod decorations;
pub mod desktop;
pub mod geom;
pub mod ipc;
pub mod keys;
pub mod menu;
pub mod overview;
pub mod palette;
pub mod shell;
pub mod snap;
pub mod systemd;
pub mod time;
pub mod tray;
pub mod ui;

pub use mcsapi;
