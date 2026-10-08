//! The bundled widget plugins, loaded and called through the sandbox.

use derisk_plugin::widget::{
    Battery, Clock, Emphasis, Inline, Input, Part, Plugins, Registration, View,
};

fn plugins() -> Plugins {
    Plugins::bundled().expect("the bundled widget plugins load")
}

fn view() -> View {
    View {
        clock: Clock {
            year: 2026,
            month: 10,
            day: 7,
            hour: 21,
            minute: 30,
            weekday: 2,
            twenty_four_hour: true,
        },
        battery: Some(Battery {
            percent: 80,
            charging: false,
        }),
        suggested: vec!["files".into()],
        failed_units: vec!["foo.service".into()],
        registered: vec![Registration {
            id: "weather".into(),
            data: r#"{"id":"weather","title":"Weather","rows":[{"type":"button","label":"Refresh","item":"refresh"}]}"#.into(),
        }],
    }
}

#[test]
fn every_bundled_widget_loads_in_board_order() {
    let names: Vec<_> = plugins().iter().map(|p| p.name().to_owned()).collect();
    assert_eq!(
        names,
        [
            "clock",
            "suggestions",
            "services",
            "calendar",
            "battery",
            "notes",
            "programs"
        ]
    );
}

#[test]
fn the_board_comes_from_the_plugins() {
    let plugins = plugins();
    let view = view();
    let keys: Vec<String> = plugins
        .iter()
        .flat_map(|p| plugins.render(p, &view).unwrap())
        .map(|card| card.key)
        .collect();
    assert_eq!(
        keys,
        [
            "clock",
            "suggestions",
            "services",
            "calendar",
            "battery",
            "notes",
            "weather"
        ]
    );
    let clock = plugins.iter().next().unwrap();
    let card = &plugins.render(clock, &view).unwrap()[0];
    let Part::Text(time) = &card.parts[0] else {
        panic!("no time")
    };
    assert_eq!(time.text, "21:30");
    assert_eq!(time.emphasis, Emphasis::Display);
}

#[test]
fn a_plugin_sees_only_what_it_asked_for() {
    let plugins = plugins();
    let view = view();
    let battery = plugins.iter().find(|p| p.name() == "battery").unwrap();
    assert!(battery.reads(Input::Battery));
    let seen = battery.view(&view);
    assert!(seen.battery.is_some());
    assert!(seen.suggested.is_empty());
    assert!(seen.failed_units.is_empty());
    assert!(seen.registered.is_empty());
    assert_eq!(seen.clock.year, 2000);
    let notes = plugins.iter().find(|p| p.name() == "notes").unwrap();
    assert_eq!(notes.view(&view), View::default());
}

#[test]
fn a_button_outside_the_manifest_drops_its_card() {
    let plugins = plugins();
    let services = plugins.iter().find(|p| p.name() == "services").unwrap();
    let mut card = plugins.render(services, &view()).unwrap().remove(0);
    assert!(services.check(&card).is_ok());
    let Some(Part::Row(items)) = card.parts.get_mut(1) else {
        panic!("no row")
    };
    let Some(Inline::Button(button)) = items.get_mut(1) else {
        panic!("no button")
    };
    button.actions = vec![r#"{"action":"session","op":"power_off"}"#.into()];
    assert!(services.check(&card).is_err());
}
