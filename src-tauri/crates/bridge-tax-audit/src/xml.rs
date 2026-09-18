//! Tally XML bytes to a small element tree, by the Tally text rule
//! (`docs/tax-audit/read-format-v1.md` section 10), which the reference Python implementation
//! follows too.
//!
//! 1. Content whose second byte is NUL is UTF-16LE without a BOM (how Tally serves master
//!    collections) and is decoded as such. Anything else goes through Bridge's own
//!    [`decode_tally_text_bytes_limited`]: a UTF-8, UTF-16LE or UTF-16BE BOM, else UTF-8.
//! 2. A document declaring a DTD or an entity is refused outright.
//! 3. A numeric character reference to a code point XML 1.0 forbids (Tally writes `&#4;` before
//!    its reserved values, e.g. `&#4; Primary`) becomes the text U+FFFD `#` *n* `;`, with *n* in
//!    decimal: `&#4; Primary` reads as `"\u{fffd}#4; Primary"`. A U+FFFD already in the document
//!    (literal, or a legal reference to it) that is directly followed by `#`, one to ten ASCII
//!    digits and `;` becomes U+FFFD `#65533;`, so the rewrite is injective and every value can
//!    be mapped back to the code points Tally sent. This is exactly what `bridge-tally-protocol`'s
//!    `tolerant_xml` does before its native group, ledger and voucher parsers read a response;
//!    [`mark_forbidden_references`] is a copy of it, because that module is private to Bridge's
//!    pinned protocol crate, and `tests/tally_text_rule.rs` holds the copy to Bridge's output.
//! 4. Raw characters are kept as themselves, C0 controls included: Tally separates the names in
//!    `PARENTSTRUCTURE` with raw U+0003, and nothing is stripped at decode time. A raw U+0000,
//!    U+FFFE or U+FFFF is refused (Tally has not been seen to send one; a NUL means the bytes
//!    were decoded with the wrong encoding). References to legal code points and the five
//!    predefined entities resolve as XML 1.0 says.
//!
//! Nothing here trims or case-folds a value. [`is_reserved_root`] decides whether a PARENT is
//! Tally's reserved root, and only the marker form is: a user group literally named `Primary`
//! is an ordinary group.
//!
//! The tree keeps what an `ElementTree` query needs: the element name, attributes (normalised as
//! XML 1.0 requires), the text before the first child (`ElementTree`'s `.text`) and the children.

use std::borrow::Cow;

use bridge_tally_protocol::{decode_tally_text_bytes_limited, TALLY_SANITIZED_ROOT_MARKER};
use quick_xml::escape::resolve_predefined_entity;
use quick_xml::events::Event;
use quick_xml::{Reader, XmlVersion};

use crate::error::{AuditError, Result};

/// Decompressed content above this size is refused (the reference engine's `MAX_XML_BYTES`).
pub const MAX_CONTENT_BYTES: usize = 512 * 1024 * 1024;

#[derive(Debug, Default)]
pub struct Element {
    pub name: String,
    pub attrs: Vec<(String, String)>,
    /// `ElementTree`'s `.text`: character data before the first child element.
    pub text: String,
    pub children: Vec<Element>,
}

impl Element {
    pub fn attr(&self, name: &str) -> Option<&str> {
        self.attrs
            .iter()
            .find(|(key, _)| key == name)
            .map(|(_, value)| value.as_str())
    }

    /// `ElementTree`'s `find(tag)`: the first direct child with this name.
    pub fn child(&self, name: &str) -> Option<&Element> {
        self.children.iter().find(|child| child.name == name)
    }

    /// `ElementTree`'s `findall(tag)`: every direct child with this name, in document order.
    pub fn children_named<'a>(&'a self, name: &'a str) -> impl Iterator<Item = &'a Element> {
        self.children.iter().filter(move |child| child.name == name)
    }

    /// The reference engine's `_t(el, tag)`: the first such child's text, stripped the way
    /// Python's `str.strip()` strips, or empty when the child is absent or has no text.
    pub fn child_text(&self, name: &str) -> &str {
        self.child(name).map_or("", |child| py_strip(&child.text))
    }

