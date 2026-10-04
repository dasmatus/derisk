use std::path::PathBuf;

use derisk_settings::{
    Accent, ColorScheme, Layout, LowPower, Page, Profile, Settings, SettingsApp,
};
use mcsapi_ui::{Theme, egui, run_frame};

fn temp_dir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("derisk-settings-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

#[test]
fn text_round_trips_every_field() {
    let mut settings = Settings::default();
    settings.appearance.scheme = ColorScheme::Light;
    settings.appearance.accent = Accent::Rose;
    settings.appearance.text_scale = 1.25;
    settings.appearance.reduce_motion = true;
    settings.desktop.layout = Layout::Monocle;
    settings.desktop.gaps = 0;
    settings.desktop.workspaces = 4;
    settings.desktop.profile = Profile::Tablet;
    settings.input.natural_scroll = false;
    settings.input.repeat_delay_ms = 250;
    settings.input.repeat_rate = 40;
    settings.notifications.do_not_disturb = true;
    settings.power.suspend_after_min = 0;
    settings.appearance.blur = 0;
    settings.appearance.panels.top_bar = 100;
    settings.appearance.panels.overview = 35;
    settings.appearance.panels.snap_assist = 20;
    settings.power.low_power = LowPower::On;
    let (parsed, warnings) = Settings::parse(&settings.to_text());
    assert!(warnings.is_empty(), "{warnings:?}");
    assert_eq!(parsed, settings);
}

#[test]
fn invalid_lines_warn_and_keep_defaults() {
    let text = "\
        # comment\n\
        desktop.workspaces = 12\n\
        desktop.gaps = 16 # trailing comment\n\
        appearance.accent = plaid\n\
        no equals sign\n\
        unknown.key = 1\n\
        input.tap_to_click = false\n";
    let (settings, warnings) = Settings::parse(text);
    assert_eq!(settings.desktop.workspaces, 9);
    assert_eq!(settings.desktop.gaps, 16);
    assert_eq!(settings.appearance.accent, Accent::Lime);
    assert!(!settings.input.tap_to_click);
    let lines: Vec<usize> = warnings.iter().map(|w| w.line).collect();
    assert_eq!(lines, [2, 4, 5, 6]);
}

#[test]
fn set_rejects_out_of_range_values() {
    let mut settings = Settings::default();
    assert!(!settings.set("appearance.text_scale", "3"));
    assert!(!settings.set("input.repeat_rate", "0"));
    assert!(!settings.set("power.dim_after_min", "-1"));
    assert!(!settings.set("appearance.blur", "11"));
    assert!(!settings.set("appearance.top_bar_opacity", "10"));
    assert!(!settings.set("power.low_power", "sometimes"));
    assert_eq!(settings, Settings::default());
    assert!(settings.set("power.dim_after_min", "0"));
    assert_eq!(settings.power.dim_after_min, 0);
}

#[test]
fn save_and_load_through_a_file() {
    let dir = temp_dir("file");
    let path = dir.join("nested/settings.conf");
    assert_eq!(
        Settings::load(&path).unwrap(),
        (Settings::default(), vec![])
    );
    let mut settings = Settings::default();
    settings.desktop.layout = Layout::Monocle;
    settings.save(&path).unwrap();
    assert_eq!(Settings::load(&path).unwrap().0, settings);
    assert!(!dir.join("nested/settings.conf.tmp").exists());
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn theme_follows_scheme_and_accent() {
    let mut settings = Settings::default();
    assert_eq!(settings.theme(), Theme::default());
    settings.appearance.accent = Accent::Sky;
    assert_eq!(settings.theme().accent, Accent::Sky.color());
    settings.appearance.scheme = ColorScheme::Light;
    let light = settings.theme();
    assert_ne!(light.background, Theme::default().background);
    assert_ne!(light.foreground, light.background);
}

#[test]
fn app_tracks_unsaved_changes() {
    let dir = temp_dir("app");
    let path = dir.join("settings.conf");
    let mut app = SettingsApp::open(Some(path.clone()));
    assert!(!app.is_dirty());
    app.settings.notifications.sounds = false;
    assert!(app.is_dirty());
    app.revert();
    assert!(app.settings.notifications.sounds);
    app.settings.desktop.gaps = 4;
    app.save();
    assert!(!app.is_dirty());
    assert_eq!(app.status(), Some("Saved"));
    assert_eq!(Settings::load(&path).unwrap().0.desktop.gaps, 4);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn app_reports_invalid_files() {
    let dir = temp_dir("invalid");
    let path = dir.join("settings.conf");
    std::fs::write(&path, "desktop.gaps = lots\n").unwrap();
    let app = SettingsApp::open(Some(path));
    assert!(app.status().unwrap().contains("line 1"));
    let mut memory_only = SettingsApp::open(None);
    memory_only.settings.desktop.gaps = 2;
    memory_only.save();
    assert!(memory_only.is_dirty());
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn every_page_renders() {
    let context = egui::Context::default();
    let mut app = SettingsApp::open(None);
    for page in Page::ALL {
        app.page = page;
        let mut output = run_frame(
            &mut app,
            &context,
            egui::RawInput::default(),
            &Theme::default(),
        );
        assert!(!output.shapes.is_empty());
        output.textures_delta.clear();
    }
}

#[test]
fn low_power_follows_the_battery_only_when_asked() {
    assert_eq!(Settings::default().power.low_power, LowPower::OnBattery);
    for on_battery in [false, true] {
        assert!(!LowPower::Off.active(on_battery));
        assert!(LowPower::On.active(on_battery));
        assert_eq!(LowPower::OnBattery.active(on_battery), on_battery);
    }
    let (settings, warnings) = Settings::parse("power.low_power = on_battery\n");
    assert!(warnings.is_empty());
    assert_eq!(settings.power.low_power, LowPower::OnBattery);
}
