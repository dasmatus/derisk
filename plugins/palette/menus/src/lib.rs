//! Menus: the focused app's own menu items, so an app gets palette commands
//! just by registering its menus, then the shell's Window menu for it.
//!
//! The host flattens the menus and says what each item does; this plugin
//! only words the rows.

use derisk_palette_sdk::{Category, Entry, Guest, Hook, Input, Manifest, View, export};

struct Menus;

impl Guest for Menus {
    fn describe() -> Manifest {
        Manifest::new(
            "menus",
            "The focused window's app menus and the shell's Window menu",
        )
        .hooks(&[Hook::Entries])
        .inputs(&[Input::Menus])
        .categories(&[Category::AppCommand, Category::Command])
        // What the shell's Window menu runs, and an app menu's pick.
        .actions(&[
            "activate_menu",
            "minimize",
            "toggle_maximize",
            "close",
            "tile",
            "float",
            "promote",
            "overview",
            "snap",
        ])
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
    view.menus
        .iter()
        .map(|item| {
            let entry = if item.shell {
                let title = if item.path.contains("Snap") {
                    format!("Snap {}", item.label)
                } else {
                    item.label.clone()
                };
                Entry::new(Category::Command, "window-maximize", title, &[])
                    .detail(format!("Window · {}", item.app))
                    .keywords("window")
            } else {
                Entry::new(Category::AppCommand, "open-menu", item.label.clone(), &[])
                    .detail(format!("{} · {}", item.app, item.path))
            };
            Entry {
                actions: item.actions.clone(),
                ..entry.shortcut(item.shortcut.clone())
            }
        })
        .collect()
}

export!(Menus with_types_in derisk_palette_sdk::bindings);

#[cfg(test)]
mod tests {
    use derisk_palette_sdk::MenuCommand;

    use super::*;

    #[test]
    fn snap_items_say_snap() {
        let view = View {
            menus: vec![MenuCommand {
                label: "Left".into(),
                path: "Window › Snap › Left".into(),
                app: "Files".into(),
                shortcut: Some("Super+Left".into()),
                shell: true,
                actions: vec![r#"{"action":"snap","zone":"left"}"#.into()],
            }],
            ..View::default()
        };
        let rows = entries(&view);
        assert_eq!(rows[0].title, "Snap Left");
        assert_eq!(rows[0].detail, "Window · Files");
        assert_eq!(rows[0].shortcut.as_deref(), Some("Super+Left"));
    }
}
