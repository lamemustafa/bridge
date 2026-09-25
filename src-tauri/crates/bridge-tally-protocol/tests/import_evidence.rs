use bridge_tally_protocol::{
    decode_tally_xml_response_bytes_limited, parse_import_evidence, parse_import_outcome,
    parse_import_result, parse_ledger_write_readback_with_evidence, ExpectedTallyTextEncoding,
    TallyImportApplicationStatus, TallyImportCounterPresence, TallyImportOutcome,
    TallyImportResult, MAX_TALLY_LINE_ERRORS, MAX_TALLY_LINE_ERROR_BYTES,
    MAX_TALLY_LINE_ERROR_CHARS,
};

const LIVE_EDUCATION_W1_LEDGER: &str =
    include_str!("fixtures/live_education_w1_ledger_sanitized.xml");
const LIVE_EDUCATION_W4_VOUCHER: &str =
    include_str!("fixtures/live_education_w4_voucher_sanitized.xml");
const LIVE_EDUCATION_W7_BADDATE: &str =
    include_str!("fixtures/live_education_w7_baddate_sanitized.xml");

fn all_counters_reported() -> TallyImportCounterPresence {
    TallyImportCounterPresence {
        created: true,
        altered: true,
        deleted: true,
        ignored: true,
        errors: true,
        cancelled: true,
        exceptions: true,
    }
}

#[test]
fn live_education_import_counter_shapes_are_clean_only_when_the_intended_write_applied() {
    // Derived from ignored 2026-07-29 live captures. The w7 LINEERROR text is
    // deliberately redacted; its observed presence is preserved.
    for xml in [LIVE_EDUCATION_W1_LEDGER, LIVE_EDUCATION_W4_VOUCHER] {
        let evidence = parse_import_evidence(xml).expect("live success shape parses");
        assert_eq!(
            evidence.application_status(),
            TallyImportApplicationStatus::NotReported
        );
        assert_eq!(evidence.counters().created, 1);
        assert!(evidence.counters().is_clean_success_for(1, 0, 0));
    }

    let rejected = parse_import_evidence(LIVE_EDUCATION_W7_BADDATE)
        .expect("failure evidence must remain available for audit");
    assert_eq!(
        rejected.application_status(),
        TallyImportApplicationStatus::NotReported
    );
    assert_eq!(rejected.counters().created, 0);
    assert_eq!(rejected.counters().errors, 0);
    assert_eq!(rejected.counters().exceptions, 1);
    assert_eq!(rejected.counters().line_error_count, 1);
    assert!(rejected.exceptions_were_reported());
    assert!(
        !rejected.counters().is_clean_success_for(1, 0, 0),
        "w7 must reject success despite ERRORS=0"
    );
}

#[test]
fn clean_import_success_requires_each_live_evidence_condition_independently() {
    let clean = TallyImportResult {
        created: 1,
        altered: 0,
        deleted: 0,
        ignored: 0,
        errors: 0,
        cancelled: 0,
        exceptions: 0,
        line_error_count: 0,
        counter_presence: all_counters_reported(),
    };
    assert!(clean.is_clean_success_for(1, 0, 0));
    assert!(!clean.is_clean_success_for(0, 1, 0));

    let all_zero = TallyImportResult {
        created: 0,
        ..clean.clone()
    };
    assert!(!all_zero.is_clean_success_for(1, 0, 0));
    assert!(!all_zero.is_clean_success_for(0, 0, 0));

    let with_errors = TallyImportResult {
        errors: 1,
        ..clean.clone()
    };
    assert!(!with_errors.is_clean_success_for(1, 0, 0));

    let with_exceptions = TallyImportResult {
        exceptions: 1,
        ..clean.clone()
    };
    assert!(!with_exceptions.is_clean_success_for(1, 0, 0));

    let with_line_error = TallyImportResult {
        line_error_count: 1,
        ..clean
    };
    assert!(!with_line_error.is_clean_success_for(1, 0, 0));

    let empty_line_error = parse_import_evidence("<RESPONSE><CREATED>1</CREATED><ALTERED>0</ALTERED><DELETED>0</DELETED><IGNORED>0</IGNORED><ERRORS>0</ERRORS><CANCELLED>0</CANCELLED><EXCEPTIONS>0</EXCEPTIONS><LINEERROR></LINEERROR></RESPONSE>")
        .expect("an empty LINEERROR is still evidence of a reported line error");
    assert_eq!(empty_line_error.counters().line_error_count, 1);
    assert!(!empty_line_error.counters().is_clean_success_for(1, 0, 0));
}

