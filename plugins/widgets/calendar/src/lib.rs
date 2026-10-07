//! Calendar: this month, a week to a row from Monday, today in the
//! accent.

use derisk_widget_sdk::{
    Card, Emphasis, Grid, Guest, Input, Manifest, Part, Text, Tone, View, export,
};

struct Calendar;

impl Guest for Calendar {
    fn describe() -> Manifest {
        Manifest::new("calendar", "This month's calendar").inputs(&[Input::Clock])
    }

    fn render(view: View) -> Vec<Card> {
        vec![card(&view)]
    }
}

/// The card, apart from the export so tests can call it.
pub fn card(view: &View) -> Card {
    let clock = &view.clock;
    let header = ["M", "T", "W", "T", "F", "S", "S"]
        .into_iter()
        .map(|d| Text::plain(d).tone(Tone::Dim));
    let blanks = (0..clock.first_weekday()).map(|_| Text::plain(""));
    let days = (1..=clock.days_in_month()).map(|day| {
        let text = Text::plain(format!("{day:>2}"));
        if day == clock.day {
            text.tone(Tone::Accent).emphasis(Emphasis::Strong)
        } else {
            text
        }
    });
    Card::new("calendar")
        .text(Text::heading(clock.date_label()))
        .part(Part::Grid(Grid {
            columns: 7,
            cells: header.chain(blanks).chain(days).collect(),
        }))
}

export!(Calendar with_types_in derisk_widget_sdk::bindings);

#[cfg(test)]
mod tests {
    use derisk_widget_sdk::Clock;

    use super::*;

    #[test]
    fn lays_the_month_out_from_monday_with_today_marked() {
        // Saturday 3 October 2026: the 1st is a Thursday.
        let view = View {
            clock: Clock {
                year: 2026,
                month: 10,
                day: 3,
                hour: 9,
                minute: 0,
                weekday: 5,
                twenty_four_hour: true,
            },
            ..View::default()
        };
        let card = card(&view);
        let Part::Grid(grid) = &card.parts[1] else {
            panic!("no grid")
        };
        assert_eq!(grid.columns, 7);
        // Seven day letters, three blanks (Mon-Wed), 31 days.
        assert_eq!(grid.cells.len(), 7 + 3 + 31);
        assert_eq!(grid.cells[10].text, " 1");
        let today = &grid.cells[12];
        assert_eq!(today.text, " 3");
        assert_eq!(today.tone, Tone::Accent);
    }
}
