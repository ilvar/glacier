//! Fuzzy date parsing shared by story, timeline, and media ingestion.
//!
//! Life stories rarely come with a precise date. We support four precise
//! forms (day/month/year/decade) plus "unknown", and never invent precision
//! the user did not give us.

use std::fmt;

pub const PRECISIONS: [&str; 5] = ["day", "month", "year", "decade", "unknown"];

/// A date known to some precision, or not known at all.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FuzzyDate {
    /// The canonical string stored in frontmatter (e.g. "1994-09-15",
    /// "1994-09", "1994", "1970s"), or `None` when precision is unknown.
    pub value: Option<String>,
    pub precision: Precision,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Precision {
    Day,
    Month,
    Year,
    Decade,
    Unknown,
}

impl Precision {
    pub fn as_str(self) -> &'static str {
        match self {
            Precision::Day => "day",
            Precision::Month => "month",
            Precision::Year => "year",
            Precision::Decade => "decade",
            Precision::Unknown => "unknown",
        }
    }

    /// Parse a precision written in frontmatter. Anything unrecognized is
    /// treated as unknown rather than rejected, so a hand-edited file with a
    /// typo still loads and can be fixed.
    pub fn from_str_lossy(raw: &str) -> Precision {
        match raw {
            "day" => Precision::Day,
            "month" => Precision::Month,
            "year" => Precision::Year,
            "decade" => Precision::Decade,
            "unknown" => Precision::Unknown,
            _unrecognized => Precision::Unknown,
        }
    }
}

impl fmt::Display for Precision {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

#[derive(Debug, PartialEq, Eq)]
pub struct DateError(pub String);

impl fmt::Display for DateError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl std::error::Error for DateError {}

fn all_digits(text: &str) -> bool {
    !text.is_empty() && text.chars().all(|character| character.is_ascii_digit())
}

fn parse_u32(text: &str) -> Option<u32> {
    if all_digits(text) {
        text.parse::<u32>().ok()
    } else {
        None
    }
}

/// Split "1994-09-15" into its dash-separated parts without indexing.
fn parts(raw: &str) -> Vec<&str> {
    raw.split('-').collect()
}

impl FuzzyDate {
    pub fn unknown() -> FuzzyDate {
        FuzzyDate {
            value: None,
            precision: Precision::Unknown,
        }
    }

    /// The `timeline/<year>/` bucket this date belongs to, or `None` for
    /// undated stories, which live in `timeline/undated/`.
    pub fn year(&self) -> Option<u32> {
        let value = self.value.as_deref()?;
        match self.precision {
            Precision::Decade => value
                .strip_suffix('s')
                .and_then(parse_u32)
                .map(|decade| decade / 10 * 10),
            Precision::Day | Precision::Month | Precision::Year => {
                parts(value).first().copied().and_then(parse_u32)
            }
            Precision::Unknown => None,
        }
    }

    /// Best-effort chronological sort key; unknown dates sort last.
    pub fn sort_key(&self) -> SortKey {
        self.precise_sort_key().unwrap_or(SortKey::UNKNOWN)
    }

    /// The sort key when the stored value actually parses, or `None` when
    /// it does not — in which case the story sorts with the undated ones.
    fn precise_sort_key(&self) -> Option<SortKey> {
        let value = self.value.as_deref()?;
        let segments = parts(value);
        let year = segments.first().copied().and_then(parse_u32);
        match self.precision {
            Precision::Day => Some(SortKey {
                year: year?,
                month: segments.get(1).copied().and_then(parse_u32)?,
                day: segments.get(2).copied().and_then(parse_u32)?,
            }),
            Precision::Month => Some(SortKey {
                year: year?,
                month: segments.get(1).copied().and_then(parse_u32)?,
                day: 0,
            }),
            Precision::Year => Some(SortKey {
                year: year?,
                month: 0,
                day: 0,
            }),
            Precision::Decade => Some(SortKey {
                year: self.year()?,
                month: 0,
                day: 0,
            }),
            Precision::Unknown => None,
        }
    }
}

/// Chronological ordering key. Field order is the comparison order, so the
/// derived `Ord` is exactly year-then-month-then-day.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct SortKey {
    pub year: u32,
    pub month: u32,
    pub day: u32,
}

impl SortKey {
    /// Undated stories sort after every real date.
    pub const UNKNOWN: SortKey = SortKey {
        year: 9999,
        month: 99,
        day: 99,
    };
}

