//! Parsing for Tally's display-formatted native dates: `1-Apr-24`,
//! `31-May-26` — day (1-2 digits), a 3-letter month abbreviation, and a
//! TWO-DIGIT year (TALLY_PROTOCOL_REFERENCE ground truth captured
//! 2026-08-07, `bills_receivable_billwise_lab.xml` /
//! `bills_receivable_ageing_lab.xml`).
//!
//! The two-digit year is resolved inside the pinned company's actual book
//! window, never against the wall clock: a Bridge process can run years after
//! the book it is reading, and the wall clock has no relationship to what
//! century that book's data lives in.

use bridge_tally_primitives::TallyDate;

use super::model::NativeOutstandingsError;

const MONTH_ABBREVIATIONS: [&str; 12] = [
    "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NativeDisplayDateRole {
    BillDate,
    DueDate,
}

/// Parses one native display date using the pinned company's book window.
/// The two-digit year in `raw` may be valid in more than one century, so
/// resolving against `BooksFrom`'s century alone can silently place an active
/// bill a century in the past. Exactly one valid calendar date must fall in
/// the role-appropriate portion of the window; zero or multiple candidates
/// fail closed.
///
/// Fails closed — rather than guessing — when the lexeme does not match the
/// exact three-part `D[D]-MMM-YY` shape, or when the resolved year/month/day
/// is not a real Gregorian calendar date.
pub fn parse_native_display_date(
    raw: &str,
    books_from: &TallyDate,
    as_of: &TallyDate,
    role: NativeDisplayDateRole,
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

    if books_from > as_of {
        return Err(NativeOutstandingsError::InvalidDate(
            "native_date_book_window_invalid",
        ));
    }
    let books_from_year = parse_year(books_from)?;
    let as_of_year = parse_year(as_of)?;
    let first_century = (books_from_year / 100) * 100;
    let last_century = (as_of_year / 100) * 100;
    let mut candidates = Vec::new();
    let mut has_calendar_candidate = false;

    for century in (first_century..=last_century).step_by(100) {
        let year = century + two_digit_year;
        let Ok(candidate) = TallyDate::parse(format!("{year:04}{month:02}{day:02}")) else {
            continue;
        };
        has_calendar_candidate = true;
        if &candidate >= books_from
            && match role {
                NativeDisplayDateRole::BillDate => &candidate <= as_of,
                NativeDisplayDateRole::DueDate => true,
            }
        {
            candidates.push(candidate);
        }
    }

    match candidates.as_slice() {
        [candidate] => Ok(candidate.clone()),
        [] if has_calendar_candidate => Err(NativeOutstandingsError::InvalidDate(
            "native_date_year_outside_book_window",
        )),
        [] => Err(NativeOutstandingsError::InvalidDate(
            "native_date_calendar_invalid",
        )),
        _ => Err(NativeOutstandingsError::InvalidDate(
            "native_date_year_ambiguous_book_window",
        )),
    }
}

fn parse_year(date: &TallyDate) -> Result<u32, NativeOutstandingsError> {
    date.as_str()[..4]
        .parse()
        .map_err(|_| NativeOutstandingsError::InvalidDate("native_date_year_invalid"))
}

#[cfg(test)]
#[path = "date_tests.rs"]
mod tests;
