use super::*;

#[test]
fn catalog_read_failure_classifier_preserves_runtime_variants() {
    assert_eq!(
        classify_runtime_catalogue_error(anyhow::Error::new(TallyTransportError::ConnectionFailed)),
        StandardLedgerCatalogReadError::Transport
    );
    assert_eq!(
        classify_runtime_catalogue_error(anyhow::Error::new(NativeReportPairDrift)),
        StandardLedgerCatalogReadError::UnstableResponse
    );
    assert_eq!(
        classify_runtime_catalogue_error(anyhow::Error::new(
            CompanyIdentityBracketError::AbsentOrAmbiguous,
        )),
        StandardLedgerCatalogReadError::CompanyIdentityMismatch
    );
}

/// Every `TallyTransportError` variant, named explicitly rather than sampled, so a
/// variant this test does not know about cannot pass silently -- the match in
/// `classify_transport_error` would fail to compile first.
#[test]
fn catalog_read_failure_classifier_splits_transport_from_response_validation() {
    let request_side = [
        TallyTransportError::EndpointInvalid { code: "test" },
        TallyTransportError::PolicyInvalid { code: "test" },
        TallyTransportError::ClientInitializationFailed,
        TallyTransportError::RequestTooLarge { limit: 1 },
        TallyTransportError::ConnectionFailed,
        TallyTransportError::RequestTimedOut,
        TallyTransportError::RequestFailed,
        TallyTransportError::HttpStatus { status: 500 },
        TallyTransportError::ResponseTruncated,
        TallyTransportError::ResponseReadFailed,
    ];
    for variant in request_side {
        assert_eq!(
            classify_transport_error(&variant),
            StandardLedgerCatalogReadError::Transport,
            "expected {variant:?} to remain the transport code"
        );
    }

    assert_eq!(
        classify_transport_error(&TallyTransportError::ResponseTooLarge {
            limit: 1,
            declared_by_peer: true,
        }),
        StandardLedgerCatalogReadError::BoundsViolation
    );
    // Both encoding faults are complete answers Bridge cannot read, so both are
    // malformed responses rather than outages a retry might clear.
    for variant in [
        TallyTransportError::UnsupportedContentEncoding,
        TallyTransportError::InvalidEncoding { code: "test" },
    ] {
        assert_eq!(
            classify_transport_error(&variant),
            StandardLedgerCatalogReadError::MalformedResponse,
            "expected {variant:?} to report an unusable response, not an outage"
        );
    }

    // A genuine request-side failure -- Tally was never reached at all -- must still
    // surface as the transport code end to end, through the anyhow chain, whether or
    // not it is tagged as a paired-report response (see the marker-gating test below
    // for the untagged/tagged response-side contrast this end-to-end path exists for).
    assert_eq!(
        classify_runtime_catalogue_error(anyhow::Error::new(TallyTransportError::RequestFailed)),
        StandardLedgerCatalogReadError::Transport
    );
}

/// The whole point of `PairedNativeReportResponseFailure`: the identical
/// `TallyTransportError` variant must classify differently depending on whether it
/// is marked as a paired native-report response failure. An untagged transport error
/// -- what a bracket read or one of the two health checks around the pair now
/// produces -- proves nothing about the shape of a catalogue response, since it
/// either predates that response or never touched it, so it must not inherit the
/// bounds/malformed split; only the marker earns that split. See
/// `PairedNativeReportResponseFailure` and `classify_runtime_catalogue_error`.
#[test]
fn catalogue_response_marker_gates_the_bounds_malformed_split() {
    for variant in [
        TallyTransportError::ResponseTooLarge {
            limit: 1,
            declared_by_peer: true,
        },
        TallyTransportError::InvalidEncoding { code: "test" },
        TallyTransportError::UnsupportedContentEncoding,
    ] {
        // Untagged: what a bracket read or a health-check failure now looks like.
        // Must stay the generic transport code even though the variant matches a
        // response-side split further down.
        assert_eq!(
            classify_runtime_catalogue_error(anyhow::Error::new(variant.clone())),
            StandardLedgerCatalogReadError::Transport,
            "expected an untagged {variant:?} to stay the generic transport code"
        );

        // The same variant, tagged as having come from a paired-report response,
        // gets the finer split.
        let expected = match variant {
            TallyTransportError::ResponseTooLarge { .. } => {
                StandardLedgerCatalogReadError::BoundsViolation
            }
            TallyTransportError::InvalidEncoding { .. }
            | TallyTransportError::UnsupportedContentEncoding => {
                StandardLedgerCatalogReadError::MalformedResponse
            }
            _ => unreachable!(),
        };
        let description = format!("{variant:?}");
        let tagged = PairedNativeReportResponseFailure::new(anyhow::Error::new(variant));
        assert_eq!(
            classify_runtime_catalogue_error(anyhow::Error::new(tagged)),
            expected,
            "expected a paired-report-tagged {description} to get its specific code"
        );
    }

    // A genuine request-side variant, tagged as though it were a paired-report
    // response, must still surface as the generic transport code: the marker only
    // gates the split, and `classify_transport_error` itself keeps request-side
    // variants at `Transport` regardless of tagging.
    let tagged_request_failure = PairedNativeReportResponseFailure::new(anyhow::Error::new(
        TallyTransportError::RequestFailed,
    ));
    assert_eq!(
        classify_runtime_catalogue_error(anyhow::Error::new(tagged_request_failure)),
        StandardLedgerCatalogReadError::Transport
    );
}

#[test]
fn catalog_read_error_codes_are_stable_and_distinct() {
    let errors = [
        StandardLedgerCatalogReadError::Transport,
        StandardLedgerCatalogReadError::UnstableResponse,
        StandardLedgerCatalogReadError::CompanyIdentityMismatch,
        StandardLedgerCatalogReadError::DuplicateIdentity,
        StandardLedgerCatalogReadError::BoundsViolation,
        StandardLedgerCatalogReadError::MalformedResponse,
    ];
    let codes = errors.map(StandardLedgerCatalogReadError::command_code);
    assert_eq!(
        codes,
        [
            "source_draft_catalogue_transport_failed",
            "source_draft_catalogue_unstable",
            "source_draft_catalogue_identity_mismatch",
            "source_draft_catalogue_duplicate_identity",
            "source_draft_catalogue_bounds_invalid",
            "source_draft_catalogue_malformed_response",
        ]
    );
}
