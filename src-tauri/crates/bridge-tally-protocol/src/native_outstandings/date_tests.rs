use super::*;

#[test]
fn resolves_two_digit_year_against_the_books_from_century() {
    let books_from = TallyDate::parse("20240401").unwrap();
    let as_of = TallyDate::parse("20260731").unwrap();
    assert_eq!(
        parse_native_display_date(
            "1-Apr-24",
            &books_from,
            &as_of,
            NativeDisplayDateRole::BillDate
        )
        .unwrap()
        .as_str(),
        "20240401"
    );
    assert_eq!(
        parse_native_display_date(
            "31-May-26",
            &books_from,
            &as_of,
            NativeDisplayDateRole::BillDate
        )
        .unwrap()
        .as_str(),
        "20260531"
    );
    assert_eq!(
        parse_native_display_date(
            "2-Jul-26",
            &books_from,
            &as_of,
            NativeDisplayDateRole::BillDate
        )
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
        parse_native_display_date(
            "1-Apr-26",
            &books_from,
            &as_of,
            NativeDisplayDateRole::BillDate
        )
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
        parse_native_display_date(
            "1-Apr-26",
            &books_from,
            &as_of,
            NativeDisplayDateRole::BillDate
        ),
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
            parse_native_display_date(raw, &books_from, &as_of, NativeDisplayDateRole::BillDate)
                .is_err(),
            "expected {raw:?} to be rejected"
        );
    }
    assert!(parse_native_display_date(
        "29-Feb-24",
        &books_from,
        &as_of,
        NativeDisplayDateRole::BillDate
    )
    .is_ok());
}

#[test]
fn due_date_can_fall_after_as_of_without_widening_the_bill_date_window() {
    let books_from = TallyDate::parse("20260401").unwrap();
    let as_of = TallyDate::parse("20260731").unwrap();
    assert_eq!(
        parse_native_display_date(
            "1-Aug-26",
            &books_from,
            &as_of,
            NativeDisplayDateRole::DueDate
        )
        .unwrap()
        .as_str(),
        "20260801"
    );
    assert_eq!(
        parse_native_display_date(
            "1-Aug-26",
            &books_from,
            &as_of,
            NativeDisplayDateRole::BillDate
        ),
        Err(NativeOutstandingsError::InvalidDate(
            "native_date_year_outside_book_window"
        ))
    );
}

#[test]
fn due_date_still_rejects_an_ambiguous_two_digit_year() {
    let books_from = TallyDate::parse("19000101").unwrap();
    let as_of = TallyDate::parse("21001231").unwrap();
    assert_eq!(
        parse_native_display_date(
            "1-Apr-26",
            &books_from,
            &as_of,
            NativeDisplayDateRole::DueDate
        ),
        Err(NativeOutstandingsError::InvalidDate(
            "native_date_year_ambiguous_book_window"
        ))
    );
}

/// bridge#612: an opening bill keeps its original date, before the books
/// begin. Captured: `31-Mar-25` against `BOOKSFROM` 20250401. The 15-Mar-25
/// case is an opening bill a fortnight before the books.
#[test]
fn an_opening_bill_dated_before_books_from_parses() {
    let books_from = TallyDate::parse("20250401").unwrap();
    let as_of = TallyDate::parse("20250930").unwrap();
    for (raw, expected) in [("31-Mar-25", "20250331"), ("15-Mar-25", "20250315")] {
        for role in [
            NativeDisplayDateRole::BillDate,
            NativeDisplayDateRole::DueDate,
        ] {
            assert_eq!(
                parse_native_display_date(raw, &books_from, &as_of, role)
                    .unwrap()
                    .as_str(),
                expected,
                "{raw} {role:?}"
            );
        }
    }
    // A bill carried in from years before the books is admitted too.
    assert_eq!(
        parse_native_display_date(
            "1-Apr-98",
            &books_from,
            &as_of,
            NativeDisplayDateRole::BillDate
        )
        .unwrap()
        .as_str(),
        "19980401"
    );
}

/// The lookback is bounded: a bill date more than fifty years before the
/// books, or after the as-of date, is not read into another century.
#[test]
fn a_bill_date_beyond_the_lookback_or_after_as_of_refuses() {
    let books_from = TallyDate::parse("20250401").unwrap();
    let as_of = TallyDate::parse("20250930").unwrap();
    for raw in ["1-Apr-75", "1-Oct-25"] {
        assert_eq!(
            parse_native_display_date(raw, &books_from, &as_of, NativeDisplayDateRole::BillDate),
            Err(NativeOutstandingsError::InvalidDate(
                "native_date_year_outside_book_window"
            )),
            "{raw}"
        );
    }
    // One day inside the lookback still parses.
    assert_eq!(
        parse_native_display_date(
            "2-Apr-75",
            &books_from,
            &as_of,
            NativeDisplayDateRole::BillDate
        )
        .unwrap()
        .as_str(),
        "19750402"
    );
}

/// A book whose own window spans the lookback or more refuses as ambiguous:
/// its two-digit years could fall in more than one century.
#[test]
fn a_book_window_of_the_lookback_or_more_refuses_as_ambiguous() {
    let books_from = TallyDate::parse("19760401").unwrap();
    for (as_of, ambiguous) in [("20260331", false), ("20260401", true)] {
        let as_of = TallyDate::parse(as_of).unwrap();
        for role in [
            NativeDisplayDateRole::BillDate,
            NativeDisplayDateRole::DueDate,
        ] {
            let parsed = parse_native_display_date("1-Jan-26", &books_from, &as_of, role);
            if ambiguous {
                assert_eq!(
                    parsed,
                    Err(NativeOutstandingsError::InvalidDate(
                        "native_date_year_ambiguous_book_window"
                    )),
                    "{as_of:?} {role:?}"
                );
            } else {
                assert_eq!(parsed.unwrap().as_str(), "20260101", "{as_of:?} {role:?}");
            }
        }
    }
}

/// A due date is read within fifty years either side of the as-of date:
/// before the books (an opening bill) and after the as-of date (credit).
#[test]
fn a_due_date_is_read_within_the_lookback_either_side_of_as_of() {
    let books_from = TallyDate::parse("20250401").unwrap();
    let as_of = TallyDate::parse("20250930").unwrap();
    for (raw, expected) in [
        ("15-Mar-25", "20250315"),
        ("30-Nov-25", "20251130"),
        ("30-Sep-75", "20750930"),
        ("1-Oct-75", "19751001"),
    ] {
        assert_eq!(
            parse_native_display_date(raw, &books_from, &as_of, NativeDisplayDateRole::DueDate)
                .unwrap()
                .as_str(),
            expected,
            "{raw}"
        );
    }
}
