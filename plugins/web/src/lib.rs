//! Web search: once a search engine has been chosen (Settings, Default
//! apps), the last row of every query searches the web for it, for what
//! nothing on the computer matches.

use derisk_palette_sdk::{
    Category, Entry, Guest, Hook, Input, Manifest, Position, View, export, json,
};

struct Web;

impl Guest for Web {
    fn describe() -> Manifest {
        Manifest::new("web", "Searching the web with the chosen engine")
            .hooks(&[Hook::Query])
            .inputs(&[Input::SearchEngine])
            .categories(&[Category::Web])
            .actions(&["search_web"])
    }

    fn entries(_: View) -> Vec<Entry> {
        Vec::new()
    }

    fn query(view: View, text: String) -> Vec<Entry> {
        query(&view, &text).into_iter().collect()
    }
}

/// The row for `text`, when an engine is chosen.
pub fn query(view: &View, text: &str) -> Option<Entry> {
    let engine = view.search_engine.as_ref().filter(|_| !text.is_empty())?;
    Some(
        Entry::new(
            Category::Web,
            "system-search",
            format!("Search the web for “{text}”"),
            &[json!({"action": "search_web", "query": text})],
        )
        .detail(engine.clone())
        .at(Position::Bottom),
    )
}

export!(Web with_types_in derisk_palette_sdk::bindings);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nothing_without_an_engine() {
        assert!(query(&View::default(), "rust").is_none());
        let view = View {
            search_engine: Some("DuckDuckGo".into()),
            ..View::default()
        };
        assert_eq!(query(&view, "rust").unwrap().detail, "DuckDuckGo");
    }
}
