use super::*;
use std::collections::BTreeMap;

#[test]
fn scoped_opening_uses_requested_boundary_without_changing_book_start_default() {
    let books = TallyDate::parse("20240401").unwrap();
    let last = TallyDate::parse("20260915").unwrap();
    let requested = TallyDate::parse("20260901").unwrap();
    let scoped = ledger_opening_period(
        DateBoundaryProfile::EducationRestricted,
        &books,
        &last,
        Some(&requested),
    )
    .unwrap();
    let default = ledger_opening_period(
        DateBoundaryProfile::EducationRestricted,
        &books,
        &last,
        None,
    )
    .unwrap();
    let xml = render_native_ledger_export_request("Bridge Synthetic Book", &scoped);
    let reader = quick_xml::Reader::from_str(&xml);
    let mut reader = reader;
    let mut observed_from = None;
    loop {
        match reader.read_event().unwrap() {
            quick_xml::events::Event::Start(tag) if tag.name().as_ref() == b"SVFROMDATE" => {
                observed_from = Some(
                    reader
                        .read_text(tag.name())
                        .unwrap()
                        .decode()
                        .unwrap()
                        .into_owned(),
                );
            }
            quick_xml::events::Event::Eof => break,
            _ => {}
        }
    }
    assert_eq!(observed_from.as_deref(), Some("20260901"));
    assert_eq!(default.from(), &books);
    assert_eq!(default.to(), &last);
    assert_eq!(scoped.to(), &last);
}

#[test]
fn scoped_opening_rejects_unsupported_and_prebook_dates_before_dispatch() {
    let books = TallyDate::parse("20240401").unwrap();
    let last = TallyDate::parse("20260915").unwrap();
    for (requested, expected) in [
        (
            "20260815",
            NativeLedgerExportPeriodError::UnsupportedBoundary,
        ),
        ("20240331", NativeLedgerExportPeriodError::InvalidRange),
    ] {
        assert_eq!(
            ledger_opening_period(
                DateBoundaryProfile::EducationRestricted,
                &books,
                &last,
                Some(&TallyDate::parse(requested).unwrap())
            ),
            Err(expected)
        );
    }
    let after_last = TallyDate::parse("20261001").unwrap();
    let period = ledger_opening_period(
        DateBoundaryProfile::EducationRestricted,
        &books,
        &last,
        Some(&after_last),
    )
    .unwrap();
    assert_eq!(period.from(), &after_last);
    assert_eq!(period.to(), &after_last);
}

#[test]
fn scoped_opening_requires_observed_mode_and_does_not_infer_unknown_as_licensed() {
    use bridge_tally_core::{
        CapabilityEvidence, CapabilityFeatureId, CapabilityProfile, CapabilityState,
        EvidenceConfidence,
    };
    let mut profile = CapabilityProfile {
        profile_version: 2,
        product: "TallyPrime".into(),
        release: None,
        mode: None,
        transports: BTreeMap::new(),
        features: BTreeMap::new(),
        packs: BTreeMap::new(),
    };
    assert_eq!(
        observed_opening_boundary(&profile),
        Err(OpeningBoundaryObservationError::Unobserved)
    );
    profile.mode = Some("Licensed".into());
    assert_eq!(
        observed_opening_boundary(&profile),
        Err(OpeningBoundaryObservationError::Unobserved)
    );
    profile.features.insert(
        CapabilityFeatureId::ProductAndMode,
        CapabilityEvidence {
            state: CapabilityState::Supported,
            confidence: EvidenceConfidence::Observed,
            safe_reason_code: None,
        },
    );
    assert_eq!(
        observed_opening_boundary(&profile),
        Ok(DateBoundaryProfile::ModeAgnostic)
    );
    profile.mode = Some("Education".into());
    let boundary = observed_opening_boundary(&profile).unwrap();
    assert_eq!(boundary, DateBoundaryProfile::EducationRestricted);
    assert_eq!(
        ledger_opening_period(
            boundary,
            &TallyDate::parse("20260401").unwrap(),
            &TallyDate::parse("20260902").unwrap(),
            Some(&TallyDate::parse("20260815").unwrap())
        ),
        Err(NativeLedgerExportPeriodError::UnsupportedBoundary)
    );
    profile.mode = None;
    assert_eq!(
        observed_opening_boundary(&profile),
        Err(OpeningBoundaryObservationError::Unobserved)
    );
}

