//! The bindings generated from `wit/palette/palette.wit` and
//! `wit/widget/widget.wit`.
//!
//! Their records are the crate's public types (a [`crate::palette::View`],
//! a [`crate::widget::Card`]): they are plain data, and mirroring each in a
//! hand-written twin would only add a copy per refresh.

#![allow(clippy::all, clippy::pedantic, missing_docs, unreachable_pub)]

pub mod palette {
    wasmtime::component::bindgen!({
        path: "wit/palette",
        world: "palette-plugin",
        // The palette compares views to tell whether a plugin's rows are
        // stale.
        additional_derives: [PartialEq, Eq, Hash, PartialOrd, Ord],
    });
}

pub mod widget {
    wasmtime::component::bindgen!({
        path: "wit/widget",
        world: "widget-plugin",
        // The overview compares views to tell whether a plugin's cards are
        // stale. A card holds an `f32`, so nothing past `PartialEq`.
        additional_derives: [PartialEq],
    });
}
