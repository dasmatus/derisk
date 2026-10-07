//! Files: the files and folders the host indexed under home, opened with
//! their default app. The only bundled plugin that sees file names.

use derisk_palette_sdk::{Category, Entry, Guest, Hook, Input, Manifest, View, export, json};

struct Files;

impl Guest for Files {
    fn describe() -> Manifest {
        Manifest::new("files", "Files and folders under home")
            .hooks(&[Hook::Entries])
            .inputs(&[Input::Files])
            .categories(&[Category::File])
            .actions(&["open"])
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
    view.files
        .iter()
        .map(|file| {
            let name = file
                .path
                .trim_end_matches('/')
                .rsplit('/')
                .next()
                .filter(|n| !n.is_empty())
                .unwrap_or(&file.path);
            let icon = if file.folder {
                "folder"
            } else {
                "text-x-generic"
            };
            Entry::new(
                Category::File,
                icon,
                name,
                &[json!({"action": "open", "path": file.path})],
            )
            .detail(file.shown.clone())
        })
        .collect()
}

export!(Files with_types_in derisk_palette_sdk::bindings);

#[cfg(test)]
mod tests {
    use derisk_palette_sdk::File;

    use super::*;

    #[test]
    fn rows_are_named_after_the_file() {
        let view = View {
            files: vec![File {
                path: "/home/me/notes/todo.txt".into(),
                shown: "~/notes/todo.txt".into(),
                folder: false,
            }],
            ..View::default()
        };
        let rows = entries(&view);
        assert_eq!(rows[0].title, "todo.txt");
        assert_eq!(rows[0].detail, "~/notes/todo.txt");
        assert_eq!(rows[0].icon, "text-x-generic");
    }
}
