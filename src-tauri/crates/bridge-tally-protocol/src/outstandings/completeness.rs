use sha2::{Digest, Sha256};
use std::collections::BTreeMap;

use super::{
    parser::parse_segment, AlterIdRange, BillReferenceKind, CompanyBookExtent, CompleteScan,
    CompleteSegment, DateWindow, EmptyDateWindowVerification, EmptyDateWindowWitness, MoneyValue,
    OutstandingsError, PartialScan, PinnedCompany, ScanResult, SegmentVerification, Voucher,
    VoucherAlterIdHighWater,
};

#[derive(Debug, Clone, Copy)]
pub struct SegmentWireEvidence<'a> {
    xml: &'a str,
    encoded_bytes: usize,
    encoded_sha256: &'a str,
}

impl<'a> SegmentWireEvidence<'a> {
    pub fn new(xml: &'a str, encoded_bytes: usize, encoded_sha256: &'a str) -> Self {
        Self {
            xml,
            encoded_bytes,
            encoded_sha256,
        }
    }
}

pub fn verify_segment_pair(
    first_xml: &str,
    second_xml: &str,
    company: &PinnedCompany,
    reporting_window: DateWindow,
    alter_id_range: AlterIdRange,
) -> Result<SegmentVerification, OutstandingsError> {
    verify_segment_pair_with_encoded_bytes(
        first_xml,
        first_xml.len(),
        second_xml,
        second_xml.len(),
        company,
        reporting_window,
        alter_id_range,
    )
}

pub fn verify_segment_pair_with_encoded_bytes(
    first_xml: &str,
    first_encoded_bytes: usize,
    second_xml: &str,
    second_encoded_bytes: usize,
    company: &PinnedCompany,
    reporting_window: DateWindow,
    alter_id_range: AlterIdRange,
) -> Result<SegmentVerification, OutstandingsError> {
    let first_encoded_sha256 = sha256_hex(first_xml.as_bytes());
    let second_encoded_sha256 = sha256_hex(second_xml.as_bytes());
    verify_segment_pair_with_wire_evidence(
        SegmentWireEvidence::new(first_xml, first_encoded_bytes, &first_encoded_sha256),
        SegmentWireEvidence::new(second_xml, second_encoded_bytes, &second_encoded_sha256),
        company,
        reporting_window,
        alter_id_range,
    )
}

pub fn verify_segment_pair_with_wire_evidence(
    first_wire: SegmentWireEvidence<'_>,
    second_wire: SegmentWireEvidence<'_>,
    company: &PinnedCompany,
    reporting_window: DateWindow,
    alter_id_range: AlterIdRange,
) -> Result<SegmentVerification, OutstandingsError> {
    let first = match parse_segment(first_wire.xml, company, &reporting_window, alter_id_range) {
        Ok(value) => value,
        Err(error) => {
            return Ok(SegmentVerification::Partial(PartialScan::new(error_code(
                &error,
            ))))
        }
    };
    let second = match parse_segment(second_wire.xml, company, &reporting_window, alter_id_range) {
        Ok(value) => value,
        Err(error) => {
            return Ok(SegmentVerification::Partial(PartialScan::new(error_code(
                &error,
            ))))
        }
    };
    if !paired_rows_match(
        &first.vouchers,
        first.raw_row_count,
        first_wire,
        &second.vouchers,
        second.raw_row_count,
        second_wire,
    ) {
        return Ok(SegmentVerification::Partial(PartialScan::new(
            "paired_segment_mismatch",
        )));
    }
    Ok(SegmentVerification::Complete(CompleteSegment {
        reporting_window,
        alter_id_range,
        vouchers: first.vouchers,
        encoded_bytes: first_wire.encoded_bytes,
    }))
}

pub fn verify_empty_date_window_with_wider_pair(
    empty_partition: &CompleteScan,
    first_xml: &str,
    second_xml: &str,
    wider_window: DateWindow,
    alter_id_range: AlterIdRange,
) -> Result<EmptyDateWindowVerification, OutstandingsError> {
    verify_empty_date_window_with_wider_pair_and_encoded_bytes(
        empty_partition,
        first_xml,
        first_xml.len(),
        second_xml,
        second_xml.len(),
        wider_window,
        alter_id_range,
    )
}

