//! Sonne's agent: a request the built-in assistant does not understand is
//! offered to the agent set up in Sonne, which can do far more with the
//! model key or agent CLI the person installed there. It only opens the
//! request in Sonne's agent panel, for the person to send.

use derisk_palette_sdk::{
    Category, Entry, Guest, Hook, Manifest, Position, View, export, interpret, json,
};

struct Agent;

impl Guest for Agent {
    fn describe() -> Manifest {
        Manifest::new(
            "agent",
            "Handing a request the assistant cannot follow to Sonne's agent",
        )
        .hooks(&[Hook::Query])
        .categories(&[Category::Agent])
        .actions(&["ask_agent"])
    }

    fn entries(_: View) -> Vec<Entry> {
        Vec::new()
    }

    fn query(_: View, text: String) -> Vec<Entry> {
        query(&text, |t| interpret(t).is_ok()).into_iter().collect()
    }
}

/// The row for `text`, unless `understood` says the assistant has it.
pub fn query(text: &str, understood: impl Fn(&str) -> bool) -> Option<Entry> {
    if text.is_empty() || understood(text) {
        return None;
    }
    Some(
        Entry::new(
            Category::Agent,
            "tool-magic",
            format!("Ask Sonne's agent: {text}"),
            &[json!({"action": "ask_agent", "text": text})],
        )
        .detail("Opens in Sonne, with the model or agent CLI set up there")
        .at(Position::Bottom),
    )
}

export!(Agent with_types_in derisk_palette_sdk::bindings);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_what_the_assistant_cannot_do() {
        assert!(query("open files", |_| true).is_none());
        let row = query("frobnicate the flux", |_| false).unwrap();
        assert_eq!(row.title, "Ask Sonne's agent: frobnicate the flux");
    }
}
