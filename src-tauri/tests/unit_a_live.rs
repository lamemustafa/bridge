use bridge_lib::tally::{OutstandingsLoadResult, TallyConfig, TallyRuntime};
use bridge_tally_protocol::outstandings::{
    parse_company_book_extent, verify_segment_pair, verify_segment_pair_with_wire_evidence,
    AlterIdRange, DateBoundaryProfile, DateWindow, SegmentVerification, SegmentWireEvidence,
};
use bridge_tally_protocol::xml_read_profiles::{ReadOnlyProfile, ValidatedCompanyName};
use bridge_tally_transport::{TallyEndpointConfig, TallyHttpTransport};
use std::{collections::BTreeMap, time::Instant};

#[tokio::test]
#[ignore = "blocked until the ordered bill-bearing company exists and its owner-reviewed sizing is encoded"]
async fn unit_a_outstandings_live_exit_check() {
    let port = std::env::var("BRIDGE_TALLY_LIVE_PORT")
        .expect("BRIDGE_TALLY_LIVE_PORT")
        .parse::<u16>()
        .expect("numeric port");
    let company = std::env::var("BRIDGE_TALLY_LIVE_COMPANY").expect("company name");
    let company_guid = std::env::var("BRIDGE_TALLY_LIVE_COMPANY_GUID").expect("company GUID");
    assert_ne!(
        company, "Aarav Trading Company Demo",
        "Unit A's reconciliation exit check is reserved for the purpose-built bill-bearing corpus from TEST_CORPUS.md section 4"
    );
    let result = TallyRuntime::default()
        .fetch_outstandings(
            TallyConfig {
                host: "127.0.0.1".to_string(),
                port,
            },
            company,
            company_guid,
        )
        .await
        .expect("live outstandings request completes");
    match result {
        OutstandingsLoadResult::Complete { report, .. } => {
            assert!(
                (200..=500).contains(&report.source_voucher_count),
                "the exit corpus must retain its reviewed 200-500 voucher bound"
            );
            assert!(
                !report.top_parties.is_empty(),
                "the bill-bearing exit corpus produced no reconcilable party balances"
            );
            assert!(
                !report.receivable_total.is_zero() || !report.payable_total.is_zero(),
                "the bill-bearing exit corpus produced zero outstandings"
            );
            assert!(
                !report.ageing.days_0_30.is_zero()
                    && !report.ageing.days_31_60.is_zero()
                    && !report.ageing.days_61_90.is_zero()
                    && !report.ageing.days_90_plus.is_zero(),
                "the purpose-built corpus must remain reconcilable across all four ageing buckets"
            );
            println!(
                "UNIT_A_LIVE_COMPLETE port={port} vouchers={} bytes={} receivable={} payable={} as_of={}",
                report.source_voucher_count,
                report.source_bytes,
                report.receivable_total.as_str(),
                report.payable_total.as_str(),
                report.as_of_yyyymmdd,
            );
        }
        OutstandingsLoadResult::Partial { reason_code, .. } => {
            panic!("live outstandings remained partial: {reason_code}")
        }
    }
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

    let mut bill_types = BTreeMap::<String, usize>::new();
    for allocation in segment
        .vouchers()
        .iter()
        .flat_map(|voucher| &voucher.ledger_entries)
        .flat_map(|entry| &entry.bill_allocations)
    {
        *bill_types.entry(allocation.bill_type.clone()).or_default() += 1;
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
        SegmentVerification::Empty(_) => {
            panic!("historical month capture unexpectedly contained no vouchers")
        }
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
