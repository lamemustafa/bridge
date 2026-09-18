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

    // The rewrite continues past a `&#` it cannot judge, so it searches once
    // per reference; each search must stay within the window, however far
    // away (or absent) the eventual `;` is. An unbounded search would scan the
    // rest of the response each time and grow quadratically.
    for distant_terminator in [false, true] {
        for references in [2_000, 10_000] {
            let searched = searched_bytes(references, distant_terminator);
            assert!(
                searched <= references * MAX_MARKER_FORM_BYTES,
                "each terminator search must stay within {MAX_MARKER_FORM_BYTES} bytes: \
                 {searched} bytes for {references} references"
            );
        }
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

/// A `&#` whose `;` is outside the bounded terminator window is not a
/// reference the rewrite can judge. Robustness regression: the provenance
/// map must still describe exactly the text the rewrite produced, and later
/// references must still be rewritten.
#[test]
fn provenance_stays_aligned_across_an_unterminated_numeric_reference() {
    let xml = "<A>&#4;&#xxxxxxxxxxxx&#4;</A>";
    assert_provenance_round_trips(xml, "<A>\u{fffd}#4;&#xxxxxxxxxxxx\u{fffd}#4;</A>");
}

/// Robustness regression: when a `&#` has a `;` in its window but no number
/// before it, the rewrite skips the whole window, including any reference
/// inside it. The provenance map must skip the same text.
#[test]
fn provenance_stays_aligned_when_a_reference_window_covers_another_reference() {
    assert_provenance_round_trips("<A>&#4;&#&#4;</A>", "<A>\u{fffd}#4;&#&#4;</A>");
}

#[test]
fn a_forbidden_reference_after_an_unterminated_one_is_still_marked() {
    assert_eq!(
        super::mark_forbidden_numeric_references("<A>&#xxxxxxxxxxxx&#4;</A>"),
        "<A>&#xxxxxxxxxxxx\u{fffd}#4;</A>"
    );
}

/// The rewrite as it stood before the unterminated-reference fix, kept
/// verbatim as the reference the public function is compared with, except
/// that it also reports where it stopped. It stopped at the first `&#` with no
/// `;` in the window and copied the rest through unchanged.
#[allow(clippy::format_push_string)] // kept verbatim
fn pre_fix_rewrite(xml: &str) -> (std::borrow::Cow<'_, str>, Option<usize>) {
    let mut scan = 0_usize;
    let mut copy_from = 0_usize;
    let mut output = None::<String>;
    let mut stopped_at = None;
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
        let Some(relative_end) = super::bounded(&xml[start + 1..], MAX_MARKER_FORM_BYTES).find(';')
        else {
            stopped_at = Some(start);
            break;
        };
        let end = start + 1 + relative_end;
        let token = &xml[start + 2..end];
        let parsed = token
            .strip_prefix('x')
            .or_else(|| token.strip_prefix('X'))
            .and_then(|hex| u32::from_str_radix(hex, 16).ok())
            .or_else(|| token.parse::<u32>().ok());
        let forbidden = parsed.is_some_and(|value| !is_xml_10_char(value));
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
    let rewritten = match output {
        Some(mut output) => {
            output.push_str(&xml[copy_from..]);
            std::borrow::Cow::Owned(output)
        }
        None => std::borrow::Cow::Borrowed(xml),
    };
    (rewritten, stopped_at)
}

/// Atoms that reach every branch of the rewrite: forbidden, legal and
/// ambiguous references in both radixes, unparsable and overflowing tokens, a
/// `&#` with no `;` in the window, marker-shaped text after a literal U+FFFD,
/// and plain text including a multi-byte character.
const REWRITE_ATOMS: &[&str] = &[
    "&#4;",
    "&#x4;",
    "&#X1f;",
    "&#1;",
    "&#9;",
    "&#x20;",
    "&#65533;",
    "&#xFFFD;",
    "&#xfffe;",
    "&#55296;",
    "&#1114112;",
    "&#4294967296;",
    "&#00000000004;",
    "&#+4;",
    "&#;",
    "&#",
    "&",
    "#",
    "#4;",
    "#65533;",
    ";",
    "\u{fffd}",
    "x",
    "1",
    "\u{e9}",
    "<A>",
    "&amp;",
];

/// Which comparison a checked input exercised.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum RewriteComparison {
    Identical,
    ContinuedPastUnterminated,
}

