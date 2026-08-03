//! Offline acceptance verifier for a live Tally test-corpus capture.
//!
//! It deliberately performs no HTTP. The operator wrapper obtains read-only
//! responses; this binary evaluates them exclusively through production
//! outstandings parsing and computation code.

use std::{collections::BTreeMap, fs, process::ExitCode};

use bridge_tally_primitives::TallyDate;
use bridge_tally_protocol::outstandings::{
    assemble_scan, compute_outstandings, parse_company_book_extent, verify_segment_pair,
    AlterIdRange, BillReferenceKind, DateBoundaryProfile, DateWindow, ScanResult,
    SegmentVerification,
};

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("tally_test_corpus_rejected:{error}");
            ExitCode::FAILURE
        }
    }
}

fn run() -> Result<(), String> {
    let arguments = Arguments::parse()?;
    let extent_xml = read_capture(&arguments.extent_xml)?;
    let voucher_xml = read_capture(&arguments.voucher_xml)?;
    let extent = parse_company_book_extent(&extent_xml, &arguments.company, &arguments.guid)
        .map_err(|error| format!("company_extent_parse_failed:{error}"))?;
    let high_water = extent
        .voucher_alter_id_high_water()
        .ok_or("company_voucher_alter_id_high_water_missing")?;
    let window = DateWindow::parse(
        DateBoundaryProfile::ModeAgnostic,
        arguments.from,
        arguments.to,
    )
    .map_err(|error| format!("reporting_window_invalid:{error}"))?;
    let range = AlterIdRange::new(0, high_water.get())
        .map_err(|error| format!("alter_id_range_invalid:{error}"))?;
    let segment = verify_segment_pair(
        &voucher_xml,
        &voucher_xml,
        extent.company(),
        window.clone(),
        range,
    )
    .map_err(|error| format!("voucher_parse_failed:{error}"))?;
    let SegmentVerification::Complete(segment) = segment else {
        return Err("voucher_pair_not_complete".to_string());
    };
    let ScanResult::Complete(scan) = assemble_scan(
        extent.company().clone(),
        window,
        high_water,
        vec![SegmentVerification::Complete(segment)],
    ) else {
        return Err("voucher_scan_not_complete".to_string());
    };
    let as_of = TallyDate::parse(arguments.as_of)
        .map_err(|_| "reconciliation_as_of_invalid".to_string())?;
    let report = compute_outstandings(&scan, as_of)
        .map_err(|error| format!("outstandings_compute_failed:{error}"))?;

    let posting = scan
        .vouchers()
        .iter()
        .filter(|voucher| !voucher.optional && !voucher.cancelled && !voucher.deleted)
        .collect::<Vec<_>>();
    if !(200..=500).contains(&posting.len()) {
        return Err(format!(
            "posting_voucher_count_outside_calibration_range:{}",
            posting.len()
        ));
    }
    let mut months = BTreeMap::<&str, Vec<u64>>::new();
    let mut named_new_ref = 0usize;
    let mut named_agst_ref = 0usize;
    for voucher in &posting {
        if !matches!(&voucher.date.as_str()[6..8], "01" | "02" | "31") {
            return Err(format!(
                "education_date_boundary_invalid:{}",
                voucher.date.as_str()
            ));
        }
        months
            .entry(&voucher.date.as_str()[..6])
            .or_default()
            .push(voucher.alter_id.get());
        for allocation in voucher
            .ledger_entries
            .iter()
            .flat_map(|entry| &entry.bill_allocations)
        {
            if allocation
                .name
                .as_deref()
                .is_some_and(|name| !name.is_empty())
            {
                if allocation.bill_type == BillReferenceKind::NewRef {
                    named_new_ref += 1;
                }
                if allocation.bill_type == BillReferenceKind::AgstRef {
                    named_agst_ref += 1;
                }
            }
        }
    }
    if named_new_ref == 0 || named_agst_ref == 0 {
        return Err(format!(
            "named_bill_reference_kinds_missing:new_ref={named_new_ref}:agst_ref={named_agst_ref}"
        ));
    }
    let min = posting
        .iter()
        .map(|voucher| voucher.alter_id.get())
        .min()
        .unwrap_or(0);
    let max = posting
        .iter()
        .map(|voucher| voucher.alter_id.get())
        .max()
        .unwrap_or(0);
    let total_span = max
        .checked_sub(min)
        .and_then(|value| value.checked_add(1))
        .ok_or("alter_id_span_invalid")?;
    let worst_percent = months
        .values()
        .map(|ids| {
            let month_min = ids.iter().copied().min().unwrap_or(0);
            let month_max = ids.iter().copied().max().unwrap_or(0);
            100_u64.saturating_mul(month_max - month_min + 1) / total_span.max(1)
        })
        .max()
        .unwrap_or(0);
    if worst_percent > 40 {
        return Err(format!(
            "alter_id_date_locality_exceeds_40_percent:{worst_percent}"
        ));
    }
    let counts = &report.ageing_bill_counts;
    if [
        counts.days_0_30,
        counts.days_31_60,
        counts.days_61_90,
        counts.days_90_plus,
    ]
    .into_iter()
    .any(|count| count == 0)
    {
        return Err("ageing_bucket_coverage_incomplete".to_string());
    }

    println!("tally_test_corpus_accepted:posting_vouchers={}:worst_month_span_percent={}:open_receivable_bills={}", posting.len(), worst_percent, report.open_receivable_bill_count);
    Ok(())
}

fn read_capture(path: &str) -> Result<String, String> {
    let bytes = fs::read(path).map_err(|_| format!("capture_unavailable:{path}"))?;
    String::from_utf8(bytes).map_err(|_| format!("capture_not_utf8:{path}"))
}

struct Arguments {
    company: String,
    guid: String,
    from: String,
    to: String,
    as_of: String,
    extent_xml: String,
    voucher_xml: String,
}

impl Arguments {
    fn parse() -> Result<Self, String> {
        let mut values = std::env::args().skip(1);
        let mut next = || {
            values.next().ok_or("usage: bridge-tally-test-corpus-verifier --company NAME --guid GUID --from YYYYMMDD --to YYYYMMDD --as-of YYYYMMDD --extent-xml PATH --voucher-xml PATH")
        };
        let company = match next()?.as_str() {
            "--company" => next()?,
            _ => return Err("company_argument_required".to_string()),
        };
        let guid = match next()?.as_str() {
            "--guid" => next()?,
            _ => return Err("guid_argument_required".to_string()),
        };
        let from = match next()?.as_str() {
            "--from" => next()?,
            _ => return Err("from_argument_required".to_string()),
        };
        let to = match next()?.as_str() {
            "--to" => next()?,
            _ => return Err("to_argument_required".to_string()),
        };
        let as_of = match next()?.as_str() {
            "--as-of" => next()?,
            _ => return Err("as_of_argument_required".to_string()),
        };
        let extent_xml = match next()?.as_str() {
            "--extent-xml" => next()?,
            _ => return Err("extent_xml_argument_required".to_string()),
        };
        let voucher_xml = match next()?.as_str() {
            "--voucher-xml" => next()?,
            _ => return Err("voucher_xml_argument_required".to_string()),
        };
        if values.next().is_some() {
            return Err("unexpected_arguments".to_string());
        }
        Ok(Self {
            company,
            guid,
            from,
            to,
            as_of,
            extent_xml,
            voucher_xml,
        })
    }
}