pub fn verify_empty_date_window_with_wider_pair_and_encoded_bytes(
    empty_partition: &CompleteScan,
    first_xml: &str,
    first_encoded_bytes: usize,
    second_xml: &str,
    second_encoded_bytes: usize,
    wider_window: DateWindow,
    alter_id_range: AlterIdRange,
) -> Result<EmptyDateWindowVerification, OutstandingsError> {
    let first_encoded_sha256 = sha256_hex(first_xml.as_bytes());
    let second_encoded_sha256 = sha256_hex(second_xml.as_bytes());
    verify_empty_date_window_with_wider_pair_and_wire_evidence(
        empty_partition,
        SegmentWireEvidence::new(first_xml, first_encoded_bytes, &first_encoded_sha256),
        SegmentWireEvidence::new(second_xml, second_encoded_bytes, &second_encoded_sha256),
        wider_window,
        alter_id_range,
    )
}

pub fn verify_empty_date_window_with_wider_pair_and_wire_evidence(
    empty_partition: &CompleteScan,
    first_wire: SegmentWireEvidence<'_>,
    second_wire: SegmentWireEvidence<'_>,
    wider_window: DateWindow,
    alter_id_range: AlterIdRange,
) -> Result<EmptyDateWindowVerification, OutstandingsError> {
    if !empty_partition.vouchers.is_empty() {
        return Ok(EmptyDateWindowVerification::Partial(PartialScan::new(
            "empty_date_witness_partition_not_empty",
        )));
    }
    let empty_window = &empty_partition.reporting_window;
    if wider_window.from() > empty_window.from()
        || wider_window.to() < empty_window.to()
        || wider_window == *empty_window
    {
        return Ok(EmptyDateWindowVerification::Partial(PartialScan::new(
            "empty_date_witness_not_strictly_wider",
        )));
    }
    let segment = match verify_segment_pair_with_wire_evidence(
        first_wire,
        second_wire,
        &empty_partition.company,
        wider_window.clone(),
        alter_id_range,
    )? {
        SegmentVerification::Complete(segment) => segment,
        SegmentVerification::Partial(partial) => {
            return Ok(EmptyDateWindowVerification::Partial(partial))
        }
    };
    if segment.vouchers.is_empty() {
        return Ok(EmptyDateWindowVerification::Partial(PartialScan::new(
            "empty_date_witness_wider_window_empty",
        )));
    }
    if segment
        .vouchers
        .iter()
        .any(|voucher| voucher.date >= *empty_window.from() && voucher.date <= *empty_window.to())
    {
        return Ok(EmptyDateWindowVerification::Partial(PartialScan::new(
            "empty_date_window_contradicted_by_wider_read",
        )));
    }
    Ok(EmptyDateWindowVerification::Complete(
        EmptyDateWindowWitness {
            empty_window: empty_window.clone(),
            wider_window,
            observed_row_count: segment.vouchers.len(),
        },
    ))
}

fn paired_rows_match(
    first: &[Voucher],
    first_raw_row_count: usize,
    first_wire: SegmentWireEvidence<'_>,
    second: &[Voucher],
    second_raw_row_count: usize,
    second_wire: SegmentWireEvidence<'_>,
) -> bool {
    first_raw_row_count == first.len()
        && second_raw_row_count == second.len()
        && first_wire.encoded_bytes == second_wire.encoded_bytes
        && first_wire.encoded_sha256 == second_wire.encoded_sha256
        && first == second
}

fn sha256_hex(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    digest.iter().map(|byte| format!("{byte:02x}")).collect()
}

