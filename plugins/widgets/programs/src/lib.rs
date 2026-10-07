//! Programs: the cards programs register over the agent socket
//! (`register_widget`), each drawn like the built-in ones: a heading with
//! the program's icon, then its text, bars and buttons. A button tells the
//! program it was pressed through `activate_widget`.
//!
//! derisk checks a registration before it gets here (`CustomWidget::check`
//! in `crates/derisk/src/widgets.rs`); a card that does not parse is left
//! out.

use derisk_widget_sdk::{
    Card, Guest, Inline, Input, Manifest, Registration, Text, Tone, Value, View, export, json,
    serde_json,
};

struct Programs;

impl Guest for Programs {
    fn describe() -> Manifest {
        Manifest::new("programs", "Cards programs register over the agent socket")
            .inputs(&[Input::Registered])
            .actions(&["activate_widget"])
    }

    fn render(view: View) -> Vec<Card> {
        cards(&view).collect()
    }
}

/// The cards, apart from the export so tests can call it.
pub fn cards(view: &View) -> impl Iterator<Item = Card> + '_ {
    view.registered.iter().filter_map(card)
}

fn card(registration: &Registration) -> Option<Card> {
    let widget: Value = serde_json::from_str(&registration.data).ok()?;
    let id = registration.id.as_str();
    let str_of = |v: &Value, key: &str| v.get(key).and_then(Value::as_str).map(str::to_owned);
    let mut heading = Vec::new();
    if let Some(icon) = str_of(&widget, "icon") {
        heading.push(Inline::Icon(icon));
    }
    heading.push(Inline::Text(Text::heading(
        str_of(&widget, "title").unwrap_or_default(),
    )));
    let card = Card::new(id).row(heading);
    let rows = widget.get("rows").and_then(Value::as_array);
    Some(rows.into_iter().flatten().fold(card, |card, row| {
        match row.get("type").and_then(Value::as_str) {
            Some("text") => {
                let dim = row.get("dim").and_then(Value::as_bool).unwrap_or(false);
                let text = Text::plain(str_of(row, "text").unwrap_or_default());
                card.text(if dim { text.tone(Tone::Dim) } else { text })
            }
            Some("progress") => {
                #[allow(clippy::cast_possible_truncation)]
                let value = row.get("value").and_then(Value::as_f64).unwrap_or(0.0) as f32;
                card.progress(value.clamp(0.0, 1.0), str_of(row, "label"))
            }
            Some("button") => {
                let item = str_of(row, "item").unwrap_or_default();
                card.row([Inline::button(
                    str_of(row, "label").unwrap_or_default(),
                    &[json!({"action": "activate_widget", "id": id, "item": item})],
                )])
            }
            _ => card,
        }
    }))
}

export!(Programs with_types_in derisk_widget_sdk::bindings);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_registered_card_keeps_its_rows_and_buttons() {
        let view = View {
            registered: vec![Registration {
                id: "weather".into(),
                data: json!({
                    "id": "weather", "title": "Weather", "icon": "weather-clear",
                    "rows": [
                        {"type": "text", "text": "18 °C, clear"},
                        {"type": "progress", "value": 0.4, "label": "Rain 40%"},
                        {"type": "button", "label": "Refresh", "item": "refresh"},
                    ],
                })
                .to_string(),
            }],
            ..View::default()
        };
        let cards: Vec<_> = cards(&view).collect();
        assert_eq!(cards.len(), 1);
        assert_eq!(
            cards[0].texts(),
            ["Weather", "18 °C, clear", "Rain 40%", "Refresh"]
        );
        assert_eq!(
            cards[0].parsed_actions(),
            [json!({"action": "activate_widget", "id": "weather", "item": "refresh"})]
        );
    }
}
