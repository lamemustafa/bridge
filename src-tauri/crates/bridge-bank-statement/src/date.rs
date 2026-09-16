//! Calendar dates as the two statement layouts print them.
//!
//! Ported from Python's `datetime.strptime` for exactly the two formats the
//! profiles use (`%d/%m/%y` and `%d%b%Y`), including its quirks: `%d` and `%m`
//! accept one digit, `%b` is case-insensitive, `%y` pivots at 69, and a
//! successful prefix that leaves characters unconsumed is an error rather than
//! a retry.
//!
//! One deliberate divergence: digits are ASCII only. Python's `\d` also accepts
//! other scripts' decimal digits there; refusing them here fails closed with
//! `unparseable_date` instead of reading a date nobody can check by eye.

use regex::Regex;
use std::sync::LazyLock;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct Date {
    pub year: u16,
    pub month: u8,
    pub day: u8,
}

const MONTHS: [&str; 12] = [
    "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
];

impl Date {
    pub fn new(year: u16, month: u8, day: u8) -> Option<Self> {
        let valid = (1..=12).contains(&month) && day >= 1 && day <= days_in_month(year, month);
        (valid && year >= 1).then_some(Self { year, month, day })
    }

    /// `YYYY-MM-DD`, the form `build_import_xml` takes.
    pub fn iso(self) -> String {
        format!("{:04}-{:02}-{:02}", self.year, self.month, self.day)
    }

    /// `DD-Mon-YYYY`, as the reference writes into a narration.
    pub fn narration(self) -> String {
        format!(
            "{:02}-{}-{:04}",
            self.day,
            MONTHS[usize::from(self.month - 1)],
            self.year
        )
    }

    /// Strict `YYYY-MM-DD`.
    pub fn parse_iso(text: &str) -> Option<Self> {
        static ISO: LazyLock<Regex> =
            LazyLock::new(|| Regex::new(r"^([0-9]{4})-([0-9]{2})-([0-9]{2})$").unwrap());
        let found = ISO.captures(text)?;
        Self::new(
            found[1].parse().ok()?,
            found[2].parse().ok()?,
            found[3].parse().ok()?,
        )
    }
}

fn days_in_month(year: u16, month: u8) -> u8 {
    match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 if (year.is_multiple_of(4) && !year.is_multiple_of(100)) || year.is_multiple_of(400) => {
            29
        }
        2 => 28,
        _ => 0,
    }
}

const DAY: &str = r"(3[01]|[12][0-9]|0[1-9]|[1-9]| [1-9])";
const MONTH: &str = r"(1[0-2]|0[1-9]|[1-9])";

fn fully_matched<'t>(pattern: &Regex, text: &'t str) -> Option<regex::Captures<'t>> {
    // Python's strptime runs `re.match` and then refuses leftover characters;
    // it does not backtrack into a match that would have consumed them.
    let found = pattern.captures(text)?;
    (found.get(0)?.end() == text.len()).then_some(found)
}

fn day(text: &str) -> Option<u8> {
    text.trim_start().parse().ok()
}

/// `%d/%m/%y`.
pub fn parse_day_month_short_year(text: &str) -> Option<Date> {
    static PATTERN: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(&format!(r"^{DAY}/{MONTH}/([0-9][0-9])")).unwrap());
    let found = fully_matched(&PATTERN, text)?;
    let short: u16 = found[3].parse().ok()?;
    let year = if short <= 68 {
        2000 + short
    } else {
        1900 + short
    };
    Date::new(year, found[2].parse().ok()?, day(&found[1])?)
}

/// `%d%b%Y`, month names case-insensitive.
pub fn parse_day_month_name_year(text: &str) -> Option<Date> {
    static PATTERN: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(&format!(
            r"^{DAY}(?i:(jan|feb|mar|apr|may|jun|jul|aug|sep|oct|nov|dec))([0-9]{{4}})"
        ))
        .unwrap()
    });
    let found = fully_matched(&PATTERN, text)?;
    let month = MONTHS
        .iter()
        .position(|name| name.eq_ignore_ascii_case(&found[2]))?;
    Date::new(
        found[3].parse().ok()?,
        u8::try_from(month + 1).ok()?,
        day(&found[1])?,
    )
}
