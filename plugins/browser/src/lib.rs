//! The browser: Danube is a bare web view, and its tabs, address entry and
//! extensions live here, in the palette.
//!
//! Danube publishes what it has over derisk's agent socket and acts on what
//! is picked:
//!
//! ```text
//! -> {"method":"register_palette","source":"danube","data":{
//!       "tabs":[{"id":"3","title":"Example","url":"https://example.com","active":true}],
//!       "extensions":[{"id":"bitwarden","name":"Bitwarden","enabled":true,"options":true}]}}
//! <- {"event":"palette","source":"danube","command":{"op":"activate_tab","id":"3"}}
//! ```
//!
//! Commands: `activate_tab`, `close_tab` (with `id`), `new_tab`, `open`
//! (with `url`), `enable_extension`, `disable_extension` and
//! `extension_options` (with `id`).
//!
//! An address typed into the palette opens in Danube when it is running,
//! and with the default browser otherwise.

use derisk_palette_sdk::{
    Category, Entry, Guest, Hook, Input, Manifest, Position, Value, View, export, json,
};

/// The program whose registration this plugin reads.
const SOURCE: &str = "danube";

struct Browser;

impl Guest for Browser {
    fn describe() -> Manifest {
        Manifest::new("browser", "A browser's tabs, address entry and extensions")
            .hooks(&[Hook::Entries, Hook::Query])
            .inputs(&[Input::Registered])
            .sources(&[SOURCE])
            .categories(&[
                Category::Tab,
                Category::Extension,
                Category::Command,
                Category::Web,
            ])
            .actions(&["palette_command", "open_url"])
    }

    fn entries(view: View) -> Vec<Entry> {
        entries(&view)
    }

    fn query(view: View, text: String) -> Vec<Entry> {
        query(&view, &text).into_iter().collect()
    }
}

/// What Danube registered, if it is running.
fn registered(view: &View) -> Option<Value> {
    let registration = view.registered.iter().find(|r| r.source == SOURCE)?;
    derisk_palette_sdk::serde_json::from_str(&registration.data).ok()
}

fn command(command: Value) -> Value {
    json!({"action": "palette_command", "source": SOURCE, "command": command})
}

fn text<'a>(value: &'a Value, key: &str) -> &'a str {
    value[key].as_str().unwrap_or_default()
}

/// The catalog: Danube's tabs, a new tab, and its extensions.
pub fn entries(view: &View) -> Vec<Entry> {
    let Some(data) = registered(view) else {
        return Vec::new();
    };
    let none = Vec::new();
    let tabs = data["tabs"].as_array().unwrap_or(&none);
    let extensions = data["extensions"].as_array().unwrap_or(&none);
    let mut out = Vec::new();
    for tab in tabs {
        let id = text(tab, "id");
        let url = text(tab, "url");
        let title = match text(tab, "title") {
            "" => url,
            title => title,
        };
        if id.is_empty() {
            continue;
        }
        let mut detail = url.to_owned();
        if tab["active"].as_bool() == Some(true) {
            detail.push_str(" · current tab");
        }
        out.push(
            Entry::new(
                Category::Tab,
                "web-browser",
                title,
                &[command(json!({"op": "activate_tab", "id": id}))],
            )
            .detail(detail)
            .keywords(format!("tab switch {url}")),
        );
        out.push(
            Entry::new(
                Category::Tab,
                "window-close",
                format!("Close Tab: {title}"),
                &[command(json!({"op": "close_tab", "id": id}))],
            )
            .detail(url)
            .keywords("tab close"),
        );
    }
    out.push(
        Entry::new(
            Category::Command,
            "tab-new",
            "New Tab",
            &[command(json!({"op": "new_tab"}))],
        )
        .detail("Danube")
        .keywords("browser tab web open"),
    );
    for extension in extensions {
        let id = text(extension, "id");
        let name = text(extension, "name");
        if id.is_empty() || name.is_empty() {
            continue;
        }
        let enabled = extension["enabled"].as_bool() == Some(true);
        let (verb, op) = if enabled {
            ("Disable", "disable_extension")
        } else {
            ("Enable", "enable_extension")
        };
        out.push(
            Entry::new(
                Category::Extension,
                "application-x-addon",
                format!("{verb} {name}"),
                &[command(json!({"op": op, "id": id}))],
            )
            .detail("Danube extension")
            .keywords(format!("extension addon {id}")),
        );
        if extension["options"].as_bool() == Some(true) {
            out.push(
                Entry::new(
                    Category::Extension,
                    "preferences-system",
                    format!("{name} Options"),
                    &[command(json!({"op": "extension_options", "id": id}))],
                )
                .detail("Danube extension")
                .keywords(format!("extension addon settings preferences {id}")),
            );
        }
    }
    out
}

