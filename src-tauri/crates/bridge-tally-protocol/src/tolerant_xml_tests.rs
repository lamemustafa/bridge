use quick_xml::{events::Event, Reader};
use serde::Deserialize;

use super::{
    collides_with_marker_form, find_numeric_reference_terminator, is_xml_10_char,
    sanitize_invalid_numeric_references,
    sanitize_invalid_numeric_references_with_marker_search_observer,
    sanitize_invalid_numeric_references_with_provenance, MARKER_FORM_SEARCH_BYTES,
    MAX_MARKER_FORM_BYTES, NUMERIC_REFERENCE_SEARCHES, NUMERIC_REFERENCE_TERMINATOR_SEARCH_BYTES,
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

#[test]
fn provenance_storage_scales_with_repair_count_not_input_size() {
    // A couple of repairs embedded in a large body of otherwise
    // unremarkable text must not force a per-byte allocation. The old
    // dense table held one `usize` per sanitized byte (~256 MiB at the
    // accepted 32 MiB response cap, since Tally's reserved-root marker
    // `&#4;` makes every real group/ledger response trigger a repair);
    // the sparse table must stay proportional to the repair count
    // instead of the response size.
    let padding = "x".repeat(300_000);
    let xml = format!("<A>{padding}&#4;{padding}&#4;{padding}</A>");
    let sanitized = sanitize_invalid_numeric_references_with_provenance(&xml);
    assert!(sanitized.as_str().len() > 600_000);
    let repair_count = sanitized
        .repairs
        .as_ref()
        .expect("two illegal references must trigger sanitisation")
        .len();
    assert_eq!(
        repair_count, 2,
        "sparse storage must hold one entry per repair, not per byte"
    );
}

/// Independent reimplementation of the old dense, one-entry-per-byte
/// provenance table (`boundaries[output_offset] == original_offset`).
/// Kept only in tests to cross-check the new sparse
/// `resolve_original_offset` translation; production code no longer
/// builds a table this large.
fn brute_force_boundaries(xml: &str) -> (String, Vec<usize>) {
    let sanitized = sanitize_invalid_numeric_references(xml);
    let std::borrow::Cow::Owned(text) = sanitized else {
        return (xml.to_string(), (0..=xml.len()).collect());
    };
    let mut source = 0;
    let mut output = 0;
    let mut boundaries = Vec::with_capacity(text.len() + 1);
    boundaries.push(0);
    while source < xml.len() {
        let source_tail = &xml[source..];
        let output_tail = &text[output..];
        if source_tail.starts_with('\u{fffd}')
            && collides_with_marker_form(&source_tail['\u{fffd}'.len_utf8()..])
            && output_tail.starts_with("\u{fffd}#65533;")
        {
            brute_force_append_replaced(
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
                    brute_force_append_replaced(
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
    (text, boundaries)
}

fn brute_force_append_replaced(
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

/// Property-style check: for every offset in a range of inputs covering
/// no repairs, a repair at the very start, one at the very end, adjacent
/// repairs with no gap between them, and a literal U+FFFD followed by a
/// marker-shaped suffix, the new sparse translation must agree with the
/// brute-force dense byte map at every single offset.
#[test]
fn sparse_translation_matches_bruteforce_byte_map_across_repair_shapes() {
    let cases = [
        "no repairs at all in this plain ASCII text",
        "&#4;repair sitting at the very start of the text",
        "repair sitting at the very end of the text&#4;",
        "&#1;&#2;adjacent repairs with no gap between them",
        "ACME\u{fffd}#4;LTD literal FFFD followed by a marker-shaped suffix",
    ];
    for xml in cases {
        let sanitized = sanitize_invalid_numeric_references_with_provenance(xml);
        let (brute_text, boundaries) = brute_force_boundaries(xml);
        assert_eq!(
            sanitized.as_str(),
            brute_text,
            "sparse and dense builders must sanitize {xml:?} identically"
        );
        for (offset, &expected) in boundaries.iter().enumerate() {
            let actual = sanitized
                .resolve(offset)
                .unwrap_or_else(|| panic!("offset {offset} must resolve for {xml:?}"));
            assert_eq!(
                actual, expected,
                "offset {offset} diverged from the dense byte map for {xml:?}"
            );
        }
    }
}
