//! The two text forms of a .NET `DateTimeOffset` the store keeps: `ToString("O")` in the `started_at`/`seen_at` columns
//! (seven fraction digits) and the JSON form inside snapshots and files (trailing zeros trimmed).

use std::time::{SystemTime, UNIX_EPOCH};

const TICKS_PER_SECOND: i64 = 10_000_000;
const TICKS_PER_DAY: i64 = 86_400 * TICKS_PER_SECOND;
/// Ticks from 0001-01-01 to 1970-01-01.
const UNIX_EPOCH_TICKS: i64 = 621_355_968_000_000_000;

/// A point in time and the offset it was written with; `Default` is .NET's `default(DateTimeOffset)`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct DateTimeOffset {
    /// UTC ticks (100 ns) since 0001-01-01.
    pub utc_ticks: i64,
    pub offset_minutes: i16,
}

impl DateTimeOffset {
    pub fn now() -> Self {
        let since = SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default();
        Self::from_unix_ticks(since.as_nanos() as i64 / 100)
    }

    pub fn from_unix_ticks(ticks: i64) -> Self {
        Self { utc_ticks: UNIX_EPOCH_TICKS + ticks, offset_minutes: 0 }
    }

    /// `DateTimeOffset.FromUnixTimeMilliseconds`.
    pub fn from_unix_milliseconds(milliseconds: i64) -> Self {
        Self::from_unix_ticks(milliseconds * 10_000)
    }

    pub fn unix_microseconds(&self) -> i64 {
        (self.utc_ticks - UNIX_EPOCH_TICKS) / 10
    }

    /// Parses the ISO 8601 forms .NET writes and reads: `2026-10-27T15:51:23.5380000+00:00`, `...Z`, no fraction.
    pub fn parse(text: &str) -> Option<Self> {
        let bytes = text.as_bytes();
        let number = |range: std::ops::Range<usize>| -> Option<i64> {
            let part = text.get(range)?;
            part.bytes().all(|b| b.is_ascii_digit()).then(|| part.parse().ok())?
        };
        if bytes.len() < 19 || bytes[4] != b'-' || bytes[7] != b'-' || !matches!(bytes[10], b'T' | b't' | b' ') || bytes[13] != b':' || bytes[16] != b':' {
            return None;
        }
        let (year, month, day) = (number(0..4)?, number(5..7)?, number(8..10)?);
        let (hour, minute, second) = (number(11..13)?, number(14..16)?, number(17..19)?);
        if !(1..=12).contains(&month) || day < 1 || day > days_in_month(year, month) || hour > 23 || minute > 59 || second > 59 || year < 1 {
            return None;
        }
        let mut index = 19;
        let mut fraction = 0i64;
        if bytes.get(index) == Some(&b'.') {
            index += 1;
            let start = index;
            while index < bytes.len() && bytes[index].is_ascii_digit() {
                index += 1;
            }
            let digits = &text[start..index];
            if digits.is_empty() {
                return None;
            }
            let padded: String = digits.chars().chain(std::iter::repeat('0')).take(7).collect();
            fraction = padded.parse().ok()?;
        }
        let offset_minutes = match &text[index..] {
            "" | "Z" | "z" => 0,
            rest if rest.len() == 6 && matches!(&rest[..1], "+" | "-") && &rest[3..4] == ":" => {
                let hours: i16 = rest[1..3].parse().ok()?;
                let minutes: i16 = rest[4..6].parse().ok()?;
                if hours > 14 || minutes > 59 {
                    return None;
                }
                let total = hours * 60 + minutes;
                if rest.starts_with('-') { -total } else { total }
            }
            _ => return None,
        };
        let local = days_from_civil(year, month, day) * TICKS_PER_DAY + (hour * 3600 + minute * 60 + second) * TICKS_PER_SECOND + fraction;
        Some(Self { utc_ticks: local - offset_minutes as i64 * 60 * TICKS_PER_SECOND, offset_minutes })
    }

    /// `ToString("O")`.
    pub fn to_round_trip(&self) -> String {
        self.format(false)
    }

    /// The System.Text.Json form.
    pub fn to_json(&self) -> String {
        self.format(true)
    }

    fn format(&self, trim: bool) -> String {
        let local = self.utc_ticks + self.offset_minutes as i64 * 60 * TICKS_PER_SECOND;
        let (days, rest) = (local.div_euclid(TICKS_PER_DAY), local.rem_euclid(TICKS_PER_DAY));
        let (year, month, day) = civil_from_days(days);
        let seconds = rest / TICKS_PER_SECOND;
        let fraction = rest % TICKS_PER_SECOND;
        let mut text = format!("{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}", seconds / 3600, seconds / 60 % 60, seconds % 60);
        let digits = format!("{fraction:07}");
        let digits = if trim { digits.trim_end_matches('0') } else { &digits };
        if !digits.is_empty() {
            text.push('.');
            text.push_str(digits);
        }
        let offset = self.offset_minutes.abs();
        text.push_str(&format!("{}{:02}:{:02}", if self.offset_minutes < 0 { '-' } else { '+' }, offset / 60, offset % 60));
        text
    }
}

impl serde::Serialize for DateTimeOffset {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.to_json())
    }
}

impl<'de> serde::Deserialize<'de> for DateTimeOffset {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let text = <std::borrow::Cow<'de, str>>::deserialize(deserializer)?;
        Self::parse(&text).ok_or_else(|| serde::de::Error::custom(format!("invalid date {text:?}")))
    }
}

fn days_in_month(year: i64, month: i64) -> i64 {
    match month {
        2 if year % 4 == 0 && (year % 100 != 0 || year % 400 == 0) => 29,
        2 => 28,
        4 | 6 | 9 | 11 => 30,
        _ => 31,
    }
}

/// Days from 0001-01-01 (Hinnant's algorithm, shifted from the Unix epoch).
fn days_from_civil(year: i64, month: i64, day: i64) -> i64 {
    let year = if month <= 2 { year - 1 } else { year };
    let era = year.div_euclid(400);
    let yoe = year - era * 400;
    let doy = (153 * (if month > 2 { month - 3 } else { month + 9 }) + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468 + 719_162
}

fn civil_from_days(days: i64) -> (i64, i64, i64) {
    let z = days - 719_162 + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    (yoe + era * 400 + i64::from(month <= 2), month, day)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_dotnet_forms() {
        let value = DateTimeOffset::parse("2026-10-27T15:51:23.538+00:00").unwrap();
        assert_eq!(value.to_round_trip(), "2026-10-27T15:51:23.5380000+00:00");
        assert_eq!(value.to_json(), "2026-10-27T15:51:23.538+00:00");
        assert_eq!(DateTimeOffset::parse("1970-01-01T00:00:00Z").unwrap().unix_microseconds(), 0);
        assert_eq!(DateTimeOffset::from_unix_milliseconds(1_794_581_963_183).to_json(), "2026-11-13T14:59:23.183+00:00");
        assert_eq!(DateTimeOffset::parse("0001-01-01T00:00:00+00:00").unwrap(), DateTimeOffset::default());
        let shifted = DateTimeOffset::parse("2026-01-01T01:30:00.1234567-03:00").unwrap();
        assert_eq!(shifted.to_round_trip(), "2026-01-01T01:30:00.1234567-03:00");
        assert_eq!(shifted.utc_ticks, DateTimeOffset::parse("2026-01-01T04:30:00.1234567Z").unwrap().utc_ticks);
        assert!(DateTimeOffset::parse("2026-02-30T00:00:00Z").is_none());
    }
}
