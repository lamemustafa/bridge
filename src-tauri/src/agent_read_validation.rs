//! Shared export admission before any collection can become accounting evidence.
use bridge_tally_protocol::{export_status, TallyExportStatus};
use quick_xml::events::Event;

pub(super) fn validate_agent_envelope(xml: &str) -> Result<(), String> {
    let invalid = || "agent_read_protocol_invalid".to_string();
    if !matches!(export_status(xml), Ok(TallyExportStatus::Success)) {
        return Err(invalid());
    }
    // Inspect elements, not substrings: escaped narration or CDATA may contain
    // error-looking text. Collection placement is enforced by its row parser.
    let mut reader = quick_xml::Reader::from_str(xml);
    loop {
        match reader.read_event() {
            Ok(Event::Start(tag) | Event::Empty(tag)) => {
                if [b"LINEERROR".as_slice(), b"ERROR", b"RESPONSE"]
                    .iter()
                    .any(|name| tag.name().as_ref().eq_ignore_ascii_case(name))
                {
                    return Err(invalid());
                }
            }
            Ok(Event::Eof) => return Ok(()),
            Err(_) => return Err(invalid()),
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
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
                crate::agent::parse_agent_rows(&faulty),
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
}
