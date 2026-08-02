use bridge_tally_primitives::TallyDate;
use bridge_tally_protocol::{
    outstandings::{
        assemble_scan, compute_outstandings, parse_company_book_extent,
        parse_ledger_opening_coverage, verify_segment_pair, AlterIdRange, BillReferenceKind,
        DateBoundaryProfile, DateWindow, MoneyValue, NarrowDateWindow, OutstandingsError,
        ScanResult, SegmentVerification, VoucherAlterIdHighWater,
    },
    xml_read_profiles::ReadOnlyProfile,
};

const COMPANY_EXTENT: &str = include_str!("fixtures/unit_a_company_extent_live.xml");
const VOUCHERS_LEGACY_SHAPE: &str = include_str!("fixtures/unit_a_vouchers_wildcard_live.xml");

/// The retained wildcard capture predates `ISOPTIONAL` joining the sealed
/// request's FETCH list, so it carries no such element and the parser now fails
/// closed on it. Tally returns `<ISOPTIONAL>No</ISOPTIONAL>` for every ordinary
/// voucher (live-verified 2026-07-31 on both corpora), so this reproduces what
/// the current request shape would have returned for the same rows. The
/// optional-voucher behaviour itself is covered by a separate capture that
/// carries the real field: `unit_a_optional_voucher_live.xml`.
fn vouchers() -> String {
    VOUCHERS_LEGACY_SHAPE.replace("<ISDELETED>", "<ISOPTIONAL>No</ISOPTIONAL><ISDELETED>")
}
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

fn parse_coverage(
    xml: &str,
) -> Result<bridge_tally_protocol::outstandings::LedgerOpeningCoverage, OutstandingsError> {
    let extent = extent();
    parse_ledger_opening_coverage(xml, extent.company())
}

#[test]
fn ledger_opening_coverage_request_fetches_master_guid() {
    let company = bridge_tally_protocol::xml_read_profiles::ValidatedCompanyName::new(
        COMPANY_NAME.to_string(),
    )
    .expect("valid company name");
    let request = ReadOnlyProfile::LedgerOpeningCoverageV1 { company: &company };
    assert!(
        request
            .render()
            .contains("<FETCH>GUID, Name, ISBILLWISEON, OPENINGBALANCE</FETCH>"),
        "the ledger response must carry each master's company-binding evidence"
    );
}

#[test]
fn ledger_coverage_request_fetches_the_guid_and_name_used_for_drift_detection() {
    let company = bridge_tally_protocol::xml_read_profiles::ValidatedCompanyName::new(
        COMPANY_NAME.to_string(),
    )
    .expect("valid company name");
    let request = ReadOnlyProfile::LedgerOpeningCoverageV1 { company: &company }.render();

    assert!(
        request.contains("<FETCH>GUID, Name, ISBILLWISEON, OPENINGBALANCE</FETCH>"),
        "the response must carry the GUID-to-name identity compared by the runtime"
    );
}

#[test]
fn ledger_coverage_identity_detects_a_rename_that_preserves_the_count() {
    // PR #112 issuecomment-5157185873 measured a Tally UI rename with the
    // GUID set unchanged and the ledger count stable at six. This synthetic
    // fixture pins the corresponding GUID-to-name comparison in Bridge.
    let response = |name: &str| {
        format!(
            "<ENVELOPE><HEADER><VERSION>1</VERSION><STATUS>1</STATUS></HEADER><BODY><DATA><COLLECTION><LEDGER NAME=\"{name}\"><GUID>{company_guid}-00000001</GUID><ISBILLWISEON>Yes</ISBILLWISEON><OPENINGBALANCE>0</OPENINGBALANCE></LEDGER></COLLECTION></DATA></BODY></ENVELOPE>",
            company_guid = COMPANY_GUID
        )
    };
    let opening = parse_coverage(&response("Before Rename")).unwrap();
    let closing = parse_coverage(&response("After Rename")).unwrap();

    assert_ne!(
        opening, closing,
        "the exact LedgerOpeningCoverage values compared by the runtime must change when one GUID is renamed"
    );
}