    /// `ElementTree`'s `iter(tag)`: this element and every descendant with this name, in
    /// document (pre-)order.
    pub fn descendants_named<'a>(&'a self, name: &'a str) -> Vec<&'a Element> {
        let mut out = Vec::new();
        let mut stack = vec![self];
        while let Some(element) = stack.pop() {
            if element.name == name {
                out.push(element);
            }
            stack.extend(element.children.iter().rev());
        }
        out
    }
}

/// Python's `str.isspace()` set: Unicode `White_Space` plus the four ASCII information
/// separators U+001C..U+001F, which Rust's `char::is_whitespace` does not include.
fn is_py_space(c: char) -> bool {
    c.is_whitespace() || ('\u{1c}'..='\u{1f}').contains(&c)
}

/// Python's `str.strip()` with no argument.
pub fn py_strip(text: &str) -> &str {
    text.trim_matches(is_py_space)
}

fn is_xml_char(c: char) -> bool {
    matches!(c, '\t' | '\n' | '\r' | '\u{20}'..='\u{d7ff}' | '\u{e000}'..='\u{fffd}' | '\u{10000}'..)
}

/// Tally's reserved value behind a `&#4;` reference (`Primary`, `Not Applicable`, ...), read
/// from decoded text: the value after the U+FFFD `#4;` marker, or `None` for any other text.
pub fn reserved_value(text: &str) -> Option<&str> {
    py_strip(text)
        .strip_prefix(TALLY_SANITIZED_ROOT_MARKER)
        .map(py_strip)
}

/// Whether a decoded PARENT names Tally's reserved top-level root (`&#4; Primary` on the wire).
/// A bare `Primary` is a group of that name, never the root.
pub fn is_reserved_root(text: &str) -> bool {
    reserved_value(text).is_some_and(|value| value.eq_ignore_ascii_case("primary"))
}

/// Decode stored content to text by the rules in the module docs (steps 1-3).
pub fn decode(content: &[u8], part: &str) -> Result<String> {
    if content.len() > MAX_CONTENT_BYTES {
        return Err(AuditError::parse(
            part,
            "decompressed size exceeds the limit",
        ));
    }
    let text = if content.len() > 1 && content[1] == 0 {
        if !content.len().is_multiple_of(2) {
            return Err(AuditError::parse(part, "truncated UTF-16LE content"));
        }
        let units: Vec<u16> = content
            .chunks_exact(2)
            .map(|pair| u16::from_le_bytes([pair[0], pair[1]]))
            .collect();
        String::from_utf16(&units)
            .map_err(|_| AuditError::parse(part, "invalid UTF-16LE content"))?
    } else {
        decode_tally_text_bytes_limited(content, MAX_CONTENT_BYTES)
            .map_err(|error| AuditError::parse(part, format!("undecodable content: {error:?}")))?
            .text
    };
    let head: String = text.chars().take(4096).collect::<String>().to_uppercase();
    if head.contains("<!DOCTYPE") || text.to_uppercase().contains("<!ENTITY") {
        return Err(AuditError::parse(
            part,
            "DTD or entity declaration in a Tally export",
        ));
    }
    Ok(match mark_forbidden_references(&text) {
        Cow::Borrowed(_) => text,
        Cow::Owned(marked) => marked,
    })
}

/// No scan may run past the longest token the rewrite could accept: `#`, at most ten u32
/// digits, `;`.
const MAX_MARKER_FORM_BYTES: usize = 12;

/// At most `limit` bytes, without splitting a UTF-8 character.
fn bounded(text: &str, limit: usize) -> &str {
    let mut end = limit.min(text.len());
    while end > 0 && !text.is_char_boundary(end) {
        end -= 1;
    }
    &text[..end]
}

/// Whether `rest` (the text after a U+FFFD) starts with the marker form `#<digits>;`.
fn collides_with_marker_form(rest: &str) -> bool {
    let window = bounded(rest, MAX_MARKER_FORM_BYTES);
    let Some(window) = window.strip_prefix('#') else {
        return false;
    };
    let digits = window.split(';').next().unwrap_or("");
    !digits.is_empty()
        && digits.len() < window.len()
        && digits.bytes().all(|byte| byte.is_ascii_digit())
}

