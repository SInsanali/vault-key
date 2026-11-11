//! Clock and calendar: the standard library has no calendar functions, so the UTC
//! conversion for backup names lives here.

use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

/// Where "now" comes from. Injected so tests are deterministic.
pub type Clock = Arc<dyn Fn() -> SystemTime + Send + Sync>;

/// The real clock.
pub fn system_clock() -> Clock {
    Arc::new(SystemTime::now)
}

/// A clock that always gives the same moment, `unix_seconds` after 1970-01-01 00:00:00 UTC.
pub fn fixed_clock(unix_seconds: u64) -> Clock {
    Arc::new(move || UNIX_EPOCH + std::time::Duration::from_secs(unix_seconds))
}

/// Whole seconds since 1970-01-01 00:00:00 UTC. A time before that counts as 0.
pub fn unix_seconds(time: SystemTime) -> u64 {
    time.duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs())
}

/// A moment in UTC, in calendar form.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct UtcDateTime {
    pub year: u64,
    /// 1 to 12.
    pub month: u32,
    /// 1 to 31.
    pub day: u32,
    pub hour: u32,
    pub minute: u32,
    pub second: u32,
}

const SECONDS_PER_DAY: u64 = 86_400;
/// Days from 0000-03-01 to 1970-01-01 in the proleptic Gregorian calendar.
const DAYS_TO_EPOCH: u64 = 719_468;
const DAYS_PER_400_YEARS: u64 = 146_097;

/// Calendar form of a count of seconds since 1970-01-01 00:00:00 UTC (no leap seconds, as
/// with every Unix clock).
pub fn utc_from_unix(unix_seconds: u64) -> UtcDateTime {
    let days = unix_seconds / SECONDS_PER_DAY;
    let rest = (unix_seconds % SECONDS_PER_DAY) as u32;

    // Count days from 1 March of year 0, so that the leap day is the last day of a year.
    let z = days + DAYS_TO_EPOCH;
    let era = z / DAYS_PER_400_YEARS;
    let day_of_era = z % DAYS_PER_400_YEARS;
    let year_of_era = (day_of_era - day_of_era / 1_460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let shifted_month = (5 * day_of_year + 2) / 153; // 0 = March
    let day = (day_of_year - (153 * shifted_month + 2) / 5 + 1) as u32;
    let month = if shifted_month < 10 { shifted_month + 3 } else { shifted_month - 9 } as u32;
    let year = year_of_era + era * 400 + u64::from(month <= 2);

    UtcDateTime { year, month, day, hour: rest / 3_600, minute: rest % 3_600 / 60, second: rest % 60 }
}

fn is_leap_year(year: u64) -> bool {
    (year.is_multiple_of(4) && !year.is_multiple_of(100)) || year.is_multiple_of(400)
}

fn days_in_month(year: u64, month: u32) -> u32 {
    match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 if is_leap_year(year) => 29,
        2 => 28,
        _ => 0,
    }
}

/// Seconds since 1970-01-01 00:00:00 UTC for a calendar moment. `None` if it is not a real
/// date and time, or is before 1970.
pub fn unix_from_utc(t: &UtcDateTime) -> Option<u64> {
    let valid = t.year >= 1970
        && (1..=12).contains(&t.month)
        && t.day >= 1
        && t.day <= days_in_month(t.year, t.month)
        && t.hour < 24
        && t.minute < 60
        && t.second < 60;
    if !valid {
        return None;
    }
    // The inverse of the conversion above: years start on 1 March.
    let year = if t.month <= 2 { t.year - 1 } else { t.year };
    let era = year / 400;
    let year_of_era = year % 400;
    let shifted_month = u64::from(if t.month > 2 { t.month - 3 } else { t.month + 9 });
    let day_of_year = (153 * shifted_month + 2) / 5 + u64::from(t.day) - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    let days = (era * DAYS_PER_400_YEARS + day_of_era).checked_sub(DAYS_TO_EPOCH)?;
    days.checked_mul(SECONDS_PER_DAY)?
        .checked_add(u64::from(t.hour) * 3_600 + u64::from(t.minute) * 60 + u64::from(t.second))
}

/// The name of a backup made at this moment: `20261109-031500Z` (UTC).
pub fn backup_stamp(unix_seconds: u64) -> String {
    let t = utc_from_unix(unix_seconds);
    format!("{:04}{:02}{:02}-{:02}{:02}{:02}Z", t.year, t.month, t.day, t.hour, t.minute, t.second)
}

/// The moment a [`backup_stamp`] names. `None` unless the text has exactly that form.
pub fn parse_backup_stamp(stamp: &str) -> Option<u64> {
    let bytes = stamp.as_bytes();
    if bytes.len() != 16 || bytes[8] != b'-' || bytes[15] != b'Z' {
        return None;
    }
    let number = |range: std::ops::Range<usize>| -> Option<u32> {
        let digits = stamp.get(range)?;
        if !digits.bytes().all(|b| b.is_ascii_digit()) {
            return None;
        }
        digits.parse().ok()
    };
    unix_from_utc(&UtcDateTime {
        year: u64::from(number(0..4)?),
        month: number(4..6)?,
        day: number(6..8)?,
        hour: number(9..11)?,
        minute: number(11..13)?,
        second: number(13..15)?,
    })
}

