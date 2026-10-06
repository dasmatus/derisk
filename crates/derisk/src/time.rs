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
        Self::from_unix(unix_now(), 0)
    }

    /// The current wall-clock time in the machine's time zone, the one
    /// first-boot setup and `timedatectl set-timezone` point /etc/localtime
    /// at. The file is read on every call, so a new zone shows at the next
    /// tick; with no readable zone file the clock stays on UTC.
    pub fn now_local() -> Self {
        let secs = unix_now();
        let offset = std::fs::read("/etc/localtime")
            .ok()
            .and_then(|tzif| utc_offset(&tzif, secs))
            .unwrap_or(0);
        Self::from_unix(secs, offset)
    }

    /// `HH:MM`.
    pub fn time_label(&self) -> String {
        format!("{:02}:{:02}", self.hour, self.minute)
    }

    /// e.g. `3:07 PM`.
    pub fn time_label_12h(&self) -> String {
        let hour = match self.hour % 12 {
            0 => 12,
            h => h,
        };
        let half = if self.hour < 12 { "AM" } else { "PM" };
        format!("{hour}:{:02} {half}", self.minute)
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

fn unix_now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs() as i64)
}

/// The UTC offset in seconds at Unix time `at`, from a TZif file (RFC 8536)
/// such as /etc/localtime. Times inside the transition table use it; later
/// ones use the POSIX TZ rule in the file's footer, which is all a "slim"
/// zone file (the zic default) carries for anything after its last
/// transition. `None` when the data is not TZif.
pub fn utc_offset(tzif: &[u8], at: i64) -> Option<i64> {
    let v1 = Header::parse(tzif)?;
    let (header, body, wide) = if tzif[4] >= b'2' {
        // Version 2 and later repeat the data with 64-bit times after the
        // version 1 block, then a footer; only that second half is read.
        let rest = tzif.get(44 + v1.len(4)..)?;
        (Header::parse(rest)?, rest.get(44..)?, true)
    } else {
        (v1, tzif.get(44..)?, false)
    };
    let tsize = if wide { 8 } else { 4 };
    let times = body.get(..header.timecnt * tsize)?;
    let idxs = body.get(header.timecnt * tsize..header.timecnt * (tsize + 1))?;
    let types = body
        .get(header.timecnt * (tsize + 1)..)?
        .get(..header.typecnt * 6)?;
    let offset_of = |i: usize| -> Option<i64> {
        let t = types.get(i * 6..i * 6 + 4)?;
        Some(i64::from(i32::from_be_bytes(t.try_into().ok()?)))
    };
    let time_at = |i: usize| -> i64 {
        let b = &times[i * tsize..(i + 1) * tsize];
        if wide {
            i64::from_be_bytes(b.try_into().unwrap_or_default())
        } else {
            i64::from(i32::from_be_bytes(b.try_into().unwrap_or_default()))
        }
    };
    // Transitions are sorted; the last one at or before `at` sets the type.
    let past = (0..header.timecnt)
        .take_while(|&i| time_at(i) <= at)
        .count();
    let footer = wide
        .then(|| body.get(header.len(8)..))
        .flatten()
        .and_then(|f| std::str::from_utf8(f).ok())
        .and_then(|f| f.trim_start_matches('\n').split('\n').next())
        .filter(|f| !f.is_empty());
    if past == header.timecnt
        && let Some(rule) = footer.and_then(PosixTz::parse)
    {
        return Some(rule.offset_at(at));
    }
    match past {
        // Before the first transition, the first type is in force.
        0 => offset_of(0),
        n => offset_of(usize::from(*idxs.get(n - 1)?)),
    }
}

/// The counts in a TZif header.
struct Header {
    isutcnt: usize,
    isstdcnt: usize,
    leapcnt: usize,
    timecnt: usize,
    typecnt: usize,
    charcnt: usize,
}

impl Header {
    fn parse(data: &[u8]) -> Option<Self> {
        if data.get(..4)? != b"TZif" {
            return None;
        }
        let n = |i: usize| -> Option<usize> {
            Some(u32::from_be_bytes(data.get(20 + i * 4..24 + i * 4)?.try_into().ok()?) as usize)
        };
        Some(Self {
            isutcnt: n(0)?,
            isstdcnt: n(1)?,
            leapcnt: n(2)?,
            timecnt: n(3)?,
            typecnt: n(4)?,
            charcnt: n(5)?,
        })
    }

    /// Bytes of data after the header, for `tsize`-byte times.
    fn len(&self, tsize: usize) -> usize {
        self.timecnt * (tsize + 1)
            + self.typecnt * 6
            + self.charcnt
            + self.leapcnt * (tsize + 4)
            + self.isstdcnt
            + self.isutcnt
    }
}

/// A POSIX TZ rule such as `CET-1CEST,M3.5.0,M10.5.0/3`: standard and
/// daylight offsets east of UTC in seconds, and when daylight time runs.
struct PosixTz {
    std: i64,
    dst: Option<(i64, Rule, Rule)>,
}

/// When daylight time starts or ends: a day of the year and the local time
/// of day, in seconds, at which it happens.
#[derive(Clone, Copy)]
enum Day {
    /// `Jn`: day 1 to 365, never counting 29 February.
    Julian(i64),
    /// `n`: day 0 to 365, counting 29 February.
    Zero(i64),
    /// `Mm.w.d`: weekday `d` (0 = Sunday) of week `w` (5 = last) of month `m`.
    Month(i64, i64, i64),
}

