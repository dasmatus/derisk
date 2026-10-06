use std::fs;

use derisk::{
    action::{Action, Effect},
    desktop::{self, DesktopEntry, exec_argv},
    geom::rect,
    palette::{self, Category, History},
    shell::{Error, Shell},
};

const FIREFOX: &str = "\
# A comment
[Desktop Entry]
Version=1.0
Type=Application
Name=Firefox
Name[de]=Feuerfuchs
GenericName=Web Browser
Comment=Browse the World Wide Web
Keywords=Internet;WWW;Browser;Web\\;Explorer;
Icon=firefox
Exec=firefox %u
Actions=new-window;new-private-window;profile-manager;missing;

[Desktop Action new-window]
Name=New Window
Exec=firefox --new-window %u

[Desktop Action new-private-window]
Name=New Private Window
Name[de]=Neues privates Fenster
Exec=firefox --private-window %u

[Desktop Action profile-manager]
Name=Profile Manager
Exec=\"/opt/firefox dir/firefox\" --ProfileManager

[Desktop Action unlisted]
Name=Not in Actions
Exec=firefox --unlisted
";

fn firefox() -> DesktopEntry {
    DesktopEntry::parse("firefox.desktop", FIREFOX).unwrap()
}

#[test]
fn parses_entries_and_listed_actions_in_order() {
    let e = firefox();
    assert_eq!(e.name, "Firefox", "localized keys are ignored");
    assert_eq!(e.generic_name, "Web Browser");
    assert_eq!(e.keywords, ["Internet", "WWW", "Browser", "Web;Explorer"]);
    assert_eq!(e.argv(), ["firefox"]);
    let ids: Vec<&str> = e.actions.iter().map(|a| a.id.as_str()).collect();
    assert_eq!(ids, ["new-window", "new-private-window", "profile-manager"]);
    assert_eq!(e.actions[1].name, "New Private Window");
    assert_eq!(
        e.action_argv("profile-manager").unwrap(),
        ["/opt/firefox dir/firefox", "--ProfileManager"]
    );
    assert!(e.action_argv("unlisted").is_none());
}

#[test]
fn skips_what_derisk_should_not_show() {
    let app = |extra: &str| {
        DesktopEntry::parse(
            "x.desktop",
            &format!("[Desktop Entry]\nType=Application\nName=X\nExec=x\n{extra}"),
        )
    };
    assert!(app("").is_some());
    assert!(app("Hidden=true").is_none());
    assert!(app("OnlyShowIn=GNOME;KDE;").is_none());
    assert!(app("OnlyShowIn=GNOME;derisk;").is_some());
    assert!(app("NotShowIn=derisk;").is_none());
    assert!(app("NoDisplay=true").unwrap().no_display);
    assert!(
        DesktopEntry::parse("l.desktop", "[Desktop Entry]\nType=Link\nName=L\nURL=x").is_none()
    );
    assert!(
        DesktopEntry::parse("n.desktop", "[Desktop Entry]\nType=Application\nName=N").is_none()
    );
}

