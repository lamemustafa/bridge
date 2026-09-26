use bridge_tally_core::TallyDate;
use bridge_tally_protocol::{
    native_outstandings::{render_party_ledger_master_request, NativeLedgerExportPeriod},
    outstandings_shared::DateBoundaryProfile,
    parse_native_party_ledger_master_records_with_evidence, GstDutyHead, GstDutyHeadObservation,
    PartyLedgerMasterFieldObservation,
};

use super::*;

const COMPANY_GUID: &str = "ae1490be-52c5-4544-9ffc-4b7da85f9797";

/// A minimized parser fixture built from the task's measured field values
/// and record shape. It is not a live-response evidence capture.
fn measured_ledger_master_response() -> String {
    let ledger = |name: &str, master_id: u8, tax_type: &str, duty_head: Option<&str>| {
        let duty_head = match duty_head {
            Some("") => "<GSTDUTYHEAD/>".to_string(),
            Some(value) => format!("<GSTDUTYHEAD>{value}</GSTDUTYHEAD>"),
            None => String::new(),
        };
        format!(
            "<LEDGER NAME=\"{name}\" RESERVEDNAME=\"\"><GUID>{COMPANY_GUID}-000000{master_id:02x}</GUID><BRIDGECOMPANYGUID>{COMPANY_GUID}</BRIDGECOMPANYGUID><MASTERID>{master_id}</MASTERID><ALTERID>{master_id}</ALTERID><PARENT>Duties &amp; Taxes</PARENT><TAXTYPE>{tax_type}</TAXTYPE>{duty_head}<OPENINGBALANCE>0.00</OPENINGBALANCE><LANGUAGENAME.LIST><NAME.LIST><NAME>Localized {name}</NAME></NAME.LIST></LANGUAGENAME.LIST></LEDGER>"
        )
    };
    format!(
        "<ENVELOPE><HEADER><STATUS>1</STATUS></HEADER><BODY><DATA><CMPINFO><LEDGER>107</LEDGER></CMPINFO><COLLECTION>{}{}{}{}</COLLECTION></DATA></BODY></ENVELOPE>",
        ledger("Input CGST", 1, "GST", Some("CGST")),
        ledger("Input SGST", 2, "GST", Some("SGST")),
        ledger("GST Head Absent", 3, "GST", Some("")),
        ledger("Non-tax ledger", 4, "Others", Some("")),
    )
}