/// Compare the public rewrite with the pre-fix rewrite, and check the
/// provenance map describes the public rewrite's output exactly.
fn check_rewrite_against_pre_fix(xml: &str) -> RewriteComparison {
    let fixed = super::mark_forbidden_numeric_references(xml);
    let (pre_fix, stopped_at) = pre_fix_rewrite(xml);
    let comparison = match stopped_at {
        None => {
            assert_eq!(fixed, pre_fix, "rewrites diverged on {xml:?}");
            assert_eq!(
                matches!(fixed, std::borrow::Cow::Borrowed(_)),
                matches!(pre_fix, std::borrow::Cow::Borrowed(_)),
                "rewrites disagree on whether {xml:?} needed a change"
            );
            RewriteComparison::Identical
        }
        Some(stop) => {
            // Up to the unterminated `&#` both agree. The fixed rewrite then
            // keeps the `&#` as written and rewrites the rest as it would on
            // its own: every decision in the rewrite looks only forward.
            let head = pre_fix
                .strip_suffix(&xml[stop..])
                .expect("the pre-fix rewrite copies its tail through unchanged");
            let rest = super::mark_forbidden_numeric_references(&xml[stop + "&#".len()..]);
            assert_eq!(fixed, format!("{head}&#{rest}"), "diverged on {xml:?}");
            RewriteComparison::ContinuedPastUnterminated
        }
    };

    let sanitized = sanitize_invalid_numeric_references_with_provenance(xml);
    assert_eq!(sanitized.as_str(), fixed, "provenance text for {xml:?}");
    assert_eq!(sanitized.resolve(fixed.len()), Some(xml.len()), "{xml:?}");
    if !xml.is_empty() {
        assert_eq!(
            sanitized
                .original_fragment(0, fixed.len())
                .expect("whole-text fragment resolves"),
            xml.as_bytes(),
            "{xml:?}"
        );
    }
    for span in sanitized.repairs.iter().flatten() {
        let emitted = &fixed[span.output_start..span.output_end];
        let source = &xml[span.source_start..span.source_end];
        assert!(
            emitted.starts_with('\u{fffd}') && collides_with_marker_form(&emitted[3..]),
            "span output {emitted:?} is not a marker, in {xml:?}"
        );
        assert!(
            source == "\u{fffd}" || (source.starts_with("&#") && source.ends_with(';')),
            "span source {source:?} is not one atom, in {xml:?}"
        );
    }
    comparison
}

#[test]
fn public_rewrite_matches_the_pre_fix_rewrite_on_every_short_atom_sequence() {
    let mut identical = 0_usize;
    let mut continued = 0_usize;
    let mut tally = |xml: &str| match check_rewrite_against_pre_fix(xml) {
        RewriteComparison::Identical => identical += 1,
        RewriteComparison::ContinuedPastUnterminated => continued += 1,
    };
    tally("");
    for first in REWRITE_ATOMS {
        tally(first);
        for second in REWRITE_ATOMS {
            tally(&format!("{first}{second}"));
            for third in REWRITE_ATOMS {
                tally(&format!("{first}{second}{third}"));
            }
        }
    }
    // Both comparisons must actually run, or this test measures nothing.
    assert!(identical > 10_000, "identical comparisons: {identical}");
    assert!(continued > 1_000, "continued comparisons: {continued}");
}

proptest::proptest! {
    #![proptest_config(proptest::test_runner::Config {
        cases: 2_048,
        rng_seed: proptest::test_runner::RngSeed::Fixed(0xB71D_6E0D_2026_0918),
        ..proptest::test_runner::Config::default()
    })]

    #[test]
    fn public_rewrite_matches_the_pre_fix_rewrite_on_long_atom_sequences(
        atoms in proptest::collection::vec(proptest::sample::select(REWRITE_ATOMS), 0..48)
    ) {
        check_rewrite_against_pre_fix(&atoms.concat());
    }
}

/// Every committed XML fixture, decoded, reads identically through the public
/// rewrite and the pre-fix one: none of them holds a `&#` the fix changes.
#[test]
fn public_rewrite_matches_the_pre_fix_rewrite_on_every_committed_fixture() {
    fn visit(dir: &std::path::Path, found: &mut Vec<std::path::PathBuf>) {
        for entry in std::fs::read_dir(dir).expect("fixture directory is readable") {
            let path = entry.expect("fixture entry is readable").path();
            if path.is_dir() {
                visit(&path, found);
            } else if path.extension().is_some_and(|extension| extension == "xml") {
                found.push(path);
            }
        }
    }
    let mut fixtures = Vec::new();
    visit(
        &std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures"),
        &mut fixtures,
    );
    let mut marked = 0_usize;
    for path in &fixtures {
        let bytes = std::fs::read(path).expect("fixture is readable");
        // Name the encoding: a BOM-less UTF-16LE file is also valid UTF-8, and
        // read that way it would hide every reference behind interleaved NULs.
        let text = if path.to_string_lossy().contains("utf16le") {
            assert!(bytes.len().is_multiple_of(2), "{}", path.display());
            let units: Vec<u16> = bytes
                .chunks_exact(2)
                .map(|pair| u16::from_le_bytes([pair[0], pair[1]]))
                .collect();
            let text = String::from_utf16(&units)
                .unwrap_or_else(|error| panic!("{} is not UTF-16LE: {error}", path.display()));
            text.strip_prefix('\u{feff}')
                .map(str::to_owned)
                .unwrap_or(text)
        } else {
            String::from_utf8(bytes)
                .unwrap_or_else(|error| panic!("{} is not UTF-8: {error}", path.display()))
        };
        assert!(!text.contains('\0'), "{} decoded with NULs", path.display());
        assert_eq!(
            check_rewrite_against_pre_fix(&text),
            RewriteComparison::Identical,
            "{}",
            path.display()
        );
        if text.contains("&#4;") {
            marked += 1;
        }
    }
    assert!(
        fixtures.len() >= 80,
        "found {} XML fixtures",
        fixtures.len()
    );
    assert!(marked >= 30, "{marked} fixtures carry `&#4;`");
}