#[tokio::test]
async fn all_ledger_openings_probe_before_export_even_with_a_stale_licensed_cache() {
    use crate::tally::TallyProduct;
    use bridge_tally_core::CapabilityProfile;
    use tokio::io::AsyncReadExt;
    for (cached, scoped) in [(false, false), (true, false), (false, true), (true, true)] {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let config = TallyConfig {
            host: address.ip().to_string(),
            port: address.port(),
        };
        let runtime = TallyRuntime::default();
        let company = TallyCompany {
            name: "Synthetic Company".into(),
            guid: Some("synthetic-guid".into()),
            company_number: Some("100001".into()),
            books_from: Some("20260401".into()),
        };
        let identity = VerifiedCompanyIdentity::from_observed_companies(
            company.name.clone(),
            company.guid.clone().unwrap(),
            "100001".into(),
            "20260401".into(),
            std::slice::from_ref(&company),
        )
        .unwrap();
        if cached {
            let session = runtime.session(config.clone()).unwrap();
            *session.cached_probe.write().unwrap() = Some(CachedProbe {
                review_id: "stale-licensed".into(),
                observed_at_unix_ms: 1,
                freshness_origin_unix_ms: 1,
                reserved: false,
                result: TallyProbeResult {
                    connection: ConnectionStatus {
                        reachable: true,
                        compatible: true,
                        server_text: String::new(),
                        product: TallyProduct::Unknown,
                        error: None,
                    },
                    companies: vec![company],
                    profile: CapabilityProfile {
                        profile_version: 2,
                        product: "TallyPrime".into(),
                        release: None,
                        mode: Some("Licensed".into()),
                        transports: BTreeMap::new(),
                        features: BTreeMap::new(),
                        packs: BTreeMap::new(),
                    },
                    selected_read_scope: None,
                    passport_snapshot_id: None,
                },
            });
        }
        let read = tokio::spawn(async move {
            runtime
                .fetch_ledger_opening_with_evidence(
                    config,
                    &identity,
                    scoped.then(|| TallyDate::parse("20260815").unwrap()),
                )
                .await
        });
        // Inspect the complete first request, then cancel without fabricating a
        // Tally response. This tests request admission order, not licence semantics.
        let first_request = tokio::time::timeout(std::time::Duration::from_secs(3), async {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut headers = Vec::new();
            while !headers.ends_with(b"\r\n\r\n") {
                headers.push(socket.read_u8().await.unwrap());
                assert!(headers.len() < 64 * 1024);
            }
            let headers = String::from_utf8(headers).unwrap();
            let length = headers
                .lines()
                .find_map(|line| {
                    let (key, value) = line.split_once(':')?;
                    key.eq_ignore_ascii_case("content-length")
                        .then(|| value.trim().parse::<usize>().unwrap())
                })
                .unwrap_or(0);
            let mut body = vec![0; length];
            socket.read_exact(&mut body).await.unwrap();
            (headers, body)
        })
        .await;
        read.abort();
        let _ = read.await;
        let (headers, body) = first_request.expect("opening must make a fresh mode probe");
        assert!(
            headers.starts_with("GET /status HTTP/1.1\r\n"),
            "opening bypassed fresh mode probe; cached={cached}; scoped={scoped}; first_line={:?}",
            headers.lines().next()
        );
        assert!(body.is_empty());
    }
}

