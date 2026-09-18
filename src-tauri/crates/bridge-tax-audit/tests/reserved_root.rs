//! How the reserved-root marker and raw control characters are represented (the Tally text
//! rule, `docs/tax-audit/read-format-v1.md` section 10).

use bridge_tax_audit::book::{chain, load_groups};
use bridge_tax_audit::xml;

#[test]
fn a_group_named_primary_is_not_the_reserved_root() {
    assert!(xml::is_reserved_root("\u{fffd}#4; Primary"));
    assert!(xml::is_reserved_root(" \u{fffd}#4;Primary "));
    assert!(!xml::is_reserved_root("Primary"));
    assert!(!xml::is_reserved_root("\u{4} Primary"));
    assert!(!xml::is_reserved_root("\u{fffd}#65533;#4; Primary"));
    assert!(!xml::is_reserved_root("\u{fffd}#4; Primary Group"));

    // A user group literally named "Primary" sits under the reserved root, and "Child" sits
    // under that user group. A ledger under "Child" must walk through the user group, not stop
    // at it as if it were Tally's root.
    let xml = "<ENVELOPE>\
        <GROUP NAME=\"Primary\"><PARENT>&#4; Primary</PARENT></GROUP>\
        <GROUP NAME=\"Child\"><PARENT>Primary</PARENT></GROUP>\
        </ENVELOPE>";
    let groups = load_groups(&xml::read(xml.as_bytes(), "g").unwrap());
    assert_eq!(groups["Primary"], None);
    assert_eq!(groups["Child"].as_deref(), Some("Primary"));
    let (walked, complete) = chain("Child", &groups);
    assert_eq!(walked, ["Child", "Primary"]);
    assert!(complete);
    let (walked, complete) = chain("\u{fffd}#4; Primary", &groups);
    assert_eq!(walked, ["\u{fffd}#4; Primary"]);
    assert!(complete);
}

#[test]
fn raw_nul_and_noncharacters_are_refused_other_c0_controls_are_kept() {
    for refused in ["\u{0}", "\u{fffe}", "\u{ffff}"] {
        let xml = format!("<A><B>x{refused}y</B></A>");
        assert!(xml::read(xml.as_bytes(), "t").is_err(), "{refused:?}");
    }
    for c in (1u32..0x20).filter(|c| ![0x9, 0xa, 0xd].contains(c)) {
        let c = char::from_u32(c).unwrap();
        let xml = format!("<A N=\"a{c}b\"><B>x{c}y</B></A>");
        let root = xml::read(xml.as_bytes(), "t").unwrap();
        assert_eq!(root.attr("N"), Some(format!("a{c}b").as_str()));
        assert_eq!(root.child("B").unwrap().text, format!("x{c}y"));
    }
}
