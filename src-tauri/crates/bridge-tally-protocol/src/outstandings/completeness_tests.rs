use super::*;
use crate::outstandings::{parse_company_book_extent, NarrowDateWindow};

const COMPANY_EXTENT: &str = include_str!("../../tests/fixtures/unit_a_company_extent_live.xml");
const VOUCHERS_LEGACY_SHAPE: &str =
    include_str!("../../tests/fixtures/unit_a_vouchers_wildcard_live.xml");

/// The capture predates `ISOPTIONAL` joining the sealed FETCH list, and the
/// parser now fails closed without it. Tally returns `No` for every
/// ordinary voucher (live-verified), so this reproduces what the current
/// request shape would have returned for these same rows.
fn vouchers() -> String {
    VOUCHERS_LEGACY_SHAPE.replace("<ISDELETED>", "<ISOPTIONAL>No</ISOPTIONAL><ISDELETED>")
}

fn company() -> PinnedCompany {
    parse_company_book_extent(
        COMPANY_EXTENT,
        "Aarav Trading Company Demo",
        "bb8ad19e-6aef-4239-a917-87fec0c6215e",
    )
    .unwrap()
    .company()
    .clone()
}

fn reporting_window() -> DateWindow {
    DateWindow::parse(
        crate::outstandings::DateBoundaryProfile::ModeAgnostic,
        "20250401",
        "20260302",
    )
    .unwrap()
}

fn high_water(value: u64) -> VoucherAlterIdHighWater {
    VoucherAlterIdHighWater::parse(&value.to_string()).unwrap()
}

fn empty_partition(company: &PinnedCompany, window: DateWindow) -> CompleteScan {
    let empty = only_vouchers_with_alter_ids(&vouchers(), &[]);
    let verification = verify_segment_pair(
        &empty,
        &empty,
        company,
        window.clone(),
        AlterIdRange::new(0, 440).unwrap(),
    )
    .unwrap();
    let ScanResult::Complete(scan) =
        assemble_scan(company.clone(), window, high_water(440), vec![verification])
    else {
        panic!("paired live empty response should complete one date partition")
    };
    scan
}

fn book_extent(
    company: &PinnedCompany,
    reporting: &DateWindow,
    high_water: VoucherAlterIdHighWater,
) -> CompanyBookExtent {
    CompanyBookExtent::new(
        company.clone(),
        reporting.from().clone(),
        reporting.to().clone(),
        Some(high_water),
        None,
    )
}

#[test]
fn live_empty_pairs_are_complete_budget_slices_but_truncation_is_partial() {
    let company = company();
    let empty = only_vouchers_with_alter_ids(&vouchers(), &[]);
    assert!(matches!(
        verify_segment_pair(
            &empty,
            &empty,
            &company,
            reporting_window(),
            AlterIdRange::new(148, 200).unwrap(),
        )
        .unwrap(),
        SegmentVerification::Complete(segment) if segment.vouchers().is_empty()
    ));
    let truncated = &vouchers()[..vouchers().len() - "</ENVELOPE>\n".len()];
    assert!(matches!(
        verify_segment_pair(
            truncated,
            truncated,
            &company,
            reporting_window(),
            AlterIdRange::new(0, 440).unwrap(),
        )
        .unwrap(),
        SegmentVerification::Partial(_)
    ));
}

#[test]
fn same_length_wire_mutation_is_partial_even_when_parsed_rows_match() {
    let company = company();
    let second = vouchers().replacen("<GROUP>0</GROUP>", "<GROUP>1</GROUP>", 1);
    assert_eq!(vouchers().len(), second.len());
    assert!(matches!(
        verify_segment_pair(
            &vouchers(),
            &second,
            &company,
            reporting_window(),
            AlterIdRange::new(0, 440).unwrap(),
        )
        .unwrap(),
        SegmentVerification::Partial(partial)
            if partial.reason_code == "paired_segment_mismatch"
    ));
}

