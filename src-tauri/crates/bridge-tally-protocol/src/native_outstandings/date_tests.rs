use super::*;

#[test]
fn resolves_a_bill_dates_two_digit_year_inside_the_book_window() {
    let books_from = TallyDate::parse("20240401").unwrap();
    let as_of = TallyDate::parse("20260731").unwrap();
    assert_eq!(
        parse_native_bill_date("1-Apr-24", &books_from, &as_of)
            .unwrap()
            .as_str(),
        "20240401"
    );
    assert_eq!(
        parse_native_bill_date("31-May-26", &books_from, &as_of)
            .unwrap()
            .as_str(),
        "20260531"
    );
    assert_eq!(
        parse_native_bill_date("2-Jul-26", &books_from, &as_of)
            .unwrap()
            .as_str(),
        "20260702"
    );
}

#[test]
fn resolves_a_century_boundary_year_into_the_active_book() {
    let books_from = TallyDate::parse("19990401").unwrap();
    let as_of = TallyDate::parse("20260731").unwrap();
    assert_eq!(
        parse_native_bill_date("1-Apr-26", &books_from, &as_of)
            .unwrap()
            .as_str(),
        "20260401",
        "a 1999 book that is active in 2026 must not parse 26 as 1926"
    );
}

#[test]
fn rejects_a_two_digit_year_with_multiple_plausible_centuries() {
    let books_from = TallyDate::parse("19000101").unwrap();
    let as_of = TallyDate::parse("21001231").unwrap();
    assert_eq!(
        parse_native_bill_date("1-Apr-26", &books_from, &as_of),
        Err(NativeOutstandingsError::InvalidDate(
            "native_date_year_ambiguous_book_window"
        ))
    );
}

#[test]
fn fails_closed_on_malformed_or_impossible_dates() {
    let books_from = TallyDate::parse("20240101").unwrap();
    let as_of = TallyDate::parse("20260731").unwrap();
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
            parse_native_bill_date(raw, &books_from, &as_of).is_err(),
            "expected {raw:?} to be rejected"
        );
    }
    assert!(parse_native_bill_date("29-Feb-24", &books_from, &as_of).is_ok());
}

#[test]
fn a_due_date_can_follow_as_of_where_its_bill_date_cannot() {
    let books_from = TallyDate::parse("20260401").unwrap();
    let as_of = TallyDate::parse("20260731").unwrap();
    let bill_date = TallyDate::parse("20260701").unwrap();
    assert_eq!(
        parse_native_due_date("1-Aug-26", &bill_date)
            .unwrap()
            .as_str(),
        "20260801"
    );
    assert_eq!(
        parse_native_bill_date("1-Aug-26", &books_from, &as_of),
        Err(NativeOutstandingsError::InvalidDate(
            "native_date_year_outside_book_window"
        ))
    );
}

/// bridge#612: a due date is resolved against its own bill date, in a window
/// that starts ten years before the bill and spans exactly one century, so it
/// can never hold two candidates. Replaces a test that a due date could be
/// ambiguous against a two-century book window; a due date no longer reads
/// the book window.
#[test]
fn a_due_date_resolves_to_the_one_century_around_its_bill_date() {
    let bill_date = TallyDate::parse("20250601").unwrap();
    for (raw, expected) in [
        // A day before the bill, and the last day of the ten-year lookback.
        ("31-May-25", "20250531"),
        ("2-Jun-15", "20150602"),
        // Exactly ten years before is outside the lookback, so it is read
        // ninety years after the bill instead.
        ("1-Jun-15", "21150601"),
        // 704 and 1000 months of credit.
        ("1-Feb-84", "20840201"),
        ("1-Oct-08", "21081001"),
    ] {
        assert_eq!(
            parse_native_due_date(raw, &bill_date).unwrap().as_str(),
            expected,
            "{raw}"
        );
    }
    // A bill fifty years before the books keeps its own due date's century.
    assert_eq!(
        parse_native_due_date("2-Apr-75", &TallyDate::parse("19750402").unwrap())
            .unwrap()
            .as_str(),
        "19750402"
    );
    for (raw, code) in [
        ("29-Feb-27", "native_date_calendar_invalid"),
        // 2000 was a leap year and falls before the window; 2100 is not.
        ("29-Feb-00", "native_date_year_outside_due_window"),
        ("1-Apr-2025", "native_date_year_invalid"),
        ("1-Apr", "native_date_shape_invalid"),
    ] {
        assert_eq!(
            parse_native_due_date(raw, &bill_date),
            Err(NativeOutstandingsError::InvalidDate(code)),
            "{raw}"
        );
    }
}

