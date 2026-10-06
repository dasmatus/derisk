//! Geometry, Windows-style snapping and server-side window decorations:
//! the layout primitives the derisk shell places windows with.
//!
//! - [`geom`]: helpers around mcsapi's logical geometry.
//! - [`snap`]: edge and corner drop zones and Super+arrow nudging.
//! - [`decorations`]: title bars with the window buttons on the left.

#![forbid(unsafe_code)]
#![deny(missing_docs)]

pub mod decorations;
pub mod geom;
pub mod snap;
