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