fn is_xml_10_code_point(value: u32) -> bool {
    matches!(value, 0x9 | 0xA | 0xD | 0x20..=0xD7FF | 0xE000..=0xFFFD | 0x10000..=0x10FFFF)
}

/// Step 3 of the module docs, before parsing: a copy of `bridge-tally-protocol`'s
/// `tolerant_xml::sanitize_invalid_numeric_references` (private to that pinned crate). Scans,
/// terminators and number parsing are the same, including where the original stops: a `&#`
/// with no `;` within twelve bytes ends the rewrite, and the parser then refuses that
/// reference.
pub fn mark_forbidden_references(xml: &str) -> Cow<'_, str> {
    let mut scan = 0_usize;
    let mut copy_from = 0_usize;
    let mut output = None::<String>;
    let mut replacement_marker = xml.find('\u{fffd}');
    let mut numeric_reference = xml.find("&#");
    while scan < xml.len() {
        if replacement_marker.is_some_and(|marker| marker < scan) {
            replacement_marker = xml[scan..].find('\u{fffd}').map(|offset| scan + offset);
        }
        if numeric_reference.is_some_and(|reference| reference < scan) {
            numeric_reference = xml[scan..].find("&#").map(|offset| scan + offset);
        }
        let Some(start) = [numeric_reference, replacement_marker]
            .into_iter()
            .flatten()
            .min()
        else {
            break;
        };
        if replacement_marker == Some(start) {
            let after = start + '\u{fffd}'.len_utf8();
            if collides_with_marker_form(&xml[after..]) {
                let target = output.get_or_insert_with(|| String::with_capacity(xml.len()));
                target.push_str(&xml[copy_from..start]);
                target.push_str("\u{fffd}#65533;");
                copy_from = after;
            }
            scan = after;
            continue;
        }
        let Some(relative_end) = bounded(&xml[start + 1..], MAX_MARKER_FORM_BYTES).find(';') else {
            break;
        };
        let end = start + 1 + relative_end;
        let token = &xml[start + 2..end];
        let parsed = token
            .strip_prefix('x')
            .or_else(|| token.strip_prefix('X'))
            .and_then(|hex| u32::from_str_radix(hex, 16).ok())
            .or_else(|| token.parse::<u32>().ok());
        let forbidden = parsed.is_some_and(|value| !is_xml_10_code_point(value));
        let ambiguous_replacement =
            parsed == Some(0xfffd) && collides_with_marker_form(&xml[end + 1..]);
        if forbidden || ambiguous_replacement {
            let target = output.get_or_insert_with(|| String::with_capacity(xml.len()));
            target.push_str(&xml[copy_from..start]);
            target.push('\u{fffd}');
            target.push_str(&format!("#{};", parsed.unwrap_or_default()));
            copy_from = end + 1;
        }
        scan = end + 1;
    }
    match output {
        Some(mut output) => {
            output.push_str(&xml[copy_from..]);
            Cow::Owned(output)
        }
        None => Cow::Borrowed(xml),
    }
}

/// Step 4 of the module docs: raw characters are kept, except these three.
fn legal(text: &str, part: &str) -> Result<()> {
    match text
        .chars()
        .find(|c| matches!(c, '\u{0}' | '\u{fffe}' | '\u{ffff}'))
    {
        Some(c) => Err(AuditError::parse(
            part,
            format!(
                "character U+{:04X} is not allowed in Tally text",
                u32::from(c)
            ),
        )),
        None => Ok(()),
    }
}

fn normalize_eols(text: &str) -> String {
    text.replace("\r\n", "\n").replace('\r', "\n")
}