#[test]
fn wider_date_witness_requires_rows_outside_the_claimed_empty_window() {
    let company = company();
    let wider = only_vouchers_with_alter_ids(&vouchers(), &[440]);
    let empty_window = DateWindow::parse(
        crate::outstandings::DateBoundaryProfile::ModeAgnostic,
        "20250531",
        "20250531",
    )
    .unwrap();
    let empty_partition = empty_partition(&company, empty_window.clone());
    let corroboration = verify_empty_date_window_with_wider_pair(
        &empty_partition,
        &wider,
        &wider,
        reporting_window(),
        AlterIdRange::new(0, 440).unwrap(),
    )
    .unwrap();
    let EmptyDateWindowVerification::Complete(witness) = corroboration else {
        panic!("a wider dated row outside the empty window should corroborate it")
    };
    assert_eq!(witness.empty_window(), &empty_window);
    assert_eq!(witness.observed_row_count(), 1);
}

#[test]
fn widening_witness_rejects_a_row_dated_inside_the_empty_window() {
    let company = company();
    let contradicting = only_vouchers_with_alter_ids(&vouchers(), &[440]);
    let claimed_empty = DateWindow::parse(
        crate::outstandings::DateBoundaryProfile::ModeAgnostic,
        "20250401",
        "20250401",
    )
    .unwrap();
    let empty_partition = empty_partition(&company, claimed_empty);
    let result = verify_empty_date_window_with_wider_pair(
        &empty_partition,
        &contradicting,
        &contradicting,
        reporting_window(),
        AlterIdRange::new(0, 440).unwrap(),
    )
    .unwrap();
    let EmptyDateWindowVerification::Partial(partial) = result else {
        panic!("a wider read contradicted the claimed empty date window")
    };
    assert_eq!(
        partial.reason_code,
        "empty_date_window_contradicted_by_wider_read"
    );
}

fn only_vouchers_with_alter_ids(xml: &str, alter_ids: &[u64]) -> String {
    let mut output = String::with_capacity(xml.len());
    let mut cursor = 0_usize;
    while let Some(relative_start) = xml[cursor..].find("<VOUCHER ") {
        let start = cursor + relative_start;
        output.push_str(&xml[cursor..start]);
        let relative_end = xml[start..]
            .find("</VOUCHER>")
            .expect("real capture voucher is complete");
        let end = start + relative_end + "</VOUCHER>".len();
        let voucher = &xml[start..end];
        if alter_ids
            .iter()
            .any(|alter_id| voucher.contains(&format!("> {alter_id}</ALTERID>")))
        {
            output.push_str(voucher);
        }
        cursor = end;
    }
    output.push_str(&xml[cursor..]);
    output
}

#[test]
fn a_row_outside_the_requested_alter_id_range_is_partial() {
    let company = company();
    let result = verify_segment_pair(
        &vouchers(),
        &vouchers(),
        &company,
        reporting_window(),
        AlterIdRange::new(0, 200).unwrap(),
    )
    .unwrap();
    let SegmentVerification::Partial(partial) = result else {
        panic!("an out-of-range AlterID became complete")
    };
    assert_eq!(
        partial.reason_code,
        "voucher_outside_requested_alter_id_range"
    );
}

#[test]
fn duplicate_alter_ids_fail_at_the_segment_boundary() {
    let company = company();
    let duplicate = vouchers().replacen(
        "<ALTERID TYPE=\"Number\"> 77</ALTERID>",
        "<ALTERID TYPE=\"Number\"> 75</ALTERID>",
        1,
    );
    assert_ne!(duplicate, vouchers());
    let result = verify_segment_pair(
        &duplicate,
        &duplicate,
        &company,
        reporting_window(),
        AlterIdRange::new(0, 440).unwrap(),
    )
    .unwrap();
    assert!(matches!(
        result,
        SegmentVerification::Partial(partial)
            if partial.reason_code == "duplicate_voucher_alter_id_within_segment"
    ));
}

#[test]
fn an_alter_id_subset_can_complete_without_spanning_the_reporting_dates() {
    let company = company();
    let subset = only_vouchers_with_alter_ids(&vouchers(), &[75]);
    let result = verify_segment_pair(
        &subset,
        &subset,
        &company,
        reporting_window(),
        AlterIdRange::new(0, 75).unwrap(),
    )
    .unwrap();
    let SegmentVerification::Complete(segment) = result else {
        panic!("a bounded AlterID subset did not complete")
    };
    assert_eq!(segment.vouchers().len(), 1);
    assert_eq!(segment.vouchers()[0].alter_id.get(), 75);
    assert_ne!(segment.vouchers()[0].date, *segment.reporting_window().to());
}

