//! A dependency-free civil clock for the top bar and widgets.

use serde::{Deserialize, Serialize};

/// A calendar date and wall-clock time.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub struct Clock {
    /// Year, e.g. 2026.
    pub year: i32,
    /// Month, 1–12.
    pub month: u8,
    /// Day of month, 1–31.
    pub day: u8,
    /// Hour, 0–23.
    pub hour: u8,
    /// Minute, 0–59.
    pub minute: u8,
    /// Day of week, 0 = Monday.
    pub weekday: u8,
}

const WEEKDAYS: [&str; 7] = ["Mon", "Tue", "Wed", "Thu", "Fri", "Sat", "Sun"];
const MONTHS: [&str; 12] = [
    "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
];

impl Clock {
    /// Converts Unix seconds plus a UTC offset into civil time.
    pub fn from_unix(seconds: i64, utc_offset: i64) -> Self {
        let local = seconds + utc_offset;
        let days = local.div_euclid(86_400);
        let secs = local.rem_euclid(86_400);
        // Howard Hinnant's civil_from_days.
        let z = days + 719_468;
        let era = z.div_euclid(146_097);
        let doe = z - era * 146_097;
        let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
        let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
        let mp = (5 * doy + 2) / 153;
        let day = doy - (153 * mp + 2) / 5 + 1;
        let month = if mp < 10 { mp + 3 } else { mp - 9 };
        let year = yoe + era * 400 + i64::from(month <= 2);
        Self {
            year: year as i32,
            month: month as u8,
            day: day as u8,
            hour: (secs / 3600) as u8,
            minute: (secs % 3600 / 60) as u8,
            // 1970-01-01 was a Thursday.
            weekday: (days + 3).rem_euclid(7) as u8,
        }
    }

    /// The current UTC time from the system clock.
    pub fn now_utc() -> Self {
        let secs = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_secs() as i64);
        Self::from_unix(secs, 0)
    }

    /// `HH:MM`.
    pub fn time_label(&self) -> String {
        format!("{:02}:{:02}", self.hour, self.minute)
    }

    /// e.g. `Sat 3 Oct`.
    pub fn date_label(&self) -> String {
        format!(
            "{} {} {}",
            WEEKDAYS[usize::from(self.weekday % 7)],
            self.day,
            MONTHS[usize::from(self.month.clamp(1, 12) - 1)]
        )
    }

    /// Number of days in this clock's month.
    pub fn days_in_month(&self) -> u8 {
        match self.month {
            2 if (self.year % 4 == 0 && self.year % 100 != 0) || self.year % 400 == 0 => 29,
            2 => 28,
            4 | 6 | 9 | 11 => 30,
            _ => 31,
        }
    }

    /// Weekday (0 = Monday) of the first day of this clock's month.
    pub fn first_weekday(&self) -> u8 {
        ((i32::from(self.weekday) - (i32::from(self.day) - 1)).rem_euclid(7)) as u8
    }
}