#[test]
fn native_ledger_opening_admission_rejects_duplicate_source_identities() {
    let captured = include_str!(
        "../../crates/bridge-tally-protocol/tests/fixtures/native/ledgers_native_master_fields_lab.utf8.xml"
    );
    let company = "56359347-3976-4d01-b44e-56fa0f6a422c";
    let baseline = admit_native_ledger_opening_rows(captured, company).unwrap();
    assert!(baseline.len() > 1);
    let start = captured.find("<LEDGER NAME=").unwrap();
    let end = start + captured[start..].find("</LEDGER>").unwrap() + "</LEDGER>".len();
    let first_row = &captured[start..end];
    // Duplicating captured source is negative fault injection, not a new fixture.
    for duplicate in [
        first_row.to_string(),
        first_row.replacen(
            "BRIDGE MFLAB CREDITOR ALPHA",
            "Synthetic Duplicate Alias",
            1,
        ),
    ] {
        let faulty = format!("{}{}{}", &captured[..end], duplicate, &captured[end..]);
        let error = admit_native_ledger_opening_rows(&faulty, company).unwrap_err();
        assert_eq!(
            error.downcast_ref::<NativeLedgerIdentityAdmissionError>(),
            Some(&NativeLedgerIdentityAdmissionError::Duplicate)
        );
    }
    let parsed = parse_native_ledger_source_records_with_evidence(captured, company).unwrap();
    let duplicate_guid = captured.replacen(
        parsed.records[1].identities.guid.as_deref().unwrap(),
        &parsed.records[0]
            .identities
            .guid
            .as_deref()
            .unwrap()
            .to_ascii_uppercase(),
        1,
    );
    assert_eq!(
        admit_native_ledger_opening_rows(&duplicate_guid, company)
            .unwrap_err()
            .downcast_ref::<NativeLedgerIdentityAdmissionError>(),
        Some(&NativeLedgerIdentityAdmissionError::Duplicate)
    );
    let first_id = parsed.records[0].identities.master_id.as_deref().unwrap();
    let second_id = parsed.records[1].identities.master_id.as_deref().unwrap();
    let original = format!("<MASTERID TYPE=\"Number\"> {second_id}</MASTERID>");
    assert!(captured.contains(&original));
    let absent = captured.replacen(&original, "", 1);
    // MASTERID was already required by the native row parser; admission does
    // not turn an optional source field into a new requirement.
    assert!(parse_native_ledger_source_records_with_evidence(&absent, company).is_err());
    assert!(admit_native_ledger_opening_rows(&absent, company).is_err());
    let alias = captured.replacen(&original, &format!("<MASTERID>0{first_id}</MASTERID>"), 1);
    // The protocol's textual duplicate registry cannot see this numeric alias.
    assert!(
        parse_native_ledger_source_records_with_evidence(&alias, company)
            .unwrap()
            .evidence
            .duplicate_identities
            .is_empty()
    );
    assert_eq!(
        admit_native_ledger_opening_rows(&alias, company)
            .unwrap_err()
            .downcast_ref::<NativeLedgerIdentityAdmissionError>(),
        Some(&NativeLedgerIdentityAdmissionError::Duplicate)
    );
    for invalid in ["Maybe", "-1", "+1", "18446744073709551616"] {
        let faulty = captured.replacen(&original, &format!("<MASTERID>{invalid}</MASTERID>"), 1);
        assert_eq!(
            admit_native_ledger_opening_rows(&faulty, company)
                .unwrap_err()
                .downcast_ref::<NativeLedgerIdentityAdmissionError>(),
            Some(&NativeLedgerIdentityAdmissionError::InvalidMasterId)
        );
    }
}