#[test]
fn scan_assembly_requires_non_overlapping_contiguous_alter_id_coverage() {
    let company = company();
    let requested = reporting_window();
    let parsed = parse_segment(
        &vouchers(),
        &company,
        &requested,
        AlterIdRange::new(0, 440).unwrap(),
    )
    .unwrap();
    let voucher = parsed.vouchers[0].clone();
    let complete = assemble_scan(
        company.clone(),
        requested.clone(),
        high_water(440),
        vec![
            SegmentVerification::Complete(CompleteSegment {
                reporting_window: requested.clone(),
                alter_id_range: AlterIdRange::new(0, 200).unwrap(),
                vouchers: vec![voucher],
                encoded_bytes: 100,
            }),
            SegmentVerification::Complete(CompleteSegment {
                reporting_window: requested.clone(),
                alter_id_range: AlterIdRange::new(200, 440).unwrap(),
                vouchers: Vec::new(),
                encoded_bytes: 120,
            }),
        ],
    );
    let ScanResult::Complete(complete) = complete else {
        panic!("contiguous paired segments should assemble")
    };
    assert_eq!(complete.vouchers().len(), 1);
    assert_eq!(complete.encoded_bytes(), 220);

    let overlap = assemble_scan(
        company.clone(),
        requested.clone(),
        high_water(440),
        vec![
            SegmentVerification::Complete(CompleteSegment {
                reporting_window: requested.clone(),
                alter_id_range: AlterIdRange::new(0, 250).unwrap(),
                vouchers: Vec::new(),
                encoded_bytes: 1,
            }),
            SegmentVerification::Complete(CompleteSegment {
                reporting_window: requested.clone(),
                alter_id_range: AlterIdRange::new(200, 440).unwrap(),
                vouchers: Vec::new(),
                encoded_bytes: 1,
            }),
        ],
    );
    assert!(matches!(overlap, ScanResult::Partial(_)));

    let gap = assemble_scan(
        company,
        requested.clone(),
        high_water(440),
        vec![
            SegmentVerification::Complete(CompleteSegment {
                reporting_window: requested.clone(),
                alter_id_range: AlterIdRange::new(0, 200).unwrap(),
                vouchers: Vec::new(),
                encoded_bytes: 1,
            }),
            SegmentVerification::Complete(CompleteSegment {
                reporting_window: requested,
                alter_id_range: AlterIdRange::new(250, 440).unwrap(),
                vouchers: Vec::new(),
                encoded_bytes: 1,
            }),
        ],
    );
    assert!(matches!(gap, ScanResult::Partial(_)));
}

