use super::*;

#[test]
fn scoped_opening_uses_requested_boundary_without_changing_book_start_default() {
    let books = TallyDate::parse("20240401").unwrap();
    let last = TallyDate::parse("20260915").unwrap();
    let requested = TallyDate::parse("20260901").unwrap();
    let scoped = ledger_opening_period(
        DateBoundaryProfile::EducationRestricted,
        &books,
        &last,
        Some(&requested),
    )
    .unwrap();
    let default = ledger_opening_period(
        DateBoundaryProfile::EducationRestricted,
        &books,
        &last,
        None,
    )
    .unwrap();
    let xml = render_native_ledger_export_request("Bridge Synthetic Book", &scoped);
    let reader = quick_xml::Reader::from_str(&xml);
    let mut reader = reader;
    let mut observed_from = None;
    loop {
        match reader.read_event().unwrap() {
            quick_xml::events::Event::Start(tag) if tag.name().as_ref() == b"SVFROMDATE" => {
                observed_from = Some(
                    reader
                        .read_text(tag.name())
                        .unwrap()
                        .decode()
                        .unwrap()
                        .into_owned(),
                );
            }
            quick_xml::events::Event::Eof => break,
            _ => {}
        }
    }
    assert_eq!(observed_from.as_deref(), Some("20260901"));
    assert_eq!(default.from(), &books);
    assert_eq!(default.to(), &last);
    assert_eq!(scoped.to(), &last);
}

#[test]
fn scoped_opening_rejects_unsupported_and_prebook_dates_before_dispatch() {
    let books = TallyDate::parse("20240401").unwrap();
    let last = TallyDate::parse("20260915").unwrap();
    for (requested, expected) in [
        (
            "20260815",
            NativeLedgerExportPeriodError::UnsupportedBoundary,
        ),
        ("20240331", NativeLedgerExportPeriodError::InvalidRange),
    ] {
        assert_eq!(
            ledger_opening_period(
                DateBoundaryProfile::EducationRestricted,
                &books,
                &last,
                Some(&TallyDate::parse(requested).unwrap())
            ),
            Err(expected)
        );
    }
    let after_last = TallyDate::parse("20261001").unwrap();
    let period = ledger_opening_period(
        DateBoundaryProfile::EducationRestricted,
        &books,
        &last,
        Some(&after_last),
    )
    .unwrap();
    assert_eq!(period.from(), &after_last);
    assert_eq!(period.to(), &after_last);
}