/// Parse a user-supplied date string.
///
/// Accepts `YYYY-MM-DD`, `YYYY-MM`, `YYYY`, `YYYYs` (decade), `unknown`, or
/// an empty string. Anything else is an error rather than a guess.
pub fn parse_fuzzy_date(raw: Option<&str>) -> Result<FuzzyDate, DateError> {
    let Some(raw) = raw else {
        return Ok(FuzzyDate::unknown());
    };
    let trimmed = raw.trim();
    if trimmed.is_empty() || trimmed.eq_ignore_ascii_case("unknown") {
        return Ok(FuzzyDate::unknown());
    }

    if let Some(decade) = trimmed.strip_suffix('s') {
        if decade.len() == 4 && all_digits(decade) && decade.ends_with('0') {
            return Ok(FuzzyDate {
                value: Some(trimmed.to_owned()),
                precision: Precision::Decade,
            });
        }
    }

    let segments = parts(trimmed);
    let lengths: Vec<usize> = segments.iter().map(|segment| segment.len()).collect();
    let numeric = segments.iter().all(|segment| all_digits(segment));

    if numeric {
        // Precision follows directly from how many components were given.
        let precision = match lengths.as_slice() {
            [4] => Some(Precision::Year),
            [4, 2] => Some(Precision::Month),
            [4, 2, 2] => Some(Precision::Day),
            _other_shape => None,
        };
        if let Some(precision) = precision {
            return Ok(FuzzyDate {
                value: Some(trimmed.to_owned()),
                precision,
            });
        }
    }

    Err(DateError(format!(
        "Unrecognized date {trimmed:?}; use YYYY-MM-DD, YYYY-MM, YYYY, YYYYs, or 'unknown'"
    )))
}

/// Rebuild a `FuzzyDate` from stored frontmatter, trusting the recorded
/// precision. Used when reading files we previously wrote.
pub fn from_frontmatter(value: Option<&str>, precision: Option<&str>) -> FuzzyDate {
    let precision = precision.map_or(Precision::Unknown, Precision::from_str_lossy);
    match value {
        None => FuzzyDate::unknown(),
        Some(value) if matches!(precision, Precision::Unknown) => FuzzyDate {
            value: Some(value.to_owned()),
            precision: Precision::Unknown,
        },
        Some(value) if value.ends_with('s') => FuzzyDate {
            value: Some(value.to_owned()),
            precision: Precision::Decade,
        },
        Some(value) => FuzzyDate {
            value: Some(value.to_owned()),
            precision,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::{parse_fuzzy_date, Precision};

    fn parse(raw: &str) -> super::FuzzyDate {
        parse_fuzzy_date(Some(raw)).expect("date should parse")
    }

    #[test]
    fn day_precision() {
        let date = parse("1994-09-15");
        assert_eq!(date.value.as_deref(), Some("1994-09-15"));
        assert_eq!(date.precision, Precision::Day);
        assert_eq!(date.year(), Some(1994));
    }

    #[test]
    fn month_precision() {
        let date = parse("1978-03");
        assert_eq!(date.precision, Precision::Month);
        assert_eq!(date.year(), Some(1978));
    }

    #[test]
    fn year_precision() {
        let date = parse("1994");
        assert_eq!(date.precision, Precision::Year);
        assert_eq!(date.year(), Some(1994));
    }

    #[test]
    fn decade_precision() {
        let date = parse("1970s");
        assert_eq!(date.precision, Precision::Decade);
        assert_eq!(date.year(), Some(1970));
    }

    #[test]
    fn unknown_forms() {
        for raw in ["", "unknown", "  unknown  "] {
            let date = parse(raw);
            assert_eq!(date.precision, Precision::Unknown);
            assert_eq!(date.value, None);
            assert_eq!(date.year(), None);
        }
        let none = parse_fuzzy_date(None).expect("None is unknown");
        assert_eq!(none.precision, Precision::Unknown);
    }

    #[test]
    fn rejects_garbage() {
        assert!(parse_fuzzy_date(Some("sometime in the 90s")).is_err());
        assert!(parse_fuzzy_date(Some("94-09-15")).is_err());
        assert!(parse_fuzzy_date(Some("1994-9-15")).is_err());
    }

    #[test]
    fn sort_key_orders_unknown_last() {
        assert!(parse("1994-09-15").sort_key() < parse("unknown").sort_key());
        assert!(parse("1970s").sort_key() < parse("1994").sort_key());
        assert!(parse("1994-09").sort_key() < parse("1994-09-15").sort_key());
    }
}