#[tokio::test]
async fn book_start_opening_requires_stable_mode_and_commits_probe_sources() {
    use tally_protocol_simulator::{
        Fixture, ProductStatus, ResponseFraming, ScenarioPlan, SequenceSimulator, WireEncoding,
    };
    let decode = |bytes: &[u8]| {
        String::from_utf16(
            &bytes
                .chunks_exact(2)
                .map(|pair| u16::from_le_bytes([pair[0], pair[1]]))
                .collect::<Vec<_>>(),
        )
        .unwrap()
    };
    let captured_company = decode(include_bytes!(
        "../../crates/bridge-tally-protocol/tests/fixtures/agent/native-licensed-companies.utf16le.xml"));
    let captured_extent = decode(include_bytes!(
        "../../crates/bridge-tally-protocol/tests/fixtures/agent/native-company-book-extents.utf16le.xml"));
    let captured_ledger = decode(include_bytes!(
        "../../crates/bridge-tally-protocol/tests/fixtures/agent/native-period-opening.utf16le.xml"
    ));
    let xml = |text: String| {
        ScenarioPlan::new(Fixture::SyntheticXml(text))
            .with_encoding(WireEncoding::Utf16Le)
            .with_framing(ResponseFraming::ContentLength)
    };
    let status = ScenarioPlan::new(Fixture::ProductStatus(ProductStatus::TallyPrime));
    let education = |text: &str| {
        text.replace(
            "<EDUMODE TYPE=\"Logical\">No</EDUMODE>",
            "<EDUMODE TYPE=\"Logical\">Yes</EDUMODE>",
        )
    };
    for (unsafe_book_start, mode_drift) in [(true, false), (false, true), (false, false)] {
        // Fault injection into captured licensed metadata, not live Education
        // qualification: either an unsafe book date or a closing-mode change.
        let company_xml = if unsafe_book_start {
            education(&captured_company).replace("20260401", "20260415")
        } else {
            captured_company.clone()
        };
        let extent_xml = if unsafe_book_start {
            captured_extent.replace("20260401", "20260415")
        } else {
            captured_extent.clone()
        };
        let companies = parse_companies_from_collection(&company_xml).unwrap();
        let observed = companies
            .iter()
            .find(|row| row.guid.as_deref() == Some("61c6de69-1748-461c-ad3f-162cb949df9f"))
            .unwrap();
        let identity = VerifiedCompanyIdentity::from_observed_companies(
            observed.name.clone(),
            observed.guid.clone().unwrap(),
            observed.company_number.clone().unwrap(),
            observed.books_from.clone().unwrap(),
            &companies,
        )
        .unwrap();
        let company = xml(company_xml);
        let extent = xml(extent_xml);
        let ledger = xml(captured_ledger.clone());
        let mut plans = vec![
            status.clone(),
            company.clone(),
            company.clone(),
            extent.clone(),
            status.clone(),
            extent.clone(),
            status.clone(),
        ];
        if !unsafe_book_start {
            plans.extend([
                ledger.clone(),
                status.clone(),
                ledger,
                status.clone(),
                extent.clone(),
                status.clone(),
                extent,
                status.clone(),
                company,
                status.clone(),
                xml(if mode_drift {
                    education(&captured_company)
                } else {
                    captured_company.clone()
                }),
            ]);
        }
        let expected_requests = plans.len();
        let responses = plans
            .iter()
            .map(|plan| tally_protocol_simulator::encode(&plan.fixture.body(), plan.encoding))
            .collect::<Vec<_>>();
        let simulator = SequenceSimulator::spawn(plans).unwrap();
        let result = TallyRuntime::default()
            .fetch_ledgers_with_evidence(
                TallyConfig {
                    host: simulator.address().ip().to_string(),
                    port: simulator.address().port(),
                },
                &identity,
            )
            .await;
        let observations = simulator.finish().unwrap();
        assert_eq!(observations.len(), expected_requests);
        if unsafe_book_start {
            let error = result.unwrap_err();
            assert_eq!(
                error.downcast_ref::<OpeningBoundaryObservationError>(),
                Some(&OpeningBoundaryObservationError::Period(
                    NativeLedgerExportPeriodError::UnsupportedBoundary
                ))
            );
        } else if mode_drift {
            assert_eq!(
                result
                    .unwrap_err()
                    .downcast_ref::<OpeningBoundaryObservationError>(),
                Some(&OpeningBoundaryObservationError::Changed)
            );
        } else {
            let (ledgers, evidence) = result.unwrap();
            assert!(!ledgers.is_empty());
            let join = |left: &str, right: &str| sha256_hex(format!("{left}:{right}").as_bytes());
            let request = |index: usize| observations[index].request_body_sha256.as_str();
            let response = |index: usize| sha256_hex(&responses[index]);
            assert_eq!(
                evidence.request_sha256,
                join(
                    &join(&join(request(0), request(1)), request(7)),
                    &join(request(16), request(17))
                )
            );
            assert_eq!(
                evidence.response_sha256,
                join(
                    &join(&join(&response(0), &response(1)), &response(7)),
                    &join(&response(16), &response(17))
                )
            );
            assert_eq!(
                evidence.bytes,
                [0, 1, 7, 9, 16, 17]
                    .iter()
                    .map(|i| responses[*i].len())
                    .sum::<usize>()
            );
        }
    }
}
