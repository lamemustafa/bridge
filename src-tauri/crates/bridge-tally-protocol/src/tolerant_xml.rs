use std::borrow::Cow;

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

pub(crate) fn sanitize_invalid_numeric_references(xml: &str) -> Cow<'_, str> {
    sanitize_invalid_numeric_references_with_marker_search_observer(xml, || {})
}

/// XML parsing sometimes needs a narrow, reversible repair for Tally's
/// invalid numeric references.  Parsers must still attest the bytes Tally
/// actually returned, rather than the repaired representation they consumed.
pub(crate) struct SanitizedXml<'a> {
    original: &'a str,
    text: Cow<'a, str>,
    /// Each sanitized byte boundary maps to the corresponding original byte
    /// boundary.  Absent means the text was borrowed and positions are exact.
    original_boundaries: Option<Vec<usize>>,
}

impl<'a> SanitizedXml<'a> {
    pub(crate) fn as_str(&self) -> &str {
        &self.text
    }

    pub(crate) fn original_fragment(&self, start: usize, end: usize) -> anyhow::Result<&'a [u8]> {
        let (start, end) = match &self.original_boundaries {
            Some(boundaries) => (
                *boundaries
                    .get(start)
                    .ok_or_else(|| anyhow::anyhow!("sanitised XML fragment start was invalid"))?,
                *boundaries
                    .get(end)
                    .ok_or_else(|| anyhow::anyhow!("sanitised XML fragment end was invalid"))?,
            ),
            None => (start, end),
        };
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
    let sanitized = sanitize_invalid_numeric_references(xml);
    let Cow::Owned(text) = sanitized else {
        return SanitizedXml {
            original: xml,
            text: Cow::Borrowed(xml),
            original_boundaries: None,
        };
    };

    // The sanitizer changes only two source atom forms.  Replaying that small
    // grammar gives every repaired-parser offset its raw-wire boundary.
    let mut source = 0;
    let mut output = 0;
    let mut boundaries = Vec::with_capacity(text.len() + 1);
    boundaries.push(0);
    while source < xml.len() {
        let source_tail = &xml[source..];
        let output_tail = &text[output..];
        // A literal U+FFFD is expanded whenever the ORIGINAL text after it
        // collides with the marker form -- i.e. matches `#<digits>;` for ANY
        // digit sequence, not only `#65533;`. This must call the exact same
        // `collides_with_marker_form` predicate the sanitizer uses (see the
        // marker-search branch above `sanitize_invalid_numeric_references_with_marker_search_observer`),
        // or the two can silently drift apart again.
        if source_tail.starts_with('\u{fffd}')
            && collides_with_marker_form(&source_tail['\u{fffd}'.len_utf8()..])
            && output_tail.starts_with("\u{fffd}#65533;")
        {
            append_replaced_boundaries(
                &mut boundaries,
                "\u{fffd}#65533;".len(),
                source,
                source + '\u{fffd}'.len_utf8(),
            );
            source += '\u{fffd}'.len_utf8();
            output += "\u{fffd}#65533;".len();
            continue;
        }
        if source_tail.starts_with("&#") {
            if let Some(relative_end) = find_numeric_reference_terminator(&source_tail[1..]) {
                let token_end = source + 1 + relative_end;
                let token = &xml[source + 2..token_end];
                let parsed = token
                    .strip_prefix('x')
                    .or_else(|| token.strip_prefix('X'))
                    .and_then(|hex| u32::from_str_radix(hex, 16).ok())
                    .or_else(|| token.parse::<u32>().ok());
                let source_end = token_end + 1;
                let ambiguous_replacement =
                    parsed == Some(0xfffd) && collides_with_marker_form(&xml[source_end..]);
                if parsed.is_some_and(|value| !is_xml_10_char(value)) || ambiguous_replacement {
                    let replacement_len =
                        "\u{fffd}".len() + format!("#{};", parsed.unwrap_or_default()).len();
                    append_replaced_boundaries(
                        &mut boundaries,
                        replacement_len,
                        source,
                        source_end,
                    );
                    source = source_end;
                    output += replacement_len;
                    continue;
                }
            }
        }
        let width = source_tail
            .chars()
            .next()
            .expect("source is non-empty")
            .len_utf8();
        boundaries.extend((source + 1)..=source + width);
        source += width;
        output += width;
    }
    debug_assert_eq!(output, text.len());
    debug_assert_eq!(boundaries.len(), text.len() + 1);
    SanitizedXml {
        original: xml,
        text: Cow::Owned(text),
        original_boundaries: Some(boundaries),
    }
}

