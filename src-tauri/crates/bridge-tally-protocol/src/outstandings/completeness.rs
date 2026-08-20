use sha2::{Digest, Sha256};
use std::collections::BTreeMap;

use super::{
    parser::{parse_segment, parse_witness_segment},
    AlterIdRange, CompanyBookExtent, CompleteScan, CompleteSegment, CompleteWitnessPair,
    CorroboratedDatePartition, DateWindow, EmptyDateWindowVerification, EmptyDateWindowWitness,
    EmptyPartitionControlProvenance, EmptyPartitionWitness, OutstandingsError, PartialScan,
    PinnedCompany, ScanResult, SegmentVerification, StrictlyWiderDateCover, Voucher,
    VoucherAlterIdHighWater, WitnessPairVerification, WitnessVoucher,
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

/// Verifies the distinct, date-only I5 witness request. Unlike the wildcard
/// outstandings reader it has no AlterID slice, and uses the ordinary response
/// cap in the transport layer. Pairing remains literal and byte-sensitive.
pub fn verify_empty_partition_witness_pair_with_wire_evidence(
    first_wire: SegmentWireEvidence<'_>,
    second_wire: SegmentWireEvidence<'_>,
    company: &PinnedCompany,
    window: DateWindow,
) -> Result<WitnessPairVerification, OutstandingsError> {
    let first = match parse_witness_segment(first_wire.xml, company, &window) {
        Ok(value) => value,
        Err(error) => {
            return Ok(WitnessPairVerification::Partial(PartialScan::new(
                error_code(&error),
            )))
        }
    };
    let second = match parse_witness_segment(second_wire.xml, company, &window) {
        Ok(value) => value,
        Err(error) => {
            return Ok(WitnessPairVerification::Partial(PartialScan::new(
                error_code(&error),
            )))
        }
    };
    if first.raw_row_count != first.vouchers.len()
        || second.raw_row_count != second.vouchers.len()
        || first.raw_row_count != second.raw_row_count
        || first_wire.encoded_bytes != second_wire.encoded_bytes
        || first_wire.encoded_sha256 != second_wire.encoded_sha256
        || first.vouchers != second.vouchers
    {
        return Ok(WitnessPairVerification::Partial(PartialScan::new(
            "paired_empty_date_witness_mismatch",
        )));
    }
    Ok(WitnessPairVerification::Complete(CompleteWitnessPair {
        window,
        vouchers: first.vouchers,
    }))
}

/// Chooses the closest non-empty primary partition. Ties deliberately prefer
/// the earlier partition, which makes the control selection reproducible.
pub fn nearest_non_empty_primary_partition<'a>(
    primary_partitions: &'a [CompleteScan],
    empty_window: &DateWindow,
) -> Option<&'a CompleteScan> {
    primary_partitions
        .iter()
        .filter(|partition| !partition.vouchers.is_empty())
        .min_by(|left, right| {
            calendar_distance(&left.reporting_window, empty_window)
                .cmp(&calendar_distance(&right.reporting_window, empty_window))
                .then_with(|| {
                    left.reporting_window
                        .from()
                        .cmp(right.reporting_window.from())
                })
        })
}

/// Combines already-verified paired witness reads into the only evidence type
/// that allows an empty date partition to enter final assembly.
pub fn corroborate_empty_date_partition(
    empty_partition: CompleteScan,
    primary_partitions: &[CompleteScan],
    cover: StrictlyWiderDateCover,
    control_pair: CompleteWitnessPair,
    cover_pairs: Vec<CompleteWitnessPair>,
) -> Result<CorroboratedDatePartition, PartialScan> {
    if !empty_partition.vouchers.is_empty()
        || empty_partition.reporting_window != *cover.primary().as_date_window()
    {
        return Err(PartialScan::new("empty_date_witness_scope_mismatch"));
    }
    let Some(control_partition) =
        nearest_non_empty_primary_partition(primary_partitions, &empty_partition.reporting_window)
    else {
        return Err(PartialScan::new("empty_date_partition_no_control"));
    };
    if control_pair.window != control_partition.reporting_window {
        return Err(PartialScan::new(
            "empty_date_witness_control_scope_mismatch",
        ));
    }
    let expected_row = control_partition
        .vouchers
        .first()
        .map(voucher_identity)
        .ok_or_else(|| PartialScan::new("empty_date_partition_no_control"))?;
    if !control_pair.vouchers.contains(&expected_row) {
        return Err(PartialScan::new("empty_date_witness_control_missing_row"));
    }
    if cover_pairs.len() != cover.slices().len()
        || cover_pairs
            .iter()
            .zip(cover.slices())
            .any(|(pair, expected)| pair.window != *expected.as_date_window())
    {
        return Err(PartialScan::new("empty_date_witness_cover_scope_mismatch"));
    }
    if cover_pairs
        .iter()
        .flat_map(|pair| pair.vouchers.iter())
        .any(|voucher| {
            voucher.date >= *empty_partition.reporting_window.from()
                && voucher.date <= *empty_partition.reporting_window.to()
        })
    {
        return Err(PartialScan::new(
            "empty_date_window_contradicted_by_wider_read",
        ));
    }
    let witness = EmptyPartitionWitness::new(
        empty_partition.reporting_window.clone(),
        EmptyPartitionControlProvenance::new(
            control_partition.reporting_window.clone(),
            expected_row,
            cover.slices().to_vec(),
        ),
    );
    CorroboratedDatePartition::empty(empty_partition, witness)
}

