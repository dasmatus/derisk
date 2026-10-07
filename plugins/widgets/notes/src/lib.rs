//! Notes: a scratch pad. derisk keeps what is typed; the plugin only asks
//! for the field and never sees its text.

use derisk_widget_sdk::{Card, Field, Guest, Manifest, Part, Text, View, export};

struct Notes;

impl Guest for Notes {
    fn describe() -> Manifest {
        Manifest::new("notes", "A scratch pad")
    }

    fn render(_: View) -> Vec<Card> {
        vec![card()]
    }
}

/// The card, apart from the export so tests can call it.
pub fn card() -> Card {
    Card::new("notes")
        .text(Text::heading("Notes"))
        .part(Part::Field(Field {
            id: "notes".into(),
            hint: "Write something down".into(),
            lines: 4,
        }))
}

export!(Notes with_types_in derisk_widget_sdk::bindings);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn is_a_heading_over_a_field() {
        assert_eq!(card().texts(), ["Notes", "Write something down"]);
    }
}
