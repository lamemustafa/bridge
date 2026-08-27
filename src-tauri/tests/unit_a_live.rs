//! Manual, owner-authorized live-capture verification for the legacy
//! voucher-scan outstandings path. Every test here exercises `outstandings`
//! scan machinery (the wildcard voucher request, segment-pair verification,
//! or the calibration harness itself), so the whole file is gated behind
//! `voucher-scan` -- with the feature off, `bridge_tally_protocol::outstandings`
//! does not exist and there is nothing left here to run.
#![cfg(feature = "voucher-scan")]

#[cfg(feature = "live-calibration-harness")]
use bridge_lib::commands::VerifiedCompanyIdentity;
#[cfg(feature = "live-calibration-harness")]
use bridge_lib::tally::{
    OutstandingsAgeingAnchor, OutstandingsCurrencyAssertion, OutstandingsLoadResult, TallyConfig,
    TallyRuntime,
};
#[cfg(feature = "live-calibration-harness")]
use bridge_tally_core::TallyDate;
use bridge_tally_protocol::outstandings::{
    parse_company_book_extent, verify_segment_pair, verify_segment_pair_with_wire_evidence,
    AlterIdRange, DateBoundaryProfile, DateWindow, SegmentVerification, SegmentWireEvidence,
};
use bridge_tally_protocol::xml_read_profiles::{ReadOnlyProfile, ValidatedCompanyName};
use bridge_tally_transport::{TallyEndpointConfig, TallyHttpTransport};
use std::{collections::BTreeMap, time::Instant};

const EXIT_COMPANY: &str = "Bridge Billwise Lab";
const EXIT_COMPANY_GUID: &str = "75f7566d-7a4f-431a-9642-e93a9d06d57d";
#[cfg(feature = "live-calibration-harness")]
const EXIT_AS_OF: &str = "20260731";

#[cfg(feature = "live-calibration-harness")]
#[tokio::test]
#[ignore = "manual owner-authorized guard only; health-check and run one port at a time"]
async fn unit_a_outstandings_live_exit_check_withholds_without_residual_coverage() {
    let port = std::env::var("BRIDGE_TALLY_LIVE_PORT")
        .expect("BRIDGE_TALLY_LIVE_PORT")
        .parse::<u16>()
        .expect("numeric port");
    let company = std::env::var("BRIDGE_TALLY_LIVE_COMPANY").expect("company name");
    let company_guid = std::env::var("BRIDGE_TALLY_LIVE_COMPANY_GUID").expect("company GUID");
    validate_exit_target(port, &company, &company_guid)
        .expect("port and company match the accepted reconciliation target");
    let transport = TallyHttpTransport::new(TallyEndpointConfig {
        host: "127.0.0.1".to_string(),
        port,
    })
    .expect("bounded Tally transport");
    transport
        .get_status_decoded()
        .await
        .expect("pre-withholding Tally status");
    let result = TallyRuntime::for_billwise_lab_reconciliation_exit_check()
        .fetch_outstandings(
            TallyConfig {
                host: "127.0.0.1".to_string(),
                port,
            },
            &VerifiedCompanyIdentity::live_calibration_harness_identity(company, company_guid),
            TallyDate::parse(EXIT_AS_OF).expect("fixed reconciliation as-of date is valid"),
            OutstandingsCurrencyAssertion::Inr,
            OutstandingsAgeingAnchor::DueDate,
        )
        .await
        .expect("live outstandings request completes");
    transport
        .get_status_decoded()
        .await
        .expect("post-withholding Tally status");
    // This guard does not reconcile accounting figures. Its companion runtime
    // regression uses an invalid endpoint to prove this partial is returned
    // before endpoint admission or a voucher request starts.
    match result {
        OutstandingsLoadResult::Partial { reason, .. } => {
            assert_eq!(
                reason.reason_code, "unallocated_direct_postings_not_covered",
                "a voucher-only scan cannot prove direct bill-wise postings on either port"
            );
            println!(
                "UNIT_A_LIVE_WITHHELD port={port} reason={} mode=preflight_no_voucher_scan",
                reason.reason_code,
            );
        }
        OutstandingsLoadResult::Complete { .. } => panic!(
            "the exit harness must not emit totals until residual coverage is independently qualified"
        ),
    }
}

