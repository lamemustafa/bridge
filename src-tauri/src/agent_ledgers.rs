//! Ledgers for the local MCP adapter.
use super::*;

impl Server {
    pub(super) async fn ledger_masters(&self, args: &Value) -> Result<ToolOutcome, ToolFailure> {
        let guid = required_string(args, "company_guid")?;
        let (company, identity, mut evidence) = self.verified_company(guid).await?;
        let result: Result<ToolOutcome, ToolFailure> = async {
            let fields = optional_string(args, "fields")?.unwrap_or_else(|| "basic".to_string());
            let compliance = ledger_master_fields(&fields)?;
            let (mut ledgers, ledger_evidence) = if compliance {
                let (records, evidence) = self
                    .runtime
                    .fetch_agent_party_ledger_masters_with_evidence(self.tally_config(), &identity)
                    .await
                    .map_err(|error| ToolFailure::from_runtime("party_ledger_master_read_failed", error))?;
                (
                    records
                        .into_iter()
                        .map(|record| {
                            json!({
                                "name": party_name(record.ledger.name),
                                "parent": record.ledger.parent.returned_text(),
                                "opening_balance": record.ledger.opening_balance,
                                "party_gstin": record.ledger.party_gstin.returned_text(),
                                "compliance": mark_compliance_party_names(
                                    serde_json::to_value(record.fields).unwrap_or_default(),
                                ),
                            })
                        })
                        .collect::<Vec<_>>(),
                    evidence,
                )
            } else {
                let (records, evidence) = self
                    .runtime
                    .fetch_ledgers_with_evidence(self.tally_config(), &identity)
                    .await
                    .map_err(|error| ToolFailure::from_runtime("ledger_export_invalid", error))?;
                (
                    records
                        .into_iter()
                        .map(|ledger| {
                            json!({
                                "name": party_name(ledger.name),
                                "parent": ledger.parent.returned_text(),
                                "opening_balance": ledger.opening_balance,
                            })
                        })
                        .collect::<Vec<_>>(),
                    evidence,
                )
            };
            evidence = combine_evidence(evidence.clone(), evidence_from_runtime_read(ledger_evidence));
            if let Some(group) = optional_string(args, "group")? {
                ledgers.retain(|ledger| ledger["parent"].as_str() == Some(group.as_str()));
            }
            let offset = arg_usize(args, "offset", 0)?;
            let limit =
                arg_positive_usize(args, "limit", self.settings.max_rows)?.min(self.settings.max_rows);
            let total = ledgers.len();
            let page = ledgers
                .into_iter()
                .skip(offset)
                .take(limit)
                .map(|ledger| redact_value(ledger, self.settings.redaction))
                .collect::<Vec<_>>();
            let truncated = offset.saturating_add(page.len()) < total;
            let result = json!({"items": page, "offset": offset, "total": total, "fields": fields, "compliance": if compliance {"paired_party_ledger_master_source"} else {"not_requested"}});
            Ok(ToolOutcome {
                payload: json!({"company": company_json(&company, std::slice::from_ref(&company)), "result": result}),
                evidence: evidence.clone(),
                company_guid: Some(guid.to_string()),
                truncated,
            })
        }
        .await;
        result.map_err(|failure| failure.with_prior_evidence(evidence))
    }
}

#[cfg(test)]
mod tests {
    use bridge_tally_core::TallyDate;
    use bridge_tally_protocol::{
        native_outstandings::{render_party_ledger_master_request, NativeLedgerExportPeriod},
        outstandings_shared::DateBoundaryProfile,
        parse_native_party_ledger_master_records_with_evidence, GstDutyHead,
        GstDutyHeadObservation, PartyLedgerMasterFieldObservation,
    };

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
    fn a_duty_head_on_a_non_gst_ledger_is_contradictory_not_recognised() {
        // <TAXTYPE>Others</TAXTYPE><GSTDUTYHEAD>CGST</GSTDUTYHEAD> is a response
        // contradicting itself. Classifying head-first recognised it and never
        // consulted TAXTYPE, releasing the contradiction as valid compliance data.
        // Neither field is now asserted, and both raw values are kept so a
        // reviewer can see what Tally actually returned.
        let response = format!(
            "<ENVELOPE><HEADER><STATUS>1</STATUS></HEADER><BODY><DATA><CMPINFO><LEDGER>1</LEDGER></CMPINFO><COLLECTION><LEDGER NAME=\"Contradictory\" RESERVEDNAME=\"\"><GUID>{COMPANY_GUID}-00000042</GUID><BRIDGECOMPANYGUID>{COMPANY_GUID}</BRIDGECOMPANYGUID><MASTERID>66</MASTERID><ALTERID>66</ALTERID><PARENT>Duties &amp; Taxes</PARENT><TAXTYPE>Others</TAXTYPE><GSTDUTYHEAD>CGST</GSTDUTYHEAD><OPENINGBALANCE>0.00</OPENINGBALANCE><LANGUAGENAME.LIST><NAME.LIST><NAME>Localized Contradictory</NAME></NAME.LIST></LANGUAGENAME.LIST></LEDGER></COLLECTION></DATA></BODY></ENVELOPE>"
        );
        let parsed =
            parse_native_party_ledger_master_records_with_evidence(&response, COMPANY_GUID)
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
        let parsed =
            parse_native_party_ledger_master_records_with_evidence(&response, COMPANY_GUID)
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
            parsed
                .records
                .iter()
                .any(|row| matches!(
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
}
