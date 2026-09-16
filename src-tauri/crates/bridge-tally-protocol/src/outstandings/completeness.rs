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
#[path = "completeness_tests.rs"]
mod tests;