#[test]
fn captured_success_requires_every_clean_counter_to_be_observed() {
    // Start with an ignored live capture, then remove one counter at a time.
    // A missing counter remains auditably absent; it is never promoted to a
    // parser-defaulted clean zero.
    for counter in [
        "CREATED",
        "ALTERED",
        "DELETED",
        "IGNORED",
        "ERRORS",
        "CANCELLED",
        "EXCEPTIONS",
    ] {
        let tag = format!("<{counter}>0</{counter}>");
        let xml = if counter == "CREATED" {
            LIVE_EDUCATION_W4_VOUCHER.replace("<CREATED>1</CREATED>", "")
        } else {
            LIVE_EDUCATION_W4_VOUCHER.replace(&tag, "")
        };
        let clean = parse_import_outcome(&xml)
            .is_ok_and(|outcome| outcome.counters().is_clean_success_for(1, 0, 0));
        assert!(!clean, "omitted {counter} must not prove a clean success");
    }
}

#[test]
fn legacy_persisted_counts_deserialize_but_cannot_prove_a_clean_import() {
    let legacy: TallyImportResult = serde_json::from_str(
        r#"{"created":1,"altered":0,"deleted":0,"ignored":0,"errors":0,"cancelled":0,"exceptions":0,"line_error_count":0}"#,
    )
    .expect("pre-presence saved records remain readable");
    assert!(!legacy.counter_presence.all_reported());
    assert!(!legacy.is_clean_success_for(1, 0, 0));
}

#[test]
fn wrapped_status_zero_rejects_clean_import_counters() {
    let xml = "<ENVELOPE><HEADER><STATUS>0</STATUS></HEADER><BODY><DATA><IMPORTRESULT><CREATED>1</CREATED><ALTERED>0</ALTERED><DELETED>0</DELETED><IGNORED>0</IGNORED><ERRORS>0</ERRORS><CANCELLED>0</CANCELLED><EXCEPTIONS>0</EXCEPTIONS></IMPORTRESULT></DATA></BODY></ENVELOPE>";
    let evidence = parse_import_evidence(xml).expect("failure evidence remains parseable");
    assert_eq!(
        evidence.application_status(),
        TallyImportApplicationStatus::Failure
    );
    assert!(evidence.counters().is_clean_success_for(1, 0, 0));
    assert!(
        parse_import_result(xml).is_err(),
        "reported STATUS=0 must override otherwise clean counters"
    );
}

#[test]
fn non_numeric_lastvchid_is_rejected_even_when_import_counters_are_clean() {
    let xml = "<RESPONSE><CREATED>1</CREATED><ALTERED>0</ALTERED><DELETED>0</DELETED><LASTVCHID>not-a-number</LASTVCHID><IGNORED>0</IGNORED><ERRORS>0</ERRORS><CANCELLED>0</CANCELLED><EXCEPTIONS>0</EXCEPTIONS></RESPONSE>";
    assert!(parse_import_evidence(xml).is_err());
}

#[test]
fn evidence_binds_counters_response_and_redacted_line_errors() {
    let xml = "<RESPONSE><CREATED>0</CREATED><ALTERED>0</ALTERED><IGNORED>1</IGNORED><ERRORS>1</ERRORS><EXCEPTIONS>0</EXCEPTIONS><LINEERROR>PRIVATE-SYNTHETIC-SENTINEL</LINEERROR></RESPONSE>";
    let evidence = parse_import_evidence(xml).unwrap();

    assert_eq!(evidence.counters().line_error_count, 1);
    assert_eq!(evidence.line_error_sha256().len(), 1);
    assert_eq!(evidence.response_sha256().len(), 64);
    let debug = format!("{evidence:?}");
    assert!(!debug.contains("PRIVATE-SYNTHETIC-SENTINEL"));
}