fn voucher_identity(voucher: &Voucher) -> WitnessVoucher {
    WitnessVoucher {
        guid: voucher.guid.clone(),
        alter_id: voucher.alter_id,
        date: voucher.date.clone(),
    }
}

fn calendar_distance(left: &DateWindow, right: &DateWindow) -> usize {
    let (mut cursor, end) = if left.to() < right.from() {
        (left.to().clone(), right.from())
    } else if right.to() < left.from() {
        (right.to().clone(), left.from())
    } else {
        return 0;
    };
    let mut days = 0usize;
    while cursor < *end {
        let Ok(next) = cursor.next_day() else {
            return usize::MAX;
        };
        cursor = next;
        days = days.saturating_add(1);
    }
    days
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
            empty_partition_witnesses: Vec::new(),
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
    ScanResult::Complete(CompleteScan {
        company,
        reporting_window,
        voucher_alter_id_high_water: high_water,
        vouchers: vouchers.into_values().collect(),
        encoded_bytes,
        empty_partition_witnesses: Vec::new(),
    })
}

/// Merge individually complete narrow-date scans into the extent's full-book scan.
/// Each narrow scan has already proven exact `0..ALTVCHID` coverage; this
/// boundary additionally proves that the date partitions are exactly the
/// deterministic, contiguous partition of `[BooksFrom, LastVoucherDate]`.
pub fn assemble_partitioned_scan(
    extent: &CompanyBookExtent,
    reporting_window: DateWindow,
    mut partitions: Vec<CorroboratedDatePartition>,
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
        left.scan()
            .reporting_window
            .from()
            .cmp(right.scan().reporting_window.from())
            .then_with(|| {
                left.scan()
                    .reporting_window
                    .to()
                    .cmp(right.scan().reporting_window.to())
            })
    });

    let mut vouchers = BTreeMap::<String, Voucher>::new();
    let mut alter_ids = BTreeMap::new();
    let mut encoded_bytes = 0_usize;
    let mut empty_partition_witnesses = Vec::new();
    for (partition, expected_window) in partitions.into_iter().zip(expected) {
        let partition = match partition {
            CorroboratedDatePartition::NonEmpty(scan) => {
                if high_water.get() == 0 {
                    return ScanResult::Partial(PartialScan::new("empty_book_partition_invalid"));
                }
                scan
            }
            CorroboratedDatePartition::Empty { scan, witness } => {
                if high_water.get() == 0 {
                    return ScanResult::Partial(PartialScan::new("empty_book_partition_invalid"));
                }
                empty_partition_witnesses.push(witness);
                scan
            }
            CorroboratedDatePartition::EmptyBook(scan) => {
                if high_water.get() != 0 {
                    return ScanResult::Partial(PartialScan::new("empty_book_partition_invalid"));
                }
                scan
            }
        };
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

    ScanResult::Complete(CompleteScan {
        company: company.clone(),
        reporting_window,
        voucher_alter_id_high_water: high_water,
        vouchers: vouchers.into_values().collect(),
        encoded_bytes,
        empty_partition_witnesses,
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
    use crate::outstandings::{parse_company_book_extent, NarrowDateWindow};

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
}
