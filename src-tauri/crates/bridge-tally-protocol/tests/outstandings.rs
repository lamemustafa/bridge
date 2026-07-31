use bridge_tally_core::TallyDate;
use bridge_tally_protocol::{
    outstandings::{
        assemble_scan, compute_outstandings, parse_company_book_extent, verify_segment_pair,
        AlterIdRange, DateBoundaryProfile, DateWindow, MoneyValue, NarrowDateWindow,
        OutstandingsError, ScanResult, SegmentVerification, VoucherAlterIdHighWater,
    },
    xml_read_profiles::ReadOnlyProfile,
};

const COMPANY_EXTENT: &str = include_str!("fixtures/unit_a_company_extent_live.xml");
const VOUCHERS: &str = include_str!("fixtures/unit_a_vouchers_wildcard_live.xml");
const COMPANY_NAME: &str = "Aarav Trading Company Demo";
const COMPANY_GUID: &str = "bb8ad19e-6aef-4239-a917-87fec0c6215e";

fn extent() -> bridge_tally_protocol::outstandings::CompanyBookExtent {
    parse_company_book_extent(COMPANY_EXTENT, COMPANY_NAME, COMPANY_GUID)
        .expect("real company extent capture parses")
}

fn full_alter_id_range() -> AlterIdRange {
    AlterIdRange::new(0, 440).unwrap()
}

fn capture_high_water() -> VoucherAlterIdHighWater {
    VoucherAlterIdHighWater::parse("440").unwrap()
}

#[test]
fn reporting_period_partitions_into_narrow_valid_non_overlapping_windows() {
    let reporting = DateWindow::parse(
        DateBoundaryProfile::EducationRestricted,
        "20240401",
        "20240602",
    )
    .unwrap();
    let partitions = reporting.narrow_partitions().unwrap();
    let boundaries = partitions
        .iter()
        .map(|window| (window.from().as_str(), window.to().as_str()))
        .collect::<Vec<_>>();
    assert_eq!(
        boundaries,
        vec![
            ("20240401", "20240501"),
            ("20240502", "20240601"),
            ("20240602", "20240602"),
        ]
    );
    assert!(NarrowDateWindow::try_from(reporting).is_err());

    let full_book = DateWindow::parse(
        DateBoundaryProfile::EducationRestricted,
        "20240401",
        "20260302",
    )
    .unwrap();
    let full_book_partitions = full_book.narrow_partitions().unwrap();
    assert_eq!(
        full_book_partitions.first().unwrap().from(),
        full_book.from()
    );
    assert_eq!(full_book_partitions.last().unwrap().to(), full_book.to());
    for pair in full_book_partitions.windows(2) {
        assert_eq!(
            pair[0].to().next_day().unwrap(),
            pair[1].from().clone(),
            "date partitions must be contiguous and non-overlapping"
        );
    }
}

#[test]
fn alter_id_ranges_are_non_empty_and_half_open_closed() {
    assert_eq!(AlterIdRange::new(400, 800).unwrap().exclusive_start(), 400);
    assert_eq!(AlterIdRange::new(400, 800).unwrap().inclusive_end(), 800);
    assert_eq!(
        AlterIdRange::new(400, 400),
        Err(OutstandingsError::InvalidAlterIdRange)
    );
}

#[test]
fn education_date_window_accepts_only_verified_boundary_days() {
    for date in ["20260401", "20260402", "20260331"] {
        DateWindow::parse(DateBoundaryProfile::EducationRestricted, date, date)
            .expect("day 1, 2, or 31 is verified");
    }
    for date in ["20260415", "20260428", "20260429", "20260430"] {
        assert_eq!(
            DateWindow::parse(DateBoundaryProfile::EducationRestricted, date, "20260501",),
            Err(OutstandingsError::InvalidDateWindow)
        );
        assert_eq!(
            DateWindow::parse(DateBoundaryProfile::EducationRestricted, "20260401", date,),
            Err(OutstandingsError::InvalidDateWindow)
        );
    }
    for profile in [
        DateBoundaryProfile::EducationRestricted,
        DateBoundaryProfile::ModeAgnostic,
    ] {
        assert_eq!(
            DateWindow::parse(profile, "20240601", "20240631"),
            Err(OutstandingsError::InvalidDateWindow),
            "a permitted day number cannot make an impossible calendar date valid"
        );
    }

    let across_june =
        DateWindow::parse(DateBoundaryProfile::ModeAgnostic, "20240531", "20240701").unwrap();
    let partitions = across_june.narrow_partitions().unwrap();
    assert_eq!(partitions[0].to().as_str(), "20240630");
    assert_eq!(partitions[1].from().as_str(), "20240701");
}

