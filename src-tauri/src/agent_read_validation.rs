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
#[path = "agent_read_validation_tests.rs"]
mod tests;