pub fn assemble_scan(
    company: PinnedCompany,
    reporting_window: DateWindow,
    high_water: VoucherAlterIdHighWater,
    segments: Vec<SegmentVerification>,
) -> ScanResult {
    let mut complete = Vec::with_capacity(segments.len());
    for segment in segments {
        match segment {
            SegmentVerification::Complete(segment) => complete.push(segment),
            SegmentVerification::Partial(partial) => return ScanResult::Partial(partial),
        }
    }
    complete.sort_by_key(|segment| segment.alter_id_range.exclusive_start());
    if high_water.get() == 0 && complete.is_empty() {
        return ScanResult::Complete(CompleteScan {
            company,
            reporting_window,
            voucher_alter_id_high_water: high_water,
            vouchers: Vec::new(),
            encoded_bytes: 0,
        });
    }
    let Some(first) = complete.first() else {
        return ScanResult::Partial(PartialScan::new("scan_has_no_segments"));
    };
    let Some(last) = complete.last() else {
        unreachable!()
    };
    if first.alter_id_range.exclusive_start() != 0
        || last.alter_id_range.inclusive_end() != high_water.get()
    {
        return ScanResult::Partial(PartialScan::new("segment_coverage_incomplete"));
    }
    for pair in complete.windows(2) {
        if !pair[0]
            .alter_id_range
            .is_adjacent_to(pair[1].alter_id_range)
        {
            return ScanResult::Partial(PartialScan::new("segment_coverage_not_contiguous"));
        }
    }
    if complete
        .iter()
        .any(|segment| segment.reporting_window != reporting_window)
    {
        return ScanResult::Partial(PartialScan::new("segment_reporting_window_mismatch"));
    }
    let mut vouchers = BTreeMap::<String, Voucher>::new();
    let mut alter_ids = BTreeMap::new();
    let mut encoded_bytes = 0_usize;
    for segment in complete {
        let Some(next_encoded_bytes) = encoded_bytes.checked_add(segment.encoded_bytes) else {
            return ScanResult::Partial(PartialScan::new("scan_encoded_bytes_overflow"));
        };
        encoded_bytes = next_encoded_bytes;
        for voucher in segment.vouchers {
            if alter_ids.insert(voucher.alter_id, ()).is_some() {
                return ScanResult::Partial(PartialScan::new(
                    "duplicate_voucher_alter_id_across_segments",
                ));
            }
            if vouchers.insert(voucher.guid.clone(), voucher).is_some() {
                return ScanResult::Partial(PartialScan::new("duplicate_voucher_across_segments"));
            }
        }
    }
    // Issue #113: no captured response contains a posted non-zero Advance
    // whose present BILLDATE differs from its voucher date. Those competing
    // dates are unverified, so return the scan's typed Partial instead of
    // letting computation publish a confidently aged amount. The guard
    // requires owner-attended evidence to remove; documentation or inference
    // is not enough.
    if vouchers.values().any(has_unverified_advance_ageing) {
        return ScanResult::Partial(PartialScan::new("advance_ageing_unverified"));
    }
    ScanResult::Complete(CompleteScan {
        company,
        reporting_window,
        voucher_alter_id_high_water: high_water,
        vouchers: vouchers.into_values().collect(),
        encoded_bytes,
    })
}

fn has_unverified_advance_ageing(voucher: &Voucher) -> bool {
    !voucher.cancelled
        && !voucher.deleted
        && !voucher.optional
        && voucher.ledger_entries.iter().any(|entry| {
            entry.bill_allocations.iter().any(|allocation| {
                matches!(allocation.bill_type, BillReferenceKind::Advance)
                    && matches!(&allocation.amount, MoneyValue::Exact(amount) if !amount.is_zero())
                    && matches!(
                        allocation.bill_date.as_ref(),
                        Some(bill_date) if bill_date != &voucher.date
                    )
            })
        })
}