/// A moment for people to read: `2026-11-09 03:15:00 UTC`.
pub fn display_utc(unix_seconds: u64) -> String {
    let t = utc_from_unix(unix_seconds);
    format!("{:04}-{:02}-{:02} {:02}:{:02}:{:02} UTC", t.year, t.month, t.day, t.hour, t.minute, t.second)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn utc(year: u64, month: u32, day: u32, hour: u32, minute: u32, second: u32) -> UtcDateTime {
        UtcDateTime { year, month, day, hour, minute, second }
    }

    /// Seconds since 1970, and the year, month, day, hour, minute and second they are.
    type Known = (u64, (u64, u32, u32, u32, u32, u32));

    /// Reference values from `date -u -d '<date>' +%s`.
    const KNOWN: [Known; 8] = [
        (0, (1970, 1, 1, 0, 0, 0)),
        (86_399, (1970, 1, 1, 23, 59, 59)),
        (86_400, (1970, 1, 2, 0, 0, 0)),
        (946_684_799, (1999, 12, 31, 23, 59, 59)),
        (951_868_800, (2000, 3, 1, 0, 0, 0)),
        (1_709_251_199, (2024, 2, 29, 23, 59, 59)),
        (1_794_194_100, (2026, 11, 9, 3, 15, 0)),
        (4_107_585_600, (2100, 3, 1, 12, 0, 0)),
    ];

    #[test]
    fn known_moments_convert_both_ways() {
        for (seconds, (y, mo, d, h, mi, s)) in KNOWN {
            let expected = utc(y, mo, d, h, mi, s);
            assert_eq!(utc_from_unix(seconds), expected, "{seconds}");
            assert_eq!(unix_from_utc(&expected), Some(seconds), "{expected:?}");
        }
    }

    #[test]
    fn every_day_for_two_centuries_round_trips_in_order() {
        // 1970 to past 2170, which covers the leap-year exceptions at 2000 (leap) and 2100 (not).
        let mut previous = utc_from_unix(0);
        for day in 1..73_500u64 {
            let seconds = day * SECONDS_PER_DAY + 12 * 3_600;
            let t = utc_from_unix(seconds);
            assert_eq!(unix_from_utc(&t), Some(seconds), "{t:?}");
            let next_day = t.day == previous.day + 1 && t.month == previous.month && t.year == previous.year;
            let next_month = t.day == 1
                && previous.day == days_in_month(previous.year, previous.month)
                && ((t.month == previous.month + 1 && t.year == previous.year)
                    || (t.month == 1 && previous.month == 12 && t.year == previous.year + 1));
            assert!(next_day || next_month, "{previous:?} then {t:?}");
            previous = t;
        }
        assert_eq!(previous.year, 2171);
    }

    #[test]
    fn leap_days() {
        assert_eq!(days_in_month(2000, 2), 29);
        assert_eq!(days_in_month(2024, 2), 29);
        assert_eq!(days_in_month(2026, 2), 28);
        assert_eq!(days_in_month(2100, 2), 28);
        assert_eq!(unix_from_utc(&utc(2026, 2, 29, 0, 0, 0)), None);
        assert_eq!(unix_from_utc(&utc(2100, 2, 29, 0, 0, 0)), None);
        assert!(unix_from_utc(&utc(2024, 2, 29, 0, 0, 0)).is_some());
    }

    #[test]
    fn impossible_moments_are_rejected() {
        for bad in [
            utc(1969, 12, 31, 23, 59, 59),
            utc(2026, 0, 1, 0, 0, 0),
            utc(2026, 13, 1, 0, 0, 0),
            utc(2026, 4, 31, 0, 0, 0),
            utc(2026, 4, 0, 0, 0, 0),
            utc(2026, 4, 1, 24, 0, 0),
            utc(2026, 4, 1, 0, 60, 0),
            utc(2026, 4, 1, 0, 0, 60),
        ] {
            assert_eq!(unix_from_utc(&bad), None, "{bad:?}");
        }
    }

    #[test]
    fn stamps_format_and_parse() {
        assert_eq!(backup_stamp(1_794_194_100), "20261109-031500Z");
        assert_eq!(backup_stamp(0), "19700101-000000Z");
        assert_eq!(parse_backup_stamp("20261109-031500Z"), Some(1_794_194_100));
        assert_eq!(display_utc(1_794_194_100), "2026-11-09 03:15:00 UTC");
        for bad in [
            "",
            "20261109-031500",
            "20261109-031500Z-2",
            "20261109T031500Z",
            "20261309-031500Z",
            "2026110a-031500Z",
            "+0261109-031500Z",
            "20261109-031500z",
            "20261109-0315éZ",
        ] {
            assert_eq!(parse_backup_stamp(bad), None, "{bad}");
        }
    }

    #[test]
    fn clocks() {
        assert_eq!(unix_seconds(fixed_clock(1_794_194_100)()), 1_794_194_100);
        assert_eq!(unix_seconds(UNIX_EPOCH - std::time::Duration::from_secs(5)), 0);
        assert!(unix_seconds(system_clock()()) > 1_700_000_000);
    }
}
