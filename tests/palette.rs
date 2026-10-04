use std::fs;

use derisk::{
    action::Action,
    geom::rect,
    keys::{self, Key, Mods},
    menu::{Menu, MenuEntry},
    palette::{self, Category, Entry, History, Scope},
    shell::{Error, Shell},
    systemd::SessionOp,
};

fn shell() -> Shell {
    Shell::new(rect(0, 0, 1920, 1080), false)
}

fn apps() -> Vec<Entry> {
    vec![
        Entry::app(
            "org.derisk.files",
            "Files",
            "Browse files",
            "🗀",
            &["folders"],
        ),
        Entry::app(
            "org.derisk.editor",
            "Text Editor",
            "Edit plain-text files",
            "📝",
            &["notepad"],
        ),
    ]
}

fn titles(entries: &[Entry], hits: &[usize]) -> Vec<String> {
    hits.iter().map(|&i| entries[i].title.clone()).collect()
}

#[test]
fn super_space_toggles_the_palette() {
    let super_ = Mods {
        logo: true,
        ..Mods::default()
    };
    assert_eq!(
        keys::binding(super_, Key::Space),
        Some(Action::Palette { visible: None })
    );
    let mut shell = shell();
    assert!(!shell.palette_visible());
    shell.apply(Action::Palette { visible: None }).unwrap();
    assert!(shell.palette_visible());
    shell
        .apply(Action::Palette {
            visible: Some(false),
        })
        .unwrap();
    assert!(!shell.palette_visible());
}

#[test]
fn fuzzy_prefers_prefixes_and_word_starts() {
    assert!(palette::fuzzy("Text Editor", "xyz").is_none());
    let prefix = palette::fuzzy("Files", "fi").unwrap();
    let inside = palette::fuzzy("Profile", "fi").unwrap();
    assert!(prefix > inside);
    let word = palette::fuzzy("Text Editor", "ed").unwrap();
    let scattered = palette::fuzzy("Text Editor", "tr").unwrap();
    assert!(word > scattered);
    assert!(palette::fuzzy("System Monitor", "sysmon").is_some());
}

#[test]
fn search_spans_apps_windows_commands_and_workspaces() {
    let mut shell = shell();
    shell.map_window("kitty", "~/src");
    let entries = palette::entries(&shell, &apps(), &[]);
    let found = |q: &str| titles(&entries, &palette::search(&entries, q, &History::default()));

    assert_eq!(found("text")[0], "Text Editor");
    assert_eq!(found("@src")[0], "~/src");
    assert!(found("~/src").is_empty(), "~ searches files");
    assert_eq!(found("maxim")[0], "Maximize");
    assert!(found("workspace 3").contains(&"Go to Workspace 3".to_owned()));
    assert!(found("workspace 3").contains(&"Move Window to Workspace 3".to_owned()));
    assert_eq!(found("lock")[0], "Lock Screen");
}

#[test]
fn window_entries_focus_across_workspaces() {
    let mut shell = shell();
    let (editor, _) = shell.map_window("editor", "notes.txt");
    shell
        .apply(Action::SwitchWorkspace { workspace: 2 })
        .unwrap();
    let entries = palette::entries(&shell, &[], &[]);
    let hits = palette::search(&entries, "@notes", &History::default());
    let entry = &entries[hits[0]];
    assert_eq!(entry.category, Category::Window);
    assert!(entry.detail.contains("workspace 1"));
    shell.run(entry.actions.clone()).unwrap();
    assert_eq!(shell.focused(), Some(editor));
    assert_eq!(shell.desktop().active().id().get(), 1);
}

