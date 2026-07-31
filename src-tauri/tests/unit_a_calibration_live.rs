use bridge_tally_protocol::outstandings::{
    parse_company_book_extent, verify_segment_pair_with_wire_evidence, AlterIdRange,
    DateBoundaryProfile, DateWindow, NarrowDateWindow, SegmentVerification, SegmentWireEvidence,
};
use bridge_tally_protocol::xml_read_profiles::{ReadOnlyProfile, ValidatedCompanyName};
use bridge_tally_transport::{TallyEndpointConfig, TallyHttpTransport};
use std::path::{Path, PathBuf};
use std::time::Instant;

const AARAV_COMPANY: &str = "Aarav Trading Company Demo";
const ACCEPTED_CALIBRATION_PORT: u16 = 9000;
const ACCEPTED_CALIBRATION_COMPANY: &str = "Bridge Billwise Lab";
const ACCEPTED_CALIBRATION_COMPANY_GUID: &str = "75f7566d-7a4f-431a-9642-e93a9d06d57d";
const MIN_ORDERED_CORPUS_HIGH_WATER: u64 = 200;
const MAX_ORDERED_CORPUS_HIGH_WATER: u64 = 600;

/// One evidence sample only. Run this ignored test manually once per sample,
/// with a fresh sample ID and a health check on each side. It never retries,
/// loops, derives a width, or mutates the production calibration policy.
#[tokio::test]
#[ignore = "manual owner-authorized calibration only; one paired sample per invocation"]
async fn unit_a_ordered_corpus_calibration_sample() {
    let port = env_u16("BRIDGE_TALLY_LIVE_PORT");
    let sample_id = env_u8("BRIDGE_TALLY_CALIBRATION_SAMPLE_ID");
    assert!(
        (1..=20).contains(&sample_id),
        "calibration sample ID must be between 1 and 20"
    );
    let company = std::env::var("BRIDGE_TALLY_LIVE_COMPANY").expect("company name");
    let company_guid = std::env::var("BRIDGE_TALLY_LIVE_COMPANY_GUID").expect("company GUID");
    validate_calibration_target(port, &company, &company_guid)
        .expect("port and company are authorized for calibration");
    let reporting_window = DateWindow::parse(
        DateBoundaryProfile::EducationRestricted,
        std::env::var("BRIDGE_TALLY_CALIBRATION_FROM").expect("calibration from date"),
        std::env::var("BRIDGE_TALLY_CALIBRATION_TO").expect("calibration to date"),
    )
    .expect("calibration dates use verified Tally boundaries");
    let segment_window = calibration_window(&reporting_window)
        .expect("one calibration sample must use exactly one narrow date window");

    let evidence = EvidencePaths::reserve(
        port,
        sample_id,
        segment_window.from().as_str(),
        segment_window.to().as_str(),
    );
    let transport = TallyHttpTransport::new(TallyEndpointConfig {
        host: "127.0.0.1".to_string(),
        port,
    })
    .expect("loopback calibration transport");
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
    write_new(&evidence.extent, extent_response.text());
    let extent = parse_company_book_extent(&extent_response.into_text(), &company, &company_guid)
        .expect("verified company extent");
    let high_water = extent
        .voucher_alter_id_high_water()
        .expect("ordered corpus extent includes ALTVCHID");
    validate_calibration_high_water(high_water.get())
        .expect("ordered calibration corpus remains inside the reviewed small-corpus bound");
    let alter_id_range =
        AlterIdRange::new(0, high_water.get()).expect("ordered corpus AlterID span is non-empty");
    let request = bridge_tally_protocol::outstandings::voucher_outstandings_request(
        extent.company(),
        &segment_window,
        alter_id_range,
    );
    write_new(&evidence.request, &request.clone().into_xml());

    let first_started = Instant::now();
    let first = transport
        .post_outstandings_xml_decoded(request.clone())
        .await
        .expect("first calibration wildcard read");
    let first_elapsed = first_started.elapsed();
    write_new(&evidence.first, first.text());
    transport
        .get_status_decoded()
        .await
        .expect("between-pair Tally status");

    let second_started = Instant::now();
    let second = transport
        .post_outstandings_xml_decoded(request)
        .await
        .expect("second calibration wildcard read");
    let second_elapsed = second_started.elapsed();
    write_new(&evidence.second, second.text());

    let verification = verify_segment_pair_with_wire_evidence(
        SegmentWireEvidence::new(first.text(), first.encoded_bytes(), first.encoded_sha256()),
        SegmentWireEvidence::new(
            second.text(),
            second.encoded_bytes(),
            second.encoded_sha256(),
        ),
        extent.company(),
        reporting_window,
        alter_id_range,
    )
    .expect("calibration pair parses");
    let SegmentVerification::Complete(segment) = verification else {
        panic!("calibration pair was empty, partial, or mismatched")
    };
    assert!(
        !segment.vouchers().is_empty(),
        "calibration window must contain ordered-corpus vouchers"
    );

    transport
        .get_status_decoded()
        .await
        .expect("post-read Tally status");
    println!(
        "UNIT_A_CALIBRATION_SAMPLE sample={} port={} date={}..{} alter_id=0..{} rows={} bytes={} first_ms={} second_ms={} policy_changed=false",
        sample_id,
        port,
        segment_window.from().as_str(),
        segment_window.to().as_str(),
        high_water.get(),
        segment.vouchers().len(),
        segment.encoded_bytes(),
        first_elapsed.as_millis(),
        second_elapsed.as_millis(),
    );
}

