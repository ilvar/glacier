//! UTC timestamps in the two formats the archive uses.
//!
//! This is ordinary calendar arithmetic, not a date library: the archive
//! only ever needs "now" as `YYYY-MM-DDTHH:MM:SSZ` and "today" as
//! `YYYY-MM-DD`, and pulling in a dependency for that would be more
//! surface than the problem deserves. The civil-from-days conversion is
//! Howard Hinnant's well-known algorithm, valid across the proleptic
//! Gregorian calendar.

use std::time::{SystemTime, UNIX_EPOCH};

/// A calendar date and time of day, in UTC.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Utc {
    pub year: i64,
    pub month: u32,
    pub day: u32,
    pub hour: u32,
    pub minute: u32,
    pub second: u32,
}

impl Utc {
    /// `YYYY-MM-DDTHH:MM:SSZ`, the form used for `recorded_at` and
    /// `created_at`.
    pub fn to_timestamp(self) -> String {
        format!(
            "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}Z",
            self.year, self.month, self.day, self.hour, self.minute, self.second
        )
    }

    /// `YYYY-MM-DD`, the form used for dates and session ids.
    pub fn to_date(self) -> String {
        format!("{:04}-{:02}-{:02}", self.year, self.month, self.day)
    }
}

/// Split a Unix timestamp into its UTC calendar parts.
pub fn from_unix_seconds(seconds: i64) -> Utc {
    let days = seconds.div_euclid(86_400);
    let seconds_of_day = seconds.rem_euclid(86_400);

    let (year, month, day) = civil_from_days(days);
    Utc {
        year,
        month,
        day,
        hour: clamp_u32(seconds_of_day / 3_600),
        minute: clamp_u32(seconds_of_day / 60 % 60),
        second: clamp_u32(seconds_of_day % 60),
    }
}

/// Narrow a small, known-in-range value without an `as` cast.
fn clamp_u32(value: i64) -> u32 {
    u32::try_from(value).unwrap_or(0)
}

/// Convert a count of days since 1970-01-01 into a civil date.
fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let shifted = days + 719_468;
    let era = if shifted >= 0 {
        shifted
    } else {
        shifted - 146_096
    } / 146_097;
    let day_of_era = shifted - era * 146_097;
    let year_of_era =
        (day_of_era - day_of_era / 1_460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let year = year_of_era + era * 400;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let shifted_month = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * shifted_month + 2) / 5 + 1;
    let month = if shifted_month < 10 {
        shifted_month + 3
    } else {
        shifted_month - 9
    };
    let year = if month <= 2 { year + 1 } else { year };
    (year, clamp_u32(month), clamp_u32(day))
}

/// Seconds since the Unix epoch, or 0 if the clock predates it.
fn unix_now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .ok()
        .and_then(|elapsed| i64::try_from(elapsed.as_secs()).ok())
        .unwrap_or(0)
}

pub fn now() -> Utc {
    from_unix_seconds(unix_now())
}

/// The current instant as `YYYY-MM-DDTHH:MM:SSZ`.
pub fn now_timestamp() -> String {
    now().to_timestamp()
}

/// Today's date as `YYYY-MM-DD`.
pub fn today() -> String {
    now().to_date()
}

/// Parse a `YYYY-MM-DD` date into a comparable ordinal. Used only to decide
/// whether a replica's sunset date has passed.
pub fn parse_date_ordinal(text: &str) -> Option<i64> {
    let segments: Vec<&str> = text.trim().split('-').collect();
    let year = segments.first()?.parse::<i64>().ok()?;
    let month = segments.get(1)?.parse::<i64>().ok()?;
    let day = segments.get(2)?.parse::<i64>().ok()?;
    if !(1..=12).contains(&month) || !(1..=31).contains(&day) {
        return None;
    }
    Some(year * 10_000 + month * 100 + day)
}

/// Today as the same ordinal `parse_date_ordinal` produces.
pub fn today_ordinal() -> i64 {
    let now = now();
    now.year * 10_000 + i64::from(now.month) * 100 + i64::from(now.day)
}

#[cfg(test)]
mod tests {
    use super::{from_unix_seconds, parse_date_ordinal, today, today_ordinal};

    #[test]
    fn converts_the_epoch() {
        let epoch = from_unix_seconds(0);
        assert_eq!(epoch.to_timestamp(), "1970-01-01T00:00:00Z");
        assert_eq!(epoch.to_date(), "1970-01-01");
    }

    #[test]
    fn converts_a_known_instant() {
        let moment = from_unix_seconds(1_786_726_005);
        assert_eq!(moment.to_timestamp(), "2026-08-14T16:46:45Z");
        assert_eq!(moment.to_date(), "2026-08-14");
    }

    #[test]
    fn handles_a_leap_day() {
        // 2024-02-29T12:00:00Z
        let leap = from_unix_seconds(1_709_208_000);
        assert_eq!(leap.to_date(), "2024-02-29");
    }

    #[test]
    fn today_is_well_formed() {
        let today = today();
        assert_eq!(today.len(), 10);
        assert_eq!(today.matches('-').count(), 2);
    }

    #[test]
    fn date_ordinals_compare_chronologically() {
        let earlier = parse_date_ordinal("2020-01-01").expect("parses");
        let later = parse_date_ordinal("2026-08-14").expect("parses");
        assert!(earlier < later);
        assert_eq!(parse_date_ordinal("not-a-date"), None);
        assert!(today_ordinal() > earlier);
    }
}