#[test]
fn licensed_or_unknown_boundaries_are_permitted_but_i12_still_fails_closed() {
    let ordinary = DateWindow::parse(DateBoundaryProfile::ModeAgnostic, "20260303", "20260315")
        .expect("unknown or licensed mode accepts an ordinary as-of date");
    let partitions = ordinary.narrow_partitions().unwrap();
    assert_eq!(partitions.len(), 1);
    assert_eq!(partitions[0].to().as_str(), "20260315");

    let extent = extent();
    let requested =
        DateWindow::parse(DateBoundaryProfile::ModeAgnostic, "20250401", "20260315").unwrap();
    let outside = VOUCHERS.replacen(
        "<DATE TYPE=\"Date\">20250401</DATE>",
        "<DATE TYPE=\"Date\">20260316</DATE>",
        1,
    );
    assert_ne!(outside, VOUCHERS, "representative live date was not found");
    let result = verify_segment_pair(
        &outside,
        &outside,
        extent.company(),
        requested,
        full_alter_id_range(),
    )
    .expect("span mismatch is an in-band partial result");
    assert!(matches!(
        result,
        SegmentVerification::Partial(partial)
            if partial.reason_code == "voucher_outside_requested_window"
    ));
}

#[test]
fn company_pin_is_created_only_after_live_identity_matches() {
    let extent = extent();
    assert_eq!(extent.company().name(), COMPANY_NAME);
    assert_eq!(extent.company().guid(), COMPANY_GUID);
    assert!(matches!(
        parse_company_book_extent(COMPANY_EXTENT, COMPANY_NAME, "wrong-guid"),
        Err(OutstandingsError::CompanyIdentityMismatch)
    ));
}

#[test]
fn company_extent_selects_the_expected_guid_in_a_multi_company_collection() {
    let company_start = COMPANY_EXTENT
        .find("    <COMPANY ")
        .expect("real capture contains a company row");
    let company_end = COMPANY_EXTENT[company_start..]
        .find("    </COMPANY>")
        .map(|offset| company_start + offset + "    </COMPANY>".len())
        .expect("real capture company row is complete");
    let expected_row = &COMPANY_EXTENT[company_start..company_end];
    let unrelated_row = expected_row
        .replace(COMPANY_NAME, "Earlier Loaded Synthetic Company")
        .replace(COMPANY_GUID, "00000000-0000-4000-8000-000000000001");
    let response =
        COMPANY_EXTENT.replacen(expected_row, &format!("{unrelated_row}\n{expected_row}"), 1);

    let selected = parse_company_book_extent(&response, COMPANY_NAME, COMPANY_GUID)
        .expect("GUID selection is independent of collection order");
    assert_eq!(selected.company().name(), COMPANY_NAME);
    assert_eq!(selected.company().guid(), COMPANY_GUID);
}

#[test]
fn company_extent_rejects_duplicate_rows_for_the_expected_guid() {
    let company_start = COMPANY_EXTENT
        .find("    <COMPANY ")
        .expect("real capture contains a company row");
    let company_end = COMPANY_EXTENT[company_start..]
        .find("    </COMPANY>")
        .map(|offset| company_start + offset + "    </COMPANY>".len())
        .expect("real capture company row is complete");
    let expected_row = &COMPANY_EXTENT[company_start..company_end];
    let response =
        COMPANY_EXTENT.replacen(expected_row, &format!("{expected_row}\n{expected_row}"), 1);

    assert_eq!(
        parse_company_book_extent(&response, COMPANY_NAME, COMPANY_GUID),
        Err(OutstandingsError::InvalidResponse(
            "company_identity_ambiguous"
        ))
    );
}

