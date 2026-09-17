use super::*;

#[test]
fn captured_exports_require_structural_success_before_rows_are_admitted() {
    let bytes = include_bytes!(
        "../crates/bridge-tally-protocol/tests/fixtures/agent/native-empty-collection.utf16le.xml"
    );
    let words = bytes
        .chunks_exact(2)
        .map(|pair| u16::from_le_bytes([pair[0], pair[1]]))
        .collect::<Vec<_>>();
    let xml = String::from_utf16(&words).unwrap();
    assert_eq!(validate_agent_envelope(&xml), Ok(()));
    for status in [
        "<STATUS>0</STATUS>",
        "<STATUS>Maybe</STATUS>",
        "<STATUS/>",
        "",
        "<STATUS>1</STATUS><STATUS>1</STATUS>",
    ] {
        let faulty = xml.replacen("<STATUS>1</STATUS>", status, 1);
        assert_ne!(faulty, xml);
        assert_eq!(
            validate_agent_envelope(&faulty),
            Err("agent_read_protocol_invalid".into())
        );
        assert_eq!(
            crate::agent::parse_agent_rows(&faulty, "unused-empty-source-company"),
            Err("agent_read_protocol_invalid".into())
        );
    }
    for error in ["<LINEERROR>rejected</LINEERROR>", "<error/>", "<Response/>"] {
        let faulty = xml.replacen("<DATA>", &format!("<DATA>{error}"), 1);
        assert_ne!(faulty, xml);
        assert_eq!(
            validate_agent_envelope(&faulty),
            Err("agent_read_protocol_invalid".into())
        );
    }
}
