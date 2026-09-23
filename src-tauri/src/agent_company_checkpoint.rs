//! Admit one company checkpoint without losing fragmented scalar content.
use super::*;
use quick_xml::events::Event;

pub(in crate::agent) fn parse_company_high_water(
    xml: &str,
    expected_guid: &str,
) -> Result<Value, String> {
    let row = company_high_water_row(xml, expected_guid)?;
    // Observe the master axis first. A company that has never held a voucher
    // returns ALTMSTID but omits ALTVCHID entirely, and `pre_import_mark` reads
    // the resulting `voucher_checkpoint_not_observed` as that empty book.
    //
    // This row already matched the requested company — `matched` guarantees that
    // whichever order these run in. What the ordering adds is narrower and is the
    // whole basis of the distinction: that the master axis was itself observed as
    // a number. Without it, a row carrying NEITHER axis would return the voucher
    // code, and a response Bridge could not read would be reported to the caller
    // as an empty book.
    let altmstid = observed_checkpoint(row.get("ALTMSTID"), "master")?;
    let altvchid = observed_checkpoint(row.get("ALTVCHID"), "voucher")?;
    Ok(json!({"altvchid": altvchid, "altmstid": altmstid}))
}

/// Both change marks of one company, `(vouchers, masters)`, with a company that
/// has never held a voucher reported as a voucher mark of zero.
///
/// The master axis must itself be observed, exactly as in
/// [`parse_company_high_water`]: only a row that carries `ALTMSTID` and omits
/// `ALTVCHID` is that empty book. A row carrying neither is refused.
pub(in crate::agent) fn parse_company_marks(
    xml: &str,
    expected_guid: &str,
) -> Result<(u64, u64), String> {
    // The voucher axis means exactly what every other read-side mark means.
    let vouchers = company_voucher_high_water(xml, expected_guid)?;
    let row = company_high_water_row(xml, expected_guid)?;
    let masters = observed_checkpoint(row.get("ALTMSTID"), "master")?;
    Ok((vouchers, masters))
}

/// The one row whose GUID is `expected_guid`. The whole response is parsed
/// before identity is checked, so a malformed row anywhere refuses as
/// `agent_read_protocol_invalid` even when the target also appears twice;
/// before bridge#574 an earlier duplicate reported the ambiguity first. Both
/// refuse, and no caller acts differently on the two codes.
fn company_high_water_row(
    xml: &str,
    expected_guid: &str,
) -> Result<BTreeMap<String, String>, String> {
    let mut matched = None;
    for row in company_high_water_rows(xml)? {
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
    matched.ok_or_else(|| "company_high_water_identity_absent".to_string())
}

/// The key under which a row keeps its `NAME` attribute. It cannot collide
/// with an element: element names never start with `@`.
const COMPANY_NAME_ATTRIBUTE: &str = "@NAME";

/// Every loaded company's row of the high-water collection, in response order,
/// with its `NAME` attribute. The collection names one company in
/// `SVCURRENTCOMPANY` but Tally returns a row for every loaded company
/// (measured 2026-09-21: 25 rows for 25 loaded companies).
fn company_high_water_rows(xml: &str) -> Result<Vec<BTreeMap<String, String>>, String> {
    let marked = mark_agent_xml(xml);
    let xml = marked.as_ref();
    validate_agent_envelope(xml)?;
    let invalid = || "agent_read_protocol_invalid".to_string();
    let mut reader = quick_xml::Reader::from_str(xml);
    reader.config_mut().trim_text(false);
    let mut current: Option<BTreeMap<String, String>> = None;
    let mut rows = Vec::new();
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
                    let mut row = BTreeMap::new();
                    for attribute in event.attributes().with_checks(true) {
                        let attribute = attribute.map_err(|_| invalid())?;
                        if attribute.key.as_ref().eq_ignore_ascii_case(b"NAME") {
                            let value = attribute
                                .decoded_and_normalized_value(
                                    quick_xml::XmlVersion::Implicit1_0,
                                    reader.decoder(),
                                )
                                .map_err(|_| invalid())?;
                            if row
                                .insert(COMPANY_NAME_ATTRIBUTE.to_string(), value.into_owned())
                                .is_some()
                            {
                                return Err(invalid());
                            }
                        }
                    }
                    current = Some(row);
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
                    rows.push(current.take().ok_or_else(invalid)?);
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
    Ok(rows)
}

/// One loaded company's change marks, as the high-water collection reported
/// them: the row's `NAME` attribute, its GUID, and both AlterID axes. A company
/// that has never held a voucher omits `ALTVCHID` and is reported as 0 on that
/// axis, exactly as [`company_voucher_high_water`] does.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(in crate::agent) struct LoadedCompanyMarks {
    pub(in crate::agent) name: String,
    pub(in crate::agent) guid: String,
    pub(in crate::agent) vouchers: u64,
    pub(in crate::agent) masters: u64,
}

