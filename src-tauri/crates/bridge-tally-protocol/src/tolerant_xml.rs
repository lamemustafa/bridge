use std::{borrow::Cow, fmt::Write as _};

#[cfg(test)]
use std::cell::Cell;

#[cfg(test)]
thread_local! {
    static NUMERIC_REFERENCE_SEARCHES: Cell<Option<usize>> = const { Cell::new(None) };
    static NUMERIC_REFERENCE_TERMINATOR_SEARCH_BYTES: Cell<Option<usize>> = const { Cell::new(None) };
    static MARKER_FORM_SEARCH_BYTES: Cell<Option<usize>> = const { Cell::new(None) };
}

/// No scan in this module may run past the longest token it could accept.
/// An emitted marker form is `#` + at most ten u32 digits + `;`.
const MAX_MARKER_FORM_BYTES: usize = 12;

/// Truncate to at most `limit` bytes without splitting a UTF-8 character.
fn bounded(text: &str, limit: usize) -> &str {
    let mut end = limit.min(text.len());
    while end > 0 && !text.is_char_boundary(end) {
        end -= 1;
    }
    &text[..end]
}

/// A U+FFFD is ambiguous with an emitted marker only when the text that follows
/// it is `#<digits>;` -- that is the exact shape this function emits. Anywhere
/// else a literal U+FFFD is ordinary, legal content and must survive untouched.
fn collides_with_marker_form(rest: &str) -> bool {
    // Bounded: an unbounded `split(';')` scans the whole remaining response
    // once per marker, which is quadratic on input carrying many `#`-prefixed
    // markers and no terminator.
    let window = bounded(rest, MAX_MARKER_FORM_BYTES);
    #[cfg(test)]
    MARKER_FORM_SEARCH_BYTES.with(|searched_bytes| {
        if let Some(count) = searched_bytes.get() {
            searched_bytes.set(Some(count + window.len()));
        }
    });
    let Some(window) = window.strip_prefix('#') else {
        return false;
    };
    let digits = window.split(';').next().unwrap_or("");
    !digits.is_empty()
        && digits.len() < window.len()
        && digits.bytes().all(|byte| byte.is_ascii_digit())
}

fn find_numeric_reference(xml: &str) -> Option<usize> {
    #[cfg(test)]
    NUMERIC_REFERENCE_SEARCHES.with(|searches| {
        if let Some(count) = searches.get() {
            searches.set(Some(count + 1));
        }
    });
    xml.find("&#")
}

fn find_numeric_reference_terminator(reference: &str) -> Option<usize> {
    // Shares the module's structural scan limit. `bounded` also preserves a
    // UTF-8 boundary because malformed responses are untrusted text.
    let search = bounded(reference, MAX_MARKER_FORM_BYTES);
    #[cfg(test)]
    NUMERIC_REFERENCE_TERMINATOR_SEARCH_BYTES.with(|searched_bytes| {
        if let Some(count) = searched_bytes.get() {
            searched_bytes.set(Some(count + search.len()));
        }
    });
    search.find(';')
}

/// Mark every numeric character reference to a code point XML 1.0 forbids.
///
/// Tally writes references XML 1.0 forbids -- `&#4;` before its reserved
/// values, as in `&#4; Primary` -- so a strict parser refuses an ordinary
/// response (`docs/tally/TALLY_PROTOCOL_REFERENCE.md` §1.1(a); this rule is
/// §1.1(d)). The rewrite runs on decoded text before parsing and changes only
/// two atom forms:
///
/// - A decimal or hexadecimal (`x` or `X`) reference whose code point is not an
///   XML 1.0 `Char` (a C0 control other than tab, LF and CR, a surrogate,
///   U+FFFE, U+FFFF, or anything beyond U+10FFFF) becomes the literal text
///   U+FFFD `#` *n* `;`, with *n* in decimal. `&#4; Primary` becomes
///   `"\u{fffd}#4; Primary"`, whose prefix is [`crate::TALLY_SANITIZED_ROOT_MARKER`].
/// - A U+FFFD already in the text -- literal, or a legal reference to it --
///   that is directly followed by `#`, one or more ASCII digits and `;` within
///   twelve bytes becomes U+FFFD `#65533;`. Every marker the rewrite emits
///   therefore denotes exactly one source atom, and the rewrite is reversible.
///
/// Everything else is left for the XML parser: legal references, raw
/// characters (C0 controls included), and a `&#` whose `;` is not within the
/// twelve bytes that follow it. The rewrite neither refuses nor strips; a
/// caller that refuses raw U+0000, U+FFFE or U+FFFF does so itself.
///
/// ```
/// use bridge_tally_protocol::{mark_forbidden_numeric_references, TALLY_SANITIZED_ROOT_MARKER};
///
/// let marked = mark_forbidden_numeric_references("<PARENT>&#4; Primary</PARENT>");
/// assert_eq!(marked, "<PARENT>\u{fffd}#4; Primary</PARENT>");
/// assert!(marked["<PARENT>".len()..].starts_with(TALLY_SANITIZED_ROOT_MARKER));
///
/// // A literal U+FFFD that could be read as a marker is escaped, not merged.
/// assert_eq!(
///     mark_forbidden_numeric_references("\u{fffd}#4;"),
///     "\u{fffd}#65533;#4;"
/// );
///
/// // Text with nothing to mark is borrowed, not copied.
/// assert!(matches!(
///     mark_forbidden_numeric_references("<A>&#9;&amp;</A>"),
///     std::borrow::Cow::Borrowed(_)
/// ));
/// ```
pub fn mark_forbidden_numeric_references(xml: &str) -> Cow<'_, str> {
    rewrite_forbidden_numeric_references(xml, || {}, None)
}

