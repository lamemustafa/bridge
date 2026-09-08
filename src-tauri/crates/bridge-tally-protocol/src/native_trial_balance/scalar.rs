//! Typed amounts, text-only XML fields and attribute admission.
use super::{NativeTrialBalanceAmount, NativeTrialBalanceError};
use bridge_tally_primitives::ExactDecimal;
use quick_xml::{
    events::{BytesStart, Event},
    name::QName,
    Reader,
};

pub(super) fn parse_amount(
    element: &BytesStart<'_>,
    value: String,
) -> Result<NativeTrialBalanceAmount, NativeTrialBalanceError> {
    if required_attribute(element, b"TYPE", "trial_balance_amount_type_missing")? != "Amount" {
        return Err(NativeTrialBalanceError::InvalidResponse(
            "trial_balance_amount_type_invalid",
        ));
    }
    if value.is_empty() {
        Ok(NativeTrialBalanceAmount::PresentEmpty)
    } else {
        ExactDecimal::parse(value)
            .map(NativeTrialBalanceAmount::Present)
            .map_err(|_| NativeTrialBalanceError::InvalidAmount)
    }
}

pub(super) fn set_once<T>(
    slot: &mut Option<T>,
    value: T,
    duplicate: &'static str,
) -> Result<(), NativeTrialBalanceError> {
    if slot.replace(value).is_some() {
        Err(NativeTrialBalanceError::InvalidResponse(duplicate))
    } else {
        Ok(())
    }
}

pub(super) fn required_attribute(
    element: &BytesStart<'_>,
    key: &[u8],
    missing: &'static str,
) -> Result<String, NativeTrialBalanceError> {
    let mut found = None;
    for attribute in element.attributes() {
        let attribute = attribute.map_err(|_| {
            NativeTrialBalanceError::InvalidResponse("trial_balance_attribute_malformed")
        })?;
        if !attribute.key.as_ref().eq_ignore_ascii_case(key) {
            continue;
        }
        if found.is_some() {
            return Err(NativeTrialBalanceError::InvalidResponse(
                "trial_balance_attribute_duplicate",
            ));
        }
        let value = attribute
            .normalized_value(quick_xml::XmlVersion::Implicit1_0)
            .map_err(|_| {
                NativeTrialBalanceError::InvalidResponse("trial_balance_attribute_malformed")
            })?
            .into_owned();
        if value.is_empty() {
            return Err(NativeTrialBalanceError::InvalidResponse(missing));
        }
        found = Some(value);
    }
    found.ok_or(NativeTrialBalanceError::InvalidResponse(missing))
}

pub(super) fn read_element_text(
    reader: &mut Reader<&[u8]>,
    name: QName<'_>,
) -> Result<String, NativeTrialBalanceError> {
    let mut value = String::new();
    loop {
        match reader
            .read_event()
            .map_err(|_| NativeTrialBalanceError::InvalidResponse("trial_balance_xml_malformed"))?
        {
            Event::Text(text) => {
                let decoded = text.decode().map_err(|_| {
                    NativeTrialBalanceError::InvalidResponse("trial_balance_xml_invalid_encoding")
                })?;
                let unescaped = quick_xml::escape::unescape(&decoded).map_err(|_| {
                    NativeTrialBalanceError::InvalidResponse("trial_balance_xml_invalid_escape")
                })?;
                value.push_str(&unescaped);
            }
            Event::GeneralRef(reference) => {
                let decoded = reference.decode().map_err(|_| {
                    NativeTrialBalanceError::InvalidResponse("trial_balance_xml_invalid_encoding")
                })?;
                match decoded.as_ref() {
                    "amp" => value.push('&'),
                    "lt" => value.push('<'),
                    "gt" => value.push('>'),
                    "quot" => value.push('"'),
                    "apos" => value.push('\''),
                    _ => {
                        return Err(NativeTrialBalanceError::InvalidResponse(
                            "trial_balance_scalar_general_reference_invalid",
                        ))
                    }
                }
            }
            Event::End(end) if end.name() == name => return Ok(value),
            Event::Start(_) | Event::Empty(_) | Event::CData(_) | Event::Comment(_) => {
                return Err(NativeTrialBalanceError::InvalidResponse(
                    "trial_balance_scalar_not_text_only",
                ))
            }
            Event::Eof => {
                return Err(NativeTrialBalanceError::InvalidResponse(
                    "trial_balance_scalar_unterminated",
                ))
            }
            _ => {
                return Err(NativeTrialBalanceError::InvalidResponse(
                    "trial_balance_scalar_not_text_only",
                ))
            }
        }
    }
}

pub(super) fn skip_subtree(reader: &mut Reader<&[u8]>) -> Result<(), NativeTrialBalanceError> {
    let mut depth = 1_u32;
    loop {
        match reader
            .read_event()
            .map_err(|_| NativeTrialBalanceError::InvalidResponse("trial_balance_xml_malformed"))?
        {
            Event::Start(_) => depth += 1,
            Event::End(_) => {
                depth -= 1;
                if depth == 0 {
                    return Ok(());
                }
            }
            Event::Eof => {
                return Err(NativeTrialBalanceError::InvalidResponse(
                    "trial_balance_subtree_unterminated",
                ))
            }
            _ => {}
        }
    }
}

pub(super) fn path_is(path: &[Vec<u8>], expected: &[&[u8]]) -> bool {
    path.len() == expected.len()
        && path
            .iter()
            .zip(expected)
            .all(|(part, expected)| part.as_slice() == *expected)
}
