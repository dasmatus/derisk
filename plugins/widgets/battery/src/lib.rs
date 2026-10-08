//! Battery: the charge as a bar, and whether it is charging. Nothing on a
//! device without a battery.

use derisk_widget_sdk::{Card, Guest, Input, Manifest, Text, View, export};

struct Battery;

impl Guest for Battery {
    fn describe() -> Manifest {
        Manifest::new("battery", "Battery level, on a device with one").inputs(&[Input::Battery])
    }

    fn render(view: View) -> Vec<Card> {
        card(&view).into_iter().collect()
    }
}

/// The card, apart from the export so tests can call it.
pub fn card(view: &View) -> Option<Card> {
    let battery = view.battery.as_ref()?;
    Some(
        Card::new("battery")
            .text(Text::plain(if battery.charging {
                "Battery · charging"
            } else {
                "Battery"
            }))
            .progress(
                f32::from(battery.percent) / 100.0,
                Some(format!("{}%", battery.percent)),
            ),
    )
}

export!(Battery with_types_in derisk_widget_sdk::bindings);

#[cfg(test)]
mod tests {
    use derisk_widget_sdk::{Battery, Part};

    use super::*;

    #[test]
    fn a_battery_is_a_bar_and_none_is_no_card() {
        assert!(card(&View::default()).is_none());
        let view = View {
            battery: Some(Battery {
                percent: 40,
                charging: true,
            }),
            ..View::default()
        };
        let card = card(&view).unwrap();
        assert_eq!(card.texts(), ["Battery · charging", "40%"]);
        let Part::Progress(bar) = &card.parts[1] else {
            panic!("no bar")
        };
        assert!((bar.value - 0.4).abs() < f32::EPSILON);
    }
}
