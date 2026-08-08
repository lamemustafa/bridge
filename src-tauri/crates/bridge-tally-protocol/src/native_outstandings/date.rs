//! Parsing for Tally's display-formatted native dates: `1-Apr-24`,
//! `31-May-26` — day (1-2 digits), a 3-letter month abbreviation, and a
//! TWO-DIGIT year (TALLY_PROTOCOL_REFERENCE ground truth captured
//! 2026-08-07, `bills_receivable_billwise_lab.xml` /
//! `bills_receivable_ageing_lab.xml`).
//!
//! The two-digit year is deliberately resolved only against the pinned
//! company's `BooksFrom` century, never the wall clock: a Bridge process can
//! run years after the book it is reading, and the wall clock has no
//! relationship to what century that book's data lives in.

use bridge_tally_primitives::TallyDate;

use super::model::NativeOutstandingsError;

const MONTH_ABBREVIATIONS: [&str; 12] = [
    "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
];

/// Parses one native display date against the pinned company's `BooksFrom`
/// year. `books_from_year` supplies the century only (`books_from_year / 100
/// * 100`); the two-digit year in `raw` is appended to that century.
///
/// Fails closed — rather than guessing — when the lexeme does not match the
/// exact three-part `D[D]-MMM-YY` shape, or when the resolved year/month/day
/// is not a real Gregorian calendar date.
pub fn parse_native_display_date(
    raw: &str,
    books_from_year: u32,
) -> Result<TallyDate, NativeOutstandingsError> {
    let trimmed = raw.trim();
    let mut parts = trimmed.split('-');
    let (Some(day_part), Some(month_part), Some(year_part), None) =
        (parts.next(), parts.next(), parts.next(), parts.next())
    else {
        return Err(NativeOutstandingsError::InvalidDate(
            "native_date_shape_invalid",
        ));
    };

    if day_part.is_empty()
        || day_part.len() > 2
        || !day_part.bytes().all(|byte| byte.is_ascii_digit())
    {
        return Err(NativeOutstandingsError::InvalidDate(
            "native_date_day_invalid",
        ));
    }
    let day: u32 = day_part
        .parse()
        .map_err(|_| NativeOutstandingsError::InvalidDate("native_date_day_invalid"))?;

    let month_index = MONTH_ABBREVIATIONS
        .iter()
        .position(|candidate| *candidate == month_part)
        .ok_or(NativeOutstandingsError::InvalidDate(
            "native_date_month_invalid",
        ))?;
    let month = month_index as u32 + 1;

    if year_part.len() != 2 || !year_part.bytes().all(|byte| byte.is_ascii_digit()) {
        return Err(NativeOutstandingsError::InvalidDate(
            "native_date_year_invalid",
        ));
    }
    let two_digit_year: u32 = year_part
        .parse()
        .map_err(|_| NativeOutstandingsError::InvalidDate("native_date_year_invalid"))?;

    let century = (books_from_year / 100) * 100;
    let year = century + two_digit_year;

    TallyDate::parse(format!("{year:04}{month:02}{day:02}"))
        .map_err(|_| NativeOutstandingsError::InvalidDate("native_date_calendar_invalid"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolves_two_digit_year_against_the_books_from_century() {
        assert_eq!(
            parse_native_display_date("1-Apr-24", 2024)
                .unwrap()
                .as_str(),
            "20240401"
        );
        assert_eq!(
            parse_native_display_date("31-May-26", 2026)
                .unwrap()
                .as_str(),
            "20260531"
        );
        assert_eq!(
            parse_native_display_date("2-Jul-26", 2024)
                .unwrap()
                .as_str(),
            "20260702"
        );
    }

    #[test]
    fn fails_closed_on_malformed_or_impossible_dates() {
        for raw in [
            "",
            "1-Apr",
            "1-Apr-24-extra",
            "1-Apr-2024",
            "1-Apr-2",
            "1-April-24",
            "32-Jan-24",
            "0-Jan-24",
            "29-Feb-25",
            "a-Apr-24",
            "1-XXX-24",
        ] {
            assert!(
                parse_native_display_date(raw, 2024).is_err(),
                "expected {raw:?} to be rejected"
            );
        }
        assert!(parse_native_display_date("29-Feb-24", 2024).is_ok());
    }
}
