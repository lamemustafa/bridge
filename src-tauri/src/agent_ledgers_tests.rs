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

    async fn call(plans: Vec<ScenarioPlan>, args: Value) -> (Value, usize) {
        call_with_settings(plans, args, Redaction::None, 200_000).await
    }

    async fn call_with_settings(
        plans: Vec<ScenarioPlan>,
        args: Value,
        redaction: Redaction,
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
            redaction,
            import_enabled: false,
            writes_enabled: false,
        });
        let response = server.call_tool("ledger_masters", args).await;
        // A refusal may legitimately stop before the remaining replay plans.
        // Cancel those plans so the assertion reports the actual tool refusal.
        let refused = response["isError"] == true;
        if refused {
            simulator.cancel();
        }
        let observed = simulator
            .finish()
            .expect("replay transport must complete without failure");
        let requests = observed.iter().filter(|request| !request.cancelled).count();
        (response, requests)
    }

    #[tokio::test]
    async fn diagnostic_mode_accounts_for_every_captured_pair() {
        let plans = compliance_plans(masters(), balances());
        let expected_requests = plans.len();
        let (response, requests) = call(
            plans,
            json!({"company_guid":GUID,"fields":"compliance_diagnostics"}),
        )
        .await;
        let rows = items(&response);
        assert_eq!(requests, expected_requests);
        assert_eq!(rows.len(), 9);
        assert!(rows.iter().all(|row| row["join_state"] == "matched"));
        let content = &response["structuredContent"];
        assert_eq!(content["result"]["state"], "complete");
        assert_eq!(content["evidence"]["state"], "complete");
        assert_eq!(
            content["result"]["coverage"],
            json!({"master_observations":9,"balance_observations":9,"matched_pairs":9,
                "unresolved_master_observations":0,"unresolved_balance_observations":0}),
        );
    }

    // Negative-only fault injection into a captured response. This is not
    // evidence that a real Tally instance emits the injected mismatch.
    fn unmatched_balance() -> String {
        let original = balances();
        let needle = "NAME=\"Bridge Nested Debtor WR4\"";
        assert_eq!(original.matches(needle).count(), 1);
        original.replace(needle, "NAME=\"Bridge Unmatched Diagnostic WR4\"")
    }

    #[tokio::test]
    async fn diagnostic_partial_state_is_global_even_on_matched_or_empty_pages() {
        let mut commitments = None;
        for (offset, limit, count) in [(0, 1, 1), (8, 10, 2), (10, 1, 0), (50, 1, 0)] {
            let plans = compliance_plans(masters(), unmatched_balance());
            let expected_requests = plans.len();
            let (response, requests) = call(
                plans,
                json!({
                    "company_guid":GUID,"fields":"compliance_diagnostics",
                    "offset":offset,"limit":limit,
                }),
            )
            .await;
            let rows = items(&response);
            assert_eq!(requests, expected_requests);
            assert_eq!(rows.len(), count);
            let content = &response["structuredContent"];
            let result = &content["result"];
            let current = (
                content["evidence"]["request_sha256"].clone(),
                content["evidence"]["response_sha256"].clone(),
            );
            if let Some(prior) = &commitments {
                assert_eq!(
                    &current, prior,
                    "unchanged source pages retain the same commitments"
                );
            }
            commitments = Some(current);
            assert_eq!(result["state"], "partial");
            assert_eq!(result["partial_reason"], "exact_join_unresolved");
            assert_eq!(content["evidence"]["state"], "partial");
            assert_eq!(content["evidence"]["reason_code"], "exact_join_unresolved");
            assert_eq!(result["total"], 10);
            assert_eq!(result["total_basis"], "diagnostic_observations");
            assert_eq!(
                result["coverage"],
                json!({
                    "master_observations":9,"balance_observations":9,"matched_pairs":8,
                    "unresolved_master_observations":1,"unresolved_balance_observations":1,
                })
            );
            assert!(content["evidence"]["bytes"].as_u64().unwrap() > 0);
            assert!(!content["evidence"]["response_sha256"]
                .as_str()
                .unwrap()
                .is_empty());
            if offset == 0 {
                assert_eq!(rows[0]["join_state"], "matched");
                assert_eq!(result["next_offset"], 1);
                assert_eq!(content["truncated"], true);
            } else {
                assert_eq!(content["truncated"], false);
                assert!(result["next_offset"].is_null());
            }
            for row in rows.iter().filter(|row| row["join_state"] == "unresolved") {
                assert!(row["source_ordinal"].is_u64());
                assert!(row["parent"].is_string());
                for forbidden in [
                    "opening_balance",
                    "closing_balance",
                    "party_gstin",
                    "compliance",
                    "guid",
                ] {
                    assert!(row.get(forbidden).is_none(), "{row}");
                }
            }
            if offset == 8 {
                assert_eq!(
                    row(rows, "Bridge Nested Debtor WR4")["reason"],
                    "master_missing_balance"
                );
                assert_eq!(
                    row(rows, "Bridge Unmatched Diagnostic WR4")["reason"],
                    "balance_without_master"
                );
            }
        }
    }

    #[tokio::test]
    async fn diagnostic_mode_rejects_every_group_filter_before_ledger_reads() {
        for extra in [
            json!({"group":"Sundry Debtors"}),
            json!({"group_scope":"immediate"}),
            json!({"group_scope":"ancestry"}),
        ] {
            let mut args = json!({"company_guid":GUID,"fields":"compliance_diagnostics"});
            args.as_object_mut()
                .unwrap()
                .extend(extra.as_object().unwrap().clone());
            let (response, requests) = call(identity_plans(), args).await;
            assert_eq!(response["isError"], true);
            assert_eq!(requests, 4);
            assert_eq!(
                response["structuredContent"]["result"]["error"]["code"],
                "compliance_diagnostics_does_not_support_group_filter"
            );
        }
    }

    #[tokio::test]
    async fn diagnostic_mode_masks_unresolved_names_and_parents() {
        let (response, _) = call_with_settings(
            compliance_plans(masters(), unmatched_balance()),
            json!({"company_guid":GUID,"fields":"compliance_diagnostics","offset":8}),
            Redaction::MaskParties,
            200_000,
        )
        .await;
        let rows = items(&response);
        assert_eq!(rows.len(), 2);
        let encoded = serde_json::to_string(&response).unwrap();
        for raw in [
            "Bridge Nested Debtor WR4",
            "Bridge Unmatched Diagnostic WR4",
            "Bridge Nested Debtors WR4",
        ] {
            assert!(!encoded.contains(raw), "raw party name leaked: {raw}");
        }
        assert!(rows.iter().all(|row| row["parent"].is_string()));
        assert_eq!(
            response["structuredContent"]["result"]["coverage"]["unresolved_master_observations"],
            1
        );
    }

    #[tokio::test]
    async fn diagnostic_byte_cap_keeps_global_coverage_and_resumable_offset() {
        let args = json!({"company_guid":GUID,"fields":"compliance_diagnostics"});
        let (full, _) = call(
            compliance_plans(masters(), unmatched_balance()),
            args.clone(),
        )
        .await;
        assert_eq!(items(&full).len(), 10);
        let (unchanged, _) = call(compliance_plans(masters(), balances()), args.clone()).await;
        assert_ne!(
            full["structuredContent"]["evidence"]["response_sha256"],
            unchanged["structuredContent"]["evidence"]["response_sha256"],
            "a changed source response must change its evidence commitment"
        );

        let cap = serde_json::to_vec(&full).unwrap().len() / 2;
        let (bounded, _) = call_with_settings(
            compliance_plans(masters(), unmatched_balance()),
            args,
            Redaction::None,
            cap,
        )
        .await;
        let rows = items(&bounded);
        assert!(!rows.is_empty());
        assert!(rows.len() < 10);
        assert!(serde_json::to_vec(&bounded).unwrap().len() <= cap);
        let content = &bounded["structuredContent"];
        assert_eq!(content["truncated"], true);
        assert_eq!(content["result"]["next_offset"], rows.len());
        assert_eq!(content["result"]["state"], "partial");
        assert_eq!(content["evidence"]["state"], "partial");
        assert_eq!(
            content["result"]["coverage"],
            full["structuredContent"]["result"]["coverage"]
        );
    }

    #[tokio::test]
    async fn compliance_remains_strict_when_diagnostic_mode_can_explain_a_mismatch() {
        let plans = compliance_plans(masters(), unmatched_balance());
        // Strict conversion refuses before the final company/status/company bracket.
        let expected_requests = plans.len() - 3;
        let (response, requests) =
            call(plans, json!({"company_guid":GUID,"fields":"compliance"})).await;
        assert_eq!(requests, expected_requests);
        assert_eq!(response["isError"], true);
        assert_eq!(
            response["structuredContent"]["result"]["error"]["code"],
            "party_ledger_master_read_failed"
        );
        assert!(response["structuredContent"]["result"]
            .get("items")
            .is_none());
        assert_eq!(
            response["structuredContent"]["evidence"]["state"],
            "partial"
        );
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

    #[tokio::test]
    async fn ancestry_scope_with_basic_fields_is_refused_before_any_ledger_read() {
        for args in [
            json!({"company_guid":GUID,"fields":"basic","group_scope":"ancestry"}),
            // `fields` defaults to basic, so omitting it must refuse too.
            json!({"company_guid":GUID,"group":"Loans (Liability)","group_scope":"ancestry"}),
        ] {
            let (response, requests) = call(identity_plans(), args.clone()).await;
            assert_eq!(response["isError"], true, "{args}");
            assert_eq!(
                response["structuredContent"]["result"]["error"]["code"],
                "group_scope_ancestry_requires_compliance_fields",
                "{args}"
            );
            // Only the identity pair was served: no profile, ledger or group
            // read followed the refusal.
            assert_eq!(requests, 4, "{args}");
        }
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
}