/// bridge#612: an opening bill keeps a date before the books begin. Captured:
/// `31-Mar-25` against `BOOKSFROM` 20250401, due the same day. The 15-Mar-25
/// and 1998 cases are synthetic.
#[test]
fn an_opening_bill_dated_before_books_from_parses() {
    let books_from = TallyDate::parse("20250401").unwrap();
    let as_of = TallyDate::parse("20250930").unwrap();
    for (raw, expected) in [
        ("31-Mar-25", "20250331"),
        ("15-Mar-25", "20250315"),
        ("1-Apr-98", "19980401"),
    ] {
        let bill_date = parse_native_bill_date(raw, &books_from, &as_of).unwrap();
        assert_eq!(bill_date.as_str(), expected, "{raw}");
        assert_eq!(
            parse_native_due_date(raw, &bill_date).unwrap().as_str(),
            expected,
            "{raw} due"
        );
    }
}

/// The lookback is bounded: a bill date fifty years or more before the books,
/// or shortly after the as-of date, refuses. A bill dated on the as-of date or
/// one day inside the lookback parses.
#[test]
fn a_bill_date_beyond_the_lookback_or_after_as_of_refuses() {
    let books_from = TallyDate::parse("20250401").unwrap();
    let as_of = TallyDate::parse("20250930").unwrap();
    for raw in ["1-Apr-75", "1-Oct-25"] {
        assert_eq!(
            parse_native_bill_date(raw, &books_from, &as_of),
            Err(NativeOutstandingsError::InvalidDate(
                "native_date_year_outside_book_window"
            )),
            "{raw}"
        );
    }
    for (raw, expected) in [("2-Apr-75", "19750402"), ("30-Sep-25", "20250930")] {
        assert_eq!(
            parse_native_bill_date(raw, &books_from, &as_of)
                .unwrap()
                .as_str(),
            expected,
            "{raw}"
        );
    }
}

/// An as-of date far past the books shortens the lookback so the window stays
/// within a century, rather than refusing: the bill-date window is then the
/// hundred years ending at the as-of date.
#[test]
fn a_far_as_of_date_shortens_the_lookback_rather_than_refuse() {
    let books_from = TallyDate::parse("20250401").unwrap();
    let as_of = TallyDate::parse("20991231").unwrap();
    for (raw, expected) in [
        ("1-Apr-25", "20250401"),
        ("31-Mar-25", "20250331"),
        ("1-Jan-00", "20000101"),
        ("31-Dec-99", "20991231"),
    ] {
        assert_eq!(
            parse_native_bill_date(raw, &books_from, &as_of)
                .unwrap()
                .as_str(),
            expected,
            "{raw}"
        );
    }
}

/// A book whose own window spans a century or more refuses as ambiguous: its
/// two-digit years could fall in more than one century.
#[test]
fn a_book_window_of_a_century_or_more_refuses_as_ambiguous() {
    let books_from = TallyDate::parse("19260401").unwrap();
    let within = TallyDate::parse("20260331").unwrap();
    assert_eq!(
        parse_native_bill_date("1-Jan-26", &books_from, &within)
            .unwrap()
            .as_str(),
        "20260101"
    );
    let century = TallyDate::parse("20260401").unwrap();
    assert_eq!(
        parse_native_bill_date("1-Jan-26", &books_from, &century),
        Err(NativeOutstandingsError::InvalidDate(
            "native_date_year_ambiguous_book_window"
        ))
    );
}