#[test]
fn app_menus_become_palette_commands() {
    let mut shell = shell();
    let (w, _) = shell.map_window("editor", "notes.txt");
    shell.menus.register(
        w.get(),
        vec![Menu {
            title: "File".into(),
            entries: vec![
                MenuEntry::item("save", "Save").with_shortcut("Ctrl+S"),
                MenuEntry::Submenu {
                    label: "Export".into(),
                    entries: vec![MenuEntry::item("pdf", "As PDF")],
                },
                MenuEntry::Item {
                    id: "print".into(),
                    label: "Print".into(),
                    shortcut: None,
                    enabled: false,
                },
            ],
        }],
    );
    let entries = palette::entries(&shell, &[], &[]);
    let app: Vec<&Entry> = entries
        .iter()
        .filter(|e| e.category == Category::AppCommand)
        .collect();
    assert_eq!(app.len(), 2, "disabled items are left out");
    assert_eq!(app[0].title, "Save");
    assert_eq!(app[0].shortcut.as_deref(), Some("Ctrl+S"));
    assert_eq!(app[1].detail, "editor · File › Export › As PDF");

    let hits = palette::search(&entries, "> export pdf", &History::default());
    let effects = shell.run(entries[hits[0]].actions.clone()).unwrap();
    assert_eq!(
        serde_json::to_value(&effects).unwrap(),
        serde_json::json!([{"effect": "menu_activated", "window": w.get(), "item": "pdf"}])
    );
}

#[test]
fn scopes_narrow_the_search() {
    assert_eq!(palette::scope("> tile"), (Scope::Commands, "tile"));
    assert_eq!(palette::scope("@fire"), (Scope::Windows, "fire"));
    assert_eq!(palette::scope("~/notes"), (Scope::Files, "notes"));
    assert_eq!(palette::scope("? open kitty"), (Scope::Ask, "open kitty"));
    assert_eq!(palette::scope(" files "), (Scope::All, "files"));

    let mut shell = shell();
    shell.map_window("files", "Files");
    let entries = palette::entries(&shell, &apps(), &[]);
    let windows = palette::search(&entries, "@files", &History::default());
    assert!(
        windows
            .iter()
            .all(|&i| entries[i].category == Category::Window)
    );
    let commands = palette::search(&entries, ">", &History::default());
    assert!(!commands.is_empty());
    assert!(
        commands
            .iter()
            .all(|&i| entries[i].category != Category::App)
    );
}

#[test]
fn history_lifts_frequent_picks() {
    let entries = palette::entries(&shell(), &apps(), &[]);
    let mut history = History::default();
    let first = |h: &History| titles(&entries, &palette::search(&entries, "s", h))[0].clone();
    let before = first(&history);
    let pick = entries.iter().find(|e| e.title == "Shut Down").unwrap();
    assert_ne!(before, "Shut Down");
    for _ in 0..3 {
        history.record(pick);
    }
    assert_eq!(first(&history), "Shut Down");
}

#[test]
fn unmatched_text_goes_to_the_assistant() {
    let mut shell = shell();
    shell.map_window("kitty", "~");
    let entries = palette::entries(&shell, &apps(), &[]);
    let query = "open firefox and snap it left";
    let hits = palette::search(&entries, query, &History::default());
    assert!(palette::prefer_assistant(&entries, &hits, query));
    let ask = palette::ask(query);
    assert_eq!(ask.category, Category::Ask);
    assert_eq!(ask.actions.len(), 2);
    assert!(!ask.confirm);

    // A close match wins over the assistant.
    let hits = palette::search(&entries, "text editor", &History::default());
    assert!(!palette::prefer_assistant(&entries, &hits, "text editor"));

    // Not understood: no actions, and the detail says why.
    let unknown = palette::ask("frobnicate the flux");
    assert!(unknown.actions.is_empty());
    assert!(unknown.detail.contains("frobnicate"));
}

#[test]
fn destructive_session_commands_need_confirmation() {
    let entries = palette::entries(&shell(), &[], &[]);
    let by_title = |t: &str| entries.iter().find(|e| e.title == t).unwrap();
    assert!(by_title("Shut Down").confirm);
    assert!(by_title("Log Out").confirm);
    assert!(!by_title("Lock Screen").confirm);
    assert_eq!(
        by_title("Shut Down").actions,
        [Action::Session {
            op: SessionOp::PowerOff,
            confirmed: true
        }]
    );
    let ask = palette::ask("reboot");
    assert!(ask.confirm);
}