#[test]
fn response_and_line_error_commitments_change_with_exact_input() {
    let make = |message: &str| {
        parse_import_evidence(&format!(
            "<RESPONSE><CREATED>0</CREATED><ALTERED>0</ALTERED><IGNORED>1</IGNORED><ERRORS>1</ERRORS><EXCEPTIONS>0</EXCEPTIONS><LINEERROR>{message}</LINEERROR></RESPONSE>"
        ))
        .unwrap()
    };
    let first = make("synthetic-a");
    let second = make("synthetic-b");
    assert_ne!(first.response_sha256(), second.response_sha256());
    assert_ne!(first.line_error_sha256(), second.line_error_sha256());
}

#[test]
fn duplicate_or_wrongly_nested_status_and_counters_are_rejected() {
    let duplicate_counter = "<RESPONSE><CREATED>0</CREATED><CREATED>1</CREATED><ALTERED>0</ALTERED><IGNORED>0</IGNORED><ERRORS>0</ERRORS><EXCEPTIONS>0</EXCEPTIONS></RESPONSE>";
    assert!(parse_import_evidence(duplicate_counter).is_err());

    let nested_counter = "<RESPONSE><UNRELATED><CREATED>1</CREATED></UNRELATED><ALTERED>0</ALTERED><IGNORED>0</IGNORED><ERRORS>0</ERRORS><EXCEPTIONS>0</EXCEPTIONS></RESPONSE>";
    assert!(parse_import_evidence(nested_counter).is_err());

    let duplicate_status = "<ENVELOPE><HEADER><STATUS>0</STATUS><STATUS>1</STATUS></HEADER><BODY><DATA><IMPORTRESULT><CREATED>1</CREATED><ALTERED>0</ALTERED><IGNORED>0</IGNORED><ERRORS>0</ERRORS><EXCEPTIONS>0</EXCEPTIONS></IMPORTRESULT></DATA></BODY></ENVELOPE>";
    assert!(parse_import_evidence(duplicate_status).is_err());

    let duplicate_container = "<ENVELOPE><HEADER><STATUS>1</STATUS></HEADER><BODY><DATA><IMPORTRESULT><CREATED>1</CREATED><ALTERED>0</ALTERED><IGNORED>0</IGNORED><ERRORS>0</ERRORS><EXCEPTIONS>0</EXCEPTIONS></IMPORTRESULT><IMPORTRESULT/></DATA></BODY></ENVELOPE>";
    assert!(parse_import_evidence(duplicate_container).is_err());

    let duplicate_header = "<ENVELOPE><HEADER/><HEADER><STATUS>1</STATUS></HEADER><BODY><DATA><CREATED>1</CREATED><ALTERED>0</ALTERED><IGNORED>0</IGNORED><ERRORS>0</ERRORS><EXCEPTIONS>0</EXCEPTIONS></DATA></BODY></ENVELOPE>";
    assert!(parse_import_evidence(duplicate_header).is_err());

    let doctype = "<!DOCTYPE ENVELOPE><ENVELOPE><HEADER><STATUS>1</STATUS></HEADER><BODY><DATA><CREATED>1</CREATED><ALTERED>0</ALTERED><IGNORED>0</IGNORED><ERRORS>0</ERRORS><EXCEPTIONS>0</EXCEPTIONS></DATA></BODY></ENVELOPE>";
    assert!(parse_import_evidence(doctype).is_err());

    let duplicate_body = "<ENVELOPE><HEADER><STATUS>1</STATUS></HEADER><BODY><DATA><CREATED>1</CREATED><ALTERED>0</ALTERED><IGNORED>0</IGNORED><ERRORS>0</ERRORS></DATA></BODY><BODY/></ENVELOPE>";
    assert!(parse_import_evidence(duplicate_body).is_err());

    let body_before_header = "<ENVELOPE><BODY/><HEADER><STATUS>1</STATUS></HEADER></ENVELOPE>";
    assert!(parse_import_evidence(body_before_header).is_err());

    let attributed_status = "<ENVELOPE><HEADER><STATUS unsafe=\"1\">1</STATUS></HEADER><BODY><DATA><CREATED>1</CREATED><ALTERED>0</ALTERED><IGNORED>0</IGNORED><ERRORS>0</ERRORS></DATA></BODY></ENVELOPE>";
    assert!(parse_import_evidence(attributed_status).is_err());

    let mixed_text = "<ENVELOPE><HEADER><STATUS>1</STATUS></HEADER><BODY>mixed<DATA><CREATED>1</CREATED><ALTERED>0</ALTERED><IGNORED>0</IGNORED><ERRORS>0</ERRORS></DATA></BODY></ENVELOPE>";
    assert!(parse_import_evidence(mixed_text).is_err());

    let unknown_wrapper = "<ENVELOPE><HEADER><STATUS>1</STATUS></HEADER><BODY><DATA><UNEXPECTED><CREATED>1</CREATED></UNEXPECTED><ALTERED>0</ALTERED><IGNORED>0</IGNORED><ERRORS>0</ERRORS></DATA></BODY></ENVELOPE>";
    assert!(parse_import_evidence(unknown_wrapper).is_err());
}

