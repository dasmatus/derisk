//! Services: each failed systemd user unit, with a button to restart it
//! and one to dismiss it, or a line saying all are running.

use derisk_widget_sdk::{Card, Guest, Inline, Input, Manifest, Text, Tone, View, export, json};

struct Services;

impl Guest for Services {
    fn describe() -> Manifest {
        Manifest::new("services", "Failed user services, with restart and dismiss")
            .inputs(&[Input::Units])
            .actions(&["restart_unit", "reset_failed"])
    }

    fn render(view: View) -> Vec<Card> {
        vec![card(&view)]
    }
}

/// The card, apart from the export so tests can call it.
pub fn card(view: &View) -> Card {
    let card = Card::new("services").text(Text::heading("Services"));
    if view.failed_units.is_empty() {
        return card.row([
            Inline::Text(Text::plain("All user services running").tone(Tone::Dim)),
            Inline::Icon("object-select".into()),
        ]);
    }
    view.failed_units.iter().fold(card, |card, unit| {
        card.row([
            Inline::Text(Text::plain(unit.clone()).tone(Tone::Danger)),
            Inline::button(
                "Restart",
                &[json!({"action": "restart_unit", "unit": unit})],
            ),
            Inline::button(
                "Dismiss",
                &[json!({"action": "reset_failed", "unit": unit})],
            ),
        ])
    })
}

export!(Services with_types_in derisk_widget_sdk::bindings);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_failed_unit_can_be_restarted_or_dismissed() {
        assert_eq!(
            card(&View::default()).texts(),
            ["Services", "All user services running"]
        );
        let view = View {
            failed_units: vec!["foo.service".into()],
            ..View::default()
        };
        let card = card(&view);
        assert_eq!(
            card.texts(),
            ["Services", "foo.service", "Restart", "Dismiss"]
        );
        assert_eq!(
            card.parsed_actions(),
            [
                json!({"action": "restart_unit", "unit": "foo.service"}),
                json!({"action": "reset_failed", "unit": "foo.service"}),
            ]
        );
    }
}