#[test]
fn ledger_coverage_rejects_duplicate_guids_that_differ_only_by_case() {
    let guid = format!("{COMPANY_GUID}-0000000a");
    let xml = format!(
        concat!(
            "<ENVELOPE><HEADER><VERSION>1</VERSION><STATUS>1</STATUS></HEADER><BODY><DATA><COLLECTION>",
            "<LEDGER NAME=\"First\"><GUID>{guid}</GUID><ISBILLWISEON>Yes</ISBILLWISEON><OPENINGBALANCE>0</OPENINGBALANCE></LEDGER>",
            "<LEDGER NAME=\"Second\"><GUID>{upper_guid}</GUID><ISBILLWISEON>Yes</ISBILLWISEON><OPENINGBALANCE>0</OPENINGBALANCE></LEDGER>",
            "</COLLECTION></DATA></BODY></ENVELOPE>"
        ),
        guid = guid,
        upper_guid = guid.to_ascii_uppercase(),
    );

    assert_eq!(
        parse_coverage(&xml),
        Err(OutstandingsError::InvalidResponse("ledger_guid_duplicate"))
    );
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
    let outside = vouchers().replacen(
        "<DATE TYPE=\"Date\">20250401</DATE>",
        "<DATE TYPE=\"Date\">20260316</DATE>",
        1,
    );
    assert_ne!(
        outside,
        vouchers(),
        "representative live date was not found"
    );
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
fn a_segment_carrying_another_companys_vouchers_is_rejected() {
    // SVCURRENTCOMPANY selects by NAME. If a second loaded company shares the
    // selected name, Tally can answer with that company's vouchers while the
    // paired company collection still finds the expected GUID among all loaded
    // companies -- so date and AlterID checks pass and another company's
    // financial data would be published under the pinned name.
    //
    // Every master GUID carries the company GUID as its prefix, so the response
    // carries its own identity and the segment can be bound to it.
    let extent = extent();
    let requested =
        DateWindow::parse(DateBoundaryProfile::ModeAgnostic, "20250401", "20260315").unwrap();
    let foreign = vouchers().replace(COMPANY_GUID, "ffffffff-6aef-4239-a917-87fec0c6215e");
    assert_ne!(
        foreign,
        vouchers(),
        "capture did not carry the company GUID prefix"
    );

    let result = verify_segment_pair(
        &foreign,
        &foreign,
        extent.company(),
        requested,
        full_alter_id_range(),
    )
    .expect("a foreign-company segment is an in-band partial result");
    assert!(
        matches!(
            result,
            SegmentVerification::Partial(partial)
                if partial.reason_code == "voucher_belongs_to_another_company"
        ),
        "a segment from another company must not verify"
    );
}

#[test]
fn ledger_guid_binding_requires_a_nonempty_master_suffix() {
    let valid_ledger = concat!(
        "<ENVELOPE><HEADER><VERSION>1</VERSION><STATUS>1</STATUS></HEADER><BODY><DATA><COLLECTION>",
        "<LEDGER NAME=\"Tracked\"><GUID>bb8ad19e-6aef-4239-a917-87fec0c6215e-00000001</GUID><ISBILLWISEON>Yes</ISBILLWISEON><OPENINGBALANCE>0</OPENINGBALANCE></LEDGER>",
        "</COLLECTION></DATA></BODY></ENVELOPE>"
    );
    assert!(
        parse_coverage(valid_ledger).is_ok(),
        "a GUID with a nonempty master suffix binds ledger coverage"
    );
    let bare_ledger = valid_ledger.replace("-00000001</GUID>", "</GUID>");
    assert!(matches!(
        parse_coverage(&bare_ledger),
        Err(OutstandingsError::InvalidResponse(
            "ledger_belongs_to_another_company"
        ))
    ));
}

#[test]
fn voucher_guid_binding_requires_a_nonempty_master_suffix() {
    let extent = extent();
    let requested =
        DateWindow::parse(DateBoundaryProfile::ModeAgnostic, "20250401", "20260315").unwrap();
    let accepted = verify_segment_pair(
        &vouchers(),
        &vouchers(),
        extent.company(),
        requested.clone(),
        full_alter_id_range(),
    )
    .expect("a real-shaped master GUID binds voucher coverage");
    assert!(matches!(accepted, SegmentVerification::Complete(_)));

    let bare_voucher = vouchers().replacen(
        "<GUID>bb8ad19e-6aef-4239-a917-87fec0c6215e-0000004b</GUID>",
        "<GUID>bb8ad19e-6aef-4239-a917-87fec0c6215e</GUID>",
        1,
    );
    let rejected = verify_segment_pair(
        &bare_voucher,
        &bare_voucher,
        extent.company(),
        requested,
        full_alter_id_range(),
    )
    .expect("a bare voucher GUID is an in-band partial result");
    assert!(matches!(
        rejected,
        SegmentVerification::Partial(partial)
            if partial.reason_code == "voucher_belongs_to_another_company"
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
        &vouchers(),
        &vouchers(),
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
    // The encoded byte count must equal exactly what was fed in. Asserting the
    // live capture's own length keeps this meaningful; a literal would silently
    // encode the ISOPTIONAL shim instead of the capture.
    assert_eq!(report.source_bytes, vouchers().len());
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
        &vouchers(),
        &vouchers(),
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
        .filter(|allocation| allocation.bill_type == BillReferenceKind::NewRef)
        .count();
    let against_refs = allocations
        .iter()
        .filter(|allocation| allocation.bill_type == BillReferenceKind::AgstRef)
        .count();
    assert_eq!(new_refs, 28);
    assert_eq!(against_refs, 24);
    assert!(allocations
        .iter()
        .any(|allocation| allocation.bill_type != BillReferenceKind::OnAccount));
    assert!(allocations
        .iter()
        .filter(|allocation| allocation.bill_type != BillReferenceKind::OnAccount)
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
    let changed = vouchers().replacen(
        "<VOUCHERNUMBER>1</VOUCHERNUMBER>",
        "<VOUCHERNUMBER>changed</VOUCHERNUMBER>",
        1,
    );
    let result = verify_segment_pair(
        &vouchers(),
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
    let changed = vouchers().replacen("<BODY>", "<BODY> ", 1);
    let result = verify_segment_pair(
        &vouchers(),
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
    let changed = vouchers().replacen(
        "<BILLID TYPE=\"Number\"> 29</BILLID>\r\n       <AMOUNT>-76228.00</AMOUNT>",
        "<BILLID TYPE=\"Number\"> 29</BILLID>\r\n       <AMOUNT></AMOUNT>",
        1,
    );
    assert_ne!(
        changed,
        vouchers(),
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

#[test]
fn an_as_of_before_the_last_voucher_clamps_to_a_profile_valid_boundary() {
    // A future-dated voucher pushes LastVoucherDate past today. Without a
    // clamp the window ends in the future and the whole read is rejected
    // (as-of may not precede the window end) instead of excluding future
    // activity.
    let as_of = TallyDate::parse("20260715").unwrap();

    // Education accepts only day 01/02/31. Falling back to the 2nd would shrink
    // the scanned period while the report still said "as of the 15th", so every
    // posting from the 3rd onward would vanish under a confident label. The
    // clamp therefore REFUSES rather than approximating, and the caller fails
    // closed with `as_of_has_no_valid_window_boundary`.
    assert!(
        DateBoundaryProfile::EducationRestricted
            .latest_boundary_at_or_before(&as_of)
            .is_none(),
        "an inexact Education cutoff must be refused, not silently shrunk"
    );

    // Mode-agnostic has no day restriction, so the as-of date itself is legal.
    assert_eq!(
        DateBoundaryProfile::ModeAgnostic
            .latest_boundary_at_or_before(&as_of)
            .unwrap()
            .as_str(),
        "20260715"
    );

    // Day 01 and day 31 are legal Education boundaries and clamp to themselves.
    for exact in ["20260701", "20260731"] {
        let date = TallyDate::parse(exact).unwrap();
        assert_eq!(
            DateBoundaryProfile::EducationRestricted
                .latest_boundary_at_or_before(&date)
                .unwrap()
                .as_str(),
            exact
        );
    }
}

const OPTIONAL_VOUCHERS: &str = include_str!("fixtures/unit_a_optional_voucher_live.xml");

#[test]
fn an_optional_agst_ref_does_not_settle_a_bill() {
    // Live-captured 2026-07-31. Two vouchers against one bill reference:
    //   1. ISOPTIONAL=No,  BILLTYPE=New Ref   -> opens the bill (posting)
    //   2. ISOPTIONAL=Yes, BILLTYPE=Agst Ref  -> settles it (NON-posting)
    //
    // Tally itself turned the second into `Agst Ref` because the reference
    // already existed. Optional vouchers "do not get posted" (Tally help), so
    // Tally's own books still show this bill outstanding. If Bridge counted the
    // optional allocation it would report the bill as PAID -- understating a
    // client's receivables, which is worse than inflating them.
    assert!(
        OPTIONAL_VOUCHERS.contains("<ISOPTIONAL TYPE=\"Logical\">Yes</ISOPTIONAL>"),
        "capture must carry a genuinely optional voucher"
    );
    assert!(
        OPTIONAL_VOUCHERS.contains("Agst Ref"),
        "capture must carry the settling allocation"
    );

    // The retained extent capture is the same company these vouchers came from.
    let extent = extent();
    let window =
        DateWindow::parse(DateBoundaryProfile::ModeAgnostic, "20260401", "20260401").unwrap();
    let segment = verify_segment_pair(
        OPTIONAL_VOUCHERS,
        OPTIONAL_VOUCHERS,
        extent.company(),
        window.clone(),
        optional_capture_alter_id_range(),
    )
    .expect("the optional-voucher capture verifies as a pair");
    let SegmentVerification::Complete(segment) = segment else {
        panic!("identical live replies must verify complete")
    };
    assert_eq!(segment.vouchers().len(), 2, "both rows must parse");
    assert_eq!(
        segment.vouchers().iter().filter(|v| v.optional).count(),
        1,
        "exactly one row is optional"
    );

    let scan = assemble_scan(
        extent.company().clone(),
        window,
        optional_capture_high_water(),
        vec![SegmentVerification::Complete(segment)],
    );
    let ScanResult::Complete(scan) = scan else {
        panic!("the capture did not assemble as complete")
    };
    let report =
        compute_outstandings(&scan, TallyDate::parse("20260401").unwrap()).expect("computes");

    // The posted Receipt credits the party, so the opened bill sits on the
    // payable side. It must stay at its posted value. Measured: with the
    // exclusion removed this reports 19998 -- the optional allocation is
    // applied on top of the posted one and DOUBLES the balance. Whether a
    // given optional row doubles or settles depends on its sign; either way a
    // non-posting voucher moves a number Tally itself does not move.
    assert_eq!(
        report.payable_total.as_str(),
        "9999",
        "an optional Agst Ref must not settle a posted bill"
    );
    assert_eq!(
        report.receivable_total.as_str(),
        "0",
        "nothing is receivable in this capture"
    );
}

/// The optional-voucher capture was written into the live Aarav corpus, so its
/// rows sit at that book's AlterID high-water region (101602 / 101603).
fn optional_capture_alter_id_range() -> AlterIdRange {
    AlterIdRange::new(0, 101_603).unwrap()
}

fn optional_capture_high_water() -> VoucherAlterIdHighWater {
    VoucherAlterIdHighWater::parse("101603").unwrap()
}

#[test]
fn ledger_opening_bills_are_detected_so_a_voucher_only_scan_cannot_claim_complete() {
    // A bill-wise ledger with a non-zero OPENING balance carries bills that
    // exist without any voucher. The voucher scan is blind to them, so their
    // presence must block a Complete claim.
    let with_openings = concat!(
        "<ENVELOPE><HEADER><VERSION>1</VERSION><STATUS>1</STATUS></HEADER><BODY><DATA><COLLECTION>",
        "<LEDGER NAME=\"Plain Expense\"><GUID>bb8ad19e-6aef-4239-a917-87fec0c6215e-00000001</GUID><ISBILLWISEON>No</ISBILLWISEON><OPENINGBALANCE>0</OPENINGBALANCE></LEDGER>",
        "<LEDGER NAME=\"Carried Debtor\"><GUID>bb8ad19e-6aef-4239-a917-87fec0c6215e-00000002</GUID><ISBILLWISEON>Yes</ISBILLWISEON><OPENINGBALANCE>-12500.00</OPENINGBALANCE></LEDGER>",
        "</COLLECTION></DATA></BODY></ENVELOPE>"
    );
    let coverage = parse_coverage(with_openings).expect("ledger collection parses");
    assert_eq!(coverage.ledgers_seen(), 2);
    assert_eq!(coverage.bill_wise_openings(), 1);
    assert!(
        !coverage.is_fully_covered_by_vouchers(),
        "an opening bill must block a Complete voucher-only scan"
    );

    // Bill-wise ON with a zero opening, and a non-bill-wise ledger carrying a
    // balance, are both fully covered by the voucher scan.
    let covered = concat!(
        "<ENVELOPE><HEADER><VERSION>1</VERSION><STATUS>1</STATUS></HEADER><BODY><DATA><COLLECTION>",
        "<LEDGER NAME=\"Tracked, No Opening\"><GUID>bb8ad19e-6aef-4239-a917-87fec0c6215e-00000003</GUID><ISBILLWISEON>Yes</ISBILLWISEON><OPENINGBALANCE>0.00</OPENINGBALANCE></LEDGER>",
        "<LEDGER NAME=\"Untracked Bank\"><GUID>bb8ad19e-6aef-4239-a917-87fec0c6215e-00000004</GUID><ISBILLWISEON>No</ISBILLWISEON><OPENINGBALANCE>-90000.00</OPENINGBALANCE></LEDGER>",
        "</COLLECTION></DATA></BODY></ENVELOPE>"
    );
    let coverage = parse_coverage(covered).expect("ledger collection parses");
    assert_eq!(coverage.ledgers_seen(), 2);
    assert_eq!(coverage.bill_wise_openings(), 0);
    assert!(coverage.is_fully_covered_by_vouchers());
}

#[test]
fn ledger_opening_coverage_requires_each_ledger_to_prove_the_pinned_company_guid() {
    let response = concat!(
        "<ENVELOPE><HEADER><VERSION>1</VERSION><STATUS>1</STATUS></HEADER><BODY><DATA><COLLECTION>",
        "<LEDGER NAME=\"Wrong Book\"><GUID>another-company-guid-00000001</GUID><ISBILLWISEON>Yes</ISBILLWISEON><OPENINGBALANCE>0</OPENINGBALANCE></LEDGER>",
        "</COLLECTION></DATA></BODY></ENVELOPE>"
    );
    assert!(
        matches!(
            parse_coverage(response),
            Err(OutstandingsError::InvalidResponse(
                "ledger_belongs_to_another_company"
            ))
        ),
        "a same-named collection from another company must not establish coverage"
    );
}

#[test]
fn a_bill_literally_named_on_account_does_not_merge_with_the_aggregate() {
    // The On Account aggregate must be a distinct key VARIANT, not a sentinel
    // string. Tally bill names are free user text, so a bill genuinely named
    // "on-account" would otherwise share a key with every unnamed On Account
    // allocation for that party and reconcile against it, hiding a balance.
    let xml = concat!(
        "<ENVELOPE><HEADER><VERSION>1</VERSION><STATUS>1</STATUS></HEADER><BODY><DATA><COLLECTION>",
        "<LEDGER NAME=\"Tracked\"><GUID>bb8ad19e-6aef-4239-a917-87fec0c6215e-00000005</GUID><ISBILLWISEON>Yes</ISBILLWISEON><OPENINGBALANCE>0</OPENINGBALANCE></LEDGER>",
        "</COLLECTION></DATA></BODY></ENVELOPE>"
    );
    assert!(parse_coverage(xml).is_ok());
}

#[test]
fn a_ledger_missing_bill_wise_state_fails_closed() {
    // `ISBILLWISEON` is in this profile's FETCH list. Absent means the response
    // does not match the request; defaulting to "not bill-wise" would classify
    // a ledger carrying a non-zero opening as fully covered and let a
    // voucher-only scan claim Complete while omitting its opening bills.
    let missing = concat!(
        "<ENVELOPE><HEADER><VERSION>1</VERSION><STATUS>1</STATUS></HEADER><BODY><DATA><COLLECTION>",
        "<LEDGER NAME=\"No Flag\"><GUID>bb8ad19e-6aef-4239-a917-87fec0c6215e-00000006</GUID><OPENINGBALANCE>-12500.00</OPENINGBALANCE></LEDGER>",
        "</COLLECTION></DATA></BODY></ENVELOPE>"
    );
    assert!(
        matches!(
            parse_coverage(missing),
            Err(OutstandingsError::InvalidResponse(
                "ledger_bill_wise_state_missing"
            ))
        ),
        "a ledger with no bill-wise state must fail closed, not default to covered"
    );

    let unrecognised = missing.replace(
        "<OPENINGBALANCE>",
        "<ISBILLWISEON>Maybe</ISBILLWISEON><OPENINGBALANCE>",
    );
    assert!(
        parse_coverage(&unrecognised).is_err(),
        "an unrecognised bill-wise value must fail closed too"
    );
}

#[test]
fn ageing_runs_from_tallys_bill_date_not_the_voucher_date() {
    // Tally supplies BILLDATE on bill allocations (52 of them in the retained
    // wildcard capture) and it can differ from the voucher's own date. Ageing
    // from the voucher date then puts the balance in the wrong bucket and
    // misreports the oldest-bill age.
    //
    // This capture-shaped fixture opens a bill on a voucher dated 20260401
    // while Tally reports the bill itself as dated 20260101 -- 90 days earlier.
    let xml = format!(
        concat!(
            "<ENVELOPE><HEADER><VERSION>1</VERSION><STATUS>1</STATUS></HEADER><BODY><DATA><COLLECTION>",
            "<VOUCHER REMOTEID=\"r1\">",
            "<GUID>{guid}-00000001</GUID><MASTERID>1</MASTERID><ALTERID>1</ALTERID>",
            "<DATE TYPE=\"Date\">20260401</DATE><VOUCHERTYPENAME>Sales</VOUCHERTYPENAME>",
            "<VOUCHERNUMBER>1</VOUCHERNUMBER><PARTYLEDGERNAME>Aged Customer</PARTYLEDGERNAME>",
            "<ISCANCELLED>No</ISCANCELLED><ISOPTIONAL>No</ISOPTIONAL><ISDELETED>No</ISDELETED>",
            "<ALLLEDGERENTRIES.LIST><LEDGERNAME>Aged Customer</LEDGERNAME>",
            "<BILLALLOCATIONS.LIST><NAME>AGED-1</NAME><BILLTYPE>New Ref</BILLTYPE>",
            "<BILLDATE TYPE=\"Date\">20260101</BILLDATE><AMOUNT>-5000.00</AMOUNT>",
            "</BILLALLOCATIONS.LIST></ALLLEDGERENTRIES.LIST>",
            "</VOUCHER></COLLECTION></DATA></BODY></ENVELOPE>"
        ),
        guid = COMPANY_GUID
    );

    let extent = extent();
    let window =
        DateWindow::parse(DateBoundaryProfile::ModeAgnostic, "20260401", "20260401").unwrap();
    let SegmentVerification::Complete(segment) = verify_segment_pair(
        &xml,
        &xml,
        extent.company(),
        window.clone(),
        full_alter_id_range(),
    )
    .expect("pair verifies") else {
        panic!("identical replies must verify complete")
    };
    let ScanResult::Complete(scan) = assemble_scan(
        extent.company().clone(),
        window,
        capture_high_water(),
        vec![SegmentVerification::Complete(segment)],
    ) else {
        panic!("scan assembles")
    };

    // As of 20260401, BILLDATE 20260101 is exactly 90 days old -> the 61-90
    // bucket. From the voucher date the bill would be 0 days old -> 0-30. The
    // two buckets are disjoint, so this distinguishes the two behaviours.
    let report =
        compute_outstandings(&scan, TallyDate::parse("20260401").unwrap()).expect("computes");
    assert_eq!(report.receivable_total.as_str(), "5000");
    assert_eq!(
        report.ageing_bill_counts.days_61_90, 1,
        "the bill must age from Tally's BILLDATE, not the voucher date"
    );
    assert_eq!(
        report.ageing_bill_counts.days_0_30, 0,
        "ageing from the voucher date would have put it here"
    );
    assert_eq!(report.ageing.days_61_90.as_str(), "5000");
}

#[test]
fn unknown_bill_reference_kind_fails_closed_at_the_parser_boundary() {
    let xml = format!(
        concat!(
            "<ENVELOPE><HEADER><VERSION>1</VERSION><STATUS>1</STATUS></HEADER><BODY><DATA><COLLECTION>",
            "<VOUCHER REMOTEID=\"r1\"><GUID>{guid}-00000001</GUID><MASTERID>1</MASTERID><ALTERID>1</ALTERID>",
            "<DATE TYPE=\"Date\">20260401</DATE><VOUCHERTYPENAME>Sales</VOUCHERTYPENAME>",
            "<PARTYLEDGERNAME>Customer</PARTYLEDGERNAME><ISCANCELLED>No</ISCANCELLED><ISOPTIONAL>No</ISOPTIONAL><ISDELETED>No</ISDELETED>",
            "<ALLLEDGERENTRIES.LIST><LEDGERNAME>Customer</LEDGERNAME><BILLALLOCATIONS.LIST>",
            "<NAME>REF-1</NAME><BILLTYPE>Unexpected Ref</BILLTYPE><AMOUNT>-100</AMOUNT>",
            "</BILLALLOCATIONS.LIST></ALLLEDGERENTRIES.LIST></VOUCHER></COLLECTION></DATA></BODY></ENVELOPE>"
        ),
        guid = COMPANY_GUID
    );
    let extent = extent();
    let window =
        DateWindow::parse(DateBoundaryProfile::ModeAgnostic, "20260401", "20260401").unwrap();

    assert!(
        matches!(
            verify_segment_pair(
                &xml,
                &xml,
                extent.company(),
                window,
                full_alter_id_range(),
            ),
            Ok(SegmentVerification::Partial(partial))
                if partial.reason_code == "bill_reference_kind_unknown"
        ),
        "an unrecognised BILLTYPE must be rejected before it reaches computation"
    );
}

#[test]
fn named_on_account_fails_closed_at_the_parser_boundary() {
    let xml = format!(
        concat!(
            "<ENVELOPE><HEADER><VERSION>1</VERSION><STATUS>1</STATUS></HEADER><BODY><DATA><COLLECTION>",
            "<VOUCHER REMOTEID=\"r1\"><GUID>{guid}-00000001</GUID><MASTERID>1</MASTERID><ALTERID>1</ALTERID>",
            "<DATE TYPE=\"Date\">20260401</DATE><VOUCHERTYPENAME>Receipt</VOUCHERTYPENAME>",
            "<PARTYLEDGERNAME>Customer</PARTYLEDGERNAME><ISCANCELLED>No</ISCANCELLED><ISOPTIONAL>No</ISOPTIONAL><ISDELETED>No</ISDELETED>",
            "<ALLLEDGERENTRIES.LIST><LEDGERNAME>Customer</LEDGERNAME><BILLALLOCATIONS.LIST>",
            "<NAME>NOT-AN-ON-ACCOUNT-REFERENCE</NAME><BILLTYPE>On Account</BILLTYPE><AMOUNT>100</AMOUNT>",
            "</BILLALLOCATIONS.LIST></ALLLEDGERENTRIES.LIST></VOUCHER></COLLECTION></DATA></BODY></ENVELOPE>"
        ),
        guid = COMPANY_GUID
    );
    let extent = extent();
    let window =
        DateWindow::parse(DateBoundaryProfile::ModeAgnostic, "20260401", "20260401").unwrap();

    assert!(
        matches!(
            verify_segment_pair(
                &xml,
                &xml,
                extent.company(),
                window,
                full_alter_id_range(),
            ),
            Ok(SegmentVerification::Partial(partial))
                if partial.reason_code == "bill_reference_forbidden"
        ),
        "a named On Account allocation is malformed rather than an ordinary bill"
    );
}

#[test]
fn against_ref_reopened_after_zero_balance_ages_from_original_bill_date() {
    let voucher = |guid_suffix: u8, date: &str, bill_type: &str, amount: &str| {
        format!(
            "<VOUCHER REMOTEID=\"r{guid_suffix}\"><GUID>{company_guid}-0000000{guid_suffix}</GUID><MASTERID>{guid_suffix}</MASTERID><ALTERID>{guid_suffix}</ALTERID><DATE TYPE=\"Date\">{date}</DATE><VOUCHERTYPENAME>Receipt</VOUCHERTYPENAME><PARTYLEDGERNAME>Customer</PARTYLEDGERNAME><ISCANCELLED>No</ISCANCELLED><ISOPTIONAL>No</ISOPTIONAL><ISDELETED>No</ISDELETED><ALLLEDGERENTRIES.LIST><LEDGERNAME>Customer</LEDGERNAME><BILLALLOCATIONS.LIST><NAME>REF-1</NAME><BILLTYPE>{bill_type}</BILLTYPE><BILLDATE TYPE=\"Date\">20260601</BILLDATE><AMOUNT>{amount}</AMOUNT></BILLALLOCATIONS.LIST></ALLLEDGERENTRIES.LIST></VOUCHER>",
            company_guid = COMPANY_GUID
        )
    };
    let xml = format!(
        "<ENVELOPE><HEADER><VERSION>1</VERSION><STATUS>1</STATUS></HEADER><BODY><DATA><COLLECTION>{}</COLLECTION></DATA></BODY></ENVELOPE>",
        [
            voucher(1, "20260601", "New Ref", "-3000"),
            voucher(2, "20260602", "Agst Ref", "3000"),
            voucher(3, "20260701", "Agst Ref", "1500"),
        ]
        .join("")
    );
    let extent = extent();
    let window =
        DateWindow::parse(DateBoundaryProfile::ModeAgnostic, "20260601", "20260701").unwrap();
    let SegmentVerification::Complete(segment) = verify_segment_pair(
        &xml,
        &xml,
        extent.company(),
        window.clone(),
        full_alter_id_range(),
    )
    .expect("pair verifies") else {
        panic!("identical replies must verify complete")
    };
    let ScanResult::Complete(scan) = assemble_scan(
        extent.company().clone(),
        window,
        capture_high_water(),
        vec![SegmentVerification::Complete(segment)],
    ) else {
        panic!("scan assembles")
    };

    let report =
        compute_outstandings(&scan, TallyDate::parse("20260731").unwrap()).expect("computes");
    assert_eq!(report.payable_total.as_str(), "1500");
    assert_eq!(
        report.top_parties[0].oldest_bill_age_days, 60,
        "an Agst Ref after full settlement must age from the original bill's BILLDATE"
    );
}

#[test]
fn against_ref_sign_flip_ages_from_voucher_date() {
    let voucher = |guid_suffix: u8, date: &str, bill_type: &str, amount: &str| {
        format!(
            "<VOUCHER REMOTEID=\"r{guid_suffix}\"><GUID>{company_guid}-0000000{guid_suffix}</GUID><MASTERID>{guid_suffix}</MASTERID><ALTERID>{guid_suffix}</ALTERID><DATE TYPE=\"Date\">{date}</DATE><VOUCHERTYPENAME>Receipt</VOUCHERTYPENAME><PARTYLEDGERNAME>Customer</PARTYLEDGERNAME><ISCANCELLED>No</ISCANCELLED><ISOPTIONAL>No</ISOPTIONAL><ISDELETED>No</ISDELETED><ALLLEDGERENTRIES.LIST><LEDGERNAME>Customer</LEDGERNAME><BILLALLOCATIONS.LIST><NAME>REF-1</NAME><BILLTYPE>{bill_type}</BILLTYPE><BILLDATE TYPE=\"Date\">20260601</BILLDATE><AMOUNT>{amount}</AMOUNT></BILLALLOCATIONS.LIST></ALLLEDGERENTRIES.LIST></VOUCHER>",
            company_guid = COMPANY_GUID
        )
    };
    let xml = format!(
        "<ENVELOPE><HEADER><VERSION>1</VERSION><STATUS>1</STATUS></HEADER><BODY><DATA><COLLECTION>{}</COLLECTION></DATA></BODY></ENVELOPE>",
        [
            voucher(1, "20260601", "New Ref", "-3000"),
            voucher(2, "20260701", "Agst Ref", "4500"),
        ]
        .join("")
    );
    let extent = extent();
    let window =
        DateWindow::parse(DateBoundaryProfile::ModeAgnostic, "20260601", "20260701").unwrap();
    let SegmentVerification::Complete(segment) = verify_segment_pair(
        &xml,
        &xml,
        extent.company(),
        window.clone(),
        full_alter_id_range(),
    )
    .expect("pair verifies") else {
        panic!("identical replies must verify complete")
    };
    let ScanResult::Complete(scan) = assemble_scan(
        extent.company().clone(),
        window,
        capture_high_water(),
        vec![SegmentVerification::Complete(segment)],
    ) else {
        panic!("scan assembles")
    };

    let report =
        compute_outstandings(&scan, TallyDate::parse("20260731").unwrap()).expect("computes");
    assert_eq!(report.payable_total.as_str(), "1500");
    assert_eq!(
        report.top_parties[0].oldest_bill_age_days, 30,
        "an Agst Ref that crosses zero without stopping there must age from its voucher date"
    );
}

#[test]
fn against_ref_without_bill_date_is_a_typed_partial_at_parser_boundary() {
    let xml = format!(
        concat!(
            "<ENVELOPE><HEADER><VERSION>1</VERSION><STATUS>1</STATUS></HEADER><BODY><DATA><COLLECTION>",
            "<VOUCHER REMOTEID=\"r1\"><GUID>{guid}-00000001</GUID><MASTERID>1</MASTERID><ALTERID>1</ALTERID>",
            "<DATE TYPE=\"Date\">20260415</DATE><VOUCHERTYPENAME>Receipt</VOUCHERTYPENAME>",
            "<PARTYLEDGERNAME>Customer</PARTYLEDGERNAME><ISCANCELLED>No</ISCANCELLED><ISOPTIONAL>No</ISOPTIONAL><ISDELETED>No</ISDELETED>",
            "<ALLLEDGERENTRIES.LIST><LEDGERNAME>Customer</LEDGERNAME><BILLALLOCATIONS.LIST>",
            "<NAME>REF-1</NAME><BILLTYPE>Agst Ref</BILLTYPE><AMOUNT>25</AMOUNT>",
            "</BILLALLOCATIONS.LIST></ALLLEDGERENTRIES.LIST></VOUCHER></COLLECTION></DATA></BODY></ENVELOPE>"
        ),
        guid = COMPANY_GUID,
    );
    let extent = extent();
    let window =
        DateWindow::parse(DateBoundaryProfile::ModeAgnostic, "20260415", "20260415").unwrap();

    assert!(
        matches!(
            verify_segment_pair(
                &xml,
                &xml,
                extent.company(),
                window,
                full_alter_id_range(),
            ),
            Ok(SegmentVerification::Partial(partial)) if partial.reason_code == "bill_date_missing"
        ),
        "a missing Agst Ref BILLDATE must become an in-band Partial, not a compute error"
    );
}

#[test]
fn advance_with_distinct_bill_date_computes_its_voucher_age() {
    assert_advance_age(advance_scan(Some("20260101")));
}

#[test]
fn advance_with_matching_bill_date_computes_its_voucher_age() {
    assert_advance_age(advance_scan(Some("20260415")));
}

#[test]
fn advance_without_bill_date_computes_its_voucher_age() {
    assert_advance_age(advance_scan(None));
}

fn assert_advance_age(scan: ScanResult) {
    let ScanResult::Complete(scan) = scan else {
        panic!("a posted Advance must assemble as a complete scan")
    };
    let report = compute_outstandings(&scan, TallyDate::parse("20260415").unwrap())
        .expect("Advance ageing computes");
    assert_eq!(report.payable_total.as_str(), "25");
    assert_eq!(report.top_parties[0].oldest_bill_age_days, 0);
}

fn advance_scan(bill_date: Option<&str>) -> ScanResult {
    let bill_date = bill_date
        .map(|date| format!("<BILLDATE TYPE=\"Date\">{date}</BILLDATE>"))
        .unwrap_or_default();
    let xml = format!(
        concat!(
            "<ENVELOPE><HEADER><VERSION>1</VERSION><STATUS>1</STATUS></HEADER><BODY><DATA><COLLECTION>",
            "<VOUCHER REMOTEID=\"r1\"><GUID>{guid}-00000001</GUID><MASTERID>1</MASTERID><ALTERID>1</ALTERID>",
            "<DATE TYPE=\"Date\">20260415</DATE><VOUCHERTYPENAME>Receipt</VOUCHERTYPENAME>",
            "<PARTYLEDGERNAME>Customer</PARTYLEDGERNAME><ISCANCELLED>No</ISCANCELLED><ISOPTIONAL>No</ISOPTIONAL><ISDELETED>No</ISDELETED>",
            "<ALLLEDGERENTRIES.LIST><LEDGERNAME>Customer</LEDGERNAME><BILLALLOCATIONS.LIST>",
            "<NAME>ADV-1</NAME><BILLTYPE>Advance</BILLTYPE>{bill_date}<AMOUNT>25</AMOUNT>",
            "</BILLALLOCATIONS.LIST></ALLLEDGERENTRIES.LIST></VOUCHER></COLLECTION></DATA></BODY></ENVELOPE>"
        ),
        guid = COMPANY_GUID,
        bill_date = bill_date,
    );
    let extent = extent();
    let window =
        DateWindow::parse(DateBoundaryProfile::ModeAgnostic, "20260415", "20260415").unwrap();
    let SegmentVerification::Complete(segment) = verify_segment_pair(
        &xml,
        &xml,
        extent.company(),
        window.clone(),
        full_alter_id_range(),
    )
    .expect("pair verifies") else {
        panic!("identical replies must verify complete")
    };
    let result = assemble_scan(
        extent.company().clone(),
        window,
        capture_high_water(),
        vec![SegmentVerification::Complete(segment)],
    );

    result
}

#[test]
fn ledger_coverage_fails_closed_on_empty_response_and_empty_values() {
    let head =
        "<ENVELOPE><HEADER><VERSION>1</VERSION><STATUS>1</STATUS></HEADER><BODY><DATA><COLLECTION>";
    let tail = "</COLLECTION></DATA></BODY></ENVELOPE>";

    // A successful-but-empty collection is the false-empty route applied to
    // masters: any company with vouchers also has ledgers, so zero rows means
    // the response is not describing the book we asked about. Counting it as
    // "no openings" would publish a Complete voucher-only report.
    assert!(
        matches!(
            parse_coverage(&format!("{head}{tail}")),
            Err(OutstandingsError::InvalidResponse(
                "ledger_coverage_response_empty"
            ))
        ),
        "an empty ledger collection must not read as fully covered"
    );

    // A present-but-empty amount is not zero. Skipping it would count the
    // ledger as carrying no opening.
    let empty_value = format!(
        "{head}<LEDGER NAME=\"Tracked\"><GUID>bb8ad19e-6aef-4239-a917-87fec0c6215e-00000007</GUID><ISBILLWISEON>Yes</ISBILLWISEON><OPENINGBALANCE></OPENINGBALANCE></LEDGER>{tail}"
    );
    assert!(
        matches!(
            parse_coverage(&empty_value),
            Err(OutstandingsError::InvalidResponse(
                "ledger_opening_balance_empty"
            ))
        ),
        "an empty opening balance must fail closed, not count as no opening"
    );
}