#[test]
fn exact_envelope_import_result_path_is_accepted() {
    let xml = "<ENVELOPE><HEADER><STATUS>1</STATUS></HEADER><BODY><DATA><IMPORTRESULT><CREATED>1</CREATED><ALTERED>0</ALTERED><DELETED>0</DELETED><IGNORED>0</IGNORED><ERRORS>0</ERRORS><CANCELLED>0</CANCELLED><EXCEPTIONS>0</EXCEPTIONS></IMPORTRESULT></DATA></BODY></ENVELOPE>";
    let evidence = parse_import_evidence(xml).unwrap();
    assert_eq!(evidence.counters().created, 1);
}

#[test]
fn documented_direct_data_import_result_path_is_accepted_without_profile_mixing() {
    let xml = "<ENVELOPE><HEADER><STATUS>1</STATUS></HEADER><BODY><DATA><CREATED>2</CREATED><ALTERED>1</ALTERED><DELETED>0</DELETED><IGNORED>0</IGNORED><ERRORS>1</ERRORS><CANCELLED>0</CANCELLED><EXCEPTIONS>1</EXCEPTIONS><LINEERROR>BRIDGE SYNTHETIC ERROR</LINEERROR></DATA></BODY></ENVELOPE>";
    let evidence = parse_import_evidence(xml).expect("documented direct DATA profile");
    assert_eq!(evidence.counters().created, 2);
    assert_eq!(evidence.counters().altered, 1);
    assert_eq!(evidence.counters().errors, 1);
    assert_eq!(evidence.counters().exceptions, 1);
    assert_eq!(evidence.counters().line_error_count, 1);

    let mixed = "<ENVELOPE><HEADER><STATUS>1</STATUS></HEADER><BODY><DATA><CREATED>1</CREATED><IMPORTRESULT><ALTERED>0</ALTERED><IGNORED>0</IGNORED><ERRORS>0</ERRORS><EXCEPTIONS>0</EXCEPTIONS></IMPORTRESULT></DATA></BODY></ENVELOPE>";
    assert!(parse_import_evidence(mixed).is_err());

    let direct_extra_then_wrapped = "<ENVELOPE><HEADER><STATUS>1</STATUS></HEADER><BODY><DATA><LASTVCHID>1</LASTVCHID><IMPORTRESULT><CREATED>1</CREATED><ALTERED>0</ALTERED><IGNORED>0</IGNORED><ERRORS>0</ERRORS></IMPORTRESULT></DATA></BODY></ENVELOPE>";
    assert!(parse_import_evidence(direct_extra_then_wrapped).is_err());

    let wrapped_then_direct_extra = "<ENVELOPE><HEADER><STATUS>1</STATUS></HEADER><BODY><DATA><IMPORTRESULT><CREATED>1</CREATED><ALTERED>0</ALTERED><IGNORED>0</IGNORED><ERRORS>0</ERRORS></IMPORTRESULT><LASTVCHID>1</LASTVCHID></DATA></BODY></ENVELOPE>";
    assert!(parse_import_evidence(wrapped_then_direct_extra).is_err());
}

