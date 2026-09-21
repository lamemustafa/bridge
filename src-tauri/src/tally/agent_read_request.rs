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

    /// The `company` part of a tally-read v1 read: a single-object export of
    /// exactly the verified company, rendered from the pinned
    /// `AuditCompanyObjectV1` template.
    ///
    /// This is the only way an Object export reaches dispatch. [`Self::parse`]
    /// stays Collection-only and refuses every `TYPE=Object` envelope, so no
    /// XML from anywhere else can use this shape: the company name is the only
    /// variable, it comes from a verified identity, and it is validated and
    /// escaped. The response still has to be admitted by GUID, because Tally
    /// resolves the `ID` by name.
    #[cfg_attr(
        not(test),
        expect(dead_code, reason = "sent by the audit_read orchestrator, plan step 7")
    )]
    pub(crate) fn company_object(
        identity: &super::VerifiedCompanyIdentity,
    ) -> Result<Self, AgentReadRequestError> {
        let company = bridge_tally_protocol::xml_read_profiles::ValidatedCompanyName::new(
            identity.display_name(),
        )
        .map_err(|_| AgentReadRequestError)?;
        Ok(Self(
            bridge_tally_protocol::xml_read_profiles::ReadOnlyProfile::AuditCompanyObjectV1 {
                company: &company,
            }
            .render(),
        ))
    }

    /// Whether every `SVFROMDATE`/`SVTODATE` this request sends is a boundary
    /// `profile` lets Tally honour. Education mode answers a read whose window
    /// starts on another day with a well-formed empty collection rather than an
    /// error (bridge#581), so under that profile an unreadable date is refused
    /// too: a boundary Bridge cannot check is not one it has admitted.
    pub(crate) fn window_accepted_by(
        &self,
        profile: bridge_tally_protocol::outstandings_shared::DateBoundaryProfile,
    ) -> bool {
        use bridge_tally_protocol::outstandings_shared::DateBoundaryProfile;
        if profile == DateBoundaryProfile::ModeAgnostic {
            return true;
        }
        let mut reader = quick_xml::Reader::from_str(&self.0);
        loop {
            match reader.read_event() {
                Ok(Event::Start(event))
                    if matches!(
                        event.name().as_ref().to_ascii_uppercase().as_slice(),
                        b"SVFROMDATE" | b"SVTODATE"
                    ) =>
                {
                    let accepted = reader
                        .read_text(event.name())
                        .ok()
                        .and_then(|text| text.decode().ok().map(|text| text.trim().to_string()))
                        .and_then(|text| bridge_tally_core::TallyDate::parse(text).ok())
                        .is_some_and(|date| profile.accepts_boundary(&date));
                    if !accepted {
                        return false;
                    }
                }
                Ok(Event::Empty(event))
                    if matches!(
                        event.name().as_ref().to_ascii_uppercase().as_slice(),
                        b"SVFROMDATE" | b"SVTODATE"
                    ) =>
                {
                    return false;
                }
                Ok(Event::Eof) => return true,
                Err(_) => return false,
                _ => {}
            }
        }
    }

    pub(crate) fn into_xml(self) -> String {
        self.0
    }
}

#[cfg(test)]
#[path = "agent_read_request_tests.rs"]
mod tests;