#[test]
fn files_are_indexed_and_opened_safely() {
    let dir = std::env::temp_dir().join(format!("derisk-palette-{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(dir.join("notes/deep/deeper")).unwrap();
    fs::create_dir_all(dir.join(".hidden")).unwrap();
    fs::write(dir.join("notes/todo.md"), "x").unwrap();
    fs::write(dir.join("notes/deep/deeper/far.txt"), "x").unwrap();
    fs::write(dir.join("run.sh"), "#!/bin/sh").unwrap();
    fs::write(dir.join("app.desktop"), "[Desktop Entry]").unwrap();
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(dir.join("run.sh"), fs::Permissions::from_mode(0o755)).unwrap();

    let files = palette::index_files(&dir, 2, 100);
    assert!(files.contains(&dir.join("notes/todo.md")));
    assert!(
        !files.contains(&dir.join("notes/deep/deeper/far.txt")),
        "depth limit"
    );
    assert!(!files.iter().any(|p| p.starts_with(dir.join(".hidden"))));
    assert_eq!(palette::index_files(&dir, 3, 2).len(), 2, "count limit");

    let mut shell = shell();
    let entries = palette::entries(&shell, &[], &files);
    // Files stay out of an empty query.
    let empty = palette::search(&entries, "", &History::default());
    assert!(empty.iter().all(|&i| entries[i].category != Category::File));
    let hits = palette::search(&entries, "/todo", &History::default());
    let todo = &entries[hits[0]];
    assert_eq!(todo.title, "todo.md");
    let effects = shell.run(todo.actions.clone()).unwrap();
    assert_eq!(serde_json::to_value(&effects).unwrap()[0]["effect"], "open");

    // Launchers that dodge a lowercase `.desktop` check: shared-mime-info
    // matches globs without case and desktop entries by content.
    fs::write(dir.join("evil.DESKTOP"), "[Desktop Entry]\n").unwrap();
    fs::write(dir.join("README"), "# x\n[Desktop Entry]\nExec=sh\n").unwrap();
    fs::write(dir.join("tool.Jar"), "PK").unwrap();
    // Programs under names that match no launcher glob, found by content.
    fs::write(dir.join("invoice"), b"MZ\x90\x00").unwrap();
    fs::write(dir.join("photo.png"), "\x7fELF\x02").unwrap();
    fs::write(dir.join("statement"), "PK\x03\x04").unwrap();
    // A document that is a zip keeps opening: its glob decides its type.
    fs::write(dir.join("report.docx"), "PK\x03\x04").unwrap();
    let mut open = |p: &str| shell.apply(Action::Open { path: p.to_owned() });
    for refused in [
        dir.join("run.sh").display().to_string(),
        dir.join("app.desktop").display().to_string(),
        dir.join("evil.DESKTOP").display().to_string(),
        dir.join("README").display().to_string(),
        dir.join("tool.Jar").display().to_string(),
        dir.join("invoice").display().to_string(),
        dir.join("photo.png").display().to_string(),
        dir.join("statement").display().to_string(),
        dir.join("missing").display().to_string(),
        "notes/todo.md".to_owned(),
    ] {
        assert!(
            matches!(open(&refused), Err(Error::NotOpenable(_))),
            "{refused}"
        );
    }
    assert!(open(&dir.join("notes").display().to_string()).is_ok());
    assert!(open(&dir.join("report.docx").display().to_string()).is_ok());
    fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn close_title_matches_beat_the_assistant() {
    let mut shell = shell();
    shell.map_window("kitty", "~");
    let entries = palette::entries(&shell, &[], &[]);
    for query in [
        "workspace 3",
        "go to workspace 3",
        "move window to workspace 2",
    ] {
        let hits = palette::search(&entries, query, &History::default());
        assert!(
            !palette::prefer_assistant(&entries, &hits, query),
            "{query}"
        );
    }
}
