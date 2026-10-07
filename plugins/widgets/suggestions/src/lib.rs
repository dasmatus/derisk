//! Suggested: the apps the person usually opens around this hour, each
//! by its icon and name, which derisk fills in from the app's ID.

use derisk_widget_sdk::{Card, Guest, Inline, Input, Manifest, Text, Tone, View, export, json};

struct Suggestions;

impl Guest for Suggestions {
    fn describe() -> Manifest {
        Manifest::new("suggestions", "Apps you usually open around now")
            .inputs(&[Input::Apps])
            .actions(&["launch", "overview"])
    }

    fn render(view: View) -> Vec<Card> {
        vec![card(&view)]
    }
}

/// How many apps it suggests.
const SHOWN: usize = 5;

/// The card, apart from the export so tests can call it.
pub fn card(view: &View) -> Card {
    let card = Card::new("suggestions").text(Text::heading("Suggested"));
    if view.suggested.is_empty() {
        return card.text(Text::plain("Apps you use will appear here.").tone(Tone::Dim));
    }
    card.flow(view.suggested.iter().take(SHOWN).map(|app| {
        Inline::app(
            app.clone(),
            &[
                json!({"action": "launch", "app": app}),
                json!({"action": "overview", "visible": false}),
            ],
        )
    }))
}

export!(Suggestions with_types_in derisk_widget_sdk::bindings);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn suggests_the_first_five_and_launches_them_from_the_overview() {
        assert_eq!(
            card(&View::default()).texts(),
            ["Suggested", "Apps you use will appear here."]
        );
        let view = View {
            suggested: (0..7).map(|i| format!("app{i}")).collect(),
            ..View::default()
        };
        let card = card(&view);
        let actions = card.parsed_actions();
        assert_eq!(actions.len(), 10);
        assert_eq!(actions[0], json!({"action": "launch", "app": "app0"}));
        assert_eq!(actions[1], json!({"action": "overview", "visible": false}));
    }
}
