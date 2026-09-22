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

/// How far before `BooksFrom` a bill's own date may fall (bridge#612), and
/// how far either side of the as-of date a due date may. Also the longest
/// book window whose two-digit years resolve unambiguously: every window below
/// then spans under a hundred years.
pub const OPENING_BILL_LOOKBACK_YEARS: u32 = 50;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NativeDisplayDateRole {
    BillDate,
    DueDate,
}

/// Parses one native display date using the pinned company's book window.
/// The two-digit year in `raw` may be valid in more than one century, so
/// resolving against `BooksFrom`'s century alone can silently place an active
/// bill a century in the past. Exactly one valid calendar date must fall in
/// the role's window; zero or multiple candidates fail closed.
///
/// `BooksFrom` is not a lower bound on a bill's dates (bridge#612): an opening
/// bill keeps its original date, before the books begin (`31-Mar-25` against
/// `BOOKSFROM` 20250401 on a captured book). The windows are:
/// - a bill date: after `books_from` less [`OPENING_BILL_LOOKBACK_YEARS`], and
///   no later than `as_of`, since an as-of Bills report lists only bills dated
///   on or before it;
/// - a due date: within [`OPENING_BILL_LOOKBACK_YEARS`] either side of
///   `as_of`, since a due date can precede the books (an opening bill) or
///   follow the as-of date (a credit period).
///
/// Each window spans less than a hundred years only while the book's own
/// window (`books_from` to `as_of`) spans less than
/// [`OPENING_BILL_LOOKBACK_YEARS`]; a longer book refuses as ambiguous rather
/// than choose a century.
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
    let from = year_month_day(books_from)?;
    let end = year_month_day(as_of)?;
    // A book whose own window spans the lookback or more has two-digit years
    // that more than one century could hold.
    if end >= (from.0 + OPENING_BILL_LOOKBACK_YEARS, from.1, from.2) {
        return Err(NativeOutstandingsError::InvalidDate(
            "native_date_year_ambiguous_book_window",
        ));
    }
    // (exclusive lower, inclusive upper), as (year, month, day).
    let (after, through) = match role {
        NativeDisplayDateRole::BillDate => (
            (
                from.0.saturating_sub(OPENING_BILL_LOOKBACK_YEARS),
                from.1,
                from.2,
            ),
            end,
        ),
        NativeDisplayDateRole::DueDate => (
            (
                end.0.saturating_sub(OPENING_BILL_LOOKBACK_YEARS),
                end.1,
                end.2,
            ),
            (end.0 + OPENING_BILL_LOOKBACK_YEARS, end.1, end.2),
        ),
    };
    let mut candidates = Vec::new();
    let mut has_calendar_candidate = false;

    for century in ((after.0 / 100) * 100..=(through.0 / 100) * 100).step_by(100) {
        let year = century + two_digit_year;
        let Ok(candidate) = TallyDate::parse(format!("{year:04}{month:02}{day:02}")) else {
            continue;
        };
        has_calendar_candidate = true;
        let at = (year, month, day);
        if at > after && at <= through {
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

fn year_month_day(date: &TallyDate) -> Result<(u32, u32, u32), NativeOutstandingsError> {
    let text = date.as_str();
    let part = |range: std::ops::Range<usize>| -> Result<u32, NativeOutstandingsError> {
        text[range]
            .parse()
            .map_err(|_| NativeOutstandingsError::InvalidDate("native_date_year_invalid"))
    };
    Ok((part(0..4)?, part(4..6)?, part(6..8)?))
}

#[cfg(test)]
#[path = "date_tests.rs"]
mod tests;