#[test]
fn partitioned_scan_requires_corroborated_partitions_and_exact_tiling() {
    let company = company();
    let reporting = DateWindow::parse(
        crate::outstandings::DateBoundaryProfile::ModeAgnostic,
        "20250401",
        "20250701",
    )
    .unwrap();
    let windows = reporting.narrow_partitions().unwrap();
    assert_eq!(windows.len(), 3);
    let extent = book_extent(&company, &reporting, high_water(440));
    let parsed = parse_segment(
        &vouchers(),
        &company,
        &reporting_window(),
        AlterIdRange::new(0, 440).unwrap(),
    )
    .unwrap();
    let mut first_voucher = parsed.vouchers[0].clone();
    first_voucher.date = windows[0].from().clone();
    let mut middle_voucher = parsed.vouchers[0].clone();
    middle_voucher.guid = "company-guid-middle".to_string();
    middle_voucher.alter_id = crate::outstandings::VoucherAlterId::parse("441").unwrap();
    middle_voucher.date = windows[1].from().clone();
    let mut last_voucher = parsed.vouchers[1].clone();
    last_voucher.guid = "company-guid-last".to_string();
    last_voucher.alter_id = crate::outstandings::VoucherAlterId::parse("442").unwrap();
    last_voucher.date = windows[2].from().clone();
    let partitions = windows
        .iter()
        .enumerate()
        .map(|(index, window)| {
            let vouchers = match index {
                0 => vec![first_voucher.clone()],
                1 => vec![middle_voucher.clone()],
                _ => vec![last_voucher.clone()],
            };
            let ScanResult::Complete(scan) = assemble_scan(
                company.clone(),
                window.as_date_window().clone(),
                high_water(440),
                vec![SegmentVerification::Complete(CompleteSegment {
                    reporting_window: window.as_date_window().clone(),
                    alter_id_range: AlterIdRange::new(0, 440).unwrap(),
                    vouchers,
                    encoded_bytes: 10,
                })],
            ) else {
                panic!("narrow partition should assemble")
            };
            CorroboratedDatePartition::non_empty(scan)
                .expect("non-empty primary partition is admissible")
        })
        .collect::<Vec<_>>();

    let complete = assemble_partitioned_scan(&extent, reporting.clone(), partitions.clone());
    let ScanResult::Complete(complete) = complete else {
        panic!("exact date partitions should assemble")
    };
    assert_eq!(complete.window(), &reporting);
    assert_eq!(complete.vouchers().len(), 3);
    assert_eq!(complete.encoded_bytes(), 30);

    let wrong_extent_window = DateWindow::parse(
        crate::outstandings::DateBoundaryProfile::ModeAgnostic,
        "20250402",
        "20250701",
    )
    .unwrap();
    let wrong_extent = book_extent(&company, &wrong_extent_window, high_water(440));
    assert!(matches!(
        assemble_partitioned_scan(&wrong_extent, reporting.clone(), partitions.clone()),
        ScanResult::Partial(partial)
            if partial.reason_code == "reporting_window_extent_mismatch"
    ));

    let missing = assemble_partitioned_scan(&extent, reporting, vec![partitions[0].clone()]);
    assert!(matches!(missing, ScanResult::Partial(_)));
}

#[test]
fn all_empty_primary_partitions_have_no_control() {
    let company = company();
    let reporting = DateWindow::parse(
        crate::outstandings::DateBoundaryProfile::ModeAgnostic,
        "20250401",
        "20250701",
    )
    .unwrap();
    let partitions = reporting
        .narrow_partitions()
        .unwrap()
        .into_iter()
        .map(|window| {
            let ScanResult::Complete(scan) = assemble_scan(
                company.clone(),
                window.as_date_window().clone(),
                high_water(440),
                vec![SegmentVerification::Complete(CompleteSegment {
                    reporting_window: window.into_date_window(),
                    alter_id_range: AlterIdRange::new(0, 440).unwrap(),
                    vouchers: Vec::new(),
                    encoded_bytes: 10,
                })],
            ) else {
                panic!("a live empty partition remains complete on the date axis")
            };
            scan
        })
        .collect::<Vec<_>>();

    assert!(nearest_non_empty_primary_partition(&partitions, &reporting).is_none());
    let primary = NarrowDateWindow::try_from(partitions[0].window().clone()).unwrap();
    let cover = StrictlyWiderDateCover::for_primary(&primary).unwrap();
    let control_pair = CompleteWitnessPair {
        window: primary.as_date_window().clone(),
        vouchers: Vec::new(),
    };
    let cover_pairs = cover
        .slices()
        .iter()
        .map(|slice| CompleteWitnessPair {
            window: slice.as_date_window().clone(),
            vouchers: Vec::new(),
        })
        .collect();
    assert!(matches!(
        corroborate_empty_date_partition(
            partitions[0].clone(),
            &partitions,
            cover,
            control_pair,
            cover_pairs,
        ),
        Err(partial) if partial.reason_code == "empty_date_partition_no_control"
    ));
}

