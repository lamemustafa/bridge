//! Typed amounts, text-only XML fields and attribute admission.
use super::{NativeTrialBalanceAmount, NativeTrialBalanceError};
use bridge_tally_primitives::ExactDecimal;
use quick_xml::{
    events::{BytesStart, Event},
    name::QName,
    Reader,
};

/// An amount element's text, once its `TYPE` is `Amount`. The text is parsed
/// only for a row that is read ([`parse_amount_text`]); a row set aside keeps
/// its values unparsed.
pub(super) fn amount_text(
    element: &BytesStart<'_>,
    value: String,
) -> Result<String, NativeTrialBalanceError> {
    if required_attribute(element, b"TYPE", "trial_balance_amount_type_missing")? != "Amount" {
        return Err(NativeTrialBalanceError::InvalidResponse(
            "trial_balance_amount_type_invalid",
        ));
    }
    Ok(value)
}

pub(super) fn parse_amount_text(
    value: String,
) -> Result<NativeTrialBalanceAmount, NativeTrialBalanceError> {
    if value.is_empty() {
        Ok(NativeTrialBalanceAmount::PresentEmpty)
    } else {
        ExactDecimal::parse(value)
            .map(NativeTrialBalanceAmount::Present)
            .map_err(|_| NativeTrialBalanceError::InvalidAmount)
    }
}

/// Whether `text` is a currency composite as Tally writes it:
/// `<amount> @ <rate> = <base amount>`, for example
/// `-$ 100.00 @ I\u{20b9} 201/$  = -I\u{20b9} 20100.00`. Each amount is an optional
/// `-`, a symbol, one space and an ASCII decimal. The rate is a symbol, one
/// space, then either a decimal or nothing, then `/` and a symbol: a captured
/// Trial Balance closing had the empty form, `$ 0.00 @ I\u{20b9} /$  = I\u{20b9} 0.00`.
/// Exactly one ` @ ` and one ` = `; anything else is not a composite, so a
/// truncated or garbled value is refused as an invalid amount rather than set
/// aside. It only classifies: no value is ever read from a composite.
pub(crate) fn is_currency_composite(text: &str) -> bool {
    let Some((foreign, rest)) = split_once_exact(text, " @ ") else {
        return false;
    };
    let Some((rate, base)) = split_once_exact(rest, " = ") else {
        return false;
    };
    is_symbol_amount(foreign) && is_rate(rate.trim_end_matches(' ')) && is_symbol_amount(base)
}

fn split_once_exact<'a>(text: &'a str, separator: &str) -> Option<(&'a str, &'a str)> {
    let (left, right) = text.split_once(separator)?;
    (!right.contains(separator)).then_some((left, right))
}

fn is_symbol_amount(text: &str) -> bool {
    let text = text.strip_prefix('-').unwrap_or(text);
    match text.split_once(' ') {
        Some((symbol, amount)) => is_symbol(symbol) && is_ascii_decimal(amount),
        None => false,
    }
}

fn is_rate(text: &str) -> bool {
    let Some((symbol, rest)) = text.split_once(' ') else {
        return false;
    };
    let Some((number, per)) = rest.split_once('/') else {
        return false;
    };
    is_symbol(symbol) && (number.is_empty() || is_ascii_decimal(number)) && is_symbol(per)
}

fn is_symbol(text: &str) -> bool {
    !text.is_empty()
        && text.chars().all(|c| {
            !c.is_whitespace() && !c.is_ascii_digit() && !matches!(c, '-' | '@' | '=' | '/')
        })
}

fn is_ascii_decimal(text: &str) -> bool {
    let (whole, fraction) = text.split_once('.').unwrap_or((text, ""));
    !whole.is_empty()
        && whole.bytes().all(|b| b.is_ascii_digit())
        && fraction.bytes().all(|b| b.is_ascii_digit())
        && (!fraction.is_empty() || !text.ends_with('.'))
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