/// Parse decoded text into an element tree (step 4 of the module docs).
pub fn parse(text: &str, part: &str) -> Result<Element> {
    let err = |detail: String| AuditError::parse(part, detail);
    legal(text, part)?;
    let mut reader = Reader::from_str(text);
    reader.config_mut().trim_text(false);
    reader.config_mut().check_end_names = true;

    let mut stack: Vec<Element> = Vec::new();
    let mut root: Option<Element> = None;
    loop {
        let event = reader.read_event().map_err(|e| {
            err(format!(
                "malformed XML at byte {}: {e}",
                reader.error_position()
            ))
        })?;
        match event {
            Event::Start(ref start) | Event::Empty(ref start) => {
                if root.is_some() {
                    return Err(err("content after the document element".to_string()));
                }
                let empty = matches!(event, Event::Empty(_));
                let name = std::str::from_utf8(start.name().as_ref())
                    .map_err(|_| err("element name is not UTF-8".to_string()))?
                    .to_string();
                let mut attrs = Vec::new();
                for attr in start.attributes().with_checks(true) {
                    let attr = attr.map_err(|e| err(format!("malformed attribute: {e}")))?;
                    let key = std::str::from_utf8(attr.key.as_ref())
                        .map_err(|_| err("attribute name is not UTF-8".to_string()))?
                        .to_string();
                    let value = attr
                        .normalized_value(XmlVersion::Implicit1_0)
                        .map_err(|e| err(format!("attribute {key}: {e}")))?;
                    legal(&value, part)?;
                    attrs.push((key, value.into_owned()));
                }
                let element = Element {
                    name,
                    attrs,
                    ..Element::default()
                };
                if empty {
                    close(&mut stack, &mut root, element);
                } else {
                    stack.push(element);
                }
            }
            Event::End(_) => {
                let element = stack
                    .pop()
                    .ok_or_else(|| err("unbalanced end tag".to_string()))?;
                close(&mut stack, &mut root, element);
            }
            Event::Text(t) => {
                let text = t.xml10_content().map_err(|e| err(format!("text: {e}")))?;
                push_text(&mut stack, &text, part)?;
            }
            Event::CData(t) => {
                let text = t.decode().map_err(|e| err(format!("CDATA: {e}")))?;
                push_text(&mut stack, &normalize_eols(&text), part)?;
            }
            Event::GeneralRef(r) => {
                let resolved = if r.is_char_ref() {
                    let c = r
                        .resolve_char_ref()
                        .map_err(|e| err(format!("character reference: {e}")))?
                        .ok_or_else(|| err("character reference".to_string()))?;
                    if !is_xml_char(c) {
                        return Err(err(format!(
                            "reference to U+{:04X}, which XML does not allow",
                            u32::from(c)
                        )));
                    }
                    c.to_string()
                } else {
                    let name = r.decode().map_err(|e| err(format!("reference: {e}")))?;
                    resolve_predefined_entity(&name)
                        .ok_or_else(|| err(format!("undefined entity &{name};")))?
                        .to_string()
                };
                push_text(&mut stack, &resolved, part)?;
            }
            Event::DocType(_) => {
                return Err(err("DTD in a Tally export".to_string()));
            }
            Event::Decl(_) | Event::PI(_) | Event::Comment(_) => {}
            Event::Eof => break,
        }
    }
    if !stack.is_empty() {
        return Err(err("unclosed element at end of document".to_string()));
    }
    root.ok_or_else(|| err("no document element".to_string()))
}

fn close(stack: &mut [Element], root: &mut Option<Element>, element: Element) {
    match stack.last_mut() {
        Some(parent) => parent.children.push(element),
        None => *root = Some(element),
    }
}

fn push_text(stack: &mut [Element], text: &str, part: &str) -> Result<()> {
    match stack.last_mut() {
        // `ElementTree` keeps text after the first child as that child's tail, which no query
        // here reads; only the text before the first child is `.text`.
        Some(element) if element.children.is_empty() => element.text.push_str(text),
        Some(_) => {}
        None if py_strip(text).is_empty() => {}
        None => {
            return Err(AuditError::parse(
                part,
                "character data outside the document element",
            ))
        }
    }
    Ok(())
}

/// Decode and parse in one step.
pub fn read(content: &[u8], part: &str) -> Result<Element> {
    parse(&decode(content, part)?, part)
}
