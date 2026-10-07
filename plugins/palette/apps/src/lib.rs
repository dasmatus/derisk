//! Apps: every installed app, its desktop actions, and the programs the
//! person tends to start at this hour.
//!
//! Suggested apps come first, so they lead an empty query. A suggested
//! program with no desktop file still gets a row, so a habit formed in a
//! terminal shows up here too. Desktop actions ("New Private Window") only
//! show once something is typed; the palette core decides that.

use derisk_palette_sdk::{Category, Entry, Guest, Hook, Input, Manifest, View, export, json};

struct Apps;

impl Guest for Apps {
    fn describe() -> Manifest {
        Manifest::new("apps", "Installed apps and their desktop actions")
            .hooks(&[Hook::Entries])
            .inputs(&[Input::Apps])
            .categories(&[Category::App, Category::AppAction])
            .actions(&["launch", "launch_action"])
    }

    fn entries(view: View) -> Vec<Entry> {
        entries(&view)
    }

    fn query(_: View, _: String) -> Vec<Entry> {
        Vec::new()
    }
}

/// The catalog, apart from the export so tests can call it.
pub fn entries(view: &View) -> Vec<Entry> {
    let rank = |id: &str| {
        view.suggested
            .iter()
            .position(|s| s == id)
            .unwrap_or(usize::MAX)
    };
    let mut apps: Vec<_> = view.apps.iter().collect();
    // Stable, so apps nobody suggested keep the host's order.
    apps.sort_by_key(|app| rank(&app.id));
    let mut out: Vec<Entry> = apps
        .iter()
        .map(|app| {
            Entry::new(
                Category::App,
                &app.icon,
                app.name.clone(),
                &[json!({"action": "launch", "app": app.id})],
            )
            .detail(app.summary.clone())
            .keywords(app.keywords.clone())
        })
        .collect();
    // Suggested programs without a desktop file (plain commands).
    out.extend(
        view.suggested
            .iter()
            .filter(|id| !view.apps.iter().any(|app| &app.id == *id))
            .map(|id| {
                Entry::new(
                    Category::App,
                    "🖥",
                    id.clone(),
                    &[json!({"action": "launch", "app": id})],
                )
                .detail("Suggested")
                .keywords(format!("{id} "))
            }),
    );
    out.extend(view.apps.iter().flat_map(|app| {
        app.actions.iter().map(|action| {
            Entry::new(
                Category::AppAction,
                &app.icon,
                action.name.clone(),
                &[json!({"action": "launch_action", "app": app.id, "id": action.id})],
            )
            .detail(app.name.clone())
            .keywords(format!("{} {}", app.name, action.id).to_lowercase())
        })
    }));
    out
}

export!(Apps with_types_in derisk_palette_sdk::bindings);

#[cfg(test)]
mod tests {
    use derisk_palette_sdk::{App, AppAction};

    use super::*;

    fn app(id: &str, name: &str) -> App {
        App {
            id: id.into(),
            name: name.into(),
            summary: String::new(),
            icon: "🖥".into(),
            keywords: id.into(),
            actions: Vec::new(),
        }
    }

    #[test]
    fn suggested_apps_lead_and_unknown_programs_get_a_row() {
        let mut firefox = app("firefox.desktop", "Firefox");
        firefox.actions.push(AppAction {
            id: "new-private-window".into(),
            name: "New Private Window".into(),
        });
        let view = View {
            apps: vec![app("files", "Files"), firefox],
            suggested: vec!["firefox.desktop".into(), "htop".into()],
            ..View::default()
        };
        let titles: Vec<_> = entries(&view).into_iter().map(|e| e.title).collect();
        assert_eq!(titles, ["Firefox", "Files", "htop", "New Private Window"]);
    }
}
