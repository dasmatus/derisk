use std::{
    path::{Path, PathBuf},
    time::{Duration, SystemTime},
};

use derisk_portal_ui::{
    Dialog, Reply, Request, SAMPLES, Update, access, app_chooser, background,
    file_chooser::{self, Filter, Pattern},
    sample, screenshot,
};
use mcsapi_ui::{
    Theme,
    egui::{self, Event, Key, Modifiers, PointerButton, Pos2, Rect, pos2, vec2},
};

fn temp_dir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("derisk-portal-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// Runs one frame of `dialog` at its own size with `events`.
fn frame(dialog: &mut dyn Dialog, ctx: &egui::Context, events: Vec<Event>) {
    let [w, h] = dialog.size();
    let input = egui::RawInput {
        screen_rect: Some(Rect::from_min_size(Pos2::ZERO, vec2(w, h))),
        events,
        ..Default::default()
    };
    let mut output = ctx.run_ui(input, |ui| {
        derisk_portal_ui::show(dialog, ui, &Theme::default())
    });
    output.textures_delta.clear();
}

fn button(pos: Pos2, pressed: bool) -> Event {
    Event::PointerButton {
        pos,
        button: PointerButton::Primary,
        pressed,
        modifiers: Modifiers::NONE,
    }
}

fn click(dialog: &mut dyn Dialog, ctx: &egui::Context, pos: Pos2) {
    frame(dialog, ctx, vec![Event::PointerMoved(pos)]);
    frame(dialog, ctx, vec![button(pos, true)]);
    frame(dialog, ctx, vec![button(pos, false)]);
}

fn key(dialog: &mut dyn Dialog, ctx: &egui::Context, key: Key) {
    frame(
        dialog,
        ctx,
        vec![Event::Key {
            key,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: Modifiers::NONE,
        }],
    );
}

fn open(request: Request) -> (Box<dyn Dialog>, egui::Context) {
    let mut dialog = derisk_portal_ui::open(request);
    let ctx = egui::Context::default();
    frame(&mut *dialog, &ctx, Vec::new());
    (dialog, ctx)
}

#[test]
fn every_sample_draws_and_escape_cancels_it() {
    for name in SAMPLES {
        let (mut dialog, ctx) = open(sample(name).unwrap());
        assert!(dialog.reply().is_none(), "{name} ended on its own");
        key(&mut *dialog, &ctx, Key::Escape);
        assert_eq!(dialog.reply(), Some(&Reply::Cancelled), "{name}");
    }
    assert!(sample("nope").is_none());
}

#[test]
fn requests_and_replies_round_trip_as_tagged_json() {
    let request = sample("background").unwrap();
    let json = serde_json::to_string(&request).unwrap();
    assert!(json.starts_with(r#"{"dialog":"background""#), "{json}");
    assert_eq!(serde_json::from_str::<Request>(&json).unwrap(), request);

    let reply = Reply::App(app_chooser::Choice {
        app_id: "org.example.Viewer".into(),
        always: true,
    });
    let json = serde_json::to_string(&reply).unwrap();
    assert_eq!(
        json,
        r#"{"outcome":"app","app_id":"org.example.Viewer","always":true}"#
    );
    assert_eq!(serde_json::from_str::<Reply>(&json).unwrap(), reply);
    assert_eq!(
        serde_json::from_str::<Reply>(r#"{"outcome":"cancelled"}"#).unwrap(),
        Reply::Cancelled
    );
}

#[test]
fn background_allow_and_dont_allow() {
    let request = Request::Background(background::Request {
        app_id: "org.example.Sync".into(),
        app_name: "Sync".into(),
        autostart: false,
        ..Default::default()
    });
    // The buttons split the bottom of the 720×476 window, 32 px in.
    let (mut dialog, ctx) = open(request.clone());
    click(&mut *dialog, &ctx, pos2(600.0, 420.0));
    assert_eq!(
        dialog.reply(),
        Some(&Reply::Background(background::Choice {
            background: true,
            autostart: false,
        }))
    );
    let (mut dialog, ctx) = open(request);
    click(&mut *dialog, &ctx, pos2(120.0, 420.0));
    assert_eq!(dialog.reply(), Some(&Reply::Cancelled));
}

#[test]
fn access_allow_returns_every_choice() {
    let (mut dialog, ctx) = open(Request::Access(access::Request::sample()));
    assert_eq!(dialog.title(), "Camera Access");
    click(&mut *dialog, &ctx, pos2(800.0, 420.0));
    assert_eq!(
        dialog.reply(),
        Some(&Reply::Access(access::Choice {
            choices: vec![
                ("camera".into(), "integrated".into()),
                ("remember".into(), "true".into()),
            ],
        }))
    );
}

#[test]
fn screenshot_keys_pick_the_mode() {
    let (mut dialog, ctx) = open(sample("screenshot").unwrap());
    key(&mut *dialog, &ctx, Key::S);
    key(&mut *dialog, &ctx, Key::Enter);
    assert_eq!(
        dialog.reply(),
        Some(&Reply::Screenshot(screenshot::Choice {
            mode: screenshot::Mode::Screen,
            area: screenshot::FULL,
            include_pointer: false,
            delay: 0,
        }))
    );

    let request = Request::Screenshot(screenshot::Request {
        app_name: "Agent".into(),
        window: Some([0.5, 0.0, 1.0, 0.5]),
        ..Default::default()
    });
    let (mut dialog, ctx) = open(request);
    key(&mut *dialog, &ctx, Key::W);
    key(&mut *dialog, &ctx, Key::Enter);
    let Some(Reply::Screenshot(choice)) = dialog.reply() else {
        panic!("no capture");
    };
    assert_eq!(choice.mode, screenshot::Mode::Window);
    assert_eq!(choice.area, [0.5, 0.0, 1.0, 0.5]);
}

#[test]
fn app_chooser_follows_updates_and_opens_the_selection() {
    let mut request = app_chooser::Request::sample();
    request.last_choice = Some("org.mozilla.firefox".into());
    let (mut dialog, ctx) = open(Request::AppChooser(request));
    let viewer = app_chooser::AppEntry {
        id: "org.example.Viewer".into(),
        name: "Viewer".into(),
        ..Default::default()
    };
    // Firefox is no longer offered, so the first app is selected instead.
    dialog.update(Update::Choices {
        choices: vec![viewer],
    });
    key(&mut *dialog, &ctx, Key::Enter);
    assert_eq!(
        dialog.reply(),
        Some(&Reply::App(app_chooser::Choice {
            app_id: "org.example.Viewer".into(),
            always: false,
        }))
    );
}

#[test]
fn app_entries_come_from_desktop_files() {
    let entry = derisk::desktop::DesktopEntry::parse(
        "org.example.Viewer.desktop",
        "[Desktop Entry]\nType=Application\nName=Viewer\nGenericName=Document viewer\nIcon=viewer\nExec=viewer %U\n",
    )
    .unwrap();
    let app = app_chooser::AppEntry::from_desktop("org.example.Viewer", &[entry]);
    assert_eq!(
        (app.name.as_str(), app.detail.as_str(), app.icon.as_str()),
        ("Viewer", "Document viewer", "viewer")
    );
    let unknown = app_chooser::AppEntry::from_desktop("org.example.Missing", &[]);
    assert_eq!(unknown.name, "Missing");
}

fn chooser(dir: &Path, request: file_chooser::Request) -> (Box<dyn Dialog>, egui::Context) {
    open(Request::FileChooser(file_chooser::Request {
        title: "Open File".into(),
        current_folder: Some(dir.to_owned()),
        ..request
    }))
}

#[test]
fn file_chooser_opens_a_double_clicked_file_through_the_filter() {
    let dir = temp_dir("open");
    std::fs::write(dir.join("b.txt"), "").unwrap();
    std::fs::write(dir.join("a.pdf"), "").unwrap();
    std::fs::create_dir(dir.join("z-folder")).unwrap();
    let pdf = Filter {
        name: "PDF documents".into(),
        patterns: vec![Pattern::Mime("application/pdf".into())],
    };
    let (mut dialog, ctx) = chooser(
        &dir,
        file_chooser::Request {
            filters: vec![pdf],
            ..Default::default()
        },
    );
    // Rows start under the 76 px toolbar, the 36 px header and 8 px of
    // padding: the folder first, then a.pdf; b.txt is filtered out.
    let second_row = pos2(600.0, 76.0 + 36.0 + 8.0 + 54.0 + 26.0);
    click(&mut *dialog, &ctx, second_row);
    frame(&mut *dialog, &ctx, vec![button(second_row, true)]);
    frame(&mut *dialog, &ctx, vec![button(second_row, false)]);
    assert_eq!(
        dialog.reply(),
        Some(&Reply::Files(file_chooser::Choice {
            paths: vec![dir.join("a.pdf")],
            filter: Some(0),
        }))
    );
}

#[test]
fn file_chooser_saves_under_the_typed_name() {
    let dir = temp_dir("save");
    let (mut dialog, ctx) = chooser(
        &dir,
        file_chooser::Request {
            save_name: Some("notes.md".into()),
            ..Default::default()
        },
    );
    key(&mut *dialog, &ctx, Key::Enter);
    assert_eq!(
        dialog.reply(),
        Some(&Reply::Files(file_chooser::Choice {
            paths: vec![dir.join("notes.md")],
            filter: None,
        }))
    );
}

#[test]
fn file_chooser_returns_the_folder_in_directory_mode() {
    let dir = temp_dir("dir");
    let (mut dialog, ctx) = chooser(
        &dir,
        file_chooser::Request {
            directory: true,
            ..Default::default()
        },
    );
    key(&mut *dialog, &ctx, Key::Enter);
    let Some(Reply::Files(choice)) = dialog.reply() else {
        panic!("no folder chosen");
    };
    assert_eq!(choice.paths, vec![std::path::absolute(&dir).unwrap()]);
}

#[test]
fn globs_and_mime_types() {
    use file_chooser::{glob_matches, mime_matches};
    assert!(glob_matches("*.pdf", "Q3 report.PDF"));
    assert!(glob_matches("*", "anything"));
    assert!(glob_matches("img_??.png", "img_01.png"));
    assert!(!glob_matches("img_??.png", "img_1.png"));
    assert!(glob_matches("[a-c]*.txt", "b.txt"));
    assert!(!glob_matches("[!a-c]*.txt", "b.txt"));
    assert!(!glob_matches("*.pdf", "report.pdf.txt"));
    assert!(mime_matches("application/pdf", "a.pdf"));
    assert!(mime_matches("image/*", "photo.JPG"));
    assert!(!mime_matches("image/*", "notes.md"));
    assert!(!mime_matches("application/x-unknown", "a.bin"));
}

#[test]
fn breadcrumbs_start_at_home_and_shorten_deep_paths() {
    let home = Path::new("/home/me");
    let names = |dir: &str| -> Vec<String> {
        file_chooser::crumbs(Path::new(dir), home)
            .into_iter()
            .map(|(n, _)| n)
            .collect()
    };
    assert_eq!(names("/home/me"), ["Home"]);
    assert_eq!(names("/home/me/Documents"), ["Home", "Documents"]);
    assert_eq!(names("/etc"), ["/", "etc"]);
    assert_eq!(names("/home/me/a/b/c/d"), ["Home", "…", "c", "d"]);
    let crumbs = file_chooser::crumbs(Path::new("/home/me/a/b/c/d"), home);
    assert_eq!(crumbs[1].1, Path::new("/home/me/a/b"));
}

#[test]
fn modification_times_read_like_the_design() {
    let day = 86_400;
    let now = SystemTime::UNIX_EPOCH + Duration::from_secs(20_000 * day + 10 * 3600);
    let at = |secs: u64| SystemTime::UNIX_EPOCH + Duration::from_secs(secs);
    assert_eq!(
        file_chooser::when(at(20_000 * day + 9 * 3600 + 14 * 60), now),
        "Today, 09:14"
    );
    assert_eq!(file_chooser::when(at(19_999 * day), now), "Yesterday");
    // 2024-09-30.
    assert_eq!(file_chooser::when(at(19_996 * day), now), "Sep 30, 2024");
}

#[test]
fn user_dirs_follow_user_dirs_dirs() {
    let text = "# comment\nXDG_DOCUMENTS_DIR=\"$HOME/Docs\"\nXDG_MUSIC_DIR=\"/srv/music\"\n";
    let home = Path::new("/home/me");
    assert_eq!(
        file_chooser::parse_user_dir(text, "DOCUMENTS", home),
        Some(PathBuf::from("/home/me/Docs"))
    );
    assert_eq!(
        file_chooser::parse_user_dir(text, "MUSIC", home),
        Some(PathBuf::from("/srv/music"))
    );
    assert_eq!(file_chooser::parse_user_dir(text, "VIDEOS", home), None);
}
