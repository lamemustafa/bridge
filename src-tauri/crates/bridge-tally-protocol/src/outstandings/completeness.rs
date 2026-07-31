use sha2::{Digest, Sha256};
use std::collections::BTreeMap;

use super::{
    parser::parse_segment, AlterIdRange, CompleteScan, CompleteSegment, CorroboratedEmptySegments,
    DateWindow, EmptySegmentCandidate, EmptySegmentCorroboration, OutstandingsError, PartialScan,
    PinnedCompany, ScanResult, SegmentVerification, Voucher, VoucherAlterIdHighWater,
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
    if first.vouchers.is_empty() {
        return Ok(SegmentVerification::Empty(EmptySegmentCandidate {
            reporting_window,
            alter_id_range,
            encoded_bytes: first_wire.encoded_bytes,
        }));
    }
    Ok(SegmentVerification::Complete(CompleteSegment {
        reporting_window,
        alter_id_range,
        vouchers: first.vouchers,
        encoded_bytes: first_wire.encoded_bytes,
    }))
}

pub fn corroborate_empty_segment_with_adjacent_pair(
    candidate: EmptySegmentCandidate,
    first_xml: &str,
    second_xml: &str,
    company: &PinnedCompany,
    adjacent: AlterIdRange,
) -> Result<EmptySegmentCorroboration, OutstandingsError> {
    corroborate_empty_segment_with_adjacent_pair_and_encoded_bytes(
        candidate,
        first_xml,
        first_xml.len(),
        second_xml,
        second_xml.len(),
        company,
        adjacent,
    )
}

pub fn corroborate_empty_segment_with_adjacent_pair_and_encoded_bytes(
    candidate: EmptySegmentCandidate,
    first_xml: &str,
    first_encoded_bytes: usize,
    second_xml: &str,
    second_encoded_bytes: usize,
    company: &PinnedCompany,
    adjacent: AlterIdRange,
) -> Result<EmptySegmentCorroboration, OutstandingsError> {
    let first_encoded_sha256 = sha256_hex(first_xml.as_bytes());
    let second_encoded_sha256 = sha256_hex(second_xml.as_bytes());
    corroborate_empty_segment_with_adjacent_pair_and_wire_evidence(
        candidate,
        SegmentWireEvidence::new(first_xml, first_encoded_bytes, &first_encoded_sha256),
        SegmentWireEvidence::new(second_xml, second_encoded_bytes, &second_encoded_sha256),
        company,
        adjacent,
    )
}