#[test]
fn official_legacy_and_wrapped_success_profiles_accept_auxiliary_fields() {
    let legacy = "<RESPONSE><CREATED>2</CREATED><ALTERED>0</ALTERED><LASTVCHID>0</LASTVCHID><LASTMID>0</LASTMID><COMBINED>0</COMBINED><IGNORED>0</IGNORED><ERRORS>0</ERRORS></RESPONSE>";
    let legacy_evidence = parse_import_evidence(legacy).expect("official legacy RESPONSE profile");
    assert_eq!(
        legacy_evidence.application_status(),
        TallyImportApplicationStatus::NotReported
    );
    assert_eq!(legacy_evidence.counters().created, 2);
    assert!(!legacy_evidence.exceptions_were_reported());

    let wrapped = "<ENVELOPE><HEADER><STATUS>1</STATUS></HEADER><BODY><DATA><IMPORTRESULT><CREATED>2</CREATED><ALTERED>0</ALTERED><LASTVCHID>119</LASTVCHID><LASTMID>0</LASTMID><COMBINED>0</COMBINED><IGNORED>0</IGNORED><ERRORS>0</ERRORS></IMPORTRESULT></DATA></BODY></ENVELOPE>";
    let wrapped_evidence =
        parse_import_evidence(wrapped).expect("official wrapped IMPORTRESULT profile");
    assert_eq!(
        wrapped_evidence.application_status(),
        TallyImportApplicationStatus::Success
    );
    assert_eq!(wrapped_evidence.counters().created, 2);
    assert!(!wrapped_evidence.exceptions_were_reported());
}

#[test]
fn documented_direct_failure_shape_retains_counters_without_becoming_success() {
    let xml = "<ENVELOPE><HEADER><STATUS>0</STATUS></HEADER><BODY><DATA><CREATED>0</CREATED><ALTERED>0</ALTERED><DELETED>0</DELETED><LASTVCHID>0</LASTVCHID><LASTMID>0</LASTMID><COMBINED>0</COMBINED><IGNORED>0</IGNORED><ERRORS>1</ERRORS><CANCELLED>0</CANCELLED><LINEERROR>BRIDGE SYNTHETIC FAILURE</LINEERROR><VCHNUMBER>BRIDGE-SYNTHETIC-1</VCHNUMBER><DESC>BRIDGE SYNTHETIC FAILURE</DESC></DATA></BODY></ENVELOPE>";
    let outcome = parse_import_outcome(xml).expect("parse documented failure counters");
    assert_eq!(
        outcome.application_status(),
        TallyImportApplicationStatus::Failure
    );
    assert_eq!(outcome.counters().created, 0);
    assert_eq!(outcome.counters().errors, 1);
    assert_eq!(outcome.counters().exceptions, 0);
    assert!(!outcome.exceptions_were_reported());
    let evidence = parse_import_evidence(xml).expect("retain documented failure evidence");
    assert_eq!(
        evidence.application_status(),
        TallyImportApplicationStatus::Failure
    );
    assert_eq!(evidence.counters().errors, 1);
    assert_eq!(evidence.line_error_sha256().len(), 1);
    assert!(!evidence.exceptions_were_reported());
    assert!(
        parse_import_result(xml).is_err(),
        "legacy success-oriented API must remain fail-closed"
    );
}

#[test]
fn malformed_line_error_entities_return_only_a_safe_error() {
    let xml = "<RESPONSE><CREATED>0</CREATED><ALTERED>0</ALTERED><IGNORED>1</IGNORED><ERRORS>1</ERRORS><EXCEPTIONS>0</EXCEPTIONS><LINEERROR>&PRIVATE_SYNTHETIC_SENTINEL;</LINEERROR></RESPONSE>";
    let error = parse_import_evidence(xml).unwrap_err().to_string();
    assert_eq!(error, "Tally import response evidence was invalid");
    assert!(!error.contains("PRIVATE"));
    assert!(!error.contains("SENTINEL"));
}