#[test]
fn final_profile_is_bounded_and_contains_no_self_reference() {
    let extent = extent();
    let window = DateWindow::parse(
        DateBoundaryProfile::EducationRestricted,
        "20260401",
        "20260401",
    )
    .unwrap()
    .narrow_partitions()
    .unwrap()
    .remove(0);
    let xml = ReadOnlyProfile::VoucherOutstandingsV1 {
        company: extent.company(),
        window: &window,
        alter_id_range: AlterIdRange::new(400, 800).unwrap(),
    }
    .render();
    assert!(xml.contains("<FILTERS>BridgeOutstandingsPartitionV1</FILTERS>"));
    assert!(xml.contains("$Date &gt;= ##SVFromDate AND $Date &lt;= ##SVToDate"));
    assert!(xml.contains("$AlterID &gt; 400 AND $AlterID &lt;= 800"));
    assert!(!xml.contains("$$NumItems"));
    assert!(!xml.contains("<COMPUTE>"));
    assert!(!xml.contains("ALLLEDGERENTRIES.BILLALLOCATIONS"));
    assert_eq!(xml.matches("ALLLEDGERENTRIES.*").count(), 1);
}

#[test]
fn wildcard_live_window_parses_and_computes_exactly() {
    let extent = extent();
    let window = DateWindow::parse(
        DateBoundaryProfile::EducationRestricted,
        "20250401",
        "20260302",
    )
    .unwrap();
    let request_window = DateWindow::parse(
        DateBoundaryProfile::EducationRestricted,
        "20250401",
        "20250501",
    )
    .unwrap()
    .narrow_partitions()
    .unwrap()
    .remove(0);
    let request = ReadOnlyProfile::VoucherOutstandingsV1 {
        company: extent.company(),
        window: &request_window,
        alter_id_range: full_alter_id_range(),
    }
    .render();
    assert!(request.contains("<FILTERS>BridgeOutstandingsPartitionV1</FILTERS>"));
    let segment = verify_segment_pair(
        VOUCHERS,
        VOUCHERS,
        extent.company(),
        window.clone(),
        full_alter_id_range(),
    )
    .expect("paired live captures verify");
    let segment = match segment {
        SegmentVerification::Complete(segment) => segment,
        SegmentVerification::Partial(partial) => {
            panic!("live segment was partial: {}", partial.reason_code)
        }
    };
    assert_eq!(segment.vouchers().len(), 75);
    assert_eq!(
        segment.vouchers().first().unwrap().date.as_str(),
        "20250401"
    );
    assert_eq!(segment.vouchers().last().unwrap().date.as_str(), "20260302");

    let scan = assemble_scan(
        extent.company().clone(),
        window,
        capture_high_water(),
        vec![SegmentVerification::Complete(segment)],
    );
    let ScanResult::Complete(scan) = scan else {
        panic!("live one-day capture did not assemble as complete")
    };
    let report = compute_outstandings(&scan, TallyDate::parse("20260302").unwrap())
        .expect("real capture computes exactly");
    assert_eq!(report.source_voucher_count, 75);
    assert_eq!(report.source_bytes, 1_504_566);
    assert_eq!(report.receivable_total.as_str(), "223055.4");
    assert_eq!(report.payable_total.as_str(), "295424.8");
    assert_eq!(report.ageing.days_0_30.as_str(), "38420.8");
    assert_eq!(report.ageing.days_31_60.as_str(), "24119.2");
    assert_eq!(report.ageing.days_61_90.as_str(), "0");
    assert_eq!(report.ageing.days_90_plus.as_str(), "160515.4");
}

