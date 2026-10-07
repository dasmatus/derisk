//! The bindings generated from `wit/palette.wit`.
//!
//! Their records are the crate's public types (a [`crate::View`], an
//! [`crate::Entry`]): they are plain data, and mirroring each in a
//! hand-written twin would only add a copy per palette refresh.

#![allow(clippy::all, clippy::pedantic, missing_docs, unreachable_pub)]

wasmtime::component::bindgen!({
    path: "wit",
    world: "palette-plugin",
    // The palette compares views to tell whether a plugin's rows are stale.
    additional_derives: [PartialEq, Eq, Hash, PartialOrd, Ord],
});