#[test]
fn write_readback_rejects_wrong_nesting_duplicate_fields_and_attributes() {
    const HASH: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
    let wrap = |ledger: &str, count: usize| {
        format!(
            r#"<ENVELOPE><HEADER><STATUS>1</STATUS></HEADER><BODY><COMPANYCONTEXT SCHEMA="bridge.tally.ledger-write-readback/1" OBJECTTYPE="LEDGER" NAME="BRIDGE SYNTHETIC BOOK" GUID="company-guid" RECORDCOUNT="{count}" QUERYIDENTITYSETSHA256="{HASH}"/>{ledger}</BODY></ENVELOPE>"#
        )
    };
    let valid = r#"<LEDGER NAME="BRIDGE LEDGER" REMOTEID="remote-1"><PARENT>BRIDGE GROUP</PARENT><OPENINGBALANCE>0</OPENINGBALANCE></LEDGER>"#;
    let valid_result = parse_ledger_write_readback_with_evidence(&wrap(valid, 1));
    assert!(valid_result.is_ok(), "{valid_result:?}");

    let nested = format!("<COLLECTION>{valid}</COLLECTION>");
    assert!(parse_ledger_write_readback_with_evidence(&wrap(&nested, 1)).is_err());
    let duplicate_field = valid.replace("</LEDGER>", "<OPENINGBALANCE>0</OPENINGBALANCE></LEDGER>");
    assert!(parse_ledger_write_readback_with_evidence(&wrap(&duplicate_field, 1)).is_err());
    let duplicate_attribute = valid.replace(
        "REMOTEID=\"remote-1\"",
        "REMOTEID=\"remote-1\" REMOTEID=\"remote-1\"",
    );
    assert!(parse_ledger_write_readback_with_evidence(&wrap(&duplicate_attribute, 1)).is_err());
    let unexpected_attribute = valid.replace(
        "REMOTEID=\"remote-1\"",
        "REMOTEID=\"remote-1\" UNSAFE=\"1\"",
    );
    assert!(parse_ledger_write_readback_with_evidence(&wrap(&unexpected_attribute, 1)).is_err());

    let case_variant_duplicate = valid.replace(
        "REMOTEID=\"remote-1\"",
        "REMOTEID=\"remote-1\" remoteid=\"other\"",
    );
    assert!(parse_ledger_write_readback_with_evidence(&wrap(&case_variant_duplicate, 1)).is_err());

    let nested_context = wrap("", 0)
        .replace("<COMPANYCONTEXT", "<WRAPPER><COMPANYCONTEXT")
        .replace("/>", "/></WRAPPER>");
    assert!(parse_ledger_write_readback_with_evidence(&nested_context).is_err());

    let duplicate_status =
        wrap("", 0).replace("<STATUS>1</STATUS>", "<STATUS>0</STATUS><STATUS>1</STATUS>");
    assert!(parse_ledger_write_readback_with_evidence(&duplicate_status).is_err());
}

const PARTIAL_COMMIT_LIVE: &[u8] =
    include_bytes!("fixtures/import_line_error_partial_commit_live.utf16le.xml");

fn captured(bytes: &[u8]) -> String {
    decode_tally_xml_response_bytes_limited(
        bytes,
        "text/xml; charset=utf-16",
        ExpectedTallyTextEncoding::Utf16Le,
        bytes.len(),
    )
    .expect("captured BOM-less UTF-16LE import response")
    .text
}

fn with_line_errors(texts: &[&str]) -> TallyImportOutcome {
    let line_errors: String = texts
        .iter()
        .map(|text| format!("<LINEERROR>{text}</LINEERROR>"))
        .collect();
    parse_import_outcome(&format!(
        "<RESPONSE>{line_errors}<CREATED>0</CREATED><ALTERED>0</ALTERED>\
         <DELETED>0</DELETED><IGNORED>0</IGNORED><ERRORS>0</ERRORS>\
         <CANCELLED>0</CANCELLED><EXCEPTIONS>{}</EXCEPTIONS></RESPONSE>",
        texts.len()
    ))
    .expect("synthetic line-error response")
}