#[test]
fn wildcard_live_capture_preserves_named_bill_type_distribution() {
    let extent = extent();
    let window = DateWindow::parse(
        DateBoundaryProfile::EducationRestricted,
        "20250401",
        "20260302",
    )
    .unwrap();
    let segment = verify_segment_pair(
        VOUCHERS,
        VOUCHERS,
        extent.company(),
        window,
        full_alter_id_range(),
    )
    .expect("paired live captures verify");
    let SegmentVerification::Complete(segment) = segment else {
        panic!("live bill-allocation capture was not complete")
    };
    let allocations = segment
        .vouchers()
        .iter()
        .flat_map(|voucher| &voucher.ledger_entries)
        .flat_map(|entry| &entry.bill_allocations)
        .collect::<Vec<_>>();
    let new_refs = allocations
        .iter()
        .filter(|allocation| allocation.bill_type == "New Ref")
        .count();
    let against_refs = allocations
        .iter()
        .filter(|allocation| allocation.bill_type == "Agst Ref")
        .count();
    assert_eq!(new_refs, 28);
    assert_eq!(against_refs, 24);
    assert!(allocations
        .iter()
        .any(|allocation| allocation.bill_type != "On Account"));
    assert!(allocations
        .iter()
        .filter(|allocation| allocation.bill_type != "On Account")
        .all(|allocation| allocation
            .name
            .as_deref()
            .is_some_and(|name| !name.is_empty())));
    assert!(allocations
        .iter()
        .all(|allocation| matches!(&allocation.amount, MoneyValue::Exact(_))));
}

#[test]
fn paired_row_difference_is_partial_not_complete() {
    let extent = extent();
    let window = DateWindow::parse(
        DateBoundaryProfile::EducationRestricted,
        "20250401",
        "20260302",
    )
    .unwrap();
    let changed = VOUCHERS.replacen(
        "<VOUCHERNUMBER>1</VOUCHERNUMBER>",
        "<VOUCHERNUMBER>changed</VOUCHERNUMBER>",
        1,
    );
    let result = verify_segment_pair(
        VOUCHERS,
        &changed,
        extent.company(),
        window,
        full_alter_id_range(),
    )
    .unwrap();
    assert!(matches!(result, SegmentVerification::Partial(_)));
}

#[test]
fn paired_wire_length_difference_is_partial_even_when_rows_parse_identically() {
    let extent = extent();
    let window = DateWindow::parse(
        DateBoundaryProfile::EducationRestricted,
        "20250401",
        "20260302",
    )
    .unwrap();
    let changed = VOUCHERS.replacen("<BODY>", "<BODY> ", 1);
    let result = verify_segment_pair(
        VOUCHERS,
        &changed,
        extent.company(),
        window,
        full_alter_id_range(),
    )
    .unwrap();
    assert!(matches!(result, SegmentVerification::Partial(_)));
}

#[test]
fn empty_bill_amount_is_quarantined_and_never_computed_as_zero() {
    let extent = extent();
    let window = DateWindow::parse(
        DateBoundaryProfile::EducationRestricted,
        "20250401",
        "20260302",
    )
    .unwrap();
    let changed = VOUCHERS.replacen(
        "<BILLID TYPE=\"Number\"> 29</BILLID>\r\n       <AMOUNT>-76228.00</AMOUNT>",
        "<BILLID TYPE=\"Number\"> 29</BILLID>\r\n       <AMOUNT></AMOUNT>",
        1,
    );
    assert_ne!(
        changed, VOUCHERS,
        "representative bill amount was not found"
    );
    let segment = verify_segment_pair(
        &changed,
        &changed,
        extent.company(),
        window.clone(),
        full_alter_id_range(),
    )
    .unwrap();
    let scan = assemble_scan(
        extent.company().clone(),
        window,
        capture_high_water(),
        vec![segment],
    );
    let scan = match scan {
        ScanResult::Complete(scan) => scan,
        ScanResult::Partial(partial) => {
            panic!("paired structure was partial: {}", partial.reason_code)
        }
    };
    assert_eq!(
        compute_outstandings(&scan, TallyDate::parse("20260401").unwrap()),
        Err(OutstandingsError::InvalidAmount)
    );
}