/// Merge individually complete narrow-date scans into the extent's full-book scan.
/// Each narrow scan has already proven exact `0..ALTVCHID` coverage; this
/// boundary additionally proves that the date partitions are exactly the
/// deterministic, contiguous partition of `[BooksFrom, LastVoucherDate]`.
pub fn assemble_partitioned_scan(
    extent: &CompanyBookExtent,
    reporting_window: DateWindow,
    mut partitions: Vec<CompleteScan>,
) -> ScanResult {
    // The window ends at the as-of cutoff, which is `LastVoucherDate` unless the
    // book contains future-dated vouchers. Requiring exact equality would reject
    // every clamped scan with a misleading "extent mismatch", so require the
    // window to start at BooksFrom and to end no later than LastVoucherDate.
    // Ending EARLIER is the deliberate exclusion of future activity; ending
    // later would mean the tiling ran past the book and is still rejected.
    if reporting_window.from() != extent.books_from()
        || reporting_window.to() > extent.last_voucher_date()
    {
        return ScanResult::Partial(PartialScan::new("reporting_window_extent_mismatch"));
    }
    let Some(high_water) = extent.voucher_alter_id_high_water() else {
        return ScanResult::Partial(PartialScan::new(
            "company_voucher_alter_id_high_water_missing",
        ));
    };
    let company = extent.company();
    let expected = match reporting_window.narrow_partitions() {
        Ok(value) => value,
        Err(_) => return ScanResult::Partial(PartialScan::new("date_partition_invalid")),
    };
    if partitions.len() != expected.len() {
        return ScanResult::Partial(PartialScan::new("date_partition_coverage_incomplete"));
    }
    partitions.sort_by(|left, right| {
        left.reporting_window
            .from()
            .cmp(right.reporting_window.from())
            .then_with(|| left.reporting_window.to().cmp(right.reporting_window.to()))
    });

    let mut vouchers = BTreeMap::<String, Voucher>::new();
    let mut alter_ids = BTreeMap::new();
    let mut encoded_bytes = 0_usize;
    for (partition, expected_window) in partitions.into_iter().zip(expected) {
        if partition.company != *company
            || partition.voucher_alter_id_high_water != high_water
            || partition.reporting_window != *expected_window.as_date_window()
        {
            return ScanResult::Partial(PartialScan::new("date_partition_scope_mismatch"));
        }
        let Some(next_encoded_bytes) = encoded_bytes.checked_add(partition.encoded_bytes) else {
            return ScanResult::Partial(PartialScan::new("scan_encoded_bytes_overflow"));
        };
        encoded_bytes = next_encoded_bytes;
        for voucher in partition.vouchers {
            if alter_ids.insert(voucher.alter_id, ()).is_some() {
                return ScanResult::Partial(PartialScan::new(
                    "duplicate_voucher_alter_id_across_date_partitions",
                ));
            }
            if vouchers.insert(voucher.guid.clone(), voucher).is_some() {
                return ScanResult::Partial(PartialScan::new(
                    "duplicate_voucher_across_date_partitions",
                ));
            }
        }
    }

    if high_water.get() > 0 && vouchers.is_empty() {
        return ScanResult::Partial(PartialScan::new("whole_book_false_empty"));
    }

    ScanResult::Complete(CompleteScan {
        company: company.clone(),
        reporting_window,
        voucher_alter_id_high_water: high_water,
        vouchers: vouchers.into_values().collect(),
        encoded_bytes,
    })
}

fn error_code(error: &OutstandingsError) -> &'static str {
    match error {
        OutstandingsError::CompanyIdentityMismatch => "company_identity_mismatch",
        OutstandingsError::InvalidAmount => "invalid_amount",
        OutstandingsError::InvalidResponse(code) => code,
        _ => "segment_parse_failed",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::outstandings::parse_company_book_extent;

    const COMPANY_EXTENT: &str =
        include_str!("../../tests/fixtures/unit_a_company_extent_live.xml");
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
    fn partitioned_scan_accepts_interior_empty_windows_and_requires_exact_tiling() {
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
        let first_voucher = parsed.vouchers[0].clone();
        let mut last_voucher = parsed.vouchers[1].clone();
        last_voucher.date = bridge_tally_primitives::TallyDate::parse("20250701").unwrap();
        let partitions = windows
            .iter()
            .enumerate()
            .map(|(index, window)| {
                let vouchers = match index {
                    0 => vec![first_voucher.clone()],
                    2 => vec![last_voucher.clone()],
                    _ => Vec::new(),
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
                scan
            })
            .collect::<Vec<_>>();

        let complete = assemble_partitioned_scan(&extent, reporting.clone(), partitions.clone());
        let ScanResult::Complete(complete) = complete else {
            panic!("exact date partitions should assemble")
        };
        assert_eq!(complete.window(), &reporting);
        assert_eq!(complete.vouchers().len(), 2);
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
    fn whole_book_false_empty_fails_closed_when_extent_has_vouchers() {
        let company = company();
        let reporting = DateWindow::parse(
            crate::outstandings::DateBoundaryProfile::ModeAgnostic,
            "20250401",
            "20250701",
        )
        .unwrap();
        let extent = book_extent(&company, &reporting, high_water(440));
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
            .collect();

        assert!(matches!(
            assemble_partitioned_scan(&extent, reporting, partitions),
            ScanResult::Partial(partial) if partial.reason_code == "whole_book_false_empty"
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
                scan
            })
            .collect();

        assert!(matches!(
            assemble_partitioned_scan(&extent, reporting, partitions),
            ScanResult::Complete(scan) if scan.vouchers().is_empty()
        ));
    }
}
