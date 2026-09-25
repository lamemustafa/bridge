//! The Tally text rule (`docs/tax-audit/read-format-v1.md` section 10) on bytes Tally really
//! sent, and against Bridge's own protocol decoder.
//!
//! The group responses here are the captures `bridge-tally-protocol` already commits and
//! parses (`tests/fixtures/native/PROVENANCE.md`): every one keeps Tally's raw U+0003
//! separators inside `PARENTSTRUCTURE` and its `&#4; Primary` references, verbatim.

use bridge_tally_protocol::{
    mark_forbidden_numeric_references, parse_native_group_source_records_with_evidence,
    PartyLedgerMasterFieldObservation,
};
use bridge_tax_audit::xml::{self, Element};

const FIXTURES: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../bridge-tally-protocol/tests/fixtures/native"
);

/// Every committed native group capture, with the company GUID its rows carry (None where the
/// capture predates the GUID field) and the raw U+0003 count measured on its bytes.
const GROUP_CAPTURES: [(&str, Option<&str>, usize); 7] = [
    ("group_snapshot_aarav.xml", None, 82),
    (
        "group_snapshot_aarav_with_computed_company_guid.xml",
        Some("bb8ad19e-6aef-4239-a917-87fec0c6215e"),
        82,
    ),
    (
        "group_snapshot_aarav_with_identity.utf16le.xml",
        Some("bb8ad19e-6aef-4239-a917-87fec0c6215e"),
        82,
    ),
    (
        "group_snapshot_master_fields_lab.utf8.xml",
        Some("56359347-3976-4d01-b44e-56fa0f6a422c"),
        82,
    ),
    (
        "group_snapshot_validation_lab.xml",
        Some("c6afd306-00e1-4f51-802a-babe44daddd3"),
        82,
    ),
    ("group_snapshot_wr2.xml", None, 88),
    (
        "group_snapshot_wr2_with_identity.utf16le.xml",
        Some("61c6de69-1748-461c-ad3f-162cb949df9f"),
        88,
    ),
];

fn capture(name: &str) -> Vec<u8> {
    std::fs::read(format!("{FIXTURES}/{name}"))
        .unwrap_or_else(|error| panic!("capture {name} unreadable: {error}"))
}

/// Bridge's own text for a capture, as its production code decodes a response.
fn bridge_text(bytes: &[u8]) -> String {
    if bytes.len() > 1 && bytes[1] == 0 {
        let units: Vec<u16> = bytes
            .chunks_exact(2)
            .map(|pair| u16::from_le_bytes([pair[0], pair[1]]))
            .collect();
        String::from_utf16(&units).expect("captured UTF-16LE decodes")
    } else {
        String::from_utf8(bytes.to_vec()).expect("captured UTF-8 decodes")
    }
}

/// Bridge's decoded PARENT for each group, in document order, from Bridge's native group
/// parser.
fn bridge_parents(text: &str, company_guid: &str) -> Vec<(String, String)> {
    parse_native_group_source_records_with_evidence(text, company_guid)
        .expect("Bridge parses this group response")
        .records
        .into_iter()
        .map(|record| {
            let parent = match record.record.parent {
                PartyLedgerMasterFieldObservation::Returned(parent) => parent,
                PartyLedgerMasterFieldObservation::NotObserved => String::new(),
            };
            (record.record.name, parent)
        })
        .collect()
}

fn groups(root: &Element) -> Vec<&Element> {
    root.descendants_named("GROUP")
        .into_iter()
        .filter(|group| group.attr("NAME").is_some())
        .collect()
}

#[test]
fn bridge_group_captures_are_read_with_their_control_characters_kept() {
    for (name, company_guid, raw_u0003) in GROUP_CAPTURES {
        let bytes = capture(name);
        let root = xml::read(&bytes, name).unwrap_or_else(|error| {
            panic!("{name}: a real Bridge group capture must read: {error}")
        });
        let groups = groups(&root);
        assert!(groups.len() >= 28, "{name}: {} groups", groups.len());

        // PARENTSTRUCTURE keeps every raw U+0003 separator: nothing is stripped at decode time.
        let kept: usize = groups
            .iter()
            .filter_map(|group| group.child("PARENTSTRUCTURE"))
            .map(|element| element.text.matches('\u{3}').count())
            .sum();
        assert_eq!(kept, raw_u0003, "{name}: U+0003 kept in PARENTSTRUCTURE");

        // `&#4; Primary` is Tally's reserved root, and nothing else is.
        let roots = groups
            .iter()
            .filter(|group| group.child_text("PARENT") == "\u{fffd}#4; Primary")
            .count();
        assert_eq!(roots, 15, "{name}: groups directly under the reserved root");
        for group in &groups {
            let parent = group.child_text("PARENT");
            assert!(
                !parent.contains('\u{4}') && parent != "Primary",
                "{name}: PARENT {parent:?} must be the marker form, never U+0004 or a bare name"
            );
        }

        // The decoded PARENT of every group is exactly the text Bridge's own decoder produces.
        if let Some(company_guid) = company_guid {
            let ours: Vec<(String, String)> = groups
                .iter()
                .map(|group| {
                    (
                        group.attr("NAME").unwrap_or_default().to_string(),
                        group
                            .child("PARENT")
                            .map(|p| p.text.clone())
                            .unwrap_or_default(),
                    )
                })
                .collect();
            assert_eq!(
                ours,
                bridge_parents(&bridge_text(&bytes), company_guid),
                "{name}: PARENT text differs from bridge-tally-protocol's"
            );
        }
    }
}