struct EvidencePaths {
    extent: PathBuf,
    request: PathBuf,
    first: PathBuf,
    second: PathBuf,
}

impl EvidencePaths {
    fn reserve(port: u16, sample_id: u8, from: &str, to: &str) -> Self {
        let directory = Path::new("../.bridge-live/calibration");
        std::fs::create_dir_all(directory).expect("calibration evidence directory");
        let prefix = format!("unit-a-calibration-s{sample_id}-port{port}-{from}-{to}");
        let reservation = directory.join(format!("{prefix}-reserved.txt"));
        let paths = Self {
            extent: directory.join(format!("{prefix}-extent.xml")),
            request: directory.join(format!("{prefix}-request.xml")),
            first: directory.join(format!("{prefix}-first.xml")),
            second: directory.join(format!("{prefix}-second.xml")),
        };
        for path in [
            &reservation,
            &paths.extent,
            &paths.request,
            &paths.first,
            &paths.second,
        ] {
            assert!(
                !path.exists(),
                "refusing to overwrite retained calibration evidence: {}",
                path.display()
            );
        }
        write_new(
            &reservation,
            "reserved before Tally contact; never reuse this sample identity\n",
        );
        paths
    }
}

fn write_new(path: &Path, text: &str) {
    use std::io::Write as _;
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .expect("create calibration evidence file without overwrite");
    file.write_all(text.as_bytes())
        .expect("retain exact calibration response bytes");
}

fn env_u16(name: &str) -> u16 {
    std::env::var(name)
        .unwrap_or_else(|_| panic!("{name}"))
        .parse::<u16>()
        .unwrap_or_else(|_| panic!("{name} must be numeric"))
}

fn env_u8(name: &str) -> u8 {
    std::env::var(name)
        .unwrap_or_else(|_| panic!("{name}"))
        .parse::<u8>()
        .unwrap_or_else(|_| panic!("{name} must be numeric"))
}

fn validate_calibration_target(
    port: u16,
    company: &str,
    company_guid: &str,
) -> Result<(), &'static str> {
    if company == AARAV_COMPANY {
        return Err("Aarav is invalid for segment-size calibration");
    }
    if port != ACCEPTED_CALIBRATION_PORT {
        return Err("calibration is authorized only on port 9000");
    }
    if company != ACCEPTED_CALIBRATION_COMPANY
        || !company_guid.eq_ignore_ascii_case(ACCEPTED_CALIBRATION_COMPANY_GUID)
    {
        return Err("company is not the accepted ordered calibration corpus");
    }
    Ok(())
}

fn validate_calibration_high_water(high_water: u64) -> Result<(), &'static str> {
    if (MIN_ORDERED_CORPUS_HIGH_WATER..=MAX_ORDERED_CORPUS_HIGH_WATER).contains(&high_water) {
        Ok(())
    } else {
        Err("calibration corpus is outside the reviewed small-corpus bound")
    }
}

fn calibration_window(reporting: &DateWindow) -> Result<NarrowDateWindow, &'static str> {
    let mut partitions = reporting
        .narrow_partitions()
        .map_err(|_| "calibration period cannot be partitioned")?;
    if partitions.len() != 1 {
        return Err("calibration period is broader than one narrow date window");
    }
    let segment = partitions.remove(0);
    if segment.as_date_window() != reporting {
        return Err("calibration period was silently narrowed");
    }
    Ok(segment)
}

#[test]
fn calibration_preflight_rejects_aarav_broad_dates_and_out_of_scope_books() {
    assert!(validate_calibration_target(
        ACCEPTED_CALIBRATION_PORT,
        AARAV_COMPANY,
        ACCEPTED_CALIBRATION_COMPANY_GUID,
    )
    .is_err());
    assert!(validate_calibration_target(
        9001,
        ACCEPTED_CALIBRATION_COMPANY,
        ACCEPTED_CALIBRATION_COMPANY_GUID,
    )
    .is_err());
    assert!(validate_calibration_target(
        ACCEPTED_CALIBRATION_PORT,
        ACCEPTED_CALIBRATION_COMPANY,
        "00000000-0000-4000-8000-000000000001",
    )
    .is_err());
    assert!(validate_calibration_target(
        ACCEPTED_CALIBRATION_PORT,
        ACCEPTED_CALIBRATION_COMPANY,
        ACCEPTED_CALIBRATION_COMPANY_GUID,
    )
    .is_ok());
    assert!(validate_calibration_high_water(199).is_err());
    assert!(validate_calibration_high_water(200).is_ok());
    assert!(validate_calibration_high_water(600).is_ok());
    assert!(validate_calibration_high_water(601).is_err());

    let narrow = DateWindow::parse(
        DateBoundaryProfile::EducationRestricted,
        "20250401",
        "20250501",
    )
    .unwrap();
    assert!(calibration_window(&narrow).is_ok());
    let broad = DateWindow::parse(
        DateBoundaryProfile::EducationRestricted,
        "20250401",
        "20250601",
    )
    .unwrap();
    assert!(calibration_window(&broad).is_err());
}
