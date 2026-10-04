use std::fs;

use derisk::{apps::Apps, desktop::DesktopEntry, icons};

fn entry(id: &str, body: &str) -> DesktopEntry {
    let text = format!("[Desktop Entry]\nType=Application\n{body}");
    DesktopEntry::parse(id, &text).unwrap()
}

#[test]
fn names_and_icons_come_from_the_matching_desktop_file() {
    let firefox = entry(
        "org.mozilla.firefox.desktop",
        "Name=Firefox\nIcon=firefox\nExec=firefox %u\n",
    );
    let code = entry(
        "code.desktop",
        "Name=Visual Studio Code\nIcon=vscode\nStartupWMClass=Code\nExec=/usr/bin/code %F\n",
    );
    let files = entry(
        "org.derisk.files.desktop",
        "Name=Files\nIcon=system-file-manager\nExec=derisk launch org.derisk.files\n",
    );
    let editor = entry(
        "org.derisk.editor.desktop",
        "Name=Text Editor\nExec=derisk launch org.derisk.editor\n",
    );
    let apps = Apps::new([(&files, "📁"), (&editor, "📝"), (&firefox, ""), (&code, "")]);

    // The desktop file ID, case-insensitively.
    let look = apps.look("org.mozilla.Firefox");
    assert_eq!((look.name.as_ref(), look.icon), ("Firefox", "firefox"));
    // StartupWMClass.
    assert_eq!(apps.look("Code").name, "Visual Studio Code");
    // The last part of a reverse-DNS ID, either way round.
    assert_eq!(apps.look("firefox").name, "Firefox");
    // The program, when it is the app's own.
    assert_eq!(apps.look("code").name, "Visual Studio Code");
    // Core apps keep their glyph; without an Icon key the app ID is tried.
    let look = apps.look("org.derisk.editor");
    assert_eq!(
        (look.name.as_ref(), look.icon, look.glyph),
        ("Text Editor", "org.derisk.editor", "📝")
    );
    // `derisk` starts every core app, so it names none of them.
    assert_eq!(apps.look("derisk").name, "Derisk");
}

#[test]
fn unknown_app_ids_still_read_as_names() {
    let apps = Apps::default();
    let look = apps.look("org.example.notes");
    assert_eq!(
        (look.name.as_ref(), look.icon, look.glyph),
        ("Notes", "org.example.notes", "")
    );
    assert_eq!(apps.look("foot").name, "Foot");
    assert_eq!(apps.look("").name, "");
}

#[test]
fn icons_come_from_hicolor_then_other_themes_then_pixmaps() {
    let root = std::env::temp_dir().join(format!("derisk-icons-{}", std::process::id()));
    let _ = fs::remove_dir_all(&root);
    let icons_dir = root.join("icons");
    let pixmaps = root.join("pixmaps");
    let png = |path: std::path::PathBuf, size: u32| {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        image::RgbaImage::from_pixel(size, size, image::Rgba([255, 0, 0, 255]))
            .save(&path)
            .unwrap();
        path
    };
    let small = png(icons_dir.join("hicolor/32x32/apps/foot.png"), 32);
    let big = png(icons_dir.join("hicolor/128x128/apps/foot.png"), 128);
    let svg = icons_dir.join("hicolor/scalable/apps/foot.svg");
    fs::create_dir_all(svg.parent().unwrap()).unwrap();
    fs::write(
        &svg,
        r##"<svg xmlns="http://www.w3.org/2000/svg" width="16" height="16"><rect width="16" height="16" fill="#00f"/></svg>"##,
    )
    .unwrap();
    let generic = png(
        icons_dir.join("Adwaita/48x48/places/system-file-manager.png"),
        48,
    );
    let pixmap = png(pixmaps.join("legacy.png"), 48);
    let roots = (vec![icons_dir.clone()], vec![pixmaps.clone()]);
    let find = |name, want| icons::find_in(name, want, &roots.0, &roots.1);

    // The smallest PNG at least as big, else the SVG, else a smaller PNG.
    assert_eq!(find("foot", 64), Some(big.clone()));
    assert_eq!(find("foot", 32), Some(small));
    assert_eq!(find("foot", 256), Some(svg.clone()));
    assert_eq!(find("system-file-manager", 64), Some(generic));
    assert_eq!(find("legacy", 64), Some(pixmap));
    assert_eq!(find(big.to_str().unwrap(), 64), Some(big.clone()));
    assert_eq!(find("../foot", 64), None);
    assert_eq!(find("missing", 64), None);

    // Decoded no bigger than asked, in both formats.
    assert_eq!(icons::load(&big, 64).unwrap().size, [64, 64]);
    let blue = icons::load(&svg, 64).unwrap();
    assert_eq!(blue.size, [64, 64]);
    assert_eq!(
        blue.pixels[0],
        mcsapi::toolkit::egui::Color32::from_rgb(0, 0, 255)
    );
    fs::remove_dir_all(&root).unwrap();
}