fn validate_exit_target(port: u16, company: &str, company_guid: &str) -> Result<(), &'static str> {
    if !matches!(port, 9000 | 9001) {
        return Err("reconciliation is authorized only on ports 9000 and 9001");
    }
    if company != EXIT_COMPANY || !company_guid.eq_ignore_ascii_case(EXIT_COMPANY_GUID) {
        return Err("company is not the accepted bill-bearing reconciliation corpus");
    }
    Ok(())
}

#[test]
fn unit_a_exit_preflight_is_bound_to_the_accepted_ports_company_and_guid() {
    assert!(validate_exit_target(9000, EXIT_COMPANY, EXIT_COMPANY_GUID).is_ok());
    assert!(validate_exit_target(9001, EXIT_COMPANY, EXIT_COMPANY_GUID).is_ok());
    assert!(validate_exit_target(9002, EXIT_COMPANY, EXIT_COMPANY_GUID).is_err());
    assert!(validate_exit_target(9000, "Aarav Trading Company Demo", EXIT_COMPANY_GUID,).is_err());
    assert!(validate_exit_target(9000, EXIT_COMPANY, "wrong-guid").is_err());
}

#[tokio::test]
#[ignore = "protocol/completeness probe only; Aarav must never calibrate segment sizing"]
async fn unit_a_outstandings_live_bounded_window() {
    let port = std::env::var("BRIDGE_TALLY_LIVE_PORT")
        .expect("BRIDGE_TALLY_LIVE_PORT")
        .parse::<u16>()
        .expect("numeric port");
    let company = std::env::var("BRIDGE_TALLY_LIVE_COMPANY").expect("company name");
    let company_guid = std::env::var("BRIDGE_TALLY_LIVE_COMPANY_GUID").expect("company GUID");
    let transport = TallyHttpTransport::new(TallyEndpointConfig {
        host: "127.0.0.1".to_string(),
        port,
    })
    .expect("bounded Tally transport");
    transport
        .get_status_decoded()
        .await
        .expect("pre-read Tally status");

    let validated_company = ValidatedCompanyName::new(company.clone()).expect("company name");
    let extent_response = transport
        .post_xml_decoded(
            ReadOnlyProfile::CompanyBookExtentV1 {
                company: &validated_company,
            }
            .render(),
        )
        .await
        .expect("company extent response");
    retain_live_text(
        "captures",
        &format!("unit-a-bounded-company-extent-port{port}.xml"),
        extent_response.text(),
    );
    let extent = parse_company_book_extent(&extent_response.into_text(), &company, &company_guid)
        .expect("verified company extent");
    let reporting_window = DateWindow::parse(
        DateBoundaryProfile::EducationRestricted,
        "20240401",
        "20240401",
    )
    .expect("verified probe boundary");
    let window = reporting_window
        .narrow_partitions()
        .expect("one-day probe has one narrow partition")
        .remove(0);
    let high_water = extent
        .voucher_alter_id_high_water()
        .expect("company extent includes ALTVCHID");
    let alter_id_range = AlterIdRange::new(0, high_water.get().min(400))
        .expect("known protocol probe AlterID range is non-empty");
    let request = ReadOnlyProfile::VoucherOutstandingsV1 {
        company: extent.company(),
        window: &window,
        alter_id_range,
    }
    .render();
    assert!(
        request.contains("<FILTERS>BridgeOutstandingsPartitionV1</FILTERS>"),
        "refuse the live request before dispatch if the bounding filter is removed"
    );
    assert!(
        request.contains("$Date &gt;= ##SVFromDate AND $Date &lt;= ##SVToDate"),
        "refuse the live request before dispatch if the date predicate is removed"
    );
    assert!(
        request.contains(&format!(
            "$AlterID &gt; {} AND $AlterID &lt;= {}",
            alter_id_range.exclusive_start(),
            alter_id_range.inclusive_end()
        )),
        "refuse the live request before dispatch if the AlterID predicate is removed"
    );
    retain_live_text(
        "requests",
        &format!("unit-a-bounded-wildcard-port{port}.xml"),
        &request,
    );

    let first_started = Instant::now();
    let first = transport
        .post_xml_decoded(request.clone())
        .await
        .expect("first bounded wildcard read");
    let first_elapsed = first_started.elapsed();
    retain_live_text(
        "captures",
        &format!("unit-a-bounded-wildcard-first-port{port}.xml"),
        first.text(),
    );
    let second_started = Instant::now();
    let second = transport
        .post_xml_decoded(request)
        .await
        .expect("second bounded wildcard read");
    let second_elapsed = second_started.elapsed();
    retain_live_text(
        "captures",
        &format!("unit-a-bounded-wildcard-second-port{port}.xml"),
        second.text(),
    );
    let verification = verify_segment_pair_with_wire_evidence(
        SegmentWireEvidence::new(first.text(), first.encoded_bytes(), first.encoded_sha256()),
        SegmentWireEvidence::new(
            second.text(),
            second.encoded_bytes(),
            second.encoded_sha256(),
        ),
        extent.company(),
        window.as_date_window().clone(),
        alter_id_range,
    )
    .expect("bounded pair parses");
    let SegmentVerification::Complete(segment) = verification else {
        panic!("bounded wildcard pair was not complete")
    };
    assert!(
        !segment.vouchers().is_empty(),
        "the known protocol-only Aarav segment unexpectedly returned no vouchers"
    );
    assert!(
        segment
            .vouchers()
            .iter()
            .all(|voucher| voucher.date.as_str() == "20240401"),
        "a voucher outside the requested one-day window crossed the live filter"
    );
    assert!(
        segment
            .vouchers()
            .iter()
            .all(|voucher| alter_id_range.contains(voucher.alter_id)),
        "a voucher outside the requested AlterID range crossed the live filter"
    );

    let mut bill_types = BTreeMap::<_, usize>::new();
    for allocation in segment
        .vouchers()
        .iter()
        .flat_map(|voucher| &voucher.ledger_entries)
        .flat_map(|entry| &entry.bill_allocations)
    {
        *bill_types.entry(allocation.bill_type).or_default() += 1;
    }
    transport
        .get_status_decoded()
        .await
        .expect("post-read Tally status");
    println!(
        "UNIT_A_PROTOCOL_ONLY_COMPLETE port={port} vouchers={} bytes={} first_ms={} second_ms={} alter_id_range={}..{} bill_types={bill_types:?} sizing_calibration=forbidden",
        segment.vouchers().len(),
        segment.encoded_bytes(),
        first_elapsed.as_millis(),
        second_elapsed.as_millis(),
        alter_id_range.exclusive_start(),
        alter_id_range.inclusive_end(),
    );
}