#[test]
fn mandatory_empty_witness_records_nearest_control_provenance() {
    let company = company();
    let control_window = DateWindow::parse(
        crate::outstandings::DateBoundaryProfile::ModeAgnostic,
        "20250401",
        "20250401",
    )
    .unwrap();
    let empty_window = DateWindow::parse(
        crate::outstandings::DateBoundaryProfile::ModeAgnostic,
        "20250402",
        "20250430",
    )
    .unwrap();
    let mut control_voucher = parse_segment(
        &vouchers(),
        &company,
        &reporting_window(),
        AlterIdRange::new(0, 440).unwrap(),
    )
    .unwrap()
    .vouchers
    .remove(0);
    control_voucher.date = control_window.from().clone();
    let ScanResult::Complete(control) = assemble_scan(
        company.clone(),
        control_window.clone(),
        high_water(440),
        vec![SegmentVerification::Complete(CompleteSegment {
            reporting_window: control_window.clone(),
            alter_id_range: AlterIdRange::new(0, 440).unwrap(),
            vouchers: vec![control_voucher.clone()],
            encoded_bytes: 1,
        })],
    ) else {
        panic!("non-empty primary scan must assemble")
    };
    let ScanResult::Complete(empty) = assemble_scan(
        company.clone(),
        empty_window.clone(),
        high_water(440),
        vec![SegmentVerification::Complete(CompleteSegment {
            reporting_window: empty_window.clone(),
            alter_id_range: AlterIdRange::new(0, 440).unwrap(),
            vouchers: Vec::new(),
            encoded_bytes: 1,
        })],
    ) else {
        panic!("empty primary scan remains complete on the AlterID axis")
    };
    let farther = CompleteScan {
        company: company.clone(),
        reporting_window: DateWindow::parse(
            crate::outstandings::DateBoundaryProfile::ModeAgnostic,
            "20250501",
            "20250501",
        )
        .unwrap(),
        voucher_alter_id_high_water: high_water(440),
        vouchers: vec![control_voucher.clone()],
        encoded_bytes: 1,
        empty_partition_witnesses: Vec::new(),
    };
    assert_eq!(
        nearest_non_empty_primary_partition(&[farther, control.clone()], empty.window())
            .expect("at least one primary partition is non-empty")
            .window(),
        &control_window
    );
    let primary = NarrowDateWindow::try_from(empty_window).unwrap();
    let cover = StrictlyWiderDateCover::for_primary(&primary).unwrap();
    let expected_row = voucher_identity(&control_voucher);
    let control_pair = CompleteWitnessPair {
        window: control_window.clone(),
        vouchers: vec![expected_row.clone()],
    };
    let cover_pairs = cover
        .slices()
        .iter()
        .map(|slice| CompleteWitnessPair {
            window: slice.as_date_window().clone(),
            vouchers: Vec::new(),
        })
        .collect();
    let corroborated =
        corroborate_empty_date_partition(empty, &[control], cover, control_pair, cover_pairs)
            .expect("nearest paired control plus shifted cover corroborates emptiness");
    let provenance = corroborated
        .empty_witness()
        .expect("empty variant preserves witness provenance")
        .control_provenance();
    assert_eq!(provenance.control_window(), &control_window);
    assert_eq!(provenance.expected_row(), &expected_row);
    assert!(!provenance.vouched_cover_slices().is_empty());
}

#[test]
fn final_scan_retains_empty_partition_control_provenance() {
    let company = company();
    let reporting = DateWindow::parse(
        crate::outstandings::DateBoundaryProfile::ModeAgnostic,
        "20250401",
        "20250502",
    )
    .unwrap();
    let windows = reporting.narrow_partitions().unwrap();
    assert_eq!(windows.len(), 2);
    let mut control_voucher = parse_segment(
        &vouchers(),
        &company,
        &reporting_window(),
        AlterIdRange::new(0, 440).unwrap(),
    )
    .unwrap()
    .vouchers
    .remove(0);
    control_voucher.date = windows[0].from().clone();
    let control = CompleteScan {
        company: company.clone(),
        reporting_window: windows[0].as_date_window().clone(),
        voucher_alter_id_high_water: high_water(440),
        vouchers: vec![control_voucher.clone()],
        encoded_bytes: 1,
        empty_partition_witnesses: Vec::new(),
    };
    let empty = CompleteScan {
        company: company.clone(),
        reporting_window: windows[1].as_date_window().clone(),
        voucher_alter_id_high_water: high_water(440),
        vouchers: Vec::new(),
        encoded_bytes: 1,
        empty_partition_witnesses: Vec::new(),
    };
    let cover = StrictlyWiderDateCover::for_primary(&windows[1]).unwrap();
    let expected_row = voucher_identity(&control_voucher);
    let corroborated = corroborate_empty_date_partition(
        empty,
        std::slice::from_ref(&control),
        cover.clone(),
        CompleteWitnessPair {
            window: control.window().clone(),
            vouchers: vec![expected_row.clone()],
        },
        cover
            .slices()
            .iter()
            .map(|slice| CompleteWitnessPair {
                window: slice.as_date_window().clone(),
                vouchers: Vec::new(),
            })
            .collect(),
    )
    .unwrap();
    let extent = book_extent(&company, &reporting, high_water(440));
    let ScanResult::Complete(complete) = assemble_partitioned_scan(
        &extent,
        reporting,
        vec![
            CorroboratedDatePartition::non_empty(control).unwrap(),
            corroborated,
        ],
    ) else {
        panic!("a control and its corroborated empty partition must complete")
    };
    let [witness] = complete.empty_partition_witnesses() else {
        panic!("completed scan must retain the empty partition witness")
    };
    assert_eq!(witness.control_provenance().expected_row(), &expected_row);
    assert_eq!(
        witness.control_provenance().control_window(),
        windows[0].as_date_window()
    );
}

