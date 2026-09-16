use super::TallyDate;

#[test]
fn adding_calendar_months_clamps_to_the_target_month_end() {
    assert_eq!(
        TallyDate::parse("20260131")
            .unwrap()
            .add_months_clamped(1)
            .unwrap()
            .as_str(),
        "20260228"
    );
    assert_eq!(
        TallyDate::parse("20240131")
            .unwrap()
            .add_months_clamped(1)
            .unwrap()
            .as_str(),
        "20240229"
    );
}

#[test]
fn adding_days_is_constant_time_calendar_arithmetic() {
    assert_eq!(
        TallyDate::parse("20260228")
            .unwrap()
            .add_days(1)
            .unwrap()
            .as_str(),
        "20260301"
    );
    assert!(TallyDate::parse("20260101")
        .unwrap()
        .add_days(u32::MAX)
        .is_err());
}
