//! Crate-internal admission for XML emitted by the agent's reviewed read profiles.
use quick_xml::events::Event;

/// An Export/Collection envelope from an internal profile builder. This validates
/// the dispatch operation; it does not prove arbitrary embedded TDL side-effect free.
/// The private field and crate-private API keep external library callers out of
/// the agent's raw dispatch path.
#[derive(Clone, Debug)]
pub(crate) struct AgentReadRequest(String);

#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
#[error("agent_write_dispatch_forbidden")]
pub(crate) struct AgentReadRequestError;

impl AgentReadRequest {
    pub(crate) fn parse(xml: String) -> Result<Self, AgentReadRequestError> {
        let mut reader = quick_xml::Reader::from_str(&xml);
        let mut path = Vec::<Vec<u8>>::new();
        let mut roots = 0;
        let mut headers = 0;
        let mut operation = None;
        let mut kind = None;
        loop {
            match reader.read_event().map_err(|_| AgentReadRequestError)? {
                Event::Start(event) => {
                    let name = event.name().as_ref().to_vec();
                    if path.is_empty() {
                        roots += 1;
                        if name != b"ENVELOPE" || roots != 1 {
                            return Err(AgentReadRequestError);
                        }
                    }
                    if path == [b"ENVELOPE".to_vec()] && name == b"HEADER" {
                        headers += 1;
                    }
                    if path == [b"ENVELOPE".to_vec(), b"HEADER".to_vec()]
                        && matches!(name.as_slice(), b"TALLYREQUEST" | b"TYPE")
                    {
                        let value = reader
                            .read_text(event.name())
                            .map_err(|_| AgentReadRequestError)?;
                        let value = value.decode().map_err(|_| AgentReadRequestError)?;
                        let target = if name == b"TALLYREQUEST" {
                            &mut operation
                        } else {
                            &mut kind
                        };
                        if target.replace(value.trim().to_string()).is_some() {
                            return Err(AgentReadRequestError);
                        }
                    } else {
                        path.push(name);
                    }
                }
                Event::End(event) => {
                    if path.pop().as_deref() != Some(event.name().as_ref()) {
                        return Err(AgentReadRequestError);
                    }
                }
                Event::Empty(event) => {
                    let name = event.name();
                    if path.is_empty()
                        || (path == [b"ENVELOPE".to_vec()] && name.as_ref() == b"HEADER")
                        || (path == [b"ENVELOPE".to_vec(), b"HEADER".to_vec()]
                            && matches!(name.as_ref(), b"TALLYREQUEST" | b"TYPE"))
                    {
                        return Err(AgentReadRequestError);
                    }
                }
                Event::DocType(_) => return Err(AgentReadRequestError),
                Event::Text(text) if path.is_empty() => {
                    if !text
                        .decode()
                        .map_err(|_| AgentReadRequestError)?
                        .trim()
                        .is_empty()
                    {
                        return Err(AgentReadRequestError);
                    }
                }
                Event::Eof => break,
                _ => {}
            }
        }
        if roots != 1
            || headers != 1
            || !path.is_empty()
            || !operation
                .as_deref()
                .is_some_and(|value| value.eq_ignore_ascii_case("Export"))
            || !kind
                .as_deref()
                .is_some_and(|value| value.eq_ignore_ascii_case("Collection"))
        {
            return Err(AgentReadRequestError);
        }
        Ok(Self(xml))
    }

    pub(crate) fn into_xml(self) -> String {
        self.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_one_complete_export_collection_header_is_admitted() {
        let request =
            bridge_tally_protocol::xml_read_profiles::ReadOnlyProfile::CompanyListV2.render();
        assert_eq!(
            AgentReadRequest::parse(request.clone()).unwrap().into_xml(),
            request
        );
        let catalogue =
            bridge_tally_protocol::xml_read_profiles::ReadOnlyProfile::StandardLedgerCatalogV1 {
                company: &bridge_tally_protocol::xml_read_profiles::ValidatedCompanyName::new(
                    "Synthetic Book",
                )
                .unwrap(),
            }
            .render();
        assert!(AgentReadRequest::parse(catalogue).is_ok());
        for denied in [
            request.replace(
                "<TALLYREQUEST>Export</TALLYREQUEST>",
                "<TALLYREQUEST> Import Data </TALLYREQUEST>",
            ),
            request.replace(
                "<TALLYREQUEST>Export</TALLYREQUEST>",
                "<TALLYREQUEST>Execute</TALLYREQUEST>",
            ),
            request.replace("<TYPE>Collection</TYPE>", "<TYPE>Function</TYPE>"),
            request.replace("</HEADER>", "<TALLYREQUEST>Export</TALLYREQUEST></HEADER>"),
            request.replace("</ENVELOPE>", ""),
            format!("{request}{request}"),
            format!("<!DOCTYPE ENVELOPE>{request}"),
        ] {
            assert_eq!(
                AgentReadRequest::parse(denied).unwrap_err(),
                AgentReadRequestError
            );
        }
    }
}
