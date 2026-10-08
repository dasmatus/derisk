//! The assistant: "Ask derisk: open firefox and snap it left", with what
//! derisk's built-in assistant would do, or why it can't.
//!
//! The palette core decides where the row goes: first when the query reads
//! like a request the assistant understands and no row's title holds every
//! word, otherwise after the others, and a request it does not understand
//! only shows when nothing else matched, to say why.

use derisk_palette_sdk::{Category, Entry, Guest, Hook, Manifest, Value, View, export, interpret};

struct Assistant;

impl Guest for Assistant {
    fn describe() -> Manifest {
        Manifest::new(
            "assistant",
            "The built-in assistant's reading of a typed request",
        )
        .hooks(&[Hook::Query])
        .categories(&[Category::Ask])
        // Everything the assistant understands.
        .actions(&[
            "launch",
            "close",
            "focus_next",
            "focus_previous",
            "promote",
            "snap",
            "nudge",
            "tile",
            "float",
            "toggle_maximize",
            "minimize",
            "switch_workspace",
            "move_to_workspace",
            "set_layout",
            "overview",
            "palette",
            "keyboard",
            "session",
        ])
    }

    fn entries(_: View) -> Vec<Entry> {
        Vec::new()
    }

    fn query(_: View, text: String) -> Vec<Entry> {
        ask(&text, interpret)
    }
}

/// The row for `text`, with `interpret` standing in for the assistant.
///
/// Session operations count as confirmed, because the person typed them;
/// the ones that lose work still need the palette's second Enter.
pub fn ask(text: &str, interpret: impl Fn(&str) -> Result<Vec<String>, String>) -> Vec<Entry> {
    let title = format!("Ask derisk: {text}");
    let entry = match interpret(text) {
        Ok(actions) => {
            let mut actions: Vec<Value> = actions
                .iter()
                .filter_map(|a| serde_json_from_str(a))
                .collect();
            let mut confirm = false;
            for action in &mut actions {
                if action["action"] == "session" {
                    confirm |= matches!(
                        action["op"].as_str(),
                        Some("logout" | "reboot" | "power_off")
                    );
                    action["confirmed"] = Value::Bool(true);
                }
            }
            let steps = actions.len();
            Entry::new(Category::Ask, "tool-magic", title, &actions)
                .detail(if steps == 1 {
                    "1 step".to_owned()
                } else {
                    format!("{steps} steps")
                })
                .confirm(confirm)
        }
        Err(reason) => Entry::new(Category::Ask, "tool-magic", title, &[]).detail(reason),
    };
    vec![entry]
}

fn serde_json_from_str(text: &str) -> Option<Value> {
    derisk_palette_sdk::serde_json::from_str(text).ok()
}

export!(Assistant with_types_in derisk_palette_sdk::bindings);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn typed_session_requests_are_confirmed_but_still_ask_twice() {
        let rows = ask("restart", |_| {
            Ok(vec![r#"{"action":"session","op":"reboot"}"#.to_owned()])
        });
        assert!(rows[0].confirm);
        assert_eq!(rows[0].detail, "1 step");
        assert_eq!(rows[0].parsed_actions()[0]["confirmed"], true);
    }

    #[test]
    fn a_request_it_does_not_follow_says_why() {
        let rows = ask("frobnicate", |t| {
            Err(format!("I don't know how to \"{t}\""))
        });
        assert!(rows[0].actions.is_empty());
        assert_eq!(rows[0].detail, "I don't know how to \"frobnicate\"");
    }
}