pub fn corroborate_empty_segment_with_adjacent_pair_and_wire_evidence(
    candidate: EmptySegmentCandidate,
    first_wire: SegmentWireEvidence<'_>,
    second_wire: SegmentWireEvidence<'_>,
    company: &PinnedCompany,
    adjacent: AlterIdRange,
) -> Result<EmptySegmentCorroboration, OutstandingsError> {
    let wider = candidate.alter_id_range.joined_with(adjacent)?;
    let first = match parse_segment(first_wire.xml, company, &candidate.reporting_window, wider) {
        Ok(value) => value,
        Err(error) => {
            return Ok(EmptySegmentCorroboration::Partial(PartialScan::new(
                error_code(&error),
            )))
        }
    };
    let second = match parse_segment(second_wire.xml, company, &candidate.reporting_window, wider) {
        Ok(value) => value,
        Err(error) => {
            return Ok(EmptySegmentCorroboration::Partial(PartialScan::new(
                error_code(&error),
            )))
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
        return Ok(EmptySegmentCorroboration::Partial(PartialScan::new(
            "empty_corroboration_pair_mismatch",
        )));
    }
    if first.vouchers.is_empty() {
        return Ok(EmptySegmentCorroboration::Partial(PartialScan::new(
            "empty_corroboration_wider_window_empty",
        )));
    }
    if first
        .vouchers
        .iter()
        .any(|voucher| candidate.alter_id_range.contains(voucher.alter_id))
    {
        return Ok(EmptySegmentCorroboration::Partial(PartialScan::new(
            "empty_segment_contradicted_by_wider_read",
        )));
    }
    if first
        .vouchers
        .iter()
        .any(|voucher| !adjacent.contains(voucher.alter_id))
    {
        return Ok(EmptySegmentCorroboration::Partial(PartialScan::new(
            "empty_corroboration_scope_ambiguous",
        )));
    }
    Ok(EmptySegmentCorroboration::Complete(
        CorroboratedEmptySegments {
            empty: CompleteSegment {
                reporting_window: candidate.reporting_window.clone(),
                alter_id_range: candidate.alter_id_range,
                vouchers: Vec::new(),
                encoded_bytes: candidate.encoded_bytes,
            },
            adjacent: CompleteSegment {
                reporting_window: candidate.reporting_window,
                alter_id_range: adjacent,
                vouchers: first.vouchers,
                encoded_bytes: first_wire.encoded_bytes,
            },
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
            SegmentVerification::Empty(_) => {
                return ScanResult::Partial(PartialScan::new("uncorroborated_empty_segment"))
            }
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
    ScanResult::Complete(CompleteScan {
        company,
        reporting_window,
        voucher_alter_id_high_water: high_water,
        vouchers: vouchers.into_values().collect(),
        encoded_bytes,
    })
}

/// Merge individually complete narrow-date scans into one report-period scan.
/// Each narrow scan has already proven exact `0..ALTVCHID` coverage; this
/// boundary additionally proves that the date partitions are exactly the
/// deterministic, contiguous partition of the requested reporting period.
pub fn assemble_partitioned_scan(
    company: PinnedCompany,
    reporting_window: DateWindow,
    high_water: VoucherAlterIdHighWater,
    mut partitions: Vec<CompleteScan>,
) -> ScanResult {
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
        if partition.company != company
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
        company,
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
    const VOUCHERS: &str = include_str!("../../tests/fixtures/unit_a_vouchers_wildcard_live.xml");

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

    #[test]
    fn empty_and_truncated_pairs_never_construct_complete_segments() {
        let company = company();
        let empty = only_vouchers_with_alter_ids(VOUCHERS, &[]);
        assert!(matches!(
            verify_segment_pair(
                &empty,
                &empty,
                &company,
                reporting_window(),
                AlterIdRange::new(148, 200).unwrap(),
            )
            .unwrap(),
            SegmentVerification::Empty(_)
        ));
        let truncated = &VOUCHERS[..VOUCHERS.len() - "</ENVELOPE>\n".len()];
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
        let second = VOUCHERS.replacen("<GROUP>0</GROUP>", "<GROUP>1</GROUP>", 1);
        assert_eq!(VOUCHERS.len(), second.len());
        assert!(matches!(
            verify_segment_pair(
                VOUCHERS,
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
    fn an_adjacent_wider_pair_is_the_only_path_from_empty_candidate_to_complete() {
        let company = company();
        let exact_empty = only_vouchers_with_alter_ids(VOUCHERS, &[]);
        let candidate = match verify_segment_pair(
            &exact_empty,
            &exact_empty,
            &company,
            reporting_window(),
            AlterIdRange::new(148, 200).unwrap(),
        )
        .unwrap()
        {
            SegmentVerification::Empty(candidate) => candidate,
            other => panic!("expected an empty candidate, got {other:?}"),
        };
        let wider = only_vouchers_with_alter_ids(VOUCHERS, &[440]);
        let corroboration = corroborate_empty_segment_with_adjacent_pair(
            candidate,
            &wider,
            &wider,
            &company,
            AlterIdRange::new(200, 440).unwrap(),
        )
        .unwrap();
        let EmptySegmentCorroboration::Complete(corroborated) = corroboration else {
            panic!("adjacent real rows should corroborate the empty boundary")
        };
        let [empty, adjacent] = corroborated.into_segments();
        assert!(empty.vouchers().is_empty());
        assert_eq!(empty.alter_id_range(), AlterIdRange::new(148, 200).unwrap());
        assert_eq!(
            adjacent.alter_id_range(),
            AlterIdRange::new(200, 440).unwrap()
        );
        assert_eq!(adjacent.vouchers().len(), 1);
        assert_eq!(adjacent.vouchers()[0].alter_id.get(), 440);
    }

    #[test]
    fn a_wider_pair_that_finds_the_target_range_contradicts_the_empty_candidate() {
        let company = company();
        let exact_empty = only_vouchers_with_alter_ids(VOUCHERS, &[]);
        let SegmentVerification::Empty(candidate) = verify_segment_pair(
            &exact_empty,
            &exact_empty,
            &company,
            reporting_window(),
            AlterIdRange::new(148, 200).unwrap(),
        )
        .unwrap() else {
            panic!("expected an empty candidate")
        };
        let contradicting = only_vouchers_with_alter_ids(VOUCHERS, &[440]).replacen(
            "<ALTERID TYPE=\"Number\"> 440</ALTERID>",
            "<ALTERID TYPE=\"Number\"> 180</ALTERID>",
            1,
        );
        let result = corroborate_empty_segment_with_adjacent_pair(
            candidate,
            &contradicting,
            &contradicting,
            &company,
            AlterIdRange::new(200, 440).unwrap(),
        )
        .unwrap();
        let EmptySegmentCorroboration::Partial(partial) = result else {
            panic!("a wider read contradicted the claimed empty AlterID range")
        };
        assert_eq!(
            partial.reason_code,
            "empty_segment_contradicted_by_wider_read"
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
            VOUCHERS,
            VOUCHERS,
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
        let duplicate = VOUCHERS.replacen(
            "<ALTERID TYPE=\"Number\"> 77</ALTERID>",
            "<ALTERID TYPE=\"Number\"> 75</ALTERID>",
            1,
        );
        assert_ne!(duplicate, VOUCHERS);
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
        let subset = only_vouchers_with_alter_ids(VOUCHERS, &[75]);
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
            VOUCHERS,
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
    fn partitioned_scan_requires_every_narrow_date_window_exactly_once() {
        let company = company();
        let reporting = DateWindow::parse(
            crate::outstandings::DateBoundaryProfile::ModeAgnostic,
            "20250401",
            "20250601",
        )
        .unwrap();
        let windows = reporting.narrow_partitions().unwrap();
        assert_eq!(windows.len(), 2);
        let partitions = windows
            .iter()
            .map(|window| {
                let ScanResult::Complete(scan) = assemble_scan(
                    company.clone(),
                    window.as_date_window().clone(),
                    high_water(2),
                    vec![SegmentVerification::Complete(CompleteSegment {
                        reporting_window: window.as_date_window().clone(),
                        alter_id_range: AlterIdRange::new(0, 2).unwrap(),
                        vouchers: Vec::new(),
                        encoded_bytes: 10,
                    })],
                ) else {
                    panic!("narrow partition should assemble")
                };
                scan
            })
            .collect::<Vec<_>>();

        let complete = assemble_partitioned_scan(
            company.clone(),
            reporting.clone(),
            high_water(2),
            partitions.clone(),
        );
        let ScanResult::Complete(complete) = complete else {
            panic!("exact date partitions should assemble")
        };
        assert_eq!(complete.window(), &reporting);
        assert_eq!(complete.encoded_bytes(), 20);

        let missing = assemble_partitioned_scan(
            company,
            reporting,
            high_water(2),
            vec![partitions[0].clone()],
        );
        assert!(matches!(missing, ScanResult::Partial(_)));
    }
}