#[derive(Clone, Copy)]
struct Rule {
    day: Day,
    time: i64,
}

impl PosixTz {
    fn parse(s: &str) -> Option<Self> {
        let mut p = Cursor(s.as_bytes());
        p.name()?;
        // POSIX offsets count west of UTC; flip them to count east.
        let std = -p.offset()?;
        if p.0.is_empty() {
            return Some(Self { std, dst: None });
        }
        p.name()?;
        let dst = if p.0.first().is_some_and(|c| *c != b',') {
            -p.offset()?
        } else {
            std + 3600
        };
        // A zone name with no rule means the US rules POSIX defaults to.
        let (start, end) = if p.eat(b',') {
            let start = p.rule()?;
            p.eat(b',').then_some(())?;
            (start, p.rule()?)
        } else {
            let at2 = 2 * 3600;
            (
                Rule {
                    day: Day::Month(3, 2, 0),
                    time: at2,
                },
                Rule {
                    day: Day::Month(11, 1, 0),
                    time: at2,
                },
            )
        };
        Some(Self {
            std,
            dst: Some((dst, start, end)),
        })
    }

    fn offset_at(&self, at: i64) -> i64 {
        let Some((dst, start, end)) = self.dst else {
            return self.std;
        };
        let year = Clock::from_unix(at, self.std).year;
        // Daylight time starts by the standard clock and ends by the
        // daylight one, both given in local time.
        let begins = start.unix(year) - self.std;
        let ends = end.unix(year) - dst;
        let in_dst = if begins < ends {
            at >= begins && at < ends
        } else {
            // Southern hemisphere: daylight time spans the new year.
            at >= begins || at < ends
        };
        if in_dst { dst } else { self.std }
    }
}

impl Rule {
    /// Local seconds since the epoch at which this rule fires in `year`.
    fn unix(self, year: i32) -> i64 {
        let jan1 = days_from_civil(i64::from(year), 1, 1);
        let leap = (year % 4 == 0 && year % 100 != 0) || year % 400 == 0;
        let day = match self.day {
            Day::Julian(n) => jan1 + n - 1 + i64::from(leap && n >= 60),
            Day::Zero(n) => jan1 + n,
            Day::Month(m, w, d) => {
                let first = days_from_civil(i64::from(year), m, 1);
                // 1970-01-01 was a Thursday, weekday 4 counting from Sunday.
                let first_wd = (first + 4).rem_euclid(7);
                let mut day = first + (d - first_wd).rem_euclid(7) + (w - 1) * 7;
                let next = if m == 12 {
                    days_from_civil(i64::from(year) + 1, 1, 1)
                } else {
                    days_from_civil(i64::from(year), m + 1, 1)
                };
                while day >= next {
                    day -= 7;
                }
                day
            }
        };
        day * 86_400 + self.time
    }
}

/// Days since 1970-01-01 of a civil date (Howard Hinnant's days_from_civil).
fn days_from_civil(year: i64, month: i64, day: i64) -> i64 {
    let y = if month <= 2 { year - 1 } else { year };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let mp = (month + 9) % 12;
    let doy = (153 * mp + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

/// Reads a POSIX TZ string front to back.
struct Cursor<'a>(&'a [u8]);

impl Cursor<'_> {
    fn eat(&mut self, c: u8) -> bool {
        let hit = self.0.first() == Some(&c);
        if hit {
            self.0 = &self.0[1..];
        }
        hit
    }

    /// A zone abbreviation: letters, or anything quoted in `<>` (`<+03>`).
    fn name(&mut self) -> Option<()> {
        let len = if self.eat(b'<') {
            let end = self.0.iter().position(|c| *c == b'>')?;
            end + 1
        } else {
            self.0
                .iter()
                .take_while(|c| c.is_ascii_alphabetic())
                .count()
        };
        (len >= 1).then(|| self.0 = &self.0[len..])
    }

    fn number(&mut self) -> Option<i64> {
        let len = self.0.iter().take_while(|c| c.is_ascii_digit()).count();
        let n = std::str::from_utf8(&self.0[..len]).ok()?.parse().ok()?;
        self.0 = &self.0[len..];
        Some(n)
    }

    /// `[+-]hh[:mm[:ss]]` in seconds; also the time of a rule, which may run
    /// past 24 hours or below zero.
    fn offset(&mut self) -> Option<i64> {
        let sign = if self.eat(b'-') {
            -1
        } else {
            self.eat(b'+');
            1
        };
        let mut secs = self.number()? * 3600;
        if self.eat(b':') {
            secs += self.number()? * 60;
            if self.eat(b':') {
                secs += self.number()?;
            }
        }
        Some(sign * secs)
    }

    fn rule(&mut self) -> Option<Rule> {
        let day = if self.eat(b'J') {
            Day::Julian(self.number()?)
        } else if self.eat(b'M') {
            let m = self.number()?;
            self.eat(b'.').then_some(())?;
            let w = self.number()?;
            self.eat(b'.').then_some(())?;
            Day::Month(m, w, self.number()?)
        } else {
            Day::Zero(self.number()?)
        };
        let time = if self.eat(b'/') {
            self.offset()?
        } else {
            2 * 3600
        };
        Some(Rule { day, time })
    }
}
