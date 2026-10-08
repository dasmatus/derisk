//! Clock: the time, large, and the date under it. The top bar leaves its
//! own copy out while this card is on the overview.

use derisk_widget_sdk::{Card, Emphasis, Guest, Input, Manifest, Text, View, export};

struct Clock;

impl Guest for Clock {
    fn describe() -> Manifest {
        Manifest::new("clock", "The time and date, large").inputs(&[Input::Clock])
    }

    fn render(view: View) -> Vec<Card> {
        vec![card(&view)]
    }
}

/// The card, apart from the export so tests can call it.
pub fn card(view: &View) -> Card {
    Card::new("clock")
        .text(Text::plain(view.clock.time_label()).emphasis(Emphasis::Display))
        .text(Text::plain(view.clock.date_label()))
}

export!(Clock with_types_in derisk_widget_sdk::bindings);

#[cfg(test)]
mod tests {
    use derisk_widget_sdk::Clock;

    use super::*;

    #[test]
    fn shows_the_time_on_the_persons_clock_and_the_date() {
        let mut view = View {
            clock: Clock {
                year: 2026,
                month: 10,
                day: 3,
                hour: 15,
                minute: 7,
                weekday: 5,
                twenty_four_hour: true,
            },
            ..View::default()
        };
        assert_eq!(card(&view).texts(), ["15:07", "Sat 3 Oct"]);
        view.clock.twenty_four_hour = false;
        assert_eq!(card(&view).texts()[0], "3:07 PM");
    }
}
