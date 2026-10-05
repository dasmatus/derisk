use std::path::{Path, PathBuf};

use derisk_settings::{
    Page, Settings, SettingsApp,
    flatpak::{App, Context, Dirs, Grant, Key, Portal, parse_permission_show},
};
use mcsapi_ui::{Theme, egui, run_frame};

fn temp_dir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("derisk-privacy-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

const METADATA: &str = "\
[Application]
name=org.example.Maps
runtime=org.gnome.Platform/x86_64/48

[Context]
shared=network;ipc;
sockets=wayland;fallback-x11;
devices=dri;
filesystems=xdg-pictures:ro;~/Maps;

[Session Bus Policy]
org.freedesktop.Notifications=talk
";

fn install(root: &Path, id: &str, name: &str, metadata: &str) {
    let active = root.join("app").join(id).join("current/active");
    std::fs::create_dir_all(active.join("export/share/applications")).unwrap();
    std::fs::write(active.join("metadata"), metadata).unwrap();
    std::fs::write(
        active.join(format!("export/share/applications/{id}.desktop")),
        format!("[Desktop Entry]\nName={name}\n"),
    )
    .unwrap();
}

#[test]
fn privacy_settings_round_trip() {
    let mut settings = Settings::default();
    settings.privacy.location = false;
    settings.privacy.camera = false;
    settings.privacy.remember_recent = false;
    settings.privacy.empty_trash_days = 7;
    let text = settings.to_text();
    assert!(text.contains("privacy.empty_trash_days = 7\n"), "{text}");
    let (parsed, warnings) = Settings::parse(&text);
    assert!(warnings.is_empty(), "{warnings:?}");
    assert_eq!(parsed, settings);
    let (_, warnings) = Settings::parse("privacy.empty_trash_days = 400\n");
    assert_eq!(warnings.len(), 1);
}

#[test]
fn context_parses_grants_and_keeps_other_groups() {
    let context = Context::parse(METADATA);
    assert_eq!(context.get(Key::Shared, "network"), Some(Grant::Yes));
    assert_eq!(
        context.get(Key::Filesystems, "xdg-pictures"),
        Some(Grant::ReadOnly)
    );
    assert_eq!(context.get(Key::Filesystems, "~/Maps"), Some(Grant::Yes));
    assert_eq!(context.get(Key::Sockets, "x11"), None);

    let mut overrides = Context::default();
    overrides.set(Key::Shared, "network", Some(Grant::No));
    overrides.set(Key::Filesystems, "home", Some(Grant::ReadOnly));
    let written = overrides.write_into("[Context]\nsockets=x11;\n\n[Environment]\nFOO=bar\n");
    assert!(written.contains("[Environment]\nFOO=bar\n"), "{written}");
    assert!(written.contains("shared=!network;\n"), "{written}");
    assert!(written.contains("filesystems=home:ro;\n"), "{written}");
    assert!(!written.contains("x11"), "{written}");
    assert_eq!(Context::parse(&written), overrides);
    assert_eq!(Context::default().write_into(""), "");
}

#[test]
fn switching_records_only_what_differs() {
    let mut app = App {
        id: "org.example.Maps".into(),
        name: "Maps".into(),
        user: true,
        requested: Context::parse(METADATA),
        overrides: Context::default(),
        bus_names: Vec::new(),
    };
    let mut global = Context::default();
    // Taking away what the app asks for records `!network`.
    app.switch(&global, Key::Shared, "network", false);
    assert_eq!(app.overrides.get(Key::Shared, "network"), Some(Grant::No));
    assert_eq!(app.effective(&global, Key::Shared, "network"), None);
    // Giving it back clears the override instead of writing `network`.
    app.switch(&global, Key::Shared, "network", true);
    assert!(app.overrides.is_empty());
    // A global override counts as what the app gets anyway.
    global.set(Key::Filesystems, "home", Some(Grant::Yes));
    assert_eq!(
        app.effective(&global, Key::Filesystems, "home"),
        Some(Grant::Yes)
    );
    app.switch(&global, Key::Filesystems, "home", false);
    assert_eq!(app.overrides.get(Key::Filesystems, "home"), Some(Grant::No));
    assert_eq!(app.effective(&global, Key::Filesystems, "home"), None);
}

#[test]
fn installed_apps_and_overrides_are_read_and_written() {
    let dir = temp_dir("apps");
    let dirs = Dirs {
        user: dir.join("user"),
        system: vec![dir.join("system")],
    };
    install(&dirs.user, "org.example.Maps", "Maps", METADATA);
    install(&dirs.system[0], "org.example.Maps", "Old Maps", METADATA);
    install(&dirs.system[0], "com.example.Chat", "Chat", "[Context]\n");
    std::fs::create_dir_all(dirs.user.join("overrides")).unwrap();
    std::fs::write(
        dirs.override_path("org.example.Maps"),
        "[Context]\nshared=!network;\n",
    )
    .unwrap();

    let apps = dirs.apps();
    let names: Vec<_> = apps.iter().map(|a| (a.name.as_str(), a.user)).collect();
    // By name, and the user's copy wins over the system one.
    assert_eq!(names, [("Chat", false), ("Maps", true)]);
    let maps = &apps[1];
    assert_eq!(maps.overrides.get(Key::Shared, "network"), Some(Grant::No));
    assert_eq!(
        maps.bus_names,
        ["org.freedesktop.Notifications (talk)".to_owned()]
    );

    let mut overrides = maps.overrides.clone();
    overrides.set(Key::Sockets, "x11", Some(Grant::Yes));
    dirs.save_overrides(&maps.id, &overrides).unwrap();
    assert_eq!(
        std::fs::read_to_string(dirs.override_path(&maps.id)).unwrap(),
        "[Context]\nshared=!network;\nsockets=x11;\n"
    );
    // Nothing left means no file, as `flatpak override --reset` leaves it.
    dirs.save_overrides(&maps.id, &Context::default()).unwrap();
    assert!(!dirs.override_path(&maps.id).exists());
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn permission_show_output_is_parsed() {
    let text = "Table\tObject\tApp\tPermissions\tData\n\
                devices\tcamera\torg.example.Maps\tno\t0x00\n\
                devices\tmicrophone\torg.example.Maps\tyes\t0x00\n\
                location\tlocation\torg.example.Maps\tEXACT,1700000000\t0x00\n\
                notifications\tnotification\torg.example.Maps\tyes\t\n\
                unknown\tthing\torg.example.Maps\tyes\t\n";
    let found = parse_permission_show(text);
    assert_eq!(found.get(&Portal::Camera), Some(&false));
    assert_eq!(found.get(&Portal::Microphone), Some(&true));
    assert_eq!(found.get(&Portal::Location), Some(&true));
    assert_eq!(found.get(&Portal::Notifications), Some(&true));
    assert_eq!(found.len(), 4);
    assert!(parse_permission_show("").is_empty());
}

#[test]
fn piped_permission_show_output_has_no_header() {
    // Flatpak 1.14.6's `flatpak permission-show org.example.App | cat`.
    let text = "location\tlocation\torg.example.App\tEXACT,0\t0x00\n\
                background\tbackground\torg.example.App\tyes\t0x00\n\
                devices\tcamera\torg.example.App\tno\t0x00\n";
    let found = parse_permission_show(text);
    assert_eq!(found.get(&Portal::Location), Some(&true));
    assert_eq!(found.get(&Portal::Background), Some(&true));
    assert_eq!(found.get(&Portal::Camera), Some(&false));
    assert_eq!(found.len(), 3);
}

#[test]
fn the_privacy_page_renders_an_app() {
    let dir = temp_dir("page");
    let dirs = Dirs {
        user: dir.join("user"),
        system: Vec::new(),
    };
    install(&dirs.user, "org.example.Maps", "Maps", METADATA);
    let context = egui::Context::default();
    let mut app = SettingsApp::open(None).with_flatpak_dirs(dirs);
    for selected in [false, true] {
        if selected {
            app.show_flatpak_app("org.example.Maps");
        } else {
            app.page = Page::Privacy;
        }
        let mut output = run_frame(
            &mut app,
            &context,
            egui::RawInput::default(),
            &Theme::default(),
        );
        assert!(!output.shapes.is_empty());
        output.textures_delta.clear();
    }
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn pictures_and_folders_get_thumbnails() {
    let dir = temp_dir("thumbs");
    let picture = image::RgbaImage::from_pixel(1600, 900, image::Rgba([10, 20, 30, 255]));
    picture.save(dir.join("wide.png")).unwrap();
    let (thumb, size) = derisk_settings::thumbnail(&dir.join("wide.png")).unwrap();
    assert_eq!(size, [1600, 900]);
    assert_eq!(thumb.size, [320, 180]);
    // A folder shows its first picture.
    let (_, size) = derisk_settings::thumbnail(&dir).unwrap();
    assert_eq!(size, [1600, 900]);
    std::fs::write(dir.join("broken.png"), b"not a png").unwrap();
    assert!(derisk_settings::thumbnail(&dir.join("broken.png")).is_none());
    let _ = std::fs::remove_dir_all(&dir);
}