/// The captured partial commit keeps Tally's text, unescaped, and still
/// counts as not clean for any intended create count.
#[test]
fn a_captured_partial_commit_keeps_tallys_line_error_text() {
    let outcome = parse_import_outcome(&captured(PARTIAL_COMMIT_LIVE)).expect("captured response");
    assert_eq!(outcome.counters().created, 49);
    assert_eq!(outcome.counters().errors, 0);
    assert_eq!(outcome.counters().exceptions, 1);
    assert_eq!(outcome.counters().line_error_count, 1);
    let line_errors = outcome.tally_line_errors();
    assert_eq!(line_errors.len(), 1);
    assert_eq!(
        line_errors[0].text(),
        "Ledger 'Lane A No Such Ledger' does not exist!"
    );
    assert!(!line_errors[0].truncated());
    assert_eq!(outcome.tally_line_errors_omitted(), 0);
    assert!(!outcome.counters().is_clean_success_for(50, 0, 0));
    assert!(!outcome.counters().is_clean_success_for(49, 0, 0));
}

/// Each text is clipped to 512 characters on a character boundary, at most
/// 64 are kept, and every clip or omission is explicit.
#[test]
fn line_error_text_is_bounded_per_entry_and_in_count() {
    let long = "\u{20b9}".repeat(MAX_TALLY_LINE_ERROR_CHARS + 1);
    let mut texts = vec![long.as_str()];
    texts.extend(std::iter::repeat_n("short", MAX_TALLY_LINE_ERRORS));
    let outcome = with_line_errors(&texts);
    assert_eq!(
        outcome.counters().line_error_count,
        (MAX_TALLY_LINE_ERRORS + 1) as u64
    );
    let kept = outcome.tally_line_errors();
    assert_eq!(kept.len(), MAX_TALLY_LINE_ERRORS);
    assert_eq!(kept[0].text().chars().count(), MAX_TALLY_LINE_ERROR_CHARS);
    assert!(kept[0].truncated());
    assert_eq!(kept[1].text(), "short");
    assert!(!kept[1].truncated());
    assert_eq!(outcome.tally_line_errors_omitted(), 1);
    let exact = "a".repeat(MAX_TALLY_LINE_ERROR_CHARS);
    let fits = with_line_errors(&[exact.as_str()]);
    assert!(!fits.tally_line_errors()[0].truncated());
}

/// The kept text never exceeds 4,096 bytes as JSON escapes it, counting a
/// quote or a backslash as two bytes, and the entries it cannot hold are
/// counted.
#[test]
fn line_error_text_is_bounded_in_escaped_bytes() {
    let backslashes = "\\".repeat(MAX_TALLY_LINE_ERROR_CHARS);
    let outcome = with_line_errors(&[backslashes.as_str(); 5]);
    assert_eq!(outcome.tally_line_errors().len(), 4, "a backslash escapes to two bytes");
    let quotes = "&quot;".repeat(MAX_TALLY_LINE_ERROR_CHARS);
    let outcome = with_line_errors(&[quotes.as_str(); 10]);
    let kept = outcome.tally_line_errors();
    assert_eq!(kept.len(), 4, "four texts of 1,024 escaped bytes fill 4,096");
    assert_eq!(outcome.tally_line_errors_omitted(), 6);
    assert_eq!(serde_json::to_value(&outcome).unwrap()["tally_line_errors_omitted"], 6);
    let escaped = serde_json::to_string(kept).unwrap();
    assert!(escaped.len() <= MAX_TALLY_LINE_ERROR_BYTES + 64 * kept.len(), "{}", escaped.len());
}