fn retain_live_text(directory: &str, file_name: &str, text: &str) {
    let directory = std::path::Path::new("../.bridge-live").join(directory);
    std::fs::create_dir_all(&directory).expect("live evidence directory");
    std::fs::write(directory.join(file_name), text).expect("retain live evidence bytes");
}

#[test]
#[ignore = "requires ignored real captures from the local live-evidence directory"]
fn unit_a_historical_month_capture_parses() {
    let extent_xml =
        std::fs::read_to_string("../.bridge-live/captures/unit-a-company-extent-v1-port9000.xml")
            .expect("company extent capture");
    let extent = parse_company_book_extent(
        &extent_xml,
        "Aarav Trading Company Demo",
        "bb8ad19e-6aef-4239-a917-87fec0c6215e",
    )
    .expect("verified company extent");
    let month = std::fs::read_to_string(
        "../.bridge-live/captures/unit-a-outstandings-v1-sample-month-port9000.xml",
    )
    .expect("historical month capture");
    let result = verify_segment_pair(
        &month,
        &month,
        extent.company(),
        DateWindow::parse(
            DateBoundaryProfile::EducationRestricted,
            "20240401",
            "20240501",
        )
        .unwrap(),
        AlterIdRange::new(0, u64::MAX).unwrap(),
    )
    .expect("typed segment verification");
    let segment = match result {
        SegmentVerification::Complete(segment) => segment,
        SegmentVerification::Partial(partial) => {
            panic!(
                "historical month capture remained partial: {}",
                partial.reason_code
            )
        }
    };
    assert_eq!(segment.vouchers().len(), 4_896);
    assert_eq!(
        segment
            .vouchers()
            .iter()
            .flat_map(|voucher| &voucher.ledger_entries)
            .map(|entry| entry.bill_allocations.len())
            .sum::<usize>(),
        4_887
    );
}
