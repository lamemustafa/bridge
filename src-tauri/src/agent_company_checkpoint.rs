//! Admit one company checkpoint without losing fragmented scalar content.
use super::*;
use quick_xml::events::Event;

pub(in crate::agent) fn parse_company_high_water(
    xml: &str,
    expected_guid: &str,
) -> Result<Value, String> {
    validate_agent_envelope(xml)?;
    let invalid = || "agent_read_protocol_invalid".to_string();
    let mut reader = quick_xml::Reader::from_str(xml);
    reader.config_mut().trim_text(false);
    let mut current: Option<BTreeMap<String, String>> = None;
    let mut matched = None;
    let mut tag = String::new();
    let mut scope = NativeCollectionScope::default();
    let scalar = |name: &str| matches!(name, "GUID" | "ALTVCHID" | "ALTMSTID");
    loop {
        match reader.read_event() {
            Ok(event @ (Event::Start(_) | Event::Empty(_))) => {
                // Empty elements need the same occurrence admission as text fields.
                let empty = matches!(event, Event::Empty(_));
                let event = match event {
                    Event::Start(event) | Event::Empty(event) => event,
                    _ => unreachable!(),
                };
                let name = String::from_utf8_lossy(event.name().as_ref()).to_ascii_uppercase();
                if scope.field("COMPANY") && scalar(&tag) {
                    return Err(invalid());
                }
                if name == "COMPANY" && scope.collection() {
                    if empty {
                        return Err(invalid());
                    }
                    current = Some(BTreeMap::new());
                }
                if scope.row("COMPANY") && scalar(&name) {
                    claim_agent_scalar(current.as_mut().ok_or_else(invalid)?, &name)?;
                }
                scope.start(name.clone());
                if empty {
                    scope.end(&name)?;
                    tag.clear();
                } else {
                    tag = name;
                }
            }
            Ok(Event::Text(text)) => {
                if let Some(row) = current
                    .as_mut()
                    .filter(|_| scope.field("COMPANY") && scalar(&tag))
                {
                    append_agent_text(row, &tag, decoded_agent_text(text)?);
                }
            }
            Ok(Event::GeneralRef(reference)) => {
                if let Some(row) = current
                    .as_mut()
                    .filter(|_| scope.field("COMPANY") && scalar(&tag))
                {
                    append_agent_text(row, &tag, decoded_agent_reference(reference)?);
                }
            }
            Ok(Event::CData(text)) => {
                if let Some(row) = current
                    .as_mut()
                    .filter(|_| scope.field("COMPANY") && scalar(&tag))
                {
                    append_agent_text(
                        row,
                        &tag,
                        text.decode().map_err(|_| invalid())?.into_owned(),
                    );
                }
            }
            Ok(Event::End(event)) => {
                let end = String::from_utf8_lossy(event.name().as_ref()).to_ascii_uppercase();
                if scope.row("COMPANY") {
                    let row = current.take().ok_or_else(invalid)?;
                    if row
                        .get("GUID")
                        .is_some_and(|guid| guid.trim().eq_ignore_ascii_case(expected_guid))
                    {
                        if matched.is_some() {
                            return Err("company_high_water_identity_ambiguous".into());
                        }
                        matched = Some(row);
                    }
                }
                scope.end(&end)?;
                tag.clear();
            }
            Ok(Event::Eof) => break,
            Err(_) => return Err(invalid()),
            _ => {}
        }
    }
    scope.finish()?;
    let row = matched.ok_or_else(|| "company_high_water_identity_absent".to_string())?;
    // Observe the master axis first. A company that has never held a voucher
    // returns ALTMSTID but omits ALTVCHID entirely, and `pre_import_mark` reads
    // the resulting `voucher_checkpoint_not_observed` as that empty book. That
    // reading is only sound while the master axis has already been admitted: it
    // is what proves this row parsed and matched the requested company, rather
    // than a response Bridge failed to read. Evaluating the voucher axis first
    // would return the same code for a row that never parsed at all, and the
    // distinction would silently become unsound.
    let altmstid = observed_checkpoint(row.get("ALTMSTID"), "master")?;
    let altvchid = observed_checkpoint(row.get("ALTVCHID"), "voucher")?;
    Ok(json!({"altvchid": altvchid, "altmstid": altmstid}))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn captured_company_checkpoint_preserves_fragments_and_refuses_ambiguity() {
        let bytes = include_bytes!("../crates/bridge-tally-protocol/tests/fixtures/agent/native-company-book-extents.utf16le.xml");
        let words = bytes
            .chunks_exact(2)
            .map(|pair| u16::from_le_bytes([pair[0], pair[1]]))
            .collect::<Vec<_>>();
        let xml = String::from_utf16(&words).unwrap();
        let guid = "bb8ad19e-6aef-4239-a917-87fec0c6215e";
        let expected = json!({"altvchid":101605,"altmstid":328});
        assert_eq!(parse_company_high_water(&xml, guid), Ok(expected.clone()));
        for replacement in ["101&#54;05", "101<![CDATA[6]]>05"] {
            let altered = xml.replacen("101605", replacement, 1);
            assert_ne!(altered, xml);
            assert_eq!(
                parse_company_high_water(&altered, guid),
                Ok(expected.clone())
            );
        }
        for replacement in ["101<OTHER>6</OTHER>05", "101605</ALTVCHID><ALTVCHID>0"] {
            let altered = xml.replacen("101605", replacement, 1);
            assert_eq!(
                parse_company_high_water(&altered, guid),
                Err("agent_read_protocol_invalid".into())
            );
        }
        let start = xml.find("<COMPANY NAME=").unwrap();
        let end = start + xml[start..].find("</COMPANY>").unwrap() + "</COMPANY>".len();
        let altered = xml.replacen(
            "</COLLECTION>",
            &format!("{}</COLLECTION>", &xml[start..end]),
            1,
        );
        assert_eq!(
            parse_company_high_water(&altered, guid),
            Err("company_high_water_identity_ambiguous".into())
        );
    }

    fn book_extents_fixture() -> String {
        let bytes = include_bytes!("../crates/bridge-tally-protocol/tests/fixtures/agent/native-company-book-extents.utf16le.xml");
        let words = bytes
            .chunks_exact(2)
            .map(|pair| u16::from_le_bytes([pair[0], pair[1]]))
            .collect::<Vec<_>>();
        String::from_utf16(&words).unwrap()
    }

    #[test]
    fn an_empty_book_omits_the_voucher_axis_and_keeps_the_master_axis() {
        let xml = book_extents_fixture();
        let guid = "bb8ad19e-6aef-4239-a917-87fec0c6215e";
        // Observed live on licensed TallyPrime 7.1 Gold: a company that has never
        // held a voucher returns ALTMSTID and omits ALTVCHID entirely — not a zero.
        let empty_book = xml.replacen("<ALTVCHID TYPE=\"Number\"> 101605</ALTVCHID>", "", 1);
        assert_ne!(empty_book, xml, "fixture no longer carries the voucher axis");
        assert_eq!(
            parse_company_high_water(&empty_book, guid),
            Err(VOUCHER_CHECKPOINT_NOT_OBSERVED.to_string())
        );
        // Ordering guard, and the reason the master axis is observed first: with
        // BOTH axes absent this row has not been shown to parse, so the voucher
        // code must not be returned. Were it returned here, pre_import_mark would
        // report an empty book for a response Bridge could not read.
        let neither = empty_book.replacen("<ALTMSTID TYPE=\"Number\"> 328</ALTMSTID>", "", 1);
        assert_ne!(neither, empty_book, "fixture no longer carries the master axis");
        assert_eq!(
            parse_company_high_water(&neither, guid),
            Err("master_checkpoint_not_observed".to_string())
        );
    }

    #[test]
    fn voucher_axis_absence_matches_its_named_code() {
        // pre_import_mark matches VOUCHER_CHECKPOINT_NOT_OBSERVED as a literal
        // against what observed_checkpoint formats from its axis argument. Pin
        // the two together so renaming the axis cannot silently stop the match
        // and quietly restore the collapsed refusal.
        assert_eq!(
            observed_checkpoint(None, "voucher"),
            Err(VOUCHER_CHECKPOINT_NOT_OBSERVED.to_string())
        );
    }
}