pub(crate) fn sanitize_invalid_numeric_references(xml: &str) -> Cow<'_, str> {
    mark_forbidden_numeric_references(xml)
}

/// One sanitizer replacement: sanitized-text bytes `[output_start, output_end)`
/// were produced from original-text bytes `[source_start, source_end)`.
/// Repairs are recorded in ascending, non-overlapping `output_start` order.
struct RepairSpan {
    output_start: usize,
    output_end: usize,
    source_start: usize,
    source_end: usize,
}

/// Translate a sanitized-text offset back to the original text using a
/// sparse repair list, reproducing the exact mapping a dense per-byte
/// boundary table would give: any offset strictly inside a repaired span
/// collapses to that span's original start (the whole replacement reads
/// back as one atom), the offset immediately after a span resolves to that
/// span's original end, and offsets outside any span translate 1:1 through
/// the constant offset ("drift") left by the nearest preceding span (zero
/// drift before the first repair).
///
/// `sanitized_len` bounds `offset` exactly like the old table's length did:
/// valid offsets are `0..=sanitized_len`.
fn resolve_original_offset(
    repairs: &[RepairSpan],
    sanitized_len: usize,
    offset: usize,
) -> Option<usize> {
    if offset > sanitized_len {
        return None;
    }
    // Last span whose output_start is <= offset, if any.
    let idx = repairs.partition_point(|span| span.output_start <= offset);
    Some(match idx.checked_sub(1).map(|i| &repairs[i]) {
        Some(span) if offset < span.output_end => span.source_start,
        Some(span) => span.source_end + (offset - span.output_end),
        None => offset,
    })
}

/// XML parsing sometimes needs a narrow, reversible repair for Tally's
/// invalid numeric references.  Parsers must still attest the bytes Tally
/// actually returned, rather than the repaired representation they consumed.
pub(crate) struct SanitizedXml<'a> {
    original: &'a str,
    text: Cow<'a, str>,
    /// Sparse provenance: only the repaired spans are recorded, not one
    /// entry per sanitized byte.  Repairs are rare (a handful per response,
    /// even on large ones), so this keeps memory proportional to the repair
    /// count instead of the response size.  Absent means the text was
    /// borrowed and positions are exact.
    repairs: Option<Vec<RepairSpan>>,
}

impl<'a> SanitizedXml<'a> {
    pub(crate) fn as_str(&self) -> &str {
        &self.text
    }

    fn resolve(&self, offset: usize) -> Option<usize> {
        match &self.repairs {
            Some(repairs) => resolve_original_offset(repairs, self.text.len(), offset),
            None => Some(offset),
        }
    }

    pub(crate) fn original_fragment(&self, start: usize, end: usize) -> anyhow::Result<&'a [u8]> {
        let start = self
            .resolve(start)
            .ok_or_else(|| anyhow::anyhow!("sanitised XML fragment start was invalid"))?;
        let end = self
            .resolve(end)
            .ok_or_else(|| anyhow::anyhow!("sanitised XML fragment end was invalid"))?;
        let fragment = self
            .original
            .as_bytes()
            .get(start..end)
            .ok_or_else(|| anyhow::anyhow!("original XML fragment boundaries were invalid"))?;
        if fragment.is_empty() {
            anyhow::bail!("original XML record fragment was empty");
        }
        Ok(fragment)
    }
}

pub(crate) fn sanitize_invalid_numeric_references_with_provenance(xml: &str) -> SanitizedXml<'_> {
    // The rewrite records each span as it emits it, so the provenance map is
    // the rewrite's own account of what it changed rather than a second parse
    // of the same grammar that could stop, or continue, somewhere else.
    let mut repairs = Vec::new();
    let text = rewrite_forbidden_numeric_references(xml, || {}, Some(&mut repairs));
    if let Cow::Borrowed(_) = text {
        debug_assert!(repairs.is_empty());
        return SanitizedXml {
            original: xml,
            text,
            repairs: None,
        };
    }
    debug_assert_eq!(
        resolve_original_offset(&repairs, text.len(), text.len()),
        Some(xml.len()),
        "sparse provenance must cover the whole sanitized text"
    );
    SanitizedXml {
        original: xml,
        text,
        repairs: Some(repairs),
    }
}

