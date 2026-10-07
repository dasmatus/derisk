//! Windows: every open window on every workspace, to switch to. A minimized
//! window is restored rather than focused.

use derisk_palette_sdk::{Category, Entry, Guest, Hook, Input, Manifest, View, export, json};

struct Windows;

impl Guest for Windows {
    fn describe() -> Manifest {
        Manifest::new("windows", "Open windows on every workspace")
            .hooks(&[Hook::Entries])
            .inputs(&[Input::Windows])
            .categories(&[Category::Window])
            .actions(&["focus", "restore"])
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
    view.windows
        .iter()
        .map(|w| {
            let action = if w.minimized {
                json!({"action": "restore", "window": w.id})
            } else {
                json!({"action": "focus", "window": w.id})
            };
            let name = if w.title.is_empty() { &w.app } else { &w.title };
            let mut detail = format!("{} · workspace {}", w.app, w.workspace);
            if w.minimized {
                detail.push_str(" · minimized");
            }
            if w.focused {
                detail.push_str(" · focused");
            }
            Entry::new(Category::Window, "view-restore", name.clone(), &[action])
                .detail(detail)
                .keywords("window switch")
        })
        .collect()
}

export!(Windows with_types_in derisk_palette_sdk::bindings);

#[cfg(test)]
mod tests {
    use derisk_palette_sdk::Window;

    use super::*;

    #[test]
    fn minimized_windows_are_restored() {
        let view = View {
            windows: vec![Window {
                id: 7,
                app: "Files".into(),
                title: String::new(),
                workspace: 2,
                minimized: true,
                focused: false,
            }],
            ..View::default()
        };
        let rows = entries(&view);
        assert_eq!(rows[0].title, "Files");
        assert_eq!(rows[0].detail, "Files · workspace 2 · minimized");
        assert_eq!(rows[0].parsed_actions()[0]["action"], "restore");
    }
}
