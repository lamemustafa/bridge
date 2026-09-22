//! Parsing for Tally's display-formatted native dates: `1-Apr-24`,
//! `31-May-26` — day (1-2 digits), a 3-letter month abbreviation, and a
//! TWO-DIGIT year (TALLY_PROTOCOL_REFERENCE ground truth captured
//! 2026-08-07, `bills_receivable_billwise_lab.xml` /
//! `bills_receivable_ageing_lab.xml`).
//!
//! A bill date's two-digit year is resolved against the pinned company's book
//! window, and a due date's against its own bill date, never against the wall
//! clock: a Bridge process can run years after the book it is reading, and the
//! wall clock has no relationship to what century that book's data lives in.

use bridge_tally_primitives::TallyDate;

use super::model::NativeOutstandingsError;

const MONTH_ABBREVIATIONS: [&str; 12] = [
    "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
];

/// The longest a bill's own date may precede `BooksFrom` (bridge#612). A
/// chosen bound, not a measured one: it admits an opening bill carried in from
/// decades before the books. Bounding it, rather than always looking back a
/// full century from `as_of`, keeps a year that fits only further back (such
/// as a bill dated after `as_of`) refusing instead of being read into the
/// previous century.
pub const OPENING_BILL_LOOKBACK_YEARS: u32 = 50;

/// How far before its bill date a due date is read. Not measured: whether
/// Tally can show a due date earlier than its bill is unknown, and this keeps
/// a modestly earlier one readable. The other ninety years of the due date's
/// century lie after the bill date and cover the longest credit period
/// measured to persist, `1000 Months`, about 83 years
/// (TALLY_PROTOCOL_REFERENCE §12a.3).
pub const DUE_DATE_LOOKBACK_YEARS: u32 = 10;

const CENTURY_YEARS: u32 = 100;

/// Parses a native bill's own display date. The two-digit year may be valid
/// in more than one century, so resolving against `BooksFrom`'s century alone
/// can silently place an active bill a century in the past. Exactly one valid
/// calendar date must fall in the window; none fails closed.
///
/// The window ends at `as_of`: an as-of Bills report is taken to list only
/// bills dated on or before it (inferred, not measured). It begins
/// [`OPENING_BILL_LOOKBACK_YEARS`] before `books_from`, because `BooksFrom` is
/// not a lower bound on a bill's date (bridge#612: two opening bills dated
/// `31-Mar-25` were captured against `BOOKSFROM` 20250401), or a hundred years
/// before `as_of` if that is later, so the window never spans more than a
/// hundred years. A book whose own window (`books_from` to `as_of`) spans a
/// hundred years or more refuses as ambiguous rather than choose a century.
///
/// Fails closed — rather than guessing — when the lexeme does not match the
/// exact three-part `D[D]-MMM-YY` shape, or when the resolved year/month/day
/// is not a real Gregorian calendar date.
pub fn parse_native_bill_date(
    raw: &str,
    books_from: &TallyDate,
    as_of: &TallyDate,
) -> Result<TallyDate, NativeOutstandingsError> {
    let lexeme = DisplayDate::parse(raw)?;
    if books_from > as_of {
        return Err(NativeOutstandingsError::InvalidDate(
            "native_date_book_window_invalid",
        ));
    }
    let from = year_month_day(books_from)?;
    let end = year_month_day(as_of)?;
    if end >= (from.0 + CENTURY_YEARS, from.1, from.2) {
        return Err(NativeOutstandingsError::InvalidDate(
            "native_date_year_ambiguous_book_window",
        ));
    }
    let after = (
        from.0.saturating_sub(OPENING_BILL_LOOKBACK_YEARS),
        from.1,
        from.2,
    )
        .max((end.0.saturating_sub(CENTURY_YEARS), end.1, end.2));
    lexeme.resolve(after, end, "native_date_year_outside_book_window")
}

/// Parses a native bill's due date against that bill's own resolved date,
/// never the book window: a due date follows its bill by the credit period,
/// which can run far past `as_of` (bridge#612). The window starts
/// [`DUE_DATE_LOOKBACK_YEARS`] before `bill_date` and spans exactly a hundred
/// years, so it holds at most one candidate.
pub fn parse_native_due_date(
    raw: &str,
    bill_date: &TallyDate,
) -> Result<TallyDate, NativeOutstandingsError> {
    let lexeme = DisplayDate::parse(raw)?;
    let bill = year_month_day(bill_date)?;
    let after = (
        bill.0.saturating_sub(DUE_DATE_LOOKBACK_YEARS),
        bill.1,
        bill.2,
    );
    lexeme.resolve(
        after,
        (after.0 + CENTURY_YEARS, after.1, after.2),
        "native_date_year_outside_due_window",
    )
}

/// A `D[D]-MMM-YY` lexeme whose century is not yet known.
struct DisplayDate {
    day: u32,
    month: u32,
    two_digit_year: u32,
}

impl DisplayDate {
    fn parse(raw: &str) -> Result<Self, NativeOutstandingsError> {
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

        if year_part.len() != 2 || !year_part.bytes().all(|byte| byte.is_ascii_digit()) {
            return Err(NativeOutstandingsError::InvalidDate(
                "native_date_year_invalid",
            ));
        }
        let two_digit_year: u32 = year_part
            .parse()
            .map_err(|_| NativeOutstandingsError::InvalidDate("native_date_year_invalid"))?;
        Ok(Self {
            day,
            month: month_index as u32 + 1,
            two_digit_year,
        })
    }

    /// The one calendar date in (`after`, `through`], each a (year, month,
    /// day). Every caller's window spans at most a hundred years, so a second
    /// candidate cannot arise; if a wider window ever reaches here it still
    /// refuses rather than choose.
    fn resolve(
        &self,
        after: (u32, u32, u32),
        through: (u32, u32, u32),
        outside: &'static str,
    ) -> Result<TallyDate, NativeOutstandingsError> {
        let (day, month) = (self.day, self.month);
        let mut candidates = Vec::new();
        let mut has_calendar_candidate = false;
        for century in ((after.0 / 100) * 100..=(through.0 / 100) * 100).step_by(100) {
            let year = century + self.two_digit_year;
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
            [] if has_calendar_candidate => Err(NativeOutstandingsError::InvalidDate(outside)),
            [] => Err(NativeOutstandingsError::InvalidDate(
                "native_date_calendar_invalid",
            )),
            _ => Err(NativeOutstandingsError::InvalidDate(
                "native_date_year_ambiguous_book_window",
            )),
        }
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
