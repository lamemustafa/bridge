use super::{count_voucher_start_elements, parse_credit_period};
use crate::outstandings::{CreditPeriod, OutstandingsError};

#[test]
fn credit_period_accepts_verified_units_and_rejects_unknown_ones() {
    assert_eq!(
        parse_credit_period("45 Days").unwrap(),
        CreditPeriod::Days(45)
    );
    assert_eq!(parse_credit_period("1 Day").unwrap(), CreditPeriod::Days(1));
    assert_eq!(
        parse_credit_period("3 Weeks").unwrap(),
        CreditPeriod::Weeks(3)
    );
    assert_eq!(
        parse_credit_period("2 Months").unwrap(),
        CreditPeriod::Months(2)
    );
    assert_eq!(parse_credit_period(" ").unwrap(), CreditPeriod::Days(0));
    // Licensed TallyPrime 7.1 read-back on 2026-08-23 retained all four
    // values. Only the day-unit ceiling is measured: 10000 Days comes
    // back empty from Tally rather than as an over-limit period.
    assert_eq!(
        parse_credit_period("9999 Days").unwrap(),
        CreditPeriod::Days(9999)
    );
    assert_eq!(
        parse_credit_period("3650 Days").unwrap(),
        CreditPeriod::Days(3650)
    );
    assert_eq!(
        parse_credit_period("100 Months").unwrap(),
        CreditPeriod::Months(100)
    );
    assert_eq!(
        parse_credit_period("1000 Months").unwrap(),
        CreditPeriod::Months(1000)
    );
    assert_eq!(
        parse_credit_period("2 Fortnights"),
        Err(OutstandingsError::InvalidResponse(
            "bill_credit_period_invalid"
        ))
    );
    assert_eq!(
        parse_credit_period("10000 Days"),
        Err(OutstandingsError::InvalidResponse(
            "bill_credit_period_invalid"
        ))
    );
}

#[test]
fn voucher_rows_are_counted_structurally_not_by_one_textual_spelling() {
    // Three serializations quick_xml deserializes identically. A
    // `"<VOUCHER "` substring scan sees only the first.
    let xml = concat!(
        "<ENVELOPE><BODY><DATA><COLLECTION>",
        "<VOUCHER REMOTEID=\"a\"></VOUCHER>",
        "<VOUCHER></VOUCHER>",
        "<VOUCHER\n  REMOTEID=\"c\"></VOUCHER>",
        "</COLLECTION></DATA></BODY></ENVELOPE>"
    );
    assert_eq!(count_voucher_start_elements(xml).unwrap(), 3);
    assert_eq!(xml.match_indices("<VOUCHER ").count(), 1);
}

#[test]
fn cmpinfo_counter_elements_are_not_counted_as_rows() {
    // CMPINFO sits under DESC and carries bare `<VOUCHER>0</VOUCHER>`
    // counters. The retained live capture has exactly one against 75 real
    // rows, so counting every VOUCHER element would over-report and fail
    // the raw-vs-parsed agreement check.
    let xml = concat!(
        "<ENVELOPE><BODY>",
        "<DESC><CMPINFO><VOUCHER>0</VOUCHER><VOUCHERTYPE>0</VOUCHERTYPE></CMPINFO></DESC>",
        "<DATA><COLLECTION><VOUCHER REMOTEID=\"a\"></VOUCHER></COLLECTION></DATA>",
        "</BODY></ENVELOPE>"
    );
    assert_eq!(count_voucher_start_elements(xml).unwrap(), 1);
}

#[test]
fn a_response_with_no_rows_counts_zero_rather_than_failing() {
    let xml = "<ENVELOPE><BODY><DATA><COLLECTION></COLLECTION></DATA></BODY></ENVELOPE>";
    assert_eq!(count_voucher_start_elements(xml).unwrap(), 0);
}