fn captured_partial_alter_ledger(name: &str) -> String {
    let capture = include_str!(
        "../crates/bridge-tally-protocol/tests/fixtures/native/master_fields_lab_partial_alter_after.response.xml"
    );
    let start_tag = format!(r#"<LEDGER NAME="{name}" RESERVEDNAME="">"#);
    let start = capture
        .find(&start_tag)
        .expect("captured partial-alter response contains the expected ledger");
    let end = start
        + capture[start..]
            .find("</LEDGER>")
            .expect("captured ledger closes")
        + "</LEDGER>".len();
    capture[start..end].to_string()
}

fn native_party_master_collection(fields: &str) -> String {
    format!(
        "<ENVELOPE><HEADER><STATUS>1</STATUS></HEADER><BODY><DATA><COLLECTION><LEDGER NAME=\"Captured partial-alter ledger\" RESERVEDNAME=\"\"><GUID>{COMPANY_GUID}-000000ce</GUID><BRIDGECOMPANYGUID>{COMPANY_GUID}</BRIDGECOMPANYGUID><MASTERID>206</MASTERID><ALTERID>208</ALTERID><PARENT>Sundry Debtors</PARENT>{fields}<OPENINGBALANCE>0.00</OPENINGBALANCE></LEDGER></COLLECTION></DATA></BODY></ENVELOPE>"
    )
}

#[test]
fn party_ledger_master_request_fetches_tax_type_and_gst_duty_head() {
    let period = NativeLedgerExportPeriod::new(
        DateBoundaryProfile::ModeAgnostic,
        TallyDate::parse("20260401").unwrap(),
        TallyDate::parse("20260910").unwrap(),
    )
    .unwrap();

    let request = render_party_ledger_master_request("BRIDGE GST RECON LAB", &period);
    assert!(request.contains("TAXTYPE, GSTDUTYHEAD"));
    assert!(request.contains(", LEDGSTREGDETAILS.LIST</FETCH>"));
}

#[test]
fn compliance_ledger_duty_heads_preserve_raw_values_and_name_attributes() {
    let parsed = parse_native_party_ledger_master_records_with_evidence(
        &measured_ledger_master_response(),
        COMPANY_GUID,
    )
    .expect("measured duty-head response shape parses");

    assert_eq!(parsed.records.len(), 4, "CMPINFO ledger count is not a row");
    let cgst = &parsed.records[0].record;
    assert_eq!(
        cgst.ledger.name, "Input CGST",
        "identity is the NAME attribute"
    );
    assert_eq!(
        cgst.fields.tax_type,
        PartyLedgerMasterFieldObservation::Returned("GST".to_string())
    );
    assert_eq!(
        cgst.fields.gst_duty_head,
        GstDutyHeadObservation::Recognized {
            raw: "CGST".to_string(),
            head: GstDutyHead::Cgst,
        }
    );
    assert_eq!(
        parsed.records[1].record.fields.gst_duty_head,
        GstDutyHeadObservation::Unrecognized {
            raw: "SGST".to_string(),
        },
        "unmeasured SGST spelling must not be normalized into State Tax"
    );
    assert_eq!(
        parsed.records[2].record.fields.gst_duty_head,
        GstDutyHeadObservation::Absent,
        "a GST ledger with no returned head is not a default tax head"
    );
    assert_eq!(
        parsed.records[3].record.fields.gst_duty_head,
        GstDutyHeadObservation::NotTaxLedger {
            tax_type: "Others".to_string(),
        },
        "a non-GST ledger remains distinct from an absent GST duty head"
    );

    let compliance = serde_json::to_value(&cgst.fields).unwrap();
    assert_eq!(compliance["tax_type"], "GST");
    assert_eq!(compliance["gst_duty_head"]["observation"], "recognized");
    assert_eq!(compliance["gst_duty_head"]["raw"], "CGST");
    assert_eq!(compliance["gst_duty_head"]["head"], "cgst");
}

#[test]
fn captured_empty_duty_head_uses_tax_type_classification() {
    let ledger = captured_partial_alter_ledger("BRIDGE MFLAB PARTIAL ALTER PROBE");
    let fields = ["<TAXTYPE>Others</TAXTYPE>", "<GSTDUTYHEAD/>"]
        .into_iter()
        .map(|field| {
            let start = ledger
                .find(field)
                .expect("the real capture contains the expected duty-head field shape");
            &ledger[start..start + field.len()]
        })
        .collect::<String>();

    let parsed = parse_native_party_ledger_master_records_with_evidence(
        &native_party_master_collection(&fields),
        COMPANY_GUID,
    )
    .expect("the captured duty-head fields parse in the collection profile");
    assert_eq!(parsed.records.len(), 1);
    assert_eq!(
        parsed.records[0].record.fields.gst_duty_head,
        GstDutyHeadObservation::NotTaxLedger {
            tax_type: "Others".to_string(),
        }
    );
}

fn captured_live_ledger_masters() -> String {
    let bytes = include_bytes!(
        "../crates/bridge-tally-protocol/tests/fixtures/agent/native-ledger-masters-duty-heads.utf16le.xml"
    );
    String::from_utf16(
        &bytes
            .chunks_exact(2)
            .map(|pair| u16::from_le_bytes([pair[0], pair[1]]))
            .collect::<Vec<_>>(),
    )
    .unwrap()
}

#[test]
fn nested_markup_in_a_classification_scalar_is_refused() {
    // <GSTDUTYHEAD><VALUE>CGST</VALUE></GSTDUTYHEAD> flattened to "CGST" and was
    // released as a recognised duty head. A field that only ever carries a scalar,
    // and whose value drives a classification, must fail at the boundary on an
    // unexpected shape rather than become compliance data.
    for (field, nested) in [
        (
            "GSTDUTYHEAD",
            "<GSTDUTYHEAD><VALUE>CGST</VALUE></GSTDUTYHEAD>",
        ),
        ("TAXTYPE", "<TAXTYPE><VALUE>GST</VALUE></TAXTYPE>"),
    ] {
        let response = format!(
            "<ENVELOPE><HEADER><STATUS>1</STATUS></HEADER><BODY><DATA><CMPINFO><LEDGER>1</LEDGER></CMPINFO><COLLECTION><LEDGER NAME=\"Nested\" RESERVEDNAME=\"\"><GUID>{COMPANY_GUID}-00000044</GUID><BRIDGECOMPANYGUID>{COMPANY_GUID}</BRIDGECOMPANYGUID><MASTERID>68</MASTERID><ALTERID>68</ALTERID><PARENT>Duties &amp; Taxes</PARENT>{nested}<OPENINGBALANCE>0.00</OPENINGBALANCE><LANGUAGENAME.LIST><NAME.LIST><NAME>Localized Nested</NAME></NAME.LIST></LANGUAGENAME.LIST></LEDGER></COLLECTION></DATA></BODY></ENVELOPE>"
        );
        assert!(
            parse_native_party_ledger_master_records_with_evidence(&response, COMPANY_GUID)
                .is_err(),
            "nested markup in {field} must be refused, not flattened"
        );
    }
}

#[test]
fn a_duty_head_on_a_non_gst_ledger_is_contradictory_not_recognised() {
    // <TAXTYPE>Others</TAXTYPE><GSTDUTYHEAD>CGST</GSTDUTYHEAD> is a response
    // contradicting itself. Classifying head-first recognised it and never
    // consulted TAXTYPE, releasing the contradiction as valid compliance data.
    // Neither field is now asserted, and both raw values are kept so a
    // reviewer can see what Tally actually returned.
    let response = format!(
        "<ENVELOPE><HEADER><STATUS>1</STATUS></HEADER><BODY><DATA><CMPINFO><LEDGER>1</LEDGER></CMPINFO><COLLECTION><LEDGER NAME=\"Contradictory\" RESERVEDNAME=\"\"><GUID>{COMPANY_GUID}-00000042</GUID><BRIDGECOMPANYGUID>{COMPANY_GUID}</BRIDGECOMPANYGUID><MASTERID>66</MASTERID><ALTERID>66</ALTERID><PARENT>Duties &amp; Taxes</PARENT><TAXTYPE>Others</TAXTYPE><GSTDUTYHEAD>CGST</GSTDUTYHEAD><OPENINGBALANCE>0.00</OPENINGBALANCE><LANGUAGENAME.LIST><NAME.LIST><NAME>Localized Contradictory</NAME></NAME.LIST></LANGUAGENAME.LIST></LEDGER></COLLECTION></DATA></BODY></ENVELOPE>"
    );
    let parsed = parse_native_party_ledger_master_records_with_evidence(&response, COMPANY_GUID)
        .expect("a contradictory response still parses; it is classified, not refused");
    assert_eq!(
        parsed.records[0].record.fields.gst_duty_head,
        GstDutyHeadObservation::Contradictory {
            tax_type: "Others".to_string(),
            raw: "CGST".to_string(),
        }
    );
}

#[test]
fn an_unobserved_tax_type_does_not_contradict_a_duty_head() {
    // Scoped deliberately. An absent or empty TAXTYPE is not evidence that the
    // ledger is non-GST, so it must not block recognition -- that would refuse
    // real GST ledgers on any Tally version that omits the field. Only an
    // OBSERVED non-GST value contradicts.
    let response = format!(
        "<ENVELOPE><HEADER><STATUS>1</STATUS></HEADER><BODY><DATA><CMPINFO><LEDGER>1</LEDGER></CMPINFO><COLLECTION><LEDGER NAME=\"NoTaxType\" RESERVEDNAME=\"\"><GUID>{COMPANY_GUID}-00000043</GUID><BRIDGECOMPANYGUID>{COMPANY_GUID}</BRIDGECOMPANYGUID><MASTERID>67</MASTERID><ALTERID>67</ALTERID><PARENT>Duties &amp; Taxes</PARENT><GSTDUTYHEAD>IGST</GSTDUTYHEAD><OPENINGBALANCE>0.00</OPENINGBALANCE><LANGUAGENAME.LIST><NAME.LIST><NAME>Localized NoTaxType</NAME></NAME.LIST></LANGUAGENAME.LIST></LEDGER></COLLECTION></DATA></BODY></ENVELOPE>"
    );
    let parsed = parse_native_party_ledger_master_records_with_evidence(&response, COMPANY_GUID)
        .expect("parses");
    assert_eq!(
        parsed.records[0].record.fields.gst_duty_head,
        GstDutyHeadObservation::Recognized {
            raw: "IGST".to_string(),
            head: GstDutyHead::Igst,
        }
    );
}

#[test]
fn live_capture_backs_the_recognised_duty_head_vocabulary() {
    // Captured from a live TallyPrime 7.1 Silver response to
    // render_party_ledger_master_request, so the vocabulary is checked against
    // bytes Tally actually sent rather than against a fixture written from the
    // same understanding as the parser. A hand-authored response can encode a
    // wrong vocabulary in both places and agree with itself.
    //
    // All FIVE recognised spellings are now covered. UT Tax and Cess were
    // absent from this book, so two ledgers carrying those heads were created
    // and the capture retaken -- they are no longer accepted on the strength of
    // a hand-written table alone.
    let parsed = parse_native_party_ledger_master_records_with_evidence(
        &captured_live_ledger_masters(),
        "ae1490be-52c5-4544-9ffc-4b7da85f9797",
    )
    .expect("live ledger-master capture parses");

    let mut recognised: Vec<(String, GstDutyHead)> = parsed
        .records
        .iter()
        .filter_map(|row| match &row.record.fields.gst_duty_head {
            GstDutyHeadObservation::Recognized { raw, head } => Some((raw.clone(), *head)),
            _ => None,
        })
        .collect();
    recognised.sort_by(|left, right| left.0.cmp(&right.0));
    recognised.dedup();

    assert_eq!(
        recognised,
        vec![
            ("CGST".to_string(), GstDutyHead::Cgst),
            ("Cess".to_string(), GstDutyHead::Cess),
            ("IGST".to_string(), GstDutyHead::Igst),
            ("State Tax".to_string(), GstDutyHead::StateTax),
            ("UT Tax".to_string(), GstDutyHead::UtTax),
        ],
        "EVERY recognised spelling must be backed by bytes Tally actually sent"
    );

    // The state head really is spelled `State Tax` on the wire, not `SGST`.
    // That irregularity is the reason this vocabulary is enumerated at all.
    let capture = captured_live_ledger_masters();
    assert!(capture.contains(">State Tax</GSTDUTYHEAD>"));
    assert!(!capture.contains(">SGST</GSTDUTYHEAD>"));

    // This instance OMITS the element for non-GST ledgers rather than emitting
    // it empty, so the captured empty-element shape is exercised separately by
    // captured_empty_duty_head_uses_tax_type_classification.
    assert!(!capture.contains("<GSTDUTYHEAD/>"));
    assert!(
        parsed.records.iter().any(|row| matches!(
            row.record.fields.gst_duty_head,
            GstDutyHeadObservation::NotTaxLedger { .. }
        )),
        "the same capture must also carry ordinary non-tax ledgers"
    );
}

#[test]
fn a_gstin_held_only_in_the_dated_registration_history_is_reported_in_force() {
    // bridge#624, over a live TallyPrime 7.1 Silver capture of the request this
    // tool sends: party A's GSTIN is only in its second dated entry, and the
    // flat PARTYGSTIN is empty. Before the fix A read as `party_gstin: null`.
    let period = NativeLedgerExportPeriod::new(
        DateBoundaryProfile::ModeAgnostic,
        TallyDate::parse("20250401").unwrap(),
        TallyDate::parse("20250401").unwrap(),
    )
    .unwrap();
    assert_eq!(
        sha256_hex(render_party_ledger_master_request("BRIDGE READS LAB", &period).as_bytes()),
        "c2f9f8e9fefab44077b222402254a2be3dfd9a9690d4e577efbae99d531c819c",
        "the fixture answers exactly the request ledger_masters sends"
    );
    let bytes = include_bytes!(
        "../crates/bridge-tally-protocol/tests/fixtures/agent/native-party-masters-gst-registrations.utf16le.xml"
    );
    let capture = String::from_utf16(
        &bytes
            .chunks_exact(2)
            .map(|pair| u16::from_le_bytes([pair[0], pair[1]]))
            .collect::<Vec<_>>(),
    )
    .unwrap();
    let parsed = parse_native_party_ledger_master_records_with_evidence(
        &capture,
        "de2e15f2-6d42-4715-b6e7-b7a95a68abe8",
    )
    .expect("the live registration-history capture parses");
    let answer = |name: &str, as_of: &str| {
        let row = parsed
            .records
            .iter()
            .find(|row| row.record.ledger.name == name)
            .unwrap_or_else(|| panic!("{name} is in the capture"));
        let gstin = party_gstin_on(
            row.record.ledger.party_gstin.returned_text(),
            &row.record.fields.gst_registrations,
            as_of,
        );
        assert!(
            !gstin.sources_disagree,
            "{name}: the capture's sources agree"
        );
        (
            gstin.gstin,
            gstin.status,
            gstin.registration_type,
            gstin.flat,
        )
    };
    let text = |value: &str| Some(value.to_string());
    assert_eq!(
        answer("G1 Party A Later GSTIN", "20250930"),
        (text("27ZZZZZ0000Z1Z5"), "in_force", text("Regular"), None)
    );
    assert_eq!(
        answer("G1 Party A Later GSTIN", "20250630"),
        (
            None,
            "no_gstin_in_force",
            text("Unregistered/Consumer"),
            None
        ),
        "the first entry is dated but carries no GSTIN"
    );
    assert_eq!(
        answer("G1 Party B Flat GSTIN", "20250930"),
        (
            text("29ZZZZZ0000Z1Z5"),
            "flat_field",
            None,
            text("29ZZZZZ0000Z1Z5")
        ),
        "Tally returns an empty placeholder list beside the flat field"
    );
    assert_eq!(
        answer("G1 Party C Unregistered", "20250930"),
        (None, "not_reported", None, None)
    );
    assert_eq!(
        answer("G1 Party D Two GSTINs", "20250930"),
        (text("27ZZZZZ0000Z1Z5"), "in_force", text("Regular"), None)
    );
    assert_eq!(
        answer("G1 Party D Two GSTINs", "20251001"),
        (text("29ZZZZZ0000Z1Z5"), "in_force", text("Regular"), None),
        "the later registration applies from its own date"
    );
}

#[test]
fn both_gstin_sources_are_reported_and_a_difference_is_flagged_not_resolved() {
    use bridge_tally_protocol::gst_registration::GstRegistrationEntry;
    let entry = |date: &str, gstin: Option<&str>, kind: &str| GstRegistrationEntry {
        applicable_from: date.to_string(),
        gstin: gstin.map(str::to_string),
        registration_type: Some(kind.to_string()),
    };
    let history = |entries| GstRegistrationHistory::Entries { entries };
    const FLAT: &str = "27ZZZZZ0000Z1Z5";
    const DATED: &str = "29ZZZZZ0000Z1Z5";
    let text = |value: &str| Some(value.to_string());

    // The flat field names a GSTIN; the history says none on that date.
    let unregistered = history(vec![entry("20170701", None, "Unregistered/Consumer")]);
    let got = party_gstin_on(Some(FLAT), &unregistered, "20260331");
    assert_eq!((got.gstin, got.status), (None, "no_gstin_in_force"));
    assert_eq!(got.flat, text(FLAT), "the flat GSTIN is still reported");
    assert!(got.sources_disagree);

    // The two sources name different GSTINs.
    let other = history(vec![entry("20170701", Some(DATED), "Regular")]);
    let got = party_gstin_on(Some(FLAT), &other, "20260331");
    assert_eq!((got.gstin, got.status), (text(DATED), "in_force"));
    assert_eq!(got.flat, text(FLAT));
    assert!(got.sources_disagree);

    // Agreement is not a disagreement, and an absent flat field is not one.
    assert!(!party_gstin_on(Some(DATED), &other, "20260331").sources_disagree);
    assert!(!party_gstin_on(None, &other, "20260331").sources_disagree);

    // An explicitly empty flat field (`<PARTYGSTIN/>`, seen live) names no
    // GSTIN: it is reported as read but neither disagrees nor is used.
    let got = party_gstin_on(Some(""), &other, "20260331");
    assert_eq!((got.status, got.flat.as_deref()), ("in_force", Some("")));
    assert!(!got.sources_disagree);
    let got = party_gstin_on(Some(""), &unregistered, "20260331");
    assert!(!got.sources_disagree);
    let got = party_gstin_on(Some(""), &history(vec![]), "20260331");
    assert_eq!((got.gstin, got.status), (None, "not_reported"));

    // A history that starts after the date names nothing yet.
    let future = history(vec![entry("20270401", Some(DATED), "Regular")]);
    let got = party_gstin_on(None, &future, "20260331");
    assert_eq!(
        (got.gstin, got.status, got.registration_type),
        (None, "no_gstin_in_force", None)
    );

    // Every source is reported under its own key.
    let fields = party_gstin_fields(party_gstin_on(Some(FLAT), &other, "20260331"), "20260331");
    assert_eq!(
        Value::Object(fields),
        json!({
            "party_gstin": DATED,
            "party_gstin_status": "in_force",
            "party_gstin_registration_type": "Regular",
            "party_gstin_as_of": "20260331",
            "party_gstin_flat": FLAT,
            "gstin_sources_disagree": true,
        })
    );

    // Registered with no GSTIN recorded is not read as unregistered.
    let regular = history(vec![entry("20170701", None, "Regular")]);
    let got = party_gstin_on(None, &regular, "20260331");
    assert_eq!(
        (got.status, got.registration_type),
        ("no_gstin_in_force", text("Regular"))
    );

    // An unreadable history never falls back to the flat field.
    let unreadable = GstRegistrationHistory::Unreadable {
        defect: bridge_tally_protocol::gst_registration::GstRegistrationDefect::DateInvalid,
    };
    let got = party_gstin_on(Some(FLAT), &unreadable, "20260331");
    assert_eq!((got.gstin, got.status), (None, "history_unreadable"));
    assert_eq!(got.flat, text(FLAT), "reported as read, not used");
    assert!(!got.sources_disagree, "nothing readable to compare");
}

#[test]
fn a_repeated_registration_field_fails_its_own_ledger_not_the_read() {
    let ledger = |name: &str, id: u8, registrations: &str| {
        format!(
            "<LEDGER NAME=\"{name}\" RESERVEDNAME=\"\"><GUID>{COMPANY_GUID}-000000{id:02x}</GUID><BRIDGECOMPANYGUID>{COMPANY_GUID}</BRIDGECOMPANYGUID><MASTERID>{id}</MASTERID><ALTERID>{id}</ALTERID><PARENT>Sundry Creditors</PARENT>{registrations}<OPENINGBALANCE>0.00</OPENINGBALANCE></LEDGER>"
        )
    };
    let repeated = |gstins: &str| {
        format!("<LEDGSTREGDETAILS.LIST><APPLICABLEFROM TYPE=\"Date\">20250401</APPLICABLEFROM>{gstins}</LEDGSTREGDETAILS.LIST>")
    };
    let response = format!(
        "<ENVELOPE><HEADER><STATUS>1</STATUS></HEADER><BODY><DATA><COLLECTION>{}{}{}{}</COLLECTION></DATA></BODY></ENVELOPE>",
        ledger(
            "Repeated",
            1,
            &repeated("<GSTIN>27ZZZZZ0000Z1Z5</GSTIN><GSTIN>29ZZZZZ0000Z1Z5</GSTIN>")
        ),
        ledger("Empty element", 2, "<LEDGSTREGDETAILS.LIST/>"),
        ledger("Empty then value", 3, &repeated("<GSTIN></GSTIN><GSTIN>29ZZZZZ0000Z1Z5</GSTIN>")),
        ledger("Self-closing then value", 4, &repeated("<GSTIN/><GSTIN>29ZZZZZ0000Z1Z5</GSTIN>")),
    );
    let parsed = parse_native_party_ledger_master_records_with_evidence(&response, COMPANY_GUID)
        .expect("one ledger's defect does not refuse the book");
    let histories = parsed
        .records
        .iter()
        .map(|row| serde_json::to_value(&row.record.fields.gst_registrations).unwrap())
        .collect::<Vec<_>>();
    assert_eq!(
        histories,
        vec![
            json!({"observation": "unreadable", "defect": "entry_repeats_a_field"}),
            json!({"observation": "entries", "entries": []}),
            json!({"observation": "unreadable", "defect": "entry_repeats_a_field"}),
            json!({"observation": "unreadable", "defect": "entry_repeats_a_field"}),
        ],
        "a field seen twice is a repeat even when the first carried no text"
    );
}

#[test]
fn gst_duty_head_vocabulary_is_explicit_and_irregular() {
    for (raw, head) in [
        ("CGST", GstDutyHead::Cgst),
        ("IGST", GstDutyHead::Igst),
        ("State Tax", GstDutyHead::StateTax),
        ("UT Tax", GstDutyHead::UtTax),
        ("Cess", GstDutyHead::Cess),
    ] {
        assert_eq!(
            GstDutyHeadObservation::from_observations(
                &PartyLedgerMasterFieldObservation::Returned("GST".to_string()),
                &PartyLedgerMasterFieldObservation::Returned(raw.to_string()),
            ),
            GstDutyHeadObservation::Recognized {
                raw: raw.to_string(),
                head,
            }
        );
    }
    for raw in [
        "SGST",
        "Central Tax",
        "Integrated Tax",
        "Central",
        "State",
        "Integrated",
        "Union Territory Tax",
    ] {
        assert_eq!(
            GstDutyHeadObservation::from_observations(
                &PartyLedgerMasterFieldObservation::Returned("GST".to_string()),
                &PartyLedgerMasterFieldObservation::Returned(raw.to_string()),
            ),
            GstDutyHeadObservation::Unrecognized {
                raw: raw.to_string(),
            }
        );
    }
}

// -- ledger_masters ancestry exposure ---------------------------------------
//
// GroupIndex::ancestry_chain itself is proven (including by mutation) in
// bridge_tally_protocol::group_ancestry's own test module. These cover the
// wire-shaping layer this file owns: rendering a chain to JSON, mapping every
// AncestryGap to a stable wire string, and the group/group_scope filter that
// now reads that chain.

fn group(
    name: &str,
    parent: &str,
    reserved: Option<&str>,
) -> bridge_tally_protocol::TallyNamedMaster {
    bridge_tally_protocol::TallyNamedMaster {
        name: name.into(),
        parent: PartyLedgerMasterFieldObservation::Returned(parent.into()),
        reserved_name: reserved.map(str::to_string),
    }
}

/// Mirrors the lab's own tree: `HDFC CC` under `Bank OD A/c` under `Loans
/// (Liability)`, and `Cash` under `Cash-in-Hand`, both reaching the reserved
/// root directly.
fn lab_shaped_groups() -> bridge_tally_protocol::group_ancestry::GroupIndex {
    bridge_tally_protocol::group_ancestry::GroupIndex::build([
        group("Bank OD A/c", "Loans (Liability)", Some("Bank OD A/c")),
        group(
            "Loans (Liability)",
            "\u{fffd}#4; Primary",
            Some("Loans (Liability)"),
        ),
        group("Cash-in-Hand", "\u{fffd}#4; Primary", Some("Cash-in-Hand")),
    ])
}

#[test]
fn ancestry_json_renders_a_complete_multi_level_chain() {
    let chain = lab_shaped_groups().ancestry_chain(Some("Bank OD A/c"));
    let rendered = ancestry_json(&chain);
    assert_eq!(rendered["complete"], true);
    assert_eq!(rendered["gap"], Value::Null);
    assert_eq!(
        rendered["chain"],
        json!([
            {"name": "Bank OD A/c", "reserved_name": "Bank OD A/c"},
            {"name": "Loans (Liability)", "reserved_name": "Loans (Liability)"},
        ])
    );
}

#[test]
fn ancestry_json_renders_a_single_hop_directly_under_a_primary_group() {
    let chain = lab_shaped_groups().ancestry_chain(Some("Cash-in-Hand"));
    let rendered = ancestry_json(&chain);
    assert_eq!(rendered["complete"], true);
    assert_eq!(rendered["gap"], Value::Null);
    assert_eq!(
        rendered["chain"],
        json!([{"name": "Cash-in-Hand", "reserved_name": "Cash-in-Hand"}])
    );
}

#[test]
fn ancestry_json_reports_an_incomplete_chain_without_padding_or_guessing() {
    // "Bank OD A/c" is present but its own parent "Loans (Liability)" is not
    // in this narrower index, so the resolved prefix must stop exactly there.
    let narrow = bridge_tally_protocol::group_ancestry::GroupIndex::build([group(
        "Bank OD A/c",
        "Loans (Liability)",
        Some("Bank OD A/c"),
    )]);
    let chain = narrow.ancestry_chain(Some("Bank OD A/c"));
    let rendered = ancestry_json(&chain);
    assert_eq!(rendered["complete"], false);
    assert_eq!(rendered["gap"], "group_absent");
    assert_eq!(
        rendered["chain"],
        json!([{"name": "Bank OD A/c", "reserved_name": "Bank OD A/c"}]),
        "the one hop actually resolved must still be reported, not dropped"
    );
}

#[test]
fn every_ancestry_gap_has_a_distinct_stable_wire_code() {
    let mut codes = std::collections::BTreeSet::new();
    for gap in [
        AncestryGap::NoParent,
        AncestryGap::ReachedRoot,
        AncestryGap::GroupAbsent,
        AncestryGap::GroupNameRepeated,
        AncestryGap::ReservedNameMissing,
        AncestryGap::Cycle,
        AncestryGap::Exhausted,
    ] {
        assert!(
            codes.insert(ancestry_gap_code(gap)),
            "{gap:?} must render to a code no other gap also uses"
        );
    }
}

#[test]
fn group_scope_defaults_to_immediate_and_rejects_an_unknown_value() {
    assert_eq!(group_scope(&json!({})), Ok(GroupScope::Immediate));
    assert_eq!(
        group_scope(&json!({"group_scope": "immediate"})),
        Ok(GroupScope::Immediate)
    );
    assert_eq!(
        group_scope(&json!({"group_scope": "ancestry"})),
        Ok(GroupScope::Ancestry)
    );
    assert_eq!(
        group_scope(&json!({"group_scope": "everything"})),
        Err("argument_invalid:group_scope".to_string())
    );
}

#[test]
fn immediate_scope_never_reaches_past_the_ledgers_own_parent() {
    let index = lab_shaped_groups();
    let chain = index.ancestry_chain(Some("Bank OD A/c"));
    let hop_names = chain
        .hops
        .iter()
        .map(|hop| hop.name.clone())
        .collect::<Vec<_>>();
    assert!(hop_names.contains(&"Loans (Liability)".to_string()));
    // The immediate parent itself still matches under either scope.
    assert!(group_matches(
        GroupScope::Immediate,
        "Bank OD A/c",
        Some("Bank OD A/c"),
        &hop_names
    ));
    // But the original tool behaviour is preserved: a deeper ancestor is
    // invisible to Immediate, exactly as it always was.
    assert!(!group_matches(
        GroupScope::Immediate,
        "Loans (Liability)",
        Some("Bank OD A/c"),
        &hop_names
    ));
}

#[test]
fn ancestry_scope_matches_any_hop_but_never_an_unresolved_tail() {
    let index = lab_shaped_groups();
    let chain = index.ancestry_chain(Some("Bank OD A/c"));
    let hop_names = chain
        .hops
        .iter()
        .map(|hop| hop.name.clone())
        .collect::<Vec<_>>();
    assert!(group_matches(
        GroupScope::Ancestry,
        "Loans (Liability)",
        Some("Bank OD A/c"),
        &hop_names
    ));
    // A name that is not anywhere in the resolved chain must not match --
    // ancestry scope broadens what counts as a hit, it never invents one.
    assert!(!group_matches(
        GroupScope::Ancestry,
        "Sundry Debtors",
        Some("Bank OD A/c"),
        &hop_names
    ));

    // A ledger whose ancestry has a gap must never match a name that only
    // the unresolved tail could have reached: the resolved prefix is all
    // `group_matches` is given, and it must not be treated as the full chain.
    let narrow = bridge_tally_protocol::group_ancestry::GroupIndex::build([group(
        "Bank OD A/c",
        "Loans (Liability)",
        Some("Bank OD A/c"),
    )]);
    let gapped = narrow.ancestry_chain(Some("Bank OD A/c"));
    let gapped_names = gapped
        .hops
        .iter()
        .map(|hop| hop.name.clone())
        .collect::<Vec<_>>();
    assert!(!gapped.is_complete());
    assert!(!group_matches(
        GroupScope::Ancestry,
        "Loans (Liability)",
        Some("Bank OD A/c"),
        &gapped_names
    ));
}

/// The filter report is not paged with the rows, so its sub-group list is
/// bounded where it is built; the counts still cover every row.
#[test]
fn a_filter_report_names_at_most_twenty_sub_groups_and_counts_them_all() {
    let mut groups = vec![group(
        "Sundry Debtors",
        "\u{fffd}#4; Primary",
        Some("Sundry Debtors"),
    )];
    let mut rows = Vec::new();
    for n in 0..25 {
        let name = format!("Debtor Group {n:02}");
        groups.push(group(&name, "Sundry Debtors", Some("")));
        rows.push(json!({"name": format!("Ledger {n:02}"), "parent": name}));
    }
    // A second ledger in one sub-group: ledgers and sub-groups are counted
    // separately.
    rows.push(json!({"name": "Ledger 00b", "parent": "Debtor Group 00"}));
    let report = apply_group_filter(
        &mut rows,
        GroupScope::Immediate,
        "Sundry Debtors",
        &bridge_tally_protocol::group_ancestry::GroupIndex::build(groups),
    );
    assert!(rows.is_empty());
    let excluded = &report["excluded_subgroup_ledgers"];
    assert_eq!(excluded["count"], 26);
    assert_eq!(excluded["group_count"], 25);
    let named = excluded["groups"].as_array().unwrap();
    assert_eq!(named.len(), 20);
    assert_eq!(named[0], "Debtor Group 00");
    assert_eq!(report["unresolved_ancestry_ledgers"], 0);
}

// -- ledger_masters ancestry through the tool call ---------------------------
//
// The tests above call the helpers directly, so they stay green if
// `Server::ledger_masters` stops calling them: an inverted or deleted
// refusal, the wrong collection handed to `GroupIndex::build`, or a `retain`
// that no longer applies. These drive `call_tool("ledger_masters", ..)` over
// a replayed Tally sequence built from the captured party-master fixtures.

mod through_the_tool {
    use super::*;
    use tally_protocol_simulator::{
        Fixture, ProductStatus, ScenarioPlan, SequenceSimulator, WireEncoding,
    };

    const GUID: &str = "61c6de69-1748-461c-ad3f-162cb949df9f";

    fn captured(bytes: &[u8]) -> String {
        String::from_utf16(
            &bytes
                .chunks_exact(2)
                .map(|pair| u16::from_le_bytes([pair[0], pair[1]]))
                .collect::<Vec<_>>(),
        )
        .unwrap()
    }

    fn companies() -> String {
        captured(include_bytes!(
            "../crates/bridge-tally-protocol/tests/fixtures/agent/native-licensed-release-companies.utf16le.xml"
        ))
    }

    fn masters() -> String {
        captured(include_bytes!(
            "../crates/bridge-tally-protocol/tests/fixtures/agent/native-party-masters.utf16le.xml"
        ))
    }

    fn balances() -> String {
        captured(include_bytes!(
            "../crates/bridge-tally-protocol/tests/fixtures/agent/native-party-balances.utf16le.xml"
        ))
    }

    fn groups() -> String {
        captured(include_bytes!(
            "../crates/bridge-tally-protocol/tests/fixtures/agent/native-party-groups.utf16le.xml"
        ))
    }

    fn xml(body: String) -> ScenarioPlan {
        ScenarioPlan::new(Fixture::SyntheticXml(body)).with_encoding(WireEncoding::Utf16Le)
    }

    fn status() -> ScenarioPlan {
        ScenarioPlan::new(Fixture::ProductStatus(ProductStatus::TallyPrime))
    }

    fn pair(plans: &mut Vec<ScenarioPlan>, source: ScenarioPlan) {
        plans.extend([source.clone(), status(), source, status()]);
    }

    /// The paired company-identity read every tool call starts with.
    fn identity_plans() -> Vec<ScenarioPlan> {
        let mut plans = Vec::new();
        pair(&mut plans, xml(companies()));
        plans
    }

    /// The whole successful `fields=compliance` sequence: identity, the
    /// extent-bracketed currency read, then the profile probe and the
    /// extent-bracketed master/balance/group triple.
    fn compliance_plans(masters: String, balances: String) -> Vec<ScenarioPlan> {
        let company = xml(companies());
        let extent = xml(include_str!(
            "../crates/bridge-tally-protocol/tests/fixtures/agent/native-company-book-extents-with-number.utf8.xml"
        )
        .to_owned());
        let currency = xml(captured(include_bytes!(
            "../crates/bridge-tally-protocol/tests/fixtures/currency_inr_modern_live.utf16le.xml"
        )));
        let mut plans = identity_plans();
        plans.push(company.clone());
        pair(&mut plans, extent.clone());
        pair(&mut plans, currency);
        pair(&mut plans, extent.clone());
        plans.push(company.clone());
        plans.extend([status(), company.clone(), company.clone()]);
        pair(&mut plans, extent.clone());
        for source in [masters, balances, groups()] {
            pair(&mut plans, xml(source));
        }
        pair(&mut plans, extent);
        plans.extend([company.clone(), status(), company]);
        plans
    }

    // -- #637: the compliance read is sized before its master request -------

    /// The captured extents with only this company's master mark (`ALTMSTID`)
    /// changed. The same text serves every extent read of the call, so the
    /// brackets stay equal unless a test changes the closing one.
    fn extent_with_master_mark(mark: u64) -> String {
        let extent = include_str!(
            "../crates/bridge-tally-protocol/tests/fixtures/agent/native-company-book-extents-with-number.utf8.xml"
        );
        let at = extent.find(GUID).expect("the captured company's extent");
        let start = extent[..at].rfind("<COMPANY ").unwrap();
        let end = at + extent[at..].find("</COMPANY>").unwrap();
        let from = "<ALTMSTID TYPE=\"Number\"> 219</ALTMSTID>";
        assert_eq!(extent[start..end].matches(from).count(), 1);
        format!(
            "{}{}{}",
            &extent[..start],
            extent[start..end].replace(
                from,
                &format!("<ALTMSTID TYPE=\"Number\"> {mark}</ALTMSTID>")
            ),
            &extent[end..]
        )
    }

    /// The compliance sequence on a book whose master mark is `mark`, with the
    /// source reads in the order given. `closing` is the source's closing
    /// extent; `None` ends the replay after the reads, for a refusal that sends
    /// nothing more.
    fn marked_compliance_plans(
        mark: u64,
        reads: Vec<String>,
        closing: Option<String>,
    ) -> Vec<ScenarioPlan> {
        let company = xml(companies());
        let extent = xml(extent_with_master_mark(mark));
        let currency = xml(captured(include_bytes!(
            "../crates/bridge-tally-protocol/tests/fixtures/currency_inr_modern_live.utf16le.xml"
        )));
        let mut plans = identity_plans();
        plans.push(company.clone());
        pair(&mut plans, extent.clone());
        pair(&mut plans, currency);
        pair(&mut plans, extent.clone());
        plans.push(company.clone());
        plans.extend([status(), company.clone(), company.clone()]);
        pair(&mut plans, extent);
        for source in reads {
            pair(&mut plans, xml(source));
        }
        if let Some(closing) = closing {
            pair(&mut plans, xml(closing));
            plans.extend([company.clone(), status(), company]);
        }
        plans
    }

    /// A master mark whose estimate is over the budget refuses right after the
    /// source's opening extent: no ledger, balance or group request is sent,
    /// and the refusal names the mark as an upper bound, not a ledger count.
    #[tokio::test]
    async fn a_book_whose_master_mark_is_over_the_bound_is_refused_before_any_ledger_read() {
        let plans = marked_compliance_plans(5_000, Vec::new(), None);
        let total = plans.len();
        let (response, requests) =
            call(plans, json!({"company_guid":GUID,"fields":"compliance"})).await;
        assert_eq!(requests, total, "nothing is sent after the opening extent");
        let error = refusal(&response);
        assert_eq!(error["code"], "party_ledger_master_read_failed");
        assert_eq!(error["cause"], "ledger_masters_too_large");
        assert_eq!(
            error["size"],
            json!({"master_alter_id": 5_000, "estimated_bytes": 18_750_000, "budget_bytes": 16_000_000})
        );
        let remediation = error["remediation"].as_str().unwrap();
        assert!(remediation.contains("UPPER BOUND"), "{error}");
        assert!(remediation.contains("fields=basic"), "{error}");
    }

    /// A mark exactly at the bound is admitted and read as it was before #637:
    /// the same three reads in the same order, and the same rows.
    #[tokio::test]
    async fn a_book_whose_master_mark_is_at_the_bound_reads_as_before() {
        let mark = 16_000_000 / 3_750;
        let plans = marked_compliance_plans(
            mark,
            vec![masters(), balances(), groups()],
            Some(extent_with_master_mark(mark)),
        );
        let total = plans.len();
        let (response, requests) =
            call(plans, json!({"company_guid":GUID,"fields":"compliance"})).await;
        assert_eq!(requests, total);
        let (unsized_response, _) = call(
            compliance_plans(masters(), balances()),
            json!({"company_guid":GUID,"fields":"compliance"}),
        )
        .await;
        assert_eq!(items(&response), items(&unsized_response));
    }

    /// The whole successful `fields=basic` sequence: identity, then the runtime's boundary
    /// probe, the extent-bracketed BOOKSFROM-pinned ledger export and the closing checks.
    fn basic_plans() -> Vec<ScenarioPlan> {
        basic_plans_reading(period_opening(), None)
    }

    fn period_opening() -> String {
        captured(include_bytes!(
            "../crates/bridge-tally-protocol/tests/fixtures/agent/native-period-opening.utf16le.xml"
        ))
    }

    /// The captured currency read of a book with one master (INR).
    fn single_currency() -> String {
        captured(include_bytes!(
            "../crates/bridge-tally-protocol/tests/fixtures/currency_inr_modern_live.utf16le.xml"
        ))
    }

    /// A basic read of a book with several Currency masters is refused after
    /// its currency read and before any ledger request: a bare opening names
    /// no currency, so a dollar ledger would read as rupees (#714).
    #[tokio::test]
    async fn a_basic_read_of_a_several_currency_book_is_refused_before_any_ledger() {
        let forex = "b14e9b2d-8a63-4779-804d-25d59eb787eb";
        let companies = xml(captured(include_bytes!(
            "../crates/bridge-tally-protocol/tests/fixtures/agent/native-licensed-release-companies.utf16le.xml"
        )));
        let extent = xml(captured(include_bytes!(
            "../crates/bridge-tally-protocol/tests/fixtures/company_extents_forex_live.utf16le.xml"
        )));
        let mut plans = Vec::new();
        pair(&mut plans, companies.clone());
        plans.extend([status(), companies.clone(), companies]);
        pair(&mut plans, extent);
        pair(
            &mut plans,
            xml(captured(include_bytes!(
                "../crates/bridge-tally-protocol/tests/fixtures/currency_multi_live.utf16le.xml"
            ))),
        );
        let total = plans.len();
        let (response, requests) = call(plans, json!({"company_guid":forex})).await;
        assert_eq!(requests, total, "no ledger request was sent");
        let error = refusal(&response);
        assert_eq!(error["code"], "ledger_export_invalid");
        assert_eq!(error["cause"], "company_several_currency_masters");
        let remediation = error["remediation"].as_str().unwrap();
        assert!(remediation.contains("#551"), "{error}");
    }

    /// A basic read whose currency collection holds no master is refused
    /// after it, before any ledger request: one master is not established.
    /// DERIVED from the captured single-master response with its one
    /// `CURRENCY` element removed (#714).
    #[tokio::test]
    async fn a_basic_read_with_no_currency_master_is_refused_before_any_ledger() {
        let captured_currency = single_currency();
        let start = captured_currency.find("<CURRENCY ").unwrap();
        let end =
            start + captured_currency[start..].find("</CURRENCY>").unwrap() + "</CURRENCY>".len();
        let mut none = captured_currency.clone();
        none.replace_range(start..end, "");
        assert!(!none.contains("<CURRENCY "), "no master left");
        let company = xml(companies());
        let extent = xml(include_str!(
            "../crates/bridge-tally-protocol/tests/fixtures/agent/native-company-book-extents-with-number.utf8.xml"
        )
        .to_owned());
        let mut plans = identity_plans();
        plans.extend([
            status(),
            company.clone(),
            company,
            extent.clone(),
            status(),
            extent,
            status(),
        ]);
        pair(&mut plans, xml(none));
        let total = plans.len();
        let (response, requests) = call(plans, json!({"company_guid":GUID})).await;
        assert_eq!(requests, total, "no ledger request was sent");
        let error = refusal(&response);
        assert_eq!(error["code"], "ledger_export_invalid");
        assert_eq!(error["cause"], "company_currency_probe_failed");
    }

    /// A basic read whose one Currency master is not INR is refused after
    /// the currency read, before any ledger request: its bare openings would
    /// name no currency (#716). DERIVED from the captured single-master
    /// response with its `MAILINGNAME` changed from `INR`; no non-INR book
    /// has been captured.
    #[tokio::test]
    async fn a_basic_read_of_a_non_inr_book_is_refused_before_any_ledger() {
        let captured_currency = single_currency();
        let inr = "<MAILINGNAME TYPE=\"String\">INR</MAILINGNAME>";
        assert_eq!(captured_currency.matches(inr).count(), 1);
        let foreign =
            captured_currency.replace(inr, "<MAILINGNAME TYPE=\"String\">UAE Dirham</MAILINGNAME>");
        let company = xml(companies());
        let extent = xml(include_str!(
            "../crates/bridge-tally-protocol/tests/fixtures/agent/native-company-book-extents-with-number.utf8.xml"
        )
        .to_owned());
        let mut plans = identity_plans();
        plans.extend([
            status(),
            company.clone(),
            company,
            extent.clone(),
            status(),
            extent,
            status(),
        ]);
        pair(&mut plans, xml(foreign));
        let total = plans.len();
        let (response, requests) = call(plans, json!({"company_guid":GUID})).await;
        assert_eq!(requests, total, "no ledger request was sent");
        let error = refusal(&response);
        assert_eq!(error["code"], "ledger_export_invalid");
        assert_eq!(error["cause"], "company_base_currency_not_inr");
    }

    /// As `basic_plans`, with the ledger export given and, when `groups` is
    /// supplied, the paired group collection a `group` filter adds inside the
    /// same extent and identity bracket.
    fn basic_plans_reading(ledgers: String, groups: Option<String>) -> Vec<ScenarioPlan> {
        let company = xml(companies());
        let extent = xml(include_str!(
            "../crates/bridge-tally-protocol/tests/fixtures/agent/native-company-book-extents-with-number.utf8.xml"
        )
        .to_owned());
        let mut plans = identity_plans();
        plans.extend([
            status(),
            company.clone(),
            company.clone(),
            extent.clone(),
            status(),
            extent.clone(),
            status(),
        ]);
        // The basic read proves the book keeps one Currency master (#714).
        pair(&mut plans, xml(single_currency()));
        pair(&mut plans, xml(ledgers));
        if let Some(groups) = groups {
            pair(&mut plans, xml(groups));
        }
        plans.extend([
            extent.clone(),
            status(),
            extent,
            status(),
            company.clone(),
            status(),
            company,
        ]);
        plans
    }

    /// The BOOKSFROM the captured extent admits for this company; both ledger_masters requests
    /// pin SVFROMDATE to it (see `ledger_movement_opening_export_is_pinned_to_admitted_books_from`).
    const ADMITTED_BOOKS_FROM: &str = "20260401";

    #[tokio::test]
    async fn basic_ledger_masters_rows_carry_their_opening_balance_as_of() {
        // Without a `group` filter the group collection is never read, whatever
        // the scope: the whole replayed sequence is the ledger export's, and
        // the result carries no filter report.
        for args in [
            json!({"company_guid":GUID,"fields":"basic"}),
            json!({"company_guid":GUID,"fields":"basic","group_scope":"ancestry"}),
        ] {
            let plans = basic_plans();
            let total = plans.len();
            let (response, requests) = call(plans, args.clone()).await;
            let rows = items(&response);
            assert_eq!(requests, total, "{args}");
            assert!(!rows.is_empty());
            for row in rows {
                assert_eq!(row["opening_balance_as_of"], ADMITTED_BOOKS_FROM, "{row}");
            }
            assert!(
                response["structuredContent"]["result"]
                    .get("group_filter")
                    .is_none(),
                "{response}"
            );
        }
    }

    #[tokio::test]
    async fn compliance_ledger_masters_rows_carry_their_opening_balance_as_of() {
        let (response, _) = call(
            compliance_plans(masters(), balances()),
            json!({"company_guid":GUID,"fields":"compliance"}),
        )
        .await;
        let rows = items(&response);
        assert!(!rows.is_empty());
        for row in rows {
            assert_eq!(row["opening_balance_as_of"], ADMITTED_BOOKS_FROM, "{row}");
            // This capture predates the registration-history FETCH, so every
            // row falls back to the flat field, and says so (bridge#624).
            let expected = if row["party_gstin"].is_null() {
                "not_reported"
            } else {
                "flat_field"
            };
            assert_eq!(row["party_gstin_status"], expected, "{row}");
            let flat = row["party_gstin_flat"]
                .as_str()
                .filter(|flat| !flat.is_empty());
            assert_eq!(flat, row["party_gstin"].as_str(), "{row}");
            assert_eq!(row["gstin_sources_disagree"], false, "{row}");
            assert!(row["party_gstin_registration_type"].is_null(), "{row}");
            assert_eq!(row["party_gstin_as_of"], tally_host_today(), "{row}");
            assert_eq!(
                row["compliance"]["gst_registrations"]["observation"], "not_observed",
                "{row}"
            );
        }
    }

    /// bridge#653: `as_of` sets the date every row's `party_gstin` is read as
    /// of, in either spelling. Which entry is in force on a date is pinned over
    /// the live registration-history capture by
    /// `a_gstin_held_only_in_the_dated_registration_history_is_reported_in_force`.
    #[tokio::test]
    async fn compliance_rows_read_their_gstin_as_of_the_date_given() {
        for as_of in ["20260331", "2026-03-31"] {
            let (response, _) = call(
                compliance_plans(masters(), balances()),
                json!({"company_guid":GUID,"fields":"compliance","as_of":as_of}),
            )
            .await;
            let rows = items(&response);
            assert!(!rows.is_empty());
            for row in rows {
                assert_eq!(row["party_gstin_as_of"], "20260331", "{row}");
                assert_eq!(row["opening_balance_as_of"], ADMITTED_BOOKS_FROM, "{row}");
            }
        }
    }

    /// `as_of` selects only the GSTIN, so a basic read refuses it before any
    /// request rather than returning rows a caller could take as dated by it.
    #[tokio::test]
    async fn as_of_without_compliance_fields_is_refused_before_any_request() {
        for args in [
            json!({"company_guid":GUID,"as_of":"20260331"}),
            json!({"company_guid":GUID,"fields":"basic","as_of":"20260331"}),
        ] {
            let (response, requests) = call_refused_before_any_request(args.clone()).await;
            assert_eq!(requests, 0, "{args}");
            let error = refusal(&response);
            assert_eq!(
                error["code"], "ledger_masters_as_of_requires_compliance",
                "{args}"
            );
            assert!(error["remediation"]
                .as_str()
                .is_some_and(|text| text.contains("fields=compliance")));
        }
    }

    #[tokio::test]
    async fn an_impossible_as_of_date_is_refused_before_any_request() {
        let (response, requests) = call_refused_before_any_request(
            json!({"company_guid":GUID,"fields":"compliance","as_of":"20260231"}),
        )
        .await;
        assert_eq!(requests, 0);
        assert_eq!(refusal(&response)["code"], "invalid_date", "{response}");
    }

    // -- #630: one read per logical listing ---------------------------------

    /// A continuation page's requests: the paired company identity read every
    /// call starts with, then the bracketed, paired extent read.
    fn continuation_plans(extent: String) -> Vec<ScenarioPlan> {
        let company = xml(companies());
        let mut plans = identity_plans();
        plans.push(company.clone());
        pair(&mut plans, xml(extent));
        plans.push(company);
        plans
    }

    /// A `fields=basic` first page on a book whose master mark is `mark`.
    fn basic_plans_marked(mark: u64) -> Vec<ScenarioPlan> {
        let captured_extent = include_str!(
            "../crates/bridge-tally-protocol/tests/fixtures/agent/native-company-book-extents-with-number.utf8.xml"
        );
        let marked = extent_with_master_mark(mark);
        basic_plans()
            .into_iter()
            .map(|plan| {
                if plan.fixture.body() == captured_extent {
                    xml(marked.clone())
                } else {
                    plan
                }
            })
            .collect()
    }

    /// One server over one replayed sequence, so a later call can be served
    /// from what an earlier call held.
    struct OneServer {
        simulator: SequenceSimulator,
        server: Server,
        _directory: tempfile::TempDir,
    }

    impl OneServer {
        fn spawn(plans: Vec<ScenarioPlan>) -> Self {
            let simulator = SequenceSimulator::spawn(plans).unwrap();
            let directory = tempfile::tempdir().unwrap();
            let server = Server::new(Settings {
                endpoint: TallyEndpointConfig {
                    host: "127.0.0.1".into(),
                    port: simulator.address().port(),
                },
                data_dir: directory.path().into(),
                max_rows: 500,
                max_bytes: 200_000,
                redaction: Redaction::None,
                import_enabled: false,
                writes_enabled: false,
                batch_post_enabled: false,
            });
            Self {
                simulator,
                server,
                _directory: directory,
            }
        }

        async fn call(&self, args: Value) -> Value {
            self.server.call_tool("ledger_masters", args).await
        }

        fn requests(self) -> usize {
            self.simulator.finish().unwrap().len()
        }
    }

    fn snapshot_of(response: &Value) -> &Value {
        assert_ne!(response["isError"], true, "{response}");
        &response["structuredContent"]["result"]["snapshot"]
    }

    fn snapshot_id(response: &Value) -> String {
        snapshot_of(response)["id"].as_str().unwrap().to_string()
    }

    /// The rows a whole, unpaged basic listing returns, for comparing pages.
    async fn whole_listing() -> Vec<Value> {
        let (response, _) = call(basic_plans(), json!({"company_guid":GUID})).await;
        items(&response).clone()
    }

    /// Page 2 costs the identity read and one extent read, and returns the
    /// rows that follow page 1 in the same read.
    #[tokio::test]
    async fn a_continuation_page_is_served_from_its_first_pages_read() {
        let mut plans = basic_plans();
        plans.extend(continuation_plans(extent_with_master_mark(219)));
        let total = plans.len();
        let one = OneServer::spawn(plans);
        let first = one.call(json!({"company_guid":GUID,"limit":4})).await;
        let id = snapshot_id(&first);
        assert_eq!(snapshot_of(&first)["reused"], false);
        assert_eq!(snapshot_of(&first)["master_alter_id"], 219);
        let second = one
            .call(json!({"company_guid":GUID,"offset":4,"limit":4,"snapshot_id":id}))
            .await;
        assert_eq!(snapshot_of(&second)["reused"], true);
        assert_eq!(snapshot_of(&second)["id"], id);
        assert_eq!(one.requests(), total);
        let whole = whole_listing().await;
        assert_eq!(items(&first).as_slice(), &whole[..4]);
        assert_eq!(items(&second).as_slice(), &whole[4..8]);
    }

    /// A book that moved after page 1 is refused when the caller named the
    /// snapshot, and read fresh when it did not.
    #[tokio::test]
    async fn a_continuation_after_the_book_moved_is_refused_by_id_or_read_fresh() {
        let mut plans = basic_plans();
        plans.extend(continuation_plans(extent_with_master_mark(220)));
        let total = plans.len();
        let one = OneServer::spawn(plans);
        let id = snapshot_id(&one.call(json!({"company_guid":GUID,"limit":4})).await);
        let refused = one
            .call(json!({"company_guid":GUID,"offset":4,"limit":4,"snapshot_id":id}))
            .await;
        let error = refusal(&refused);
        assert_eq!(error["code"], "listing_snapshot_changed");
        assert_eq!(error["cause"], "book_changed_since_first_page");
        assert_eq!(
            one.requests(),
            total,
            "nothing is read after the extent check"
        );

        let mut plans = basic_plans();
        plans.extend(continuation_plans(extent_with_master_mark(220)));
        plans.extend(
            basic_plans_marked(220)
                .into_iter()
                .skip(identity_plans().len()),
        );
        let total = plans.len();
        let one = OneServer::spawn(plans);
        let id = snapshot_id(&one.call(json!({"company_guid":GUID,"limit":4})).await);
        let fresh = one
            .call(json!({"company_guid":GUID,"offset":4,"limit":4}))
            .await;
        assert_eq!(snapshot_of(&fresh)["reused"], false);
        assert_ne!(snapshot_of(&fresh)["id"], id.as_str());
        assert_eq!(snapshot_of(&fresh)["master_alter_id"], 220);
        assert_eq!(one.requests(), total);
    }

    /// #653 with #630: a compliance listing's rows are rendered with
    /// `party_gstin` read as of one date, so its snapshot serves only a page
    /// asking for that date. Named, another date is refused; the same date is
    /// served.
    #[tokio::test]
    async fn a_compliance_continuation_for_another_as_of_is_not_served_from_the_snapshot() {
        let mut plans = compliance_plans(masters(), balances());
        plans.extend(continuation_plans(extent_with_master_mark(219)));
        let total = plans.len();
        let one = OneServer::spawn(plans);
        let first = one
            .call(json!({"company_guid":GUID,"fields":"compliance","as_of":"20260331","limit":1}))
            .await;
        let id = snapshot_id(&first);
        let refused = one
            .call(
                json!({"company_guid":GUID,"fields":"compliance","as_of":"20250630",
                "offset":1,"limit":1,"snapshot_id":id}),
            )
            .await;
        let error = refusal(&refused);
        assert_eq!(error["code"], "listing_snapshot_changed");
        assert_eq!(error["cause"], "snapshot_not_held");
        assert_eq!(
            one.requests(),
            total,
            "nothing is read after the extent check"
        );

        let mut plans = compliance_plans(masters(), balances());
        plans.extend(continuation_plans(extent_with_master_mark(219)));
        let total = plans.len();
        let one = OneServer::spawn(plans);
        let id = snapshot_id(
            &one.call(
                json!({"company_guid":GUID,"fields":"compliance","as_of":"20260331","limit":1}),
            )
            .await,
        );
        let served = one
            .call(
                json!({"company_guid":GUID,"fields":"compliance","as_of":"2026-03-31",
                "offset":1,"limit":1,"snapshot_id":id}),
            )
            .await;
        assert_eq!(
            snapshot_of(&served)["reused"],
            true,
            "the same date, spelled either way"
        );
        assert_eq!(one.requests(), total);
    }

    /// Unnamed, a page for another `as_of` reads fresh, and its rows carry the
    /// date it asked for.
    #[tokio::test]
    async fn a_compliance_continuation_for_another_as_of_reads_fresh_when_unnamed() {
        let mut plans = compliance_plans(masters(), balances());
        plans.extend(continuation_plans(extent_with_master_mark(219)));
        plans.extend(
            compliance_plans(masters(), balances())
                .into_iter()
                .skip(identity_plans().len()),
        );
        let total = plans.len();
        let one = OneServer::spawn(plans);
        let id = snapshot_id(
            &one.call(
                json!({"company_guid":GUID,"fields":"compliance","as_of":"20260331","limit":1}),
            )
            .await,
        );
        let fresh = one
            .call(
                json!({"company_guid":GUID,"fields":"compliance","as_of":"20250630",
                "offset":1,"limit":1}),
            )
            .await;
        assert_eq!(snapshot_of(&fresh)["reused"], false);
        assert_ne!(snapshot_of(&fresh)["id"], id.as_str());
        for row in items(&fresh) {
            assert_eq!(row["party_gstin_as_of"], "20250630", "{row}");
        }
        assert_eq!(one.requests(), total);
    }

    /// A continuation that names no snapshot is still served from the held
    /// read while the book is unchanged: the id only makes a change loud.
    #[tokio::test]
    async fn a_continuation_without_an_id_is_served_from_the_held_read_while_the_book_is_unchanged()
    {
        let mut plans = basic_plans();
        plans.extend(continuation_plans(extent_with_master_mark(219)));
        let total = plans.len();
        let one = OneServer::spawn(plans);
        let first = one.call(json!({"company_guid":GUID,"limit":4})).await;
        let second = one
            .call(json!({"company_guid":GUID,"offset":4,"limit":4}))
            .await;
        assert_eq!(snapshot_of(&second)["reused"], true);
        assert_eq!(snapshot_of(&second)["id"], snapshot_of(&first)["id"]);
        assert_eq!(one.requests(), total);
    }

    /// A second first page replaces the held snapshot, so a continuation
    /// naming the first page's id is refused rather than served from the
    /// newer read, even though the book did not change.
    #[tokio::test]
    async fn a_continuation_naming_a_replaced_snapshot_is_refused() {
        let mut plans = basic_plans();
        plans.extend(basic_plans());
        plans.extend(continuation_plans(extent_with_master_mark(219)));
        let total = plans.len();
        let one = OneServer::spawn(plans);
        let replaced = snapshot_id(&one.call(json!({"company_guid":GUID,"limit":4})).await);
        let _newer = one.call(json!({"company_guid":GUID,"limit":4})).await;
        let refused = one
            .call(json!({"company_guid":GUID,"offset":4,"limit":4,"snapshot_id":replaced}))
            .await;
        let error = refusal(&refused);
        assert_eq!(error["code"], "listing_snapshot_changed");
        assert_eq!(error["cause"], "snapshot_not_held");
        assert_eq!(one.requests(), total);
    }

    /// A first page is a new question: it always reads fresh, even when an
    /// unexpired snapshot of the same listing is held.
    #[tokio::test]
    async fn a_first_page_always_reads_fresh() {
        let mut plans = basic_plans();
        plans.extend(basic_plans());
        let total = plans.len();
        let one = OneServer::spawn(plans);
        let first = one.call(json!({"company_guid":GUID,"limit":4})).await;
        let again = one.call(json!({"company_guid":GUID,"limit":4})).await;
        assert_eq!(snapshot_of(&again)["reused"], false);
        assert_ne!(snapshot_of(&again)["id"], snapshot_of(&first)["id"]);
        assert_eq!(one.requests(), total);
    }

    /// A snapshot that is no longer held refuses a continuation that names
    /// it: after the TTL, after a write through this server drops it, and
    /// when it was larger than the byte cap.
    #[tokio::test]
    async fn a_snapshot_no_longer_held_refuses_a_continuation_that_names_it() {
        for case in ["expired", "written", "over_cap"] {
            let mut plans = basic_plans();
            plans.extend(continuation_plans(extent_with_master_mark(219)));
            let total = plans.len();
            let one = OneServer::spawn(plans);
            {
                let mut listings = one.server.listings.lock().unwrap();
                match case {
                    "expired" => listings.ttl = std::time::Duration::ZERO,
                    "over_cap" => listings.max_bytes = 1,
                    _ => {}
                }
            }
            let id = snapshot_id(&one.call(json!({"company_guid":GUID,"limit":4})).await);
            if case == "written" {
                one.server.drop_listing_snapshots(GUID);
            }
            let refused = one
                .call(json!({"company_guid":GUID,"offset":4,"limit":4,"snapshot_id":id}))
                .await;
            let error = refusal(&refused);
            assert_eq!(error["code"], "listing_snapshot_changed", "{case}");
            assert_eq!(error["cause"], "snapshot_not_held", "{case}");
            assert_eq!(one.requests(), total, "{case}");
        }
    }

    /// A page served from a snapshot records only the requests it sent: the
    /// identity read and the extent pair, never its first page's read again.
    #[tokio::test]
    async fn a_page_served_from_a_snapshot_records_only_the_reads_it_sent() {
        let bytes = |response: &Value| {
            response["structuredContent"]["evidence"]["bytes"]
                .as_u64()
                .unwrap()
        };
        let mut plans = basic_plans();
        plans.extend(continuation_plans(extent_with_master_mark(219)));
        let one = OneServer::spawn(plans);
        let first = one.call(json!({"company_guid":GUID,"limit":4})).await;
        let served = one
            .call(json!({"company_guid":GUID,"offset":4,"limit":4}))
            .await;
        assert_eq!(snapshot_of(&served)["reused"], true);
        assert!(bytes(&served) < bytes(&first), "{served}");

        // The extent pair is what it counts: an extent one character longer
        // (a four-digit mark, UTF-16) costs 2 bytes more per read of the pair.
        let mut plans = basic_plans_marked(2_200);
        plans.extend(continuation_plans(extent_with_master_mark(2_200)));
        let one = OneServer::spawn(plans);
        let _first = one.call(json!({"company_guid":GUID,"limit":4})).await;
        let longer = one
            .call(json!({"company_guid":GUID,"offset":4,"limit":4}))
            .await;
        assert_eq!(snapshot_of(&longer)["reused"], true);
        assert_eq!(bytes(&longer), bytes(&served) + 4);
    }

    /// An extent read refused after both its requests were sent (the pair
    /// disagreed) records both: the refusal's evidence counts what was sent.
    #[tokio::test]
    async fn a_refused_extent_read_still_records_the_requests_it_sent() {
        let refused_under = |mark: u64| async move {
            let mut plans = basic_plans_marked(mark);
            plans.extend(identity_plans());
            plans.push(xml(companies()));
            plans.extend([
                xml(extent_with_master_mark(mark)),
                status(),
                xml(extent_with_master_mark(mark + 1)),
                status(),
            ]);
            let total = plans.len();
            let one = OneServer::spawn(plans);
            let first = one.call(json!({"company_guid":GUID,"limit":4})).await;
            assert_ne!(first["isError"], true, "{first}");
            let refused = one
                .call(json!({"company_guid":GUID,"offset":4,"limit":4}))
                .await;
            assert_eq!(
                refusal(&refused)["code"],
                "listing_extent_read_failed",
                "{refused}"
            );
            let bytes = refused["structuredContent"]["evidence"]["bytes"]
                .as_u64()
                .unwrap();
            assert_eq!(one.requests(), total);
            bytes
        };
        // Both extent responses are counted: each is one character longer
        // under a four-digit mark, 2 bytes each in UTF-16.
        assert_eq!(refused_under(2_200).await, refused_under(219).await + 4);
    }

    /// An extent pair that completed, followed by a closing identity bracket
    /// that no longer finds the company, still records both extent requests.
    #[tokio::test]
    async fn a_closing_bracket_refusal_still_records_the_extent_pair() {
        let refused_under = |mark: u64| async move {
            let gone = companies();
            assert_eq!(gone.matches(GUID).count(), 1, "one row names the company");
            let mut plans = basic_plans_marked(mark);
            plans.extend(identity_plans());
            plans.push(xml(companies()));
            pair(&mut plans, xml(extent_with_master_mark(mark)));
            plans.push(xml(
                gone.replace(GUID, "00000000-0000-0000-0000-000000000000")
            ));
            let total = plans.len();
            let one = OneServer::spawn(plans);
            let first = one.call(json!({"company_guid":GUID,"limit":4})).await;
            assert_ne!(first["isError"], true, "{first}");
            let refused = one
                .call(json!({"company_guid":GUID,"offset":4,"limit":4}))
                .await;
            assert_eq!(
                refusal(&refused)["code"],
                "listing_extent_read_failed",
                "{refused}"
            );
            let bytes = refused["structuredContent"]["evidence"]["bytes"]
                .as_u64()
                .unwrap();
            assert_eq!(one.requests(), total);
            bytes
        };
        // Both extent responses are counted: each is one character longer
        // under a four-digit mark, 2 bytes each in UTF-16.
        assert_eq!(refused_under(2_200).await, refused_under(219).await + 4);
    }

    /// An expired snapshot is not only skipped but dropped the next time the
    /// store is touched: by holding another listing, or by any write's drop.
    #[tokio::test]
    async fn an_expired_snapshot_is_no_longer_held_once_the_store_is_next_touched() {
        let mut plans = basic_plans();
        plans.extend(basic_plans_reading(period_opening(), Some(groups())));
        let total = plans.len();
        let one = OneServer::spawn(plans);
        one.server.listings.lock().unwrap().ttl = std::time::Duration::ZERO;
        let _basic = one.call(json!({"company_guid":GUID,"limit":4})).await;
        assert_eq!(one.server.listings.lock().unwrap().held.len(), 1);
        let _grouped = one
            .call(json!({"company_guid":GUID,"limit":4,"group":"Sundry Debtors"}))
            .await;
        assert_eq!(
            one.server.listings.lock().unwrap().held.len(),
            1,
            "holding the grouped listing dropped the expired basic one"
        );
        one.server
            .drop_listing_snapshots("00000000-0000-0000-0000-000000000000");
        assert!(
            one.server.listings.lock().unwrap().held.is_empty(),
            "a drop for another company still drops what has expired"
        );
        assert_eq!(one.requests(), total);
    }

    /// A write's drop still happens after the store's lock was poisoned: a
    /// drop that did nothing would let a snapshot outlive the write.
    #[tokio::test]
    async fn a_write_drops_snapshots_even_from_a_poisoned_store() {
        let one = OneServer::spawn(basic_plans());
        let _first = one.call(json!({"company_guid":GUID,"limit":4})).await;
        let listings = one.server.listings.clone();
        let _ = std::thread::spawn(move || {
            let _held = listings.lock().unwrap();
            panic!("poison the listing store");
        })
        .join();
        assert!(one.server.listings.is_poisoned());
        one.server.drop_listing_snapshots(GUID);
        let store = one
            .server
            .listings
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        assert!(store.held.is_empty());
    }

    /// A listing that holds the group collection counts it toward the byte
    /// cap: the same rows with groups weigh more than without.
    #[tokio::test]
    async fn a_grouped_listing_counts_its_groups_toward_the_cap() {
        let mut plans = basic_plans();
        plans.extend(basic_plans_reading(period_opening(), Some(groups())));
        let one = OneServer::spawn(plans);
        let _basic = one.call(json!({"company_guid":GUID,"limit":4})).await;
        let _grouped = one
            .call(json!({"company_guid":GUID,"limit":4,"group":"Sundry Debtors"}))
            .await;
        let store = one.server.listings.lock().unwrap();
        let [basic, grouped] = store.held.as_slice() else {
            panic!("two listings held");
        };
        assert_eq!(basic.rows, grouped.rows);
        assert!(grouped.bytes > basic.bytes);
    }

    /// The byte cap drops the oldest snapshot to make room for a newer one.
    #[tokio::test]
    async fn the_byte_cap_evicts_the_oldest_listing_first() {
        // Each listing's size, measured on its own server first.
        let sizes = {
            let mut plans = basic_plans();
            plans.extend(basic_plans_reading(period_opening(), Some(groups())));
            let one = OneServer::spawn(plans);
            let _basic = one.call(json!({"company_guid":GUID,"limit":4})).await;
            let _grouped = one
                .call(json!({"company_guid":GUID,"limit":4,"group":"Sundry Debtors"}))
                .await;
            let store = one.server.listings.lock().unwrap();
            store.held.iter().map(|held| held.bytes).collect::<Vec<_>>()
        };
        let mut plans = basic_plans();
        plans.extend(basic_plans_reading(period_opening(), Some(groups())));
        plans.extend(continuation_plans(extent_with_master_mark(219)));
        let total = plans.len();
        let one = OneServer::spawn(plans);
        // Room for either listing, not both.
        one.server.listings.lock().unwrap().max_bytes = sizes.iter().sum::<usize>() - 1;
        let basic = one.call(json!({"company_guid":GUID,"limit":4})).await;
        let _grouped = one
            .call(json!({"company_guid":GUID,"limit":4,"group":"Sundry Debtors"}))
            .await;
        let refused = one
            .call(
                json!({"company_guid":GUID,"offset":4,"limit":4,"snapshot_id":snapshot_id(&basic)}),
            )
            .await;
        assert_eq!(refusal(&refused)["cause"], "snapshot_not_held");
        assert_eq!(
            one.server.listings.lock().unwrap().held.len(),
            1,
            "the newer listing is held"
        );
        assert_eq!(one.requests(), total);
    }

    async fn call(plans: Vec<ScenarioPlan>, args: Value) -> (Value, usize) {
        call_with_max_bytes(plans, args, 200_000).await
    }

    /// A call that should be refused before it sends anything. The simulator
    /// needs at least one plan, so it holds one it serves only if a request is
    /// sent; the requests Bridge actually sent are counted after a cancel,
    /// whose wake-up connection carries no method.
    async fn call_refused_before_any_request(args: Value) -> (Value, usize) {
        let simulator = SequenceSimulator::spawn(vec![status()]).unwrap();
        let directory = tempfile::tempdir().unwrap();
        let server = Server::new(Settings {
            endpoint: TallyEndpointConfig {
                host: "127.0.0.1".into(),
                port: simulator.address().port(),
            },
            data_dir: directory.path().into(),
            max_rows: 500,
            max_bytes: 200_000,
            redaction: Redaction::None,
            import_enabled: false,
            writes_enabled: false,
            batch_post_enabled: false,
        });
        let response = server.call_tool("ledger_masters", args).await;
        simulator.cancel();
        let requests = simulator
            .finish()
            .unwrap()
            .into_iter()
            .filter(|request| !request.method.is_empty())
            .count();
        (response, requests)
    }

    async fn call_with_max_bytes(
        plans: Vec<ScenarioPlan>,
        args: Value,
        max_bytes: usize,
    ) -> (Value, usize) {
        let simulator = SequenceSimulator::spawn(plans).unwrap();
        let directory = tempfile::tempdir().unwrap();
        let server = Server::new(Settings {
            endpoint: TallyEndpointConfig {
                host: "127.0.0.1".into(),
                port: simulator.address().port(),
            },
            data_dir: directory.path().into(),
            max_rows: 500,
            max_bytes,
            redaction: Redaction::None,
            import_enabled: false,
            writes_enabled: false,
            batch_post_enabled: false,
        });
        let response = server.call_tool("ledger_masters", args).await;
        let requests = simulator.finish().unwrap().len();
        (response, requests)
    }

    /// Identity, then the extent-bracketed currency read that returns the
    /// captured two-Currency-master response (protocol reference §9.10a.1).
    fn multi_currency_plans() -> Vec<ScenarioPlan> {
        let company = xml(companies());
        let extent = xml(include_str!(
            "../crates/bridge-tally-protocol/tests/fixtures/agent/native-company-book-extents-with-number.utf8.xml"
        )
        .to_owned());
        let currency = xml(captured(include_bytes!(
            "../crates/bridge-tally-protocol/tests/fixtures/currency_multi_live.utf16le.xml"
        )));
        let mut plans = identity_plans();
        plans.push(company.clone());
        pair(&mut plans, extent.clone());
        pair(&mut plans, currency);
        pair(&mut plans, extent);
        plans.push(company);
        plans
    }

    /// Identity, then a currency pair whose second read disagrees with its
    /// first: the paired-read stability check refuses before admission.
    fn drifting_currency_plans() -> Vec<ScenarioPlan> {
        let company = xml(companies());
        let extent = xml(include_str!(
            "../crates/bridge-tally-protocol/tests/fixtures/agent/native-company-book-extents-with-number.utf8.xml"
        )
        .to_owned());
        let single = xml(captured(include_bytes!(
            "../crates/bridge-tally-protocol/tests/fixtures/currency_inr_modern_live.utf16le.xml"
        )));
        let multi = xml(captured(include_bytes!(
            "../crates/bridge-tally-protocol/tests/fixtures/currency_multi_live.utf16le.xml"
        )));
        let mut plans = identity_plans();
        plans.push(company);
        pair(&mut plans, extent);
        plans.extend([single, status(), multi, status()]);
        plans
    }

    fn refusal(response: &Value) -> &Value {
        assert_eq!(response["isError"], true, "{response}");
        &response["structuredContent"]["result"]["error"]
    }

    #[tokio::test]
    async fn currency_refusal_names_its_cause_beside_the_operation_code() {
        let (response, _) = call(
            multi_currency_plans(),
            json!({"company_guid":GUID,"fields":"compliance"}),
        )
        .await;
        let error = refusal(&response);
        assert_eq!(error["code"], "party_ledger_master_read_failed");
        assert_eq!(error["cause"], "company_base_currency_undetermined");
    }

    #[tokio::test]
    async fn join_refusal_names_the_validation_variant_as_its_cause() {
        let balance_name = "NAME=\"Bridge Nested Debtor WR4\"";
        let source = balances();
        assert_eq!(source.matches(balance_name).count(), 1);
        let renamed = source.replace(balance_name, "NAME=\"Bridge Renamed Debtor WR4\"");
        // The join refuses before the closing re-bracket, so those three
        // reads are never sent.
        let mut plans = compliance_plans(masters(), renamed);
        plans.truncate(plans.len() - 3);
        let (response, _) = call(plans, json!({"company_guid":GUID,"fields":"compliance"})).await;
        let error = refusal(&response);
        assert_eq!(error["code"], "party_ledger_master_read_failed");
        assert_eq!(error["cause"], "balance_missing_master_ledger");
    }

    #[tokio::test]
    async fn paired_read_drift_names_the_changed_source_as_its_cause() {
        let (response, _) = call(
            drifting_currency_plans(),
            json!({"company_guid":GUID,"fields":"compliance"}),
        )
        .await;
        let error = refusal(&response);
        assert_eq!(error["code"], "party_ledger_master_read_failed");
        assert_eq!(error["cause"], "currency_master_changed");
    }

    #[tokio::test]
    async fn cause_is_omitted_below_the_guidance_budget_and_the_code_survives() {
        // Control: the same refusal carries a cause at the default budget, so
        // its absence below is the budget rule and not a missing cause.
        let (response, _) = call(
            multi_currency_plans(),
            json!({"company_guid":GUID,"fields":"compliance"}),
        )
        .await;
        assert_eq!(
            refusal(&response)["cause"],
            "company_base_currency_undetermined"
        );
        let (response, _) = call_with_max_bytes(
            multi_currency_plans(),
            json!({"company_guid":GUID,"fields":"compliance"}),
            REMEDIATION_MIN_RESPONSE_BUDGET - 1,
        )
        .await;
        let error = refusal(&response);
        assert_eq!(error["code"], "party_ledger_master_read_failed");
        assert!(error.get("cause").is_none(), "{error}");
    }

    fn items(response: &Value) -> &Vec<Value> {
        assert_ne!(response["isError"], true, "{response}");
        response["structuredContent"]["result"]["items"]
            .as_array()
            .unwrap_or_else(|| panic!("no items: {response}"))
    }

    fn row<'a>(items: &'a [Value], name: &str) -> &'a Value {
        items
            .iter()
            .find(|item| item["name"] == name)
            .unwrap_or_else(|| panic!("{name} missing from {items:?}"))
    }

    fn names(items: &[Value]) -> std::collections::BTreeSet<String> {
        items
            .iter()
            .map(|item| item["name"].as_str().unwrap().to_string())
            .collect()
    }

    fn group_filter(response: &Value) -> &Value {
        assert_ne!(response["isError"], true, "{response}");
        &response["structuredContent"]["result"]["group_filter"]
    }

    /// #631: "the ledgers in Sundry Debtors" means the subtree, and the default
    /// scope answers a narrower question. The captured book files one ledger
    /// under a user group below Sundry Debtors; the immediate filter leaves it
    /// out, and the result must say so, under either field set.
    #[tokio::test]
    async fn an_immediate_group_filter_names_the_sub_group_ledgers_it_left_out() {
        for (fields, plans) in [
            (
                "basic",
                basic_plans_reading(period_opening(), Some(groups())),
            ),
            ("compliance", compliance_plans(masters(), balances())),
        ] {
            let total = plans.len();
            let (response, requests) = call(
                plans,
                json!({"company_guid":GUID,"fields":fields,"group":"Sundry Debtors"}),
            )
            .await;
            assert_eq!(requests, total, "{fields}");
            let found = names(items(&response));
            assert_eq!(found.len(), 5, "{fields}: {found:?}");
            assert!(!found.contains("Bridge Nested Debtor WR4"), "{fields}");
            assert_eq!(
                group_filter(&response),
                &json!({
                    "group": "Sundry Debtors",
                    "scope": "immediate",
                    "excluded_subgroup_ledgers": {
                        "count": 1,
                        "group_count": 1,
                        "groups": ["Bridge Nested Debtors WR4"],
                    },
                    "unresolved_ancestry_ledgers": 0,
                }),
                "{fields}"
            );
        }
    }

    /// Ancestry scope needs the group collection, not the compliance fields:
    /// `fields=basic` (also the default) reads it once, paired, inside the
    /// ledger export's bracket, and its rows stay basic.
    #[tokio::test]
    async fn basic_ancestry_scope_admits_the_sub_group_ledger_from_one_paired_group_read() {
        let plans = basic_plans_reading(period_opening(), Some(groups()));
        let total = plans.len();
        let (response, requests) = call(
            plans,
            json!({"company_guid":GUID,"group":"Sundry Debtors","group_scope":"ancestry"}),
        )
        .await;
        assert_eq!(requests, total);
        let found = items(&response);
        assert_eq!(found.len(), 6, "{found:?}");
        assert_eq!(
            row(found, "Bridge Nested Debtor WR4")["parent"],
            "Bridge Nested Debtors WR4"
        );
        for item in found {
            assert!(item.get("ancestry").is_none(), "{item}");
        }
        assert_eq!(
            group_filter(&response),
            &json!({
                "group": "Sundry Debtors",
                "scope": "ancestry",
                "excluded_subgroup_ledgers": {"count": 0, "group_count": 0, "groups": []},
                "unresolved_ancestry_ledgers": 0,
            })
        );
    }

    /// A ledger whose chain stops before reaching the group might sit under it
    /// or not; Bridge cannot say, so it is counted rather than silently
    /// treated as outside. Only PARENT changes: `WR2 Sales` is re-parented to
    /// a group the captured collection does not hold. The baseline tests above
    /// report 0 for the same book, including the root-parented ledger.
    #[tokio::test]
    async fn a_ledger_whose_chain_breaks_before_the_group_is_counted_unresolved() {
        let from = "<PARENT TYPE=\"String\">Sales Accounts</PARENT>";
        let ledgers = period_opening();
        assert_eq!(ledgers.matches(from).count(), 1);
        let ledgers = ledgers.replace(from, "<PARENT TYPE=\"String\">Group Absent WR631</PARENT>");
        for scope in ["immediate", "ancestry"] {
            let (response, _) = call(
                basic_plans_reading(ledgers.clone(), Some(groups())),
                json!({"company_guid":GUID,"group":"Sundry Debtors","group_scope":scope}),
            )
            .await;
            assert!(!names(items(&response)).contains("WR2 Sales"), "{scope}");
            assert_eq!(
                group_filter(&response)["unresolved_ancestry_ledgers"],
                1,
                "{scope}"
            );
        }
    }

    /// The added group read is paired like the ledger export beside it: a
    /// collection that changes between its two reads is refused, not resolved
    /// against. Only one group NAME differs in the second read; the closing
    /// extent and identity reads are never sent.
    #[tokio::test]
    async fn a_group_collection_that_changes_between_its_paired_reads_is_refused() {
        let from = "Bridge Nested Debtors WR4";
        let source = groups();
        assert!(source.contains(from));
        let mut plans = basic_plans_reading(period_opening(), Some(source.clone()));
        let closing = 7;
        let second_group_read = plans.len() - closing - 2;
        plans[second_group_read] = xml(source.replace(from, "Bridge Nested Debtors WR631"));
        plans.truncate(plans.len() - closing);
        let total = plans.len();
        let (response, requests) =
            call(plans, json!({"company_guid":GUID,"group":"Sundry Debtors"})).await;
        assert_eq!(requests, total);
        let error = refusal(&response);
        assert_eq!(error["code"], "ledger_export_invalid");
        assert_eq!(error["cause"], "native_ledger_group_changed");
    }

    /// A basic read of a book holding a foreign-currency opening is refused as
    /// before, but names why and what to do (#675). The composite is the one in
    /// the captured several-currency ledgers, placed in the captured basic
    /// export; the closing extent and identity reads are never sent.
    #[tokio::test]
    async fn a_foreign_currency_opening_refuses_the_basic_read_with_its_cause() {
        let forex = captured(include_bytes!(
            "../crates/bridge-tally-protocol/tests/fixtures/ledgers_currency_forex_live.utf16le.xml"
        ));
        let composite = forex
            .split("<OPENINGBALANCE")
            .skip(1)
            .filter_map(|tail| {
                let text = &tail[tail.find('>')? + 1..tail.find("</OPENINGBALANCE>")?];
                text.contains(" @ ").then(|| text.to_string())
            })
            .collect::<Vec<_>>();
        assert_eq!(composite.len(), 1, "one composite opening in the capture");
        let row = "<OPENINGBALANCE TYPE=\"Amount\">-50000.00</OPENINGBALANCE>";
        let source = period_opening();
        assert_eq!(source.matches(row).count(), 1);
        let foreign = source.replace(
            row,
            &format!(
                "<OPENINGBALANCE TYPE=\"Amount\">{}</OPENINGBALANCE>",
                composite[0]
            ),
        );
        let mut plans = basic_plans_reading(foreign, None);
        plans.truncate(plans.len() - 7);
        let total = plans.len();
        let (response, requests) = call(plans, json!({"company_guid":GUID})).await;
        assert_eq!(requests, total);
        let error = refusal(&response);
        assert_eq!(error["code"], "ledger_export_invalid");
        assert_eq!(error["cause"], "foreign_currency_ledger_balance");
        let remediation = error["remediation"].as_str().unwrap();
        assert!(remediation.contains("#683"), "{error}");
    }

    #[tokio::test]
    async fn compliance_rows_carry_the_chain_resolved_from_the_captured_groups() {
        let plans = compliance_plans(masters(), balances());
        let total = plans.len();
        let (response, requests) =
            call(plans, json!({"company_guid":GUID,"fields":"compliance"})).await;
        let items = items(&response);
        assert_eq!(requests, total);
        assert_eq!(items.len(), 9);
        // A user-created group (empty RESERVEDNAME) is a hop like any other,
        // and the walk continues through it to the reserved root.
        assert_eq!(
            row(items, "Bridge Nested Debtor WR4")["ancestry"],
            json!({
                "chain": [
                    {"name": "Bridge Nested Debtors WR4", "reserved_name": ""},
                    {"name": "Sundry Debtors", "reserved_name": "Sundry Debtors"},
                    {"name": "Current Assets", "reserved_name": "Current Assets"},
                ],
                "complete": true,
                "gap": null,
            })
        );
        assert_eq!(
            row(items, "Cash")["ancestry"],
            json!({
                "chain": [
                    {"name": "Cash-in-Hand", "reserved_name": "Cash-in-Hand"},
                    {"name": "Current Assets", "reserved_name": "Current Assets"},
                ],
                "complete": true,
                "gap": null,
            })
        );
        for item in items {
            assert!(item["ancestry"].is_object(), "{item}");
        }
    }

    #[tokio::test]
    async fn ancestry_scope_admits_a_ledger_whose_parent_is_below_the_group() {
        let sundry = |scope: &str| json!({"company_guid":GUID,"fields":"compliance","group":"Sundry Debtors","group_scope":scope});
        let (immediate, _) =
            call(compliance_plans(masters(), balances()), sundry("immediate")).await;
        let (ancestry, _) = call(compliance_plans(masters(), balances()), sundry("ancestry")).await;
        let immediate = names(items(&immediate));
        let ancestry = names(items(&ancestry));
        assert!(!immediate.is_empty());
        assert!(!immediate.contains("Bridge Nested Debtor WR4"));
        let mut expected = immediate.clone();
        expected.insert("Bridge Nested Debtor WR4".to_string());
        assert_eq!(ancestry, expected);
    }

    #[tokio::test]
    async fn ancestry_scope_reaches_loans_liability_through_bank_od() {
        // The capture has the `Bank OD A/c` -> `Loans (Liability)` groups but
        // no ledger under them, so one captured ledger is re-parented in both
        // the master and the balance response. Only PARENT changes; this is
        // not live evidence of such a ledger.
        let reparent = |source: String| {
            let from = "<PARENT TYPE=\"String\">Sales Accounts</PARENT>";
            assert_eq!(source.matches(from).count(), 1);
            source.replace(from, "<PARENT TYPE=\"String\">Bank OD A/c</PARENT>")
        };
        let loans = |scope: &str| json!({"company_guid":GUID,"fields":"compliance","group":"Loans (Liability)","group_scope":scope});
        let (response, _) = call(
            compliance_plans(reparent(masters()), reparent(balances())),
            loans("ancestry"),
        )
        .await;
        let found = items(&response);
        assert_eq!(names(found), ["WR2 Sales".to_string()].into());
        assert_eq!(found[0]["parent"], "Bank OD A/c");
        assert_eq!(
            found[0]["ancestry"],
            json!({
                "chain": [
                    {"name": "Bank OD A/c", "reserved_name": "Bank OD A/c"},
                    {"name": "Loans (Liability)", "reserved_name": "Loans (Liability)"},
                ],
                "complete": true,
                "gap": null,
            })
        );
        assert_eq!(response["structuredContent"]["result"]["total"], 1);

        // The default scope still matches only the immediate parent.
        let (response, _) = call(
            compliance_plans(reparent(masters()), reparent(balances())),
            loans("immediate"),
        )
        .await;
        assert!(items(&response).is_empty());
    }

    /// #554 through the stdio server: `ledger_masters` basic is two queued
    /// operations (the identity read, then the ledger read). The identity read's
    /// first leg is held; while it runs the client sends `second`. Returns the
    /// response to request 7 and how many requests reached the simulator.
    async fn serve_ledger_masters_then(second: &[&str]) -> (Value, usize) {
        let (responses, sent) = serve_ledger_masters_collecting(second, 1).await;
        let response = responses
            .into_iter()
            .find(|value| value["id"] == 7)
            .expect("a response to request 7");
        (response, sent)
    }

    /// As `serve_ledger_masters_then`, returning every response in the order
    /// written, once `expected` responses other than `initialize`'s arrived.
    async fn serve_ledger_masters_collecting(
        second: &[&str],
        expected: usize,
    ) -> (Vec<Value>, usize) {
        use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
        let mut plans = basic_plans();
        plans[0] = plans[0]
            .clone()
            .with_delivery(tally_protocol_simulator::Delivery::SlowHeaders(
                std::time::Duration::from_millis(900),
            ));
        let simulator = SequenceSimulator::spawn(plans).unwrap();
        let directory = tempfile::tempdir().unwrap();
        let server = Server::new(Settings {
            endpoint: TallyEndpointConfig {
                host: "127.0.0.1".into(),
                port: simulator.address().port(),
            },
            data_dir: directory.path().into(),
            max_rows: 500,
            max_bytes: 200_000,
            redaction: Redaction::None,
            import_enabled: false,
            writes_enabled: false,
            batch_post_enabled: false,
        });
        let (client, source) = tokio::io::duplex(1 << 20);
        let (client_read, mut client_write) = tokio::io::split(client);
        let (source_read, mut source_write) = tokio::io::split(source);
        let serve = async move {
            crate::agent::agent_protocol::serve_stdio(
                server,
                BufReader::new(source_read),
                &mut source_write,
            )
            .await
        };
        let client = async move {
            let call = json!({"jsonrpc":"2.0","id":7,"method":"tools/call","params":{
                "name":"ledger_masters","arguments":{"company_guid":GUID}}});
            let opening = format!(
                "{}\n{}\n{}\n",
                json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18"}}),
                json!({"jsonrpc":"2.0","method":"notifications/initialized"}),
                call
            );
            client_write.write_all(opening.as_bytes()).await.unwrap();
            tokio::time::sleep(std::time::Duration::from_millis(200)).await;
            for frame in second {
                if *frame == CLOSE_INPUT {
                    client_write.shutdown().await.unwrap();
                    continue;
                }
                client_write.write_all(frame.as_bytes()).await.unwrap();
                client_write.write_all(b"\n").await.unwrap();
            }
            let mut lines = BufReader::new(client_read).lines();
            let mut responses = Vec::new();
            let mut answered = 0;
            while answered < expected {
                let line = lines
                    .next_line()
                    .await
                    .unwrap()
                    .expect("every expected response");
                let value: Value = serde_json::from_str(&line).unwrap();
                if value["id"] != 1 {
                    answered += 1;
                }
                responses.push(value);
            }
            drop(client_write);
            responses
        };
        let (served, responses) = tokio::time::timeout(std::time::Duration::from_secs(20), async {
            tokio::join!(serve, client)
        })
        .await
        .unwrap();
        served.unwrap();
        simulator.cancel();
        // `cancel` wakes the simulator with an empty connection; count only the
        // requests Bridge actually sent.
        let sent = simulator
            .finish()
            .unwrap()
            .iter()
            .filter(|request| !request.method.is_empty())
            .count();
        (responses, sent)
    }

    /// In `second`, closes the client's input instead of sending a frame.
    const CLOSE_INPUT: &str = "<close input>";

    #[tokio::test]
    async fn closing_the_input_during_a_read_does_not_withdraw_it() {
        // A client may write its requests, close its side and still read the
        // answers: a closed input is not a cancellation, so the read completes
        // in full and every request of it is sent.
        let (response, requests) = serve_ledger_masters_then(&[CLOSE_INPUT]).await;
        assert_eq!(response["result"]["isError"], false, "{response}");
        assert_eq!(requests, basic_plans().len());
    }

    #[tokio::test]
    async fn input_is_left_in_the_pipe_once_eight_requests_wait() {
        // The bound on what a read holds: once eight requests are queued, input
        // is not read until the call ends, so a cancellation behind them is not
        // seen (the read completes in full, as before #554) and all eight are
        // then answered. Without the bound the queue would grow without limit.
        let mut frames: Vec<String> = (100..108)
            .map(|id| json!({"jsonrpc":"2.0","id":id,"method":"tools/list"}).to_string())
            .collect();
        frames.push(
            json!({"jsonrpc":"2.0","method":"notifications/cancelled","params":{"requestId":7}})
                .to_string(),
        );
        let frames: Vec<&str> = frames.iter().map(String::as_str).collect();
        let (responses, sent) = serve_ledger_masters_collecting(&frames, 9).await;
        let call = responses.iter().find(|value| value["id"] == 7).unwrap();
        assert_eq!(call["result"]["isError"], false, "{call}");
        assert_eq!(sent, basic_plans().len());
    }

    #[tokio::test]
    async fn requests_sent_during_a_read_are_all_served_after_it() {
        // Before #554 no input was read while a read ran, so a burst of requests
        // waited in the pipe and was served in order afterwards. Watching input
        // must not turn that into refusals: ten requests during one held read
        // are all answered, after the read, in the order sent.
        let burst: Vec<String> = (100..110)
            .map(|id| json!({"jsonrpc":"2.0","id":id,"method":"tools/list"}).to_string())
            .collect();
        let frames: Vec<&str> = burst.iter().map(String::as_str).collect();
        let (responses, _) = serve_ledger_masters_collecting(&frames, 11).await;
        let ids: Vec<Value> = responses
            .iter()
            .map(|value| value["id"].clone())
            .filter(|id| *id != 1)
            .collect();
        let expected: Vec<Value> = std::iter::once(json!(7))
            .chain((100..110).map(|id| json!(id)))
            .collect();
        assert_eq!(ids, expected, "{responses:?}");
        assert!(
            responses.iter().all(|value| value.get("error").is_none()),
            "{responses:?}"
        );
    }

    /// Input that yields `data`, then waits until `fail` fires and returns an
    /// I/O error, as a host whose pipe breaks mid-call would.
    struct FailingInput {
        data: std::io::Cursor<Vec<u8>>,
        fail: tokio::sync::oneshot::Receiver<()>,
    }

    impl tokio::io::AsyncRead for FailingInput {
        fn poll_read(
            mut self: std::pin::Pin<&mut Self>,
            cx: &mut std::task::Context<'_>,
            buf: &mut tokio::io::ReadBuf<'_>,
        ) -> std::task::Poll<std::io::Result<()>> {
            use std::io::Read;
            let remaining = buf.remaining();
            let mut chunk = vec![0; remaining];
            let read = self.data.read(&mut chunk).unwrap();
            if read > 0 {
                buf.put_slice(&chunk[..read]);
                return std::task::Poll::Ready(Ok(()));
            }
            match std::future::Future::poll(std::pin::Pin::new(&mut self.fail), cx) {
                std::task::Poll::Ready(_) => {
                    std::task::Poll::Ready(Err(std::io::Error::other("pipe broke")))
                }
                std::task::Poll::Pending => std::task::Poll::Pending,
            }
        }
    }

    #[tokio::test]
    async fn an_input_failure_still_answers_the_call_in_flight_then_ends() {
        // The input breaks while the identity read is held: the call stops
        // before its next operation, its answer is still written (the output
        // works), and only then does the session end with the input error.
        let mut plans = basic_plans();
        plans[0] = plans[0]
            .clone()
            .with_delivery(tally_protocol_simulator::Delivery::SlowHeaders(
                std::time::Duration::from_millis(900),
            ));
        let simulator = SequenceSimulator::spawn(plans).unwrap();
        let directory = tempfile::tempdir().unwrap();
        let server = Server::new(Settings {
            endpoint: TallyEndpointConfig {
                host: "127.0.0.1".into(),
                port: simulator.address().port(),
            },
            data_dir: directory.path().into(),
            max_rows: 500,
            max_bytes: 200_000,
            redaction: Redaction::None,
            import_enabled: false,
            writes_enabled: false,
            batch_post_enabled: false,
        });
        let frames = format!(
            "{}\n{}\n",
            json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18"}}),
            json!({"jsonrpc":"2.0","id":7,"method":"tools/call","params":{
                "name":"ledger_masters","arguments":{"company_guid":GUID}}})
        );
        let (fail, failed) = tokio::sync::oneshot::channel();
        let input = FailingInput {
            data: std::io::Cursor::new(frames.into_bytes()),
            fail: failed,
        };
        tokio::spawn(async move {
            tokio::time::sleep(std::time::Duration::from_millis(200)).await;
            let _ = fail.send(());
        });
        let mut output = Vec::new();
        let served = tokio::time::timeout(
            std::time::Duration::from_secs(20),
            crate::agent::agent_protocol::serve_stdio(
                server,
                tokio::io::BufReader::new(input),
                &mut output,
            ),
        )
        .await
        .unwrap();
        assert!(served.is_err(), "the session ends with the input error");
        let responses: Vec<Value> = String::from_utf8(output)
            .unwrap()
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect();
        let call = responses
            .iter()
            .find(|value| value["id"] == 7)
            .expect("the call in flight is still answered");
        assert_eq!(
            call["result"]["structuredContent"]["result"]["error"]["code"],
            "request_cancelled"
        );
        simulator.cancel();
        let sent = simulator
            .finish()
            .unwrap()
            .iter()
            .filter(|request| !request.method.is_empty())
            .count();
        assert_eq!(sent, identity_plans().len());
    }

    #[tokio::test]
    async fn a_withdrawn_call_stops_before_its_next_operation() {
        let (response, requests) = serve_ledger_masters_then(&[
            r#"{"jsonrpc":"2.0","method":"notifications/cancelled","params":{"requestId":7}}"#,
        ])
        .await;
        // The identity read in flight completes (abandoning it would not stop
        // Tally); the ledger read is never sent, and the call is refused as
        // withdrawn with partial evidence, never answered with a partial read.
        assert_eq!(requests, identity_plans().len(), "{response}");
        let result = &response["result"];
        assert_eq!(result["isError"], true, "{response}");
        assert_eq!(
            result["structuredContent"]["result"]["error"]["code"],
            "request_cancelled"
        );
        assert_eq!(result["structuredContent"]["evidence"]["state"], "partial");
    }

    #[tokio::test]
    async fn an_unrelated_notification_does_not_stop_the_call() {
        // Control: a cancellation naming another request, and a plain
        // notification, arrive while the call runs; it completes in full.
        let (response, requests) = serve_ledger_masters_then(&[
            r#"{"jsonrpc":"2.0","method":"notifications/cancelled","params":{"requestId":99}}"#,
            r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#,
        ])
        .await;
        assert_eq!(response["result"]["isError"], false, "{response}");
        assert_eq!(requests, basic_plans().len());
    }
}