/// Control, format, separator, private-use and unassigned characters are
/// replaced, so the text cannot reorder, hide or break what a person reads,
/// nor grow six-fold when escaped. U+0600 is format but not ignorable, and
/// U+115F ignorable but not format, so each check is needed on its own.
#[test]
fn control_and_format_characters_are_replaced() {
    let outcome = with_line_errors(&[
        "a&#1;b\u{202e}c\u{200b}d\u{2066}e\u{0600}f\u{115f}g\u{2028}h\u{2029}i\u{e000}j\u{0378}k",
    ]);
    assert_eq!(
        outcome.tally_line_errors()[0].text(),
        "a\u{fffd}b\u{fffd}c\u{fffd}d\u{fffd}e\u{fffd}f\u{fffd}g\u{fffd}h\u{fffd}i\u{fffd}j\u{fffd}k"
    );
}

/// An empty LINEERROR is kept as an empty text, so the kept list stays in
/// step with the elements Tally sent.
#[test]
fn an_empty_line_error_is_kept_as_empty_text() {
    let outcome = with_line_errors(&["", "second"]);
    let texts: Vec<_> = outcome.tally_line_errors().iter().map(|e| e.text()).collect();
    assert_eq!(texts, ["", "second"]);
}

/// Two responses that differ only in their text give equal counters.
#[test]
fn line_error_text_does_not_change_the_counters() {
    let one = with_line_errors(&["first wording"]);
    let other = with_line_errors(&["other wording"]);
    assert_ne!(one.tally_line_errors(), other.tally_line_errors());
    assert_eq!(one.counters(), other.counters());
}

/// A record without the field reads as no text, and an outcome with no
/// LINEERROR records exactly as before: neither new key appears.
#[test]
fn line_error_text_is_absent_from_old_and_clean_records() {
    let clean = parse_import_outcome("<RESPONSE><CREATED>1</CREATED><ALTERED>0</ALTERED><DELETED>0</DELETED><IGNORED>0</IGNORED><ERRORS>0</ERRORS><CANCELLED>0</CANCELLED><EXCEPTIONS>0</EXCEPTIONS></RESPONSE>").unwrap();
    let stored = serde_json::to_value(&clean).unwrap();
    assert!(stored.get("tally_line_errors").is_none(), "{stored}");
    assert!(stored.get("tally_line_errors_omitted").is_none(), "{stored}");
    let reread: TallyImportOutcome = serde_json::from_value(stored).unwrap();
    assert_eq!(reread, clean);
    // A record written before the text was kept counts its LINEERROR as
    // omitted.
    let mut old = serde_json::to_value(with_line_errors(&["x"])).unwrap();
    old.as_object_mut().unwrap().remove("tally_line_errors");
    let reread: TallyImportOutcome = serde_json::from_value(old).unwrap();
    assert!(reread.tally_line_errors().is_empty());
    assert_eq!(reread.tally_line_errors_omitted(), 1);
}

/// A stored record is bounded again on read: an over-long text is clipped
/// and marked, the list never exceeds the record's own count, and a
/// malformed list reads as none kept instead of failing the record.
#[test]
fn a_stored_record_is_bounded_again_on_read_and_never_fails_over_text() {
    let stored = serde_json::to_value(with_line_errors(&["x", "y"])).unwrap();
    let reread = |line_errors: serde_json::Value| {
        let mut record = stored.clone();
        record["tally_line_errors"] = line_errors;
        serde_json::from_value::<TallyImportOutcome>(record).expect("display text never fails a record")
    };
    let long = "y".repeat(MAX_TALLY_LINE_ERROR_CHARS + 7);
    let clipped = reread(serde_json::json!([
        {"text": long, "truncated": false},
        {"text": "z\u{202e}", "truncated": false},
        {"text": "beyond the count", "truncated": false}
    ]));
    let kept = clipped.tally_line_errors();
    assert_eq!(kept.len(), 2, "never more than line_error_count");
    assert_eq!(kept[0].text().chars().count(), MAX_TALLY_LINE_ERROR_CHARS);
    assert!(kept[0].truncated());
    assert_eq!(kept[1].text(), "z\u{fffd}");
    assert!(!kept[1].truncated());
    for malformed in [
        serde_json::Value::Null,
        serde_json::json!("text"),
        serde_json::json!([{"no_text": 1}]),
    ] {
        let reread = reread(malformed);
        assert!(reread.tally_line_errors().is_empty());
        assert_eq!(reread.tally_line_errors_omitted(), 2);
    }
}