/// The address `text` names, if it reads like one: a URL with a scheme
/// derisk opens, `localhost` with an optional port and path, or a host name
/// with a dot and a top-level domain of letters.
pub fn address(text: &str) -> Option<String> {
    if text.is_empty() || text.chars().any(|c| c.is_whitespace() || c.is_control()) {
        return None;
    }
    let lower = text.to_ascii_lowercase();
    if lower.starts_with("https://") || lower.starts_with("http://") {
        return (text.len() > lower.find("://")? + 3).then(|| text.to_owned());
    }
    let host_port = text.split(['/', '?', '#']).next()?;
    let host = host_port.split(':').next()?;
    if let Some(port) = host_port
        .strip_prefix(host)
        .and_then(|p| p.strip_prefix(':'))
        && !(1..=5).contains(&port.len()) | !port.bytes().all(|b| b.is_ascii_digit())
    {
        return None;
    }
    if host.eq_ignore_ascii_case("localhost") {
        return Some(format!("http://{text}"));
    }
    let labels: Vec<&str> = host.split('.').collect();
    let valid = labels.len() >= 2
        && labels
            .iter()
            .all(|l| !l.is_empty() && l.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-'))
        && labels
            .last()
            .is_some_and(|tld| tld.len() >= 2 && tld.bytes().all(|b| b.is_ascii_alphabetic()));
    valid.then(|| format!("https://{text}"))
}

/// The row opening a typed address. One with a scheme, or `localhost`, is
/// plainly an address and goes first; a bare `name.tld` could as well be a
/// file being searched for, so it goes after what the catalog matched.
pub fn query(view: &View, text: &str) -> Option<Entry> {
    let url = address(text)?;
    let plain = text.contains("://") || text.to_ascii_lowercase().starts_with("localhost");
    let (action, detail) = if registered(view).is_some() {
        (command(json!({"op": "open", "url": url})), "Danube")
    } else {
        (json!({"action": "open_url", "url": url}), "Default browser")
    };
    Some(
        Entry::new(
            Category::Web,
            "web-browser",
            format!("Open {url}"),
            &[action],
        )
        .detail(detail)
        .at(if plain {
            Position::Top
        } else {
            Position::Bottom
        }),
    )
}

export!(Browser with_types_in derisk_palette_sdk::bindings);

#[cfg(test)]
mod tests {
    use derisk_palette_sdk::Registration;

    use super::*;

    fn danube() -> View {
        View {
            registered: vec![Registration {
                source: SOURCE.into(),
                data: json!({
                    "tabs": [
                        {"id": "1", "title": "Example", "url": "https://example.com", "active": true},
                        {"id": "2", "title": "", "url": "https://rust-lang.org"},
                    ],
                    "extensions": [
                        {"id": "bitwarden", "name": "Bitwarden", "enabled": true, "options": true},
                    ],
                })
                .to_string(),
            }],
            ..View::default()
        }
    }

    #[test]
    fn tabs_and_extensions_become_rows() {
        let titles: Vec<_> = entries(&danube()).into_iter().map(|e| e.title).collect();
        assert_eq!(
            titles,
            [
                "Example",
                "Close Tab: Example",
                "https://rust-lang.org",
                "Close Tab: https://rust-lang.org",
                "New Tab",
                "Disable Bitwarden",
                "Bitwarden Options",
            ]
        );
        assert!(entries(&View::default()).is_empty());
    }

    #[test]
    fn addresses_are_recognised() {
        assert_eq!(
            address("example.com").as_deref(),
            Some("https://example.com")
        );
        assert_eq!(
            address("localhost:8080/x").as_deref(),
            Some("http://localhost:8080/x")
        );
        assert_eq!(address("HTTP://a.b").as_deref(), Some("HTTP://a.b"));
        for not in [
            "files",
            "open firefox",
            "v1.2",
            "a.b:port",
            "notes.txt2",
            "https://",
        ] {
            assert_eq!(address(not), None, "{not}");
        }
    }

    #[test]
    fn an_address_opens_in_danube_when_it_runs() {
        let row = query(&danube(), "example.com").unwrap();
        assert_eq!(row.parsed_actions()[0]["command"]["op"], "open");
        let row = query(&View::default(), "example.com").unwrap();
        assert_eq!(row.parsed_actions()[0]["action"], "open_url");
    }
}