#[cfg(test)]
fn sanitize_invalid_numeric_references_with_marker_search_observer(
    xml: &str,
    observe_replacement_marker_search: impl FnMut(),
) -> Cow<'_, str> {
    rewrite_forbidden_numeric_references(xml, observe_replacement_marker_search, None)
}

/// Append one replacement to `target`, recording its span when asked.
fn emit_replacement(
    target: &mut String,
    repairs: &mut Option<&mut Vec<RepairSpan>>,
    source: std::ops::Range<usize>,
    code_point: u32,
) {
    let output_start = target.len();
    target.push('\u{fffd}');
    // `write!` to a `String` cannot fail.
    let _ = write!(target, "#{code_point};");
    if let Some(repairs) = repairs {
        repairs.push(RepairSpan {
            output_start,
            output_end: target.len(),
            source_start: source.start,
            source_end: source.end,
        });
    }
}

/// The one implementation of the rewrite. `repairs`, when given, receives
/// every replacement span in emission order; provenance is built from those
/// spans and from nothing else.
fn rewrite_forbidden_numeric_references<'a>(
    xml: &'a str,
    mut observe_replacement_marker_search: impl FnMut(),
    mut repairs: Option<&mut Vec<RepairSpan>>,
) -> Cow<'a, str> {
    let mut scan = 0_usize;
    let mut copy_from = 0_usize;
    let mut output = None::<String>;
    observe_replacement_marker_search();
    let mut replacement_marker = xml.find('\u{fffd}');
    let mut numeric_reference = find_numeric_reference(xml);
    while scan < xml.len() {
        if replacement_marker.is_some_and(|marker| marker < scan) {
            observe_replacement_marker_search();
            replacement_marker = xml[scan..].find('\u{fffd}').map(|offset| scan + offset);
        }
        if numeric_reference.is_some_and(|reference| reference < scan) {
            numeric_reference = find_numeric_reference(&xml[scan..]).map(|offset| scan + offset);
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
            if !collides_with_marker_form(&xml[after..]) {
                scan = after;
                continue;
            }
            let target = output.get_or_insert_with(|| String::with_capacity(xml.len()));
            target.push_str(&xml[copy_from..start]);
            // U+FFFD is XML-legal source text, so it cannot be used as an
            // unescaped marker for an illegal reference. Encode literal U+FFFD
            // through the same grammar; this keeps the transformation
            // injective even for source containing the previous `\u{fffd}#4;`
            // representation.
            emit_replacement(target, &mut repairs, start..after, 0xfffd);
            copy_from = after;
            scan = after;
            continue;
        }

        let Some(relative_end) = find_numeric_reference_terminator(&xml[start + 1..]) else {
            // No `;` within the longest reference this rewrite accepts. This
            // `&#` is not a reference the rewrite can judge, so it is left as
            // written for the XML parser; the scan continues after it, so a
            // later forbidden reference is still marked.
            scan = start + "&#".len();
            continue;
        };
        let end = start + 1 + relative_end;
        let token = &xml[start + 2..end];
        let parsed = token
            .strip_prefix('x')
            .or_else(|| token.strip_prefix('X'))
            .and_then(|hex| u32::from_str_radix(hex, 16).ok())
            .or_else(|| token.parse::<u32>().ok());
        let replaced = match parsed {
            Some(value) if !is_xml_10_char(value) => Some(value),
            // A legal reference to U+FFFD decodes to the marker character, so
            // it is ambiguous under exactly the same condition as a literal
            // one -- and only then. Rewriting it unconditionally corrupts a
            // legitimate value.
            Some(0xfffd) if collides_with_marker_form(&xml[end + 1..]) => Some(0xfffd),
            _ => None,
        };
        if let Some(code_point) = replaced {
            let target = output.get_or_insert_with(|| String::with_capacity(xml.len()));
            target.push_str(&xml[copy_from..start]);
            // Preserve the numeric identity in a self-escaping marker.
            // Mapping every illegal code point to bare U+FFFD is lossy, and a
            // marker that leaves literal U+FFFD untouched collides with source
            // text already holding that marker. Literal U+FFFD is encoded
            // above as `U+FFFD#65533;`, so every emitted `U+FFFD#<n>;` denotes
            // exactly one source atom while remaining XML-1.0 legal.
            emit_replacement(target, &mut repairs, start..end + 1, code_point);
            copy_from = end + 1;
        }
        scan = end + 1;
    }
    if let Some(mut output) = output {
        output.push_str(&xml[copy_from..]);
        Cow::Owned(output)
    } else {
        Cow::Borrowed(xml)
    }
}

fn is_xml_10_char(value: u32) -> bool {
    matches!(value, 0x9 | 0xA | 0xD | 0x20..=0xD7FF | 0xE000..=0xFFFD | 0x10000..=0x10FFFF)
}

#[cfg(test)]
#[path = "tolerant_xml_tests.rs"]
mod tests;