fn append_replaced_boundaries(
    boundaries: &mut Vec<usize>,
    replacement_len: usize,
    source_start: usize,
    source_end: usize,
) {
    debug_assert_eq!(boundaries.last().copied(), Some(source_start));
    boundaries.extend(std::iter::repeat_n(source_start, replacement_len));
    *boundaries
        .last_mut()
        .expect("replacement has a final boundary") = source_end;
}

fn sanitize_invalid_numeric_references_with_marker_search_observer(
    xml: &str,
    mut observe_replacement_marker_search: impl FnMut(),
) -> Cow<'_, str> {
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
            target.push('\u{fffd}');
            target.push_str("#65533;");
            copy_from = after;
            scan = after;
            continue;
        }

        let Some(relative_end) = find_numeric_reference_terminator(&xml[start + 1..]) else {
            break;
        };
        let end = start + 1 + relative_end;
        let token = &xml[start + 2..end];
        let parsed = token
            .strip_prefix('x')
            .or_else(|| token.strip_prefix('X'))
            .and_then(|hex| u32::from_str_radix(hex, 16).ok())
            .or_else(|| token.parse::<u32>().ok());
        let illegal = parsed.is_some_and(|value| !is_xml_10_char(value));
        // A legal reference to U+FFFD decodes to the marker character, so it is
        // ambiguous under exactly the same condition as a literal one -- and
        // only then. Rewriting it unconditionally corrupts a legitimate value.
        let ambiguous_replacement =
            parsed == Some(0xfffd) && collides_with_marker_form(&xml[end + 1..]);
        if illegal || ambiguous_replacement {
            let target = output.get_or_insert_with(|| String::with_capacity(xml.len()));
            target.push_str(&xml[copy_from..start]);
            // Preserve the numeric identity in a self-escaping marker.
            // Mapping every illegal code point to bare U+FFFD is lossy, and a
            // marker that leaves literal U+FFFD untouched collides with source
            // text already holding that marker. Literal U+FFFD is encoded
            // above as `U+FFFD#65533;`, so every emitted `U+FFFD#<n>;` denotes
            // exactly one source atom while remaining XML-1.0 legal.
            target.push('\u{fffd}');
            target.push_str(&format!("#{};", parsed.unwrap_or_default()));
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
mod tests {
    use quick_xml::{events::Event, Reader};
    use serde::Deserialize;

    use super::{
        sanitize_invalid_numeric_references,
        sanitize_invalid_numeric_references_with_marker_search_observer,
        sanitize_invalid_numeric_references_with_provenance, MARKER_FORM_SEARCH_BYTES,
        MAX_MARKER_FORM_BYTES, NUMERIC_REFERENCE_SEARCHES,
        NUMERIC_REFERENCE_TERMINATOR_SEARCH_BYTES,
    };

    #[derive(Debug, Deserialize)]
    struct TextValue {
        #[serde(rename = "$text")]
        value: String,
    }

    #[test]
    fn real_invalid_character_reference_is_narrowly_repaired() {
        let capture = include_str!("../tests/fixtures/unit_a_invalid_char_ref_live.xml");
        assert!(capture.contains("&#4;"));
        let sanitized = sanitize_invalid_numeric_references(capture);
        assert!(!sanitized.contains("&#4;"));
        assert!(sanitized.contains('\u{fffd}'));
        let mut reader = Reader::from_str(&sanitized);
        loop {
            if matches!(
                reader.read_event().expect("all other XML remains strict"),
                Event::Eof
            ) {
                break;
            }
        }
    }

    #[test]
    fn legal_numeric_reference_is_not_changed() {
        let input = "<A>&#9;&#x20;</A>";
        assert!(matches!(
            sanitize_invalid_numeric_references(input),
            std::borrow::Cow::Borrowed(_)
        ));
    }

    #[test]
    fn numeric_references_remain_injective_after_xml_deserialisation() {
        let parse = |xml| {
            quick_xml::de::from_str::<TextValue>(&sanitize_invalid_numeric_references(xml))
                .expect("sanitised XML deserialises like parser.rs")
                .value
        };

        let first_illegal_reference = parse("<A>ACME&#1;LTD</A>");
        let illegal_reference = parse("<A>ACME&#4;LTD</A>");
        let literal_replacement = parse("<A>ACME\u{fffd}LTD</A>");
        let decimal_replacement = parse("<A>ACME&#65533;LTD</A>");
        let non_numeric_marker_form = parse("<A>ACME\u{fffd}#abc;LTD</A>");
        let literal_encoded_form = parse("<A>ACME\u{fffd}#4;LTD</A>");
        let decimal_replacement_reference = parse("<A>ACME&#65533;#4;LTD</A>");
        let hex_replacement_reference = parse("<A>ACME&#xFFFD;#4;LTD</A>");

        assert_eq!(first_illegal_reference, "ACME\u{fffd}#1;LTD");
        assert_eq!(illegal_reference, "ACME\u{fffd}#4;LTD");
        assert_eq!(literal_replacement, "ACME\u{fffd}LTD");
        assert_eq!(decimal_replacement, literal_replacement);
        assert_eq!(non_numeric_marker_form, "ACME\u{fffd}#abc;LTD");
        assert_ne!(first_illegal_reference, illegal_reference);
        assert_eq!(literal_encoded_form, "ACME\u{fffd}#65533;#4;LTD");
        assert_ne!(illegal_reference, literal_encoded_form);
        assert_ne!(illegal_reference, decimal_replacement_reference);
        assert_ne!(illegal_reference, hex_replacement_reference);
        assert_eq!(decimal_replacement_reference, literal_encoded_form);
        assert_eq!(hex_replacement_reference, literal_encoded_form);
    }

    #[test]
    fn no_marker_search_work_stays_constant_as_numeric_references_grow() {
        let replacement_marker_searches = |references| {
            let xml = format!("<A>{}</A>", "&#4;".repeat(references));
            let mut searches = 0usize;
            let _ = sanitize_invalid_numeric_references_with_marker_search_observer(&xml, || {
                searches += 1;
            });
            searches
        };

        let small = replacement_marker_searches(1_000);
        let large = replacement_marker_searches(20_000);
        assert!(
            large <= small + 1,
            "a no-marker input must not rescan for U+FFFD once per numeric reference: {small} -> {large}"
        );
    }

    #[test]
    fn no_numeric_reference_search_work_stays_constant_as_markers_grow() {
        let numeric_reference_searches = |markers| {
            let xml = format!("<A>{}</A>", "\u{fffd}#4;".repeat(markers));
            NUMERIC_REFERENCE_SEARCHES.with(|searches| {
                assert!(searches.replace(Some(0)).is_none());
                let _ = sanitize_invalid_numeric_references(&xml);
                searches
                    .replace(None)
                    .expect("numeric searches were enabled")
            })
        };

        let small = numeric_reference_searches(1_000);
        let large = numeric_reference_searches(20_000);
        assert!(
            large <= small + 1,
            "a marker-heavy input must not rescan for numeric references once per marker: {small} -> {large}"
        );
    }

    #[test]
    fn numeric_reference_terminator_search_is_bounded_with_or_without_a_terminator() {
        let searched_bytes = |references, distant_terminator| {
            let mut xml = format!("<A>{}</A>", "&#1234567890".repeat(references));
            if distant_terminator {
                xml.push(';');
            }
            NUMERIC_REFERENCE_TERMINATOR_SEARCH_BYTES.with(|counter| {
                assert!(counter.replace(Some(0)).is_none());
                let _ = sanitize_invalid_numeric_references(&xml);
                counter
                    .replace(None)
                    .expect("terminator search accounting was enabled")
            })
        };

        for distant_terminator in [false, true] {
            let small = searched_bytes(2_000, distant_terminator);
            let large = searched_bytes(10_000, distant_terminator);
            assert!(
                large <= small + MAX_MARKER_FORM_BYTES,
                "numeric-reference terminator searches must stay bounded: {small} -> {large}"
            );
        }
    }

    #[test]
    fn marker_form_search_work_scales_linearly_without_terminators() {
        let searched_bytes = |markers| {
            let xml = format!("<A>{}</A>", "\u{fffd}#abc".repeat(markers));
            MARKER_FORM_SEARCH_BYTES.with(|counter| {
                assert!(counter.replace(Some(0)).is_none());
                let _ = sanitize_invalid_numeric_references(&xml);
                counter
                    .replace(None)
                    .expect("marker-form search accounting was enabled")
            })
        };

        let small = searched_bytes(2_000);
        let large = searched_bytes(10_000);
        assert!(
            large <= small * 6,
            "marker-form searches without terminators must scale linearly: {small} -> {large}"
        );
    }

    /// Regression coverage for the sanitizer/replay desync: a literal U+FFFD
    /// followed by ANY `#<digits>;` text (not only the `#65533;` literal) is
    /// expanded by the sanitizer, so the provenance replay must recognise the
    /// same general marker form or its boundary bookkeeping desyncs from the
    /// sanitized text -- tripping the `debug_assert_eq!`s in
    /// `sanitize_invalid_numeric_references_with_provenance` in debug builds,
    /// and returning wrong-offset bytes from `original_fragment` in release.
    fn assert_provenance_round_trips(xml: &str, expected_sanitized: &str) {
        let sanitized = sanitize_invalid_numeric_references_with_provenance(xml);
        assert_eq!(
            sanitized.as_str(),
            expected_sanitized,
            "provenance replay must sanitize identically to the plain sanitizer"
        );
        assert_eq!(
            sanitize_invalid_numeric_references(xml).as_ref(),
            expected_sanitized,
            "both sanitizer entry points must agree on the sanitized text"
        );
        let whole = sanitized
            .original_fragment(0, sanitized.as_str().len())
            .expect("boundaries must cover the whole sanitized text without desyncing");
        assert_eq!(
            whole,
            xml.as_bytes(),
            "the full-span original fragment must be exactly the original source bytes"
        );
    }

    #[test]
    fn provenance_round_trips_literal_marker_colliding_with_short_digit_reference() {
        // "#4;" is not "#65533;", so the naive literal check in the replay
        // used to miss this and desync the boundary vector.
        assert_provenance_round_trips("ACME\u{fffd}#4;LTD", "ACME\u{fffd}#65533;#4;LTD");
    }

    #[test]
    fn provenance_round_trips_literal_marker_colliding_with_another_digit_reference() {
        // A second, distinct digit sequence confirms the fix covers the
        // general `#<digits>;` marker form rather than special-casing `#4;`.
        assert_provenance_round_trips("ACME\u{fffd}#12;LTD", "ACME\u{fffd}#65533;#12;LTD");
    }

    #[test]
    fn provenance_round_trips_literal_marker_colliding_with_65533_digit_reference() {
        // The original "#65533;" literal must keep working once the check is
        // generalised, since it is itself just one instance of the marker form.
        assert_provenance_round_trips("ACME\u{fffd}#65533;LTD", "ACME\u{fffd}#65533;#65533;LTD");
    }

    #[test]
    fn provenance_original_fragment_returns_correct_original_bytes_for_a_row() {
        // Mirrors how native_outstandings/wire.rs and lib.rs actually use the
        // provenance API: slice a record's worth of SANITIZED text and demand
        // the exact ORIGINAL bytes back, not merely that no panic occurs.
        let xml = "<ROW>ACME\u{fffd}#4;LTD</ROW><ROW>OTHER</ROW>";
        let sanitized = sanitize_invalid_numeric_references_with_provenance(xml);
        let sanitized_text = sanitized.as_str();
        assert_eq!(
            sanitized_text,
            "<ROW>ACME\u{fffd}#65533;#4;LTD</ROW><ROW>OTHER</ROW>"
        );

        let row_start = sanitized_text.find("<ROW>").expect("first ROW open tag") + "<ROW>".len();
        let row_end = sanitized_text.find("</ROW>").expect("first ROW close tag");

        let fragment = sanitized
            .original_fragment(row_start, row_end)
            .expect("row boundaries must resolve to a valid original slice");
        assert_eq!(fragment, "ACME\u{fffd}#4;LTD".as_bytes());
    }
}