/// Every PARENT spelled from these atoms, up to four atoms long, decodes to the same text here
/// as in Bridge's protocol crate: character references to forbidden code points (decimal, hex,
/// both cases of `x`, and ten digits, the longest the rewrite's twelve-byte scan accepts), the
/// U+FFFD marker's own ingredients, and a legal reference to U+FFFD.
#[test]
fn decoding_agrees_with_bridge_protocol_on_every_short_reference_spelling() {
    const ATOMS: [&str; 15] = [
        "&#4;",
        "&#0000000004;",
        "#1234567890;",
        "&#x4;",
        "&#X1f;",
        "&#0;",
        "&#65535;",
        "&#65533;",
        "&#xFFFD;",
        "\u{fffd}",
        "#4;",
        "#",
        "7",
        ";",
        " Primary",
    ];
    const GUID: &str = "61c6de69-1748-461c-ad3f-162cb949df9f";
    let mut spellings = vec![String::new()];
    let mut frontier = vec![String::new()];
    for _ in 0..4 {
        frontier = frontier
            .iter()
            .flat_map(|prefix| ATOMS.iter().map(move |atom| format!("{prefix}{atom}")))
            .collect();
        spellings.extend(frontier.iter().cloned());
    }
    let mut compared = 0;
    for (index, parent) in spellings.iter().enumerate() {
        let xml = format!(
            "<ENVELOPE><HEADER><STATUS>1</STATUS></HEADER><BODY><DATA><COLLECTION>\
             <GROUP NAME=\"G\"><GUID>{GUID}-{index:08}</GUID><MASTERID>1</MASTERID>\
             <ALTERID>1</ALTERID><PARENT>A{parent}B</PARENT></GROUP>\
             </COLLECTION></DATA></BODY></ENVELOPE>"
        );
        let bridge = bridge_parents(&xml, GUID);
        let root = xml::read(xml.as_bytes(), "spelling");
        let root = root.unwrap_or_else(|error| {
            panic!("PARENT A{parent}B: Bridge reads it, so must this crate: {error}")
        });
        let ours = groups(&root)[0].child("PARENT").map(|p| p.text.clone());
        assert_eq!(
            ours.as_deref(),
            Some(bridge[0].1.as_str()),
            "PARENT A{parent}B"
        );
        compared += 1;
    }
    assert_eq!(
        compared,
        1 + 15 + 15usize.pow(2) + 15usize.pow(3) + 15usize.pow(4)
    );
}

/// A reference the rewrite does not mark, because its `;` is past the twelve-byte scan, is
/// refused here when it names a forbidden code point. Bridge's text reader resolves it to the
/// raw control character instead, which is one of the spellings the rule exists to remove.
#[test]
fn a_forbidden_reference_the_rewrite_does_not_mark_is_refused() {
    const GUID: &str = "61c6de69-1748-461c-ad3f-162cb949df9f";
    let xml = format!(
        "<ENVELOPE><HEADER><STATUS>1</STATUS></HEADER><BODY><DATA><COLLECTION>\
         <GROUP NAME=\"G\"><GUID>{GUID}-00000001</GUID><MASTERID>1</MASTERID>\
         <ALTERID>1</ALTERID><PARENT>A&#00000000004;B</PARENT></GROUP>\
         </COLLECTION></DATA></BODY></ENVELOPE>"
    );
    let bridge = bridge_parents(&xml, GUID);
    assert_eq!(bridge[0].1, "A\u{4}B");
    assert!(xml::read(xml.as_bytes(), "unmarked").is_err());
    // Ten digits fit the scan, so the same code point is marked and read.
    let marked = xml.replace("&#00000000004;", "&#0000000004;");
    let root = xml::read(marked.as_bytes(), "marked").expect("a marked reference reads");
    assert_eq!(groups(&root)[0].child_text("PARENT"), "A\u{fffd}#4;B");
}

/// An unterminated `&#` -- no `;` within the rewrite's twelve-byte scan window -- must not stop
/// the scan: a later, fully-formed forbidden reference in the same document is still marked.
///
/// The pre-#503 copy of this rule that `bridge-tax-audit` carried in its own `xml` module (before
/// it called `bridge-tally-protocol`'s public `mark_forbidden_numeric_references`) broke on
/// exactly this input: reaching the unterminated `&#` ended its whole scan with `break`, so a
/// later, otherwise ordinary `&#4;` was left unmarked and reached the XML parser, which refuses
/// it (a raw U+0004 is not an XML 1.0 `Char`).
#[test]
fn an_unterminated_reference_does_not_stop_marking_a_later_forbidden_one() {
    // "#" plus these eleven digits exactly fills the twelve-byte scan window with no `;`, so
    // this reference is left exactly as written by both the old and the fixed rule.
    let unterminated_digits = "0".repeat(11);
    let xml = format!("<ROOT>A&#{unterminated_digits}B&#4; Primary</ROOT>");

    // bridge-tally-protocol's own public rule (already exercised by the rest of this file)
    // is the reference behaviour: the unterminated reference survives untouched, and the scan
    // continues past it to mark the later, well-terminated forbidden reference.
    let expected = format!("<ROOT>A&#{unterminated_digits}B\u{fffd}#4; Primary</ROOT>");
    assert_eq!(
        mark_forbidden_numeric_references(&xml),
        expected,
        "bridge-tally-protocol must keep scanning past an unterminated reference"
    );

    // bridge-tax-audit's `xml::decode` (steps 1-3 of the module docs: bytes to text, the DTD
    // check, then the forbidden-reference rewrite) now calls that same public rule, so it must
    // produce the identical marked text, not the pre-#503 copy's truncated one.
    let decoded = xml::decode(xml.as_bytes(), "unterminated-then-forbidden")
        .expect("no DTD or entity declaration here, so decode must succeed");
    assert_eq!(
        decoded, expected,
        "bridge-tax-audit's decode step must mark the later forbidden reference too"
    );
}
