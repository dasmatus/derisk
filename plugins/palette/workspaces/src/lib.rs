//! Workspaces: go to another workspace, or send the focused window there or
//! to a new one. Workspaces are dynamic, so they are named by position.

use derisk_palette_sdk::{Category, Entry, Guest, Hook, Input, Manifest, View, export, json};

struct Workspaces;

impl Guest for Workspaces {
    fn describe() -> Manifest {
        Manifest::new(
            "workspaces",
            "Switching workspaces and moving the window between them",
        )
        .hooks(&[Hook::Entries])
        .inputs(&[Input::Workspaces])
        .categories(&[Category::Workspace])
        .actions(&["switch_workspace", "move_to_workspace"])
    }

    fn entries(view: View) -> Vec<Entry> {
        entries(&view)
    }

    fn query(_: View, _: String) -> Vec<Entry> {
        Vec::new()
    }
}

/// `Super+1` to `Super+9`; workspaces past nine have no shortcut.
fn shortcut(chord: &str, n: u32) -> Option<String> {
    (n <= 9).then(|| format!("{chord}{n}"))
}

fn move_window(n: u32, title: String, keywords: &str) -> Entry {
    Entry::new(
        Category::Workspace,
        "go-jump",
        title,
        &[json!({"action": "move_to_workspace", "workspace": n})],
    )
    .keywords(keywords)
    .shortcut(shortcut("Super+Shift+", n))
}

/// The catalog, apart from the export so tests can call it.
pub fn entries(view: &View) -> Vec<Entry> {
    let desk = &view.desk;
    let mut out = Vec::new();
    for ws in desk.workspaces.iter().filter(|ws| !ws.active) {
        let n = ws.number;
        out.push(
            Entry::new(
                Category::Workspace,
                "video-display",
                format!("Go to Workspace {n}"),
                &[json!({"action": "switch_workspace", "workspace": n})],
            )
            .detail(match ws.windows {
                0 => "Empty".to_owned(),
                1 => "1 window".to_owned(),
                c => format!("{c} windows"),
            })
            .keywords("switch desktop")
            .shortcut(shortcut("Super+", n)),
        );
        if desk.window_focused {
            out.push(move_window(
                n,
                format!("Move Window to Workspace {n}"),
                "send throw desktop",
            ));
        }
    }
    if desk.window_focused && desk.can_add_workspace {
        let n = desk.workspaces.len() as u32 + 1;
        out.push(move_window(
            n,
            "Move Window to New Workspace".to_owned(),
            "send throw desktop space add",
        ));
    }
    out
}

export!(Workspaces with_types_in derisk_palette_sdk::bindings);

#[cfg(test)]
mod tests {
    use derisk_palette_sdk::{Desk, Workspace};

    use super::*;

    #[test]
    fn the_active_workspace_is_not_offered() {
        let ws = |number, active| Workspace {
            number,
            windows: 1,
            active,
        };
        let view = View {
            desk: Desk {
                workspaces: vec![ws(1, true), ws(2, false)],
                window_focused: true,
                can_add_workspace: true,
            },
            ..View::default()
        };
        let titles: Vec<_> = entries(&view).into_iter().map(|e| e.title).collect();
        assert_eq!(
            titles,
            [
                "Go to Workspace 2",
                "Move Window to Workspace 2",
                "Move Window to New Workspace"
            ]
        );
    }
}