#[test]
fn witness_row_inside_primary_partition_is_typed_partial() {
    let company = company();
    let control_window = DateWindow::parse(
        crate::outstandings::DateBoundaryProfile::ModeAgnostic,
        "20250401",
        "20250401",
    )
    .unwrap();
    let empty_window = DateWindow::parse(
        crate::outstandings::DateBoundaryProfile::ModeAgnostic,
        "20250402",
        "20250430",
    )
    .unwrap();
    let mut control_voucher = parse_segment(
        &vouchers(),
        &company,
        &reporting_window(),
        AlterIdRange::new(0, 440).unwrap(),
    )
    .unwrap()
    .vouchers
    .remove(0);
    control_voucher.date = control_window.from().clone();
    let control = CompleteScan {
        company: company.clone(),
        reporting_window: control_window.clone(),
        voucher_alter_id_high_water: high_water(440),
        vouchers: vec![control_voucher.clone()],
        encoded_bytes: 1,
        empty_partition_witnesses: Vec::new(),
    };
    let empty = CompleteScan {
        company,
        reporting_window: empty_window.clone(),
        voucher_alter_id_high_water: high_water(440),
        vouchers: Vec::new(),
        encoded_bytes: 1,
        empty_partition_witnesses: Vec::new(),
    };
    let primary = NarrowDateWindow::try_from(empty_window.clone()).unwrap();
    let cover = StrictlyWiderDateCover::for_primary(&primary).unwrap();
    let control_pair = CompleteWitnessPair {
        window: control_window,
        vouchers: vec![voucher_identity(&control_voucher)],
    };
    let mut cover_pairs = cover
        .slices()
        .iter()
        .map(|slice| CompleteWitnessPair {
            window: slice.as_date_window().clone(),
            vouchers: Vec::new(),
        })
        .collect::<Vec<_>>();
    cover_pairs[0].vouchers.push(WitnessVoucher {
        guid: control_voucher.guid,
        alter_id: control_voucher.alter_id,
        date: empty_window.from().clone(),
    });
    assert!(matches!(
        corroborate_empty_date_partition(empty, &[control], cover, control_pair, cover_pairs),
        Err(partial) if partial.reason_code == "empty_date_window_contradicted_by_wider_read"
    ));
}

#[test]
fn zero_high_water_all_empty_book_is_complete() {
    let company = company();
    let reporting = DateWindow::parse(
        crate::outstandings::DateBoundaryProfile::ModeAgnostic,
        "20250401",
        "20250701",
    )
    .unwrap();
    let extent = book_extent(&company, &reporting, high_water(0));
    let partitions = reporting
        .narrow_partitions()
        .unwrap()
        .into_iter()
        .map(|window| {
            let ScanResult::Complete(scan) = assemble_scan(
                company.clone(),
                window.into_date_window(),
                high_water(0),
                Vec::new(),
            ) else {
                panic!("zero high-water partition should be complete")
            };
            CorroboratedDatePartition::empty_book(scan)
                .expect("zero high-water is the distinct empty-book case")
        })
        .collect::<Vec<_>>();

    assert!(matches!(
        assemble_partitioned_scan(&extent, reporting, partitions),
        ScanResult::Complete(scan) if scan.vouchers().is_empty()
    ));
}
