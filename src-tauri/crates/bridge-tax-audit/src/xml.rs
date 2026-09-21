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
//!    be mapped back to the code points Tally sent. This is `bridge-tally-protocol`'s public
//!    [`bridge_tally_protocol::mark_forbidden_numeric_references`], which its own native group,
//!    ledger and voucher parsers apply before reading a response too
//!    (`docs/tally/TALLY_PROTOCOL_REFERENCE.md` §1.1(d)). A `&#` whose `;` is not within the
//!    rewrite's twelve-byte scan window is left as written, and the scan continues past it so a
//!    later forbidden reference in the same document is still marked; step 4 below is what then
//!    rejects that unmarked reference, not the rewrite itself.
//! 4. Raw characters are kept as themselves, C0 controls included: Tally separates the names in
//!    `PARENTSTRUCTURE` with raw U+0003, and nothing is stripped at decode time. A raw U+0000,
//!    U+FFFE or U+FFFF is refused (Tally has not been seen to send one; a NUL means the bytes
//!    were decoded with the wrong encoding). A numeric reference to one of those three, or to a
//!    surrogate or a code point beyond U+10FFFF, is refused the same way; quick-xml itself only
//!    refuses a reference to U+0000, a surrogate or beyond U+10FFFF; a reference it resolves to a
//!    raw C0 control, U+FFFE or U+FFFF is caught by this same check afterwards. Legal references
//!    and the five predefined entities resolve as XML 1.0 says.
//!
//! Nothing here trims or case-folds a value. [`is_reserved_root`] decides whether a PARENT is
//! Tally's reserved root, and only the marker form is: a user group literally named `Primary`
//! is an ordinary group.
//!
//! The tree keeps what an `ElementTree` query needs: the element name, attributes (normalised as
//! XML 1.0 requires), the text before the first child (`ElementTree`'s `.text`) and the children.

use std::borrow::Cow;

use bridge_tally_protocol::{
    decode_tally_text_bytes_limited, mark_forbidden_numeric_references, TALLY_SANITIZED_ROOT_MARKER,
};
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
pub(crate) fn is_py_space(c: char) -> bool {
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
    Ok(match mark_forbidden_numeric_references(&text) {
        Cow::Borrowed(_) => text,
        Cow::Owned(marked) => marked,
    })
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