#[test]
fn exec_lines_split_and_drop_field_codes() {
    let argv = |exec| exec_argv(exec, "My App", "my-icon");
    assert_eq!(argv("app %F --flag"), ["app", "--flag"]);
    assert_eq!(
        argv("app --name=%c %i"),
        ["app", "--name=My App", "--icon", "my-icon"]
    );
    assert_eq!(argv("app 100%%"), ["app", "100%"]);
    assert_eq!(
        argv(r#"sh -c "echo \"%u\" \$HOME""#),
        ["sh", "-c", r#"echo "%u" $HOME"#]
    );
    assert_eq!(
        argv(r"app\sname"),
        ["app", "name"],
        "\\s is a string escape"
    );
    assert!(argv(r#"app "unterminated"#).is_empty());
}

#[test]
fn scan_follows_xdg_precedence_and_desktop_file_ids() {
    let root = std::env::temp_dir().join(format!("derisk-desktop-{}", std::process::id()));
    let _ = fs::remove_dir_all(&root);
    let (home, system) = (root.join("home"), root.join("system"));
    fs::create_dir_all(home.join("kde")).unwrap();
    fs::create_dir_all(&system).unwrap();
    let write = |dir: &std::path::Path, name: &str, body: &str| {
        fs::write(
            dir.join(name),
            format!("[Desktop Entry]\nType=Application\nExec=x\n{body}"),
        )
        .unwrap()
    };
    write(&home, "editor.desktop", "Name=My Editor");
    write(&system, "editor.desktop", "Name=System Editor");
    write(&home, "gone.desktop", "Name=Gone\nHidden=true");
    write(&system, "gone.desktop", "Name=Gone Too");
    write(&home.join("kde"), "konsole.desktop", "Name=Konsole");

    let apps = desktop::scan(&[home, system]);
    let names: Vec<(&str, &str)> = apps
        .iter()
        .map(|a| (a.id.as_str(), a.name.as_str()))
        .collect();
    assert_eq!(
        names,
        [
            ("kde-konsole.desktop", "Konsole"),
            ("editor.desktop", "My Editor")
        ],
        "user files win, Hidden shadows the system copy, subdirectories join with -"
    );
    fs::remove_dir_all(&root).unwrap();
}

#[test]
fn palette_lists_app_actions() {
    let mut shell = Shell::new(rect(0, 0, 1920, 1080), false);
    let extra = palette::desktop_app(&firefox(), "🖥");
    let entries = palette::entries(&shell, &extra, &[]);

    let hits = palette::search(&entries, "private", &History::default());
    let private = &entries[hits[0]];
    assert_eq!(private.category, Category::AppAction);
    assert_eq!(private.title, "New Private Window");
    assert_eq!(private.detail, "Firefox");
    assert_eq!(
        private.actions,
        [Action::LaunchAction {
            app: "firefox.desktop".into(),
            id: "new-private-window".into()
        }]
    );

    // "firefox new window" finds the action through the app name.
    let hits = palette::search(&entries, "firefox new win", &History::default());
    assert_eq!(entries[hits[0]].title, "New Window");

    // Actions stay out of the empty list until picked once.
    let mut history = History::default();
    let empty = |h: &History| palette::search(&entries, "", h);
    assert!(
        empty(&history)
            .iter()
            .all(|&i| entries[i].category != Category::AppAction)
    );
    history.record(private);
    assert_eq!(entries[empty(&history)[0]].title, "New Private Window");

    let effects = shell.run(private.actions.clone()).into_result().unwrap();
    assert_eq!(
        effects,
        [Effect::LaunchAction {
            app: "firefox.desktop".into(),
            id: "new-private-window".into()
        }]
    );
    // Agents cannot smuggle commands through either field.
    let bad = |app: &str, id: &str| {
        Shell::new(rect(0, 0, 800, 600), false).apply(Action::LaunchAction {
            app: app.into(),
            id: id.into(),
        })
    };
    assert!(matches!(bad("rm -rf", "x"), Err(Error::NotLaunchable(_))));
    assert!(matches!(
        bad("firefox.desktop", "x; rm"),
        Err(Error::UnknownAction(_))
    ));
}

#[cfg(feature = "host")]
#[test]
fn core_apps_ship_desktop_files_whose_actions_they_implement() {
    use mcsapi_runtime::AppId;

    let mut session = derisk_apps::Session::new().unwrap();
    for app in derisk_apps::APPS {
        let entry = DesktopEntry::parse(&app.desktop_id(), app.desktop_file)
            .unwrap_or_else(|| panic!("{} has a valid .desktop file", app.id));
        assert_eq!(entry.name, app.name);
        assert_eq!(entry.argv(), ["derisk", "launch", app.id]);
        assert!(!entry.actions.is_empty(), "{} has actions", app.id);
        for action in &entry.actions {
            assert_eq!(
                entry.action_argv(&action.id).unwrap(),
                ["derisk", "launch", app.id, "--action", &action.id]
            );
            assert!(
                app.create_action(&action.id).is_some(),
                "{} {}",
                app.id,
                action.id
            );
            let instance = session
                .launch_action(&AppId::new(app.id).unwrap(), &action.id)
                .unwrap();
            session.stop(instance).unwrap();
        }
        assert!(app.create_action("nope").is_none());
    }
    let settings = derisk_apps::find("org.derisk.settings.desktop").unwrap();
    assert_eq!(settings.id, derisk_apps::SETTINGS);
}