/// Every loaded company's marks. A row without a name, a GUID or an observed
/// master axis is refused, so an unreadable row can never pass as "unchanged".
pub(in crate::agent) fn parse_all_company_marks(
    xml: &str,
) -> Result<Vec<LoadedCompanyMarks>, String> {
    company_high_water_rows(xml)?
        .into_iter()
        .map(|row| {
            let name = row
                .get(COMPANY_NAME_ATTRIBUTE)
                .filter(|name| !name.trim().is_empty())
                .ok_or_else(|| "company_marks_name_absent".to_string())?
                .clone();
            let guid = row
                .get("GUID")
                .map(|guid| guid.trim().to_string())
                .filter(|guid| !guid.is_empty())
                .ok_or_else(|| "company_marks_guid_absent".to_string())?;
            let masters = observed_checkpoint(row.get("ALTMSTID"), "master")?;
            let vouchers = match row.get("ALTVCHID") {
                None => 0,
                value => observed_checkpoint(value, "voucher")?,
            };
            Ok(LoadedCompanyMarks {
                name,
                guid,
                vouchers,
                masters,
            })
        })
        .collect()
}

/// The company's voucher AlterID high-water for a read-side corroboration.
///
/// A company that has never held a voucher omits ALTVCHID while its master axis
/// is observed; `parse_company_high_water` reports exactly that case as
/// `VOUCHER_CHECKPOINT_NOT_OBSERVED`, and for a read it means a mark of 0. Every
/// other refusal propagates, including a row carrying neither axis, which the
/// parser keeps distinct by observing the master axis first.
pub(in crate::agent) fn company_voucher_high_water(
    xml: &str,
    expected_guid: &str,
) -> Result<u64, String> {
    match parse_company_high_water(xml, expected_guid) {
        Ok(mark) => mark["altvchid"]
            .as_u64()
            .ok_or_else(|| "voucher_checkpoint_invalid".to_string()),
        Err(code) if code == VOUCHER_CHECKPOINT_NOT_OBSERVED => Ok(0),
        Err(code) => Err(code),
    }
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
        // A malformed later row takes precedence over the ambiguity: the whole
        // response is parsed before identity is checked. Both refuse.
        let malformed = altered.replacen(
            "</COLLECTION>",
            "<COMPANY NAME=\"Synthetic Broken\"><GUID>a</GUID><GUID>b</GUID></COMPANY></COLLECTION>",
            1,
        );
        assert_eq!(
            parse_company_high_water(&malformed, guid),
            Err("agent_read_protocol_invalid".into())
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
        assert_ne!(
            empty_book, xml,
            "fixture no longer carries the voucher axis"
        );
        assert_eq!(
            parse_company_high_water(&empty_book, guid),
            Err(VOUCHER_CHECKPOINT_NOT_OBSERVED.to_string())
        );
        // Ordering guard, and the reason the master axis is observed first: with
        // BOTH axes absent this row has not been shown to parse, so the voucher
        // code must not be returned. Were it returned here, pre_import_mark would
        // report an empty book for a response Bridge could not read.
        let neither = empty_book.replacen("<ALTMSTID TYPE=\"Number\"> 328</ALTMSTID>", "", 1);
        assert_ne!(
            neither, empty_book,
            "fixture no longer carries the master axis"
        );
        assert_eq!(
            parse_company_high_water(&neither, guid),
            Err("master_checkpoint_not_observed".to_string())
        );
    }

    #[test]
    fn corroboration_mark_reads_only_the_empty_book_as_zero() {
        let xml = book_extents_fixture();
        let guid = "bb8ad19e-6aef-4239-a917-87fec0c6215e";
        assert_eq!(company_voucher_high_water(&xml, guid), Ok(101_605));
        let empty_book = xml.replacen("<ALTVCHID TYPE=\"Number\"> 101605</ALTVCHID>", "", 1);
        assert_eq!(company_voucher_high_water(&empty_book, guid), Ok(0));
        // Every other refusal must propagate unchanged, never read as an empty book.
        let neither = empty_book.replacen("<ALTMSTID TYPE=\"Number\"> 328</ALTMSTID>", "", 1);
        let invalid_scalar = xml.replacen("101605", "101<OTHER>6</OTHER>05", 1);
        let empty_scalar = xml.replacen(
            "<ALTVCHID TYPE=\"Number\"> 101605</ALTVCHID>",
            "<ALTVCHID/>",
            1,
        );
        for (altered, code) in [
            (&neither, "master_checkpoint_not_observed"),
            (&invalid_scalar, "agent_read_protocol_invalid"),
            (&empty_scalar, "voucher_checkpoint_invalid"),
            (&xml, "company_high_water_identity_absent"),
        ] {
            let expected_guid = if altered == &xml {
                "missing-guid"
            } else {
                guid
            };
            assert_eq!(
                company_voucher_high_water(altered, expected_guid),
                Err(code.to_string()),
                "{code}"
            );
            assert_eq!(
                parse_company_high_water(altered, expected_guid),
                Err(code.to_string()),
                "helper and parser refuse identically: {code}"
            );
        }
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
