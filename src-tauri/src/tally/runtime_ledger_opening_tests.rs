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
async fn scoped_opening_probes_before_export_even_with_a_stale_licensed_cache() {
    use crate::tally::TallyProduct;
    use bridge_tally_core::CapabilityProfile;
    use tokio::io::AsyncReadExt;
    for cached in [false, true] {
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
                .fetch_ledger_opening_at_with_evidence(
                    config,
                    &identity,
                    TallyDate::parse("20260815").unwrap(),
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
            "opening bypassed fresh mode probe; cached={cached}; first_line={:?}",
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
