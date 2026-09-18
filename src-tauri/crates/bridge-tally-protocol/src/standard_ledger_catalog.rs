//! Tally's standard List of Ledgers: the catalogue rows, their company binding,
//! and the identity observation, with display-name safety checks on every name.

use std::collections::HashSet;

use quick_xml::{events::Event, Reader};

use crate::{
    attr_value, configured_reader, normalized_standard_company_guid, normalized_standard_value,
    path_eq, pop_expected_path, read_identifier_text, read_required_text, validate_export_response,
    validate_only_attributes, PartyLedgerMasterFieldObservation, TallyLedger,
};

pub const MAX_STANDARD_LEDGER_IDENTITY_ROWS: usize = 1_000;

/// A failed standard-ledger catalog is never a usable catalog. Keep the
/// failure class at the XML boundary so callers can retain their fail-closed
/// behavior without interpreting parser text.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StandardLedgerCatalogError {
    MalformedResponse,
    /// A row's ledger name was refused on its own merits -- blank, past the
    /// length bound, or carrying a character that makes it read as a different
    /// name. Split out from `MalformedResponse` because the two say different
    /// things to whoever has to act: a malformed response is Tally or the
    /// transport, and this is one master in an otherwise well-formed export.
    ///
    /// Diagnosing a real instance of this took four rounds of instrumentation
    /// precisely because it arrived as `MalformedResponse` -- the response was
    /// not malformed at all, and every hypothesis started from the wrong half
    /// of the system.
    LedgerNameUnusable,
    CompanyIdentityMismatch,
    DuplicateIdentity,
    BoundsViolation,
}

impl std::fmt::Display for StandardLedgerCatalogError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(match self {
            Self::MalformedResponse => "standard ledger catalog response was malformed",
            Self::LedgerNameUnusable => "standard ledger catalog held an unusable ledger name",
            Self::CompanyIdentityMismatch => {
                "standard ledger catalog did not confirm the selected company"
            }
            Self::DuplicateIdentity => "standard ledger catalog contained a duplicate identity",
            Self::BoundsViolation => "standard ledger catalog exceeded a safety bound",
        })
    }
}

impl std::error::Error for StandardLedgerCatalogError {}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StandardLedgerIdentityObservation {
    pub company_guid: String,
    pub ledger_count: u64,
}

/// Validates the fixed, documented `List of Ledgers` collection used only to
/// bootstrap a scoped company identity on responders that reject Bridge's
/// custom report profile. Ledger names, balances, and identities are inspected
/// in memory only and never returned by this parser.
pub fn parse_standard_ledger_identity_observation(
    xml: &str,
    expected_company_name: &str,
) -> anyhow::Result<StandardLedgerIdentityObservation> {
    // The one rule every Tally read applies first (§1.1(d)).
    let marked = crate::mark_forbidden_numeric_references(xml);
    let xml = marked.as_ref();
    validate_export_response(xml)?;
    let expected_company_name = normalized_standard_value(expected_company_name, "company name")?;
    let mut reader = configured_reader(xml);
    let mut path = Vec::<Vec<u8>>::new();
    let mut ledger_count = 0_usize;
    let mut company_guid = None::<String>;
    loop {
        match reader.read_event()? {
            Event::Start(element)
                if path_eq(&path, &[b"ENVELOPE", b"BODY", b"DATA", b"COLLECTION"]) =>
            {
                if ledger_count >= MAX_STANDARD_LEDGER_IDENTITY_ROWS {
                    anyhow::bail!(
                        "standard ledger identity collection exceeded the safe row limit"
                    );
                }
                let observed = parse_standard_ledger_identity_row(&mut reader, &element, false)?;
                if observed.company_name != expected_company_name {
                    anyhow::bail!(
                        "standard ledger identity collection did not confirm the requested company"
                    );
                }
                if let Some(previous) = &company_guid {
                    if previous != &observed.company_guid {
                        anyhow::bail!(
                            "standard ledger identity collection contained inconsistent company context"
                        );
                    }
                } else {
                    company_guid = Some(observed.company_guid);
                }
                ledger_count += 1;
            }
            Event::Start(element) => path.push(element.name().as_ref().to_ascii_uppercase()),
            Event::Empty(_element)
                if path_eq(&path, &[b"ENVELOPE", b"BODY", b"DATA", b"COLLECTION"]) =>
            {
                anyhow::bail!("standard ledger identity collection contained an empty row");
            }
            Event::End(element) => pop_expected_path(&mut path, element.name().as_ref())?,
            Event::Eof => break,
            _ => {}
        }
    }
    if !path.is_empty() {
        anyhow::bail!("standard ledger identity collection ended before its root closed");
    }
    Ok(StandardLedgerIdentityObservation {
        company_guid: company_guid.ok_or_else(|| {
            anyhow::anyhow!(
                "standard ledger identity collection did not return a usable ledger row"
            )
        })?,
        ledger_count: ledger_count as u64,
    })
}

/// Opaque identities from one validated standard catalog. GUIDs remain
/// internal to the admission path and are never serialized into a tool result
/// or desktop review.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StandardLedgerCatalog {
    entries: Vec<StandardLedgerCatalogEntry>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct StandardLedgerCatalogEntry {
    name: String,
    guid: String,
    /// The immediate `PARENT` group Tally returned for this ledger, or `None`
    /// when it returned none. A ledger exposes no `PARENTSTRUCTURE`, so this
    /// single hop is all the ancestry one catalog response carries; a caller
    /// that needs the group's own identity must read the Group collection.
    parent: Option<String>,
}

impl StandardLedgerCatalog {
    pub fn names(&self) -> impl Iterator<Item = &str> {
        self.entries.iter().map(|entry| entry.name.as_str())
    }

    /// Each ledger paired with the immediate parent group Tally returned for
    /// it. `None` is an unobserved parent, never an empty group name.
    pub fn parents(&self) -> impl Iterator<Item = (&str, Option<&str>)> {
        self.entries
            .iter()
            .map(|entry| (entry.name.as_str(), entry.parent.as_deref()))
    }

    pub fn bind_selected(
        &self,
        requested_names: impl IntoIterator<Item = String>,
    ) -> anyhow::Result<StandardLedgerCatalogBinding> {
        let mut requested = requested_names.into_iter().collect::<Vec<_>>();
        requested.sort();
        requested.dedup();
        let entries = requested
            .into_iter()
            .map(|name| {
                let entry = self
                    .entries
                    .iter()
                    .find(|candidate| candidate.name == name)
                    .ok_or_else(|| {
                        anyhow::anyhow!("standard ledger catalog omitted requested ledger")
                    })?;
                Ok((name, entry.guid.clone()))
            })
            .collect::<anyhow::Result<Vec<_>>>()?;
        Ok(StandardLedgerCatalogBinding { entries })
    }
}

/// Opaque selected-master identities from one validated standard catalog.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StandardLedgerCatalogBinding {
    entries: Vec<(String, String)>,
}

impl StandardLedgerCatalogBinding {
    /// True when every selected pair is still present in an already-parsed
    /// catalog.
    ///
    /// TALLY_PROTOCOL_REFERENCE.md §12a.9: Tally can retain a GUID while
    /// changing a visible ledger name, so admission binds the selected pair
    /// rather than either half.
    ///
    /// Callers holding a parsed response should prefer this: a caller checking
    /// several bindings against one response would otherwise reparse it once
    /// per binding.
    pub fn matches_catalog(&self, current: &StandardLedgerCatalog) -> bool {
        self.entries.iter().all(|(name, guid)| {
            current.entries.iter().any(|candidate| {
                &candidate.name == name && candidate.guid.eq_ignore_ascii_case(guid)
            })
        })
    }

    /// [`Self::matches_catalog`] for a caller that holds only the response body.
    pub fn matches(
        &self,
        xml: &str,
        expected_company_name: &str,
        expected_company_guid: &str,
    ) -> Result<bool, StandardLedgerCatalogError> {
        let current = parse_standard_ledger_catalog_with_identities(
            xml,
            expected_company_name,
            expected_company_guid,
        )?;
        Ok(self.matches_catalog(&current))
    }
}

/// Parses the standard catalog once, retaining source GUIDs only in an opaque
/// in-memory value so an admission caller can select bindings without a second
/// parse of the same response.
pub fn parse_standard_ledger_catalog_with_identities(
    xml: &str,
    expected_company_name: &str,
    expected_company_guid: &str,
) -> Result<StandardLedgerCatalog, StandardLedgerCatalogError> {
    let rows =
        parse_standard_ledger_catalog_rows(xml, expected_company_name, expected_company_guid)?;
    Ok(StandardLedgerCatalog {
        entries: rows
            .into_iter()
            .map(|row| StandardLedgerCatalogEntry {
                name: row.ledger.name,
                guid: row.guid,
                parent: row
                    .ledger
                    .parent
                    .nonempty_returned_text()
                    .map(str::to_string),
            })
            .collect(),
    })
}

/// Parses the documented `List of Ledgers` collection as a deliberately
/// limited interactive catalog. The source GUIDs prove row uniqueness and
/// company scope in memory only; callers receive no GUIDs or raw XML.
pub fn parse_standard_ledger_catalog(
    xml: &str,
    expected_company_name: &str,
    expected_company_guid: &str,
) -> Result<Vec<TallyLedger>, StandardLedgerCatalogError> {
    Ok(
        parse_standard_ledger_catalog_rows(xml, expected_company_name, expected_company_guid)?
            .into_iter()
            .map(|row| row.ledger)
            .collect(),
    )
}

struct StandardLedgerCatalogRow {
    ledger: TallyLedger,
    guid: String,
}

fn parse_standard_ledger_catalog_rows(
    xml: &str,
    expected_company_name: &str,
    expected_company_guid: &str,
) -> Result<Vec<StandardLedgerCatalogRow>, StandardLedgerCatalogError> {
    // The one rule every Tally read applies first (§1.1(d)): ledger names and
    // parents here must spell a forbidden reference exactly as the voucher
    // rows they are matched against do, and `&#4; Primary` must reach
    // `group_ancestry` as the reserved root rather than as a control
    // character `safe_standard_ledger_parent` would discard.
    let marked = crate::mark_forbidden_numeric_references(xml);
    let xml = marked.as_ref();
    validate_export_response(xml).map_err(|_| StandardLedgerCatalogError::MalformedResponse)?;
    let expected_company_name = normalized_standard_value(expected_company_name, "company name")
        .map_err(|_| StandardLedgerCatalogError::BoundsViolation)?;
    let expected_company_guid = normalized_standard_company_guid(expected_company_guid)
        .map_err(|_| StandardLedgerCatalogError::BoundsViolation)?;
    let mut reader = configured_reader(xml);
    let mut path = Vec::<Vec<u8>>::new();
    let mut rows = Vec::new();
    let mut seen_names = HashSet::new();
    let mut seen_guids = HashSet::new();
    loop {
        match reader
            .read_event()
            .map_err(|_| StandardLedgerCatalogError::MalformedResponse)?
        {
            Event::Start(element)
                if path_eq(&path, &[b"ENVELOPE", b"BODY", b"DATA", b"COLLECTION"]) =>
            {
                if rows.len() >= MAX_STANDARD_LEDGER_IDENTITY_ROWS {
                    return Err(StandardLedgerCatalogError::BoundsViolation);
                }
                // The row parser reports through `anyhow`, so a class raised
                // inside it arrives boxed. Recover it rather than flattening
                // every failure to "malformed": a refused ledger name is not a
                // malformed response, and saying so sent a previous diagnosis
                // at the transport for three rounds.
                let observed = parse_standard_ledger_identity_row(&mut reader, &element, true)
                    .map_err(|error| {
                        error
                            .downcast_ref::<StandardLedgerCatalogError>()
                            .copied()
                            .unwrap_or(StandardLedgerCatalogError::MalformedResponse)
                    })?;
                if observed.company_name != expected_company_name
                    || !observed
                        .company_guid
                        .eq_ignore_ascii_case(&expected_company_guid)
                {
                    return Err(StandardLedgerCatalogError::CompanyIdentityMismatch);
                }
                let ledger_name = observed
                    .ledger_name
                    .ok_or(StandardLedgerCatalogError::MalformedResponse)?;
                let ledger_guid = observed
                    .ledger_guid
                    .ok_or(StandardLedgerCatalogError::MalformedResponse)?;
                if !seen_names.insert(standard_ledger_name_comparison_key(&ledger_name))
                    || !seen_guids.insert(ledger_guid.to_ascii_lowercase())
                {
                    return Err(StandardLedgerCatalogError::DuplicateIdentity);
                }
                rows.push(StandardLedgerCatalogRow {
                    ledger: TallyLedger {
                        name: ledger_name,
                        parent: observed.parent,
                        party_gstin: PartyLedgerMasterFieldObservation::NotObserved,
                        opening_balance: None,
                    },
                    guid: ledger_guid,
                });
            }
            Event::Start(element) => path.push(element.name().as_ref().to_ascii_uppercase()),
            Event::Empty(_) if path_eq(&path, &[b"ENVELOPE", b"BODY", b"DATA", b"COLLECTION"]) => {
                return Err(StandardLedgerCatalogError::MalformedResponse);
            }
            Event::End(element) => pop_expected_path(&mut path, element.name().as_ref())
                .map_err(|_| StandardLedgerCatalogError::MalformedResponse)?,
            Event::Eof => break,
            _ => {}
        }
    }
    if path.is_empty() && !rows.is_empty() {
        Ok(rows)
    } else {
        Err(StandardLedgerCatalogError::MalformedResponse)
    }
}

struct StandardLedgerIdentityRow {
    company_name: String,
    company_guid: String,
    ledger_name: Option<String>,
    ledger_guid: Option<String>,
    parent: PartyLedgerMasterFieldObservation,
}

fn parse_standard_ledger_identity_row(
    reader: &mut Reader<&[u8]>,
    element: &quick_xml::events::BytesStart<'_>,
    include_ledger_name: bool,
) -> anyhow::Result<StandardLedgerIdentityRow> {
    validate_only_attributes(element, &[b"NAME", b"RESERVEDNAME"])?;
    let mut ledger_name = include_ledger_name
        .then(|| attr_value(reader, element, b"NAME"))
        .flatten()
        .map(|value| observed_standard_ledger_name(&value))
        .transpose()?;
    let row_name = element.name().as_ref().to_ascii_uppercase();
    let mut company_name = None;
    let mut company_guid = None;
    let mut ledger_guid = None;
    let mut parent = PartyLedgerMasterFieldObservation::NotObserved;
    let mut parent_seen = false;
    loop {
        match reader.read_event()? {
            Event::Start(child) => {
                let child_name = child.name().as_ref().to_ascii_uppercase();
                match child_name.as_slice() {
                    b"NAME" if include_ledger_name => {
                        validate_only_attributes(&child, &[b"TYPE"])?;
                        if ledger_name
                            .replace(observed_standard_ledger_name(&read_required_text(
                                reader,
                                child.name(),
                            )?)?)
                            .is_some()
                        {
                            anyhow::bail!("standard ledger collection repeated ledger name");
                        }
                    }
                    b"NAME" => {
                        validate_only_attributes(&child, &[b"TYPE"])?;
                        skip_standard_ledger_identity_child(
                            reader,
                            child.name().as_ref().to_ascii_uppercase(),
                        )?;
                    }
                    b"BRIDGECOMPANYNAME" => {
                        validate_only_attributes(&child, &[b"TYPE"])?;
                        set_bootstrap_context_once(
                            &mut company_name,
                            normalized_standard_value(
                                &read_required_text(reader, child.name())?,
                                "company name",
                            )?,
                            "company name",
                        )?;
                    }
                    b"BRIDGECOMPANYGUID" => {
                        validate_only_attributes(&child, &[b"TYPE"])?;
                        set_bootstrap_context_once(
                            &mut company_guid,
                            normalized_standard_company_guid(&read_required_text(
                                reader,
                                child.name(),
                            )?)?,
                            "company GUID",
                        )?;
                    }
                    b"GUID" if include_ledger_name => {
                        validate_only_attributes(&child, &[b"TYPE"])?;
                        if ledger_guid
                            .replace(normalized_standard_value(
                                &read_required_text(reader, child.name())?,
                                "ledger GUID",
                            )?)
                            .is_some()
                        {
                            anyhow::bail!("standard ledger collection repeated ledger GUID");
                        }
                    }
                    b"GUID" => {
                        validate_only_attributes(&child, &[b"TYPE"])?;
                        skip_standard_ledger_identity_child(
                            reader,
                            child.name().as_ref().to_ascii_uppercase(),
                        )?;
                    }
                    b"PARENT" if include_ledger_name => {
                        validate_only_attributes(&child, &[b"TYPE"])?;
                        if parent_seen {
                            anyhow::bail!("standard ledger collection repeated ledger parent");
                        }
                        parent_seen = true;
                        // `read_identifier_text`, not `read_optional_text`: the latter
                        // trims, which would hand `safe_standard_ledger_parent` an
                        // already-normalized name and defeat the byte preservation the
                        // function below exists to provide.
                        parent = match read_identifier_text(reader, child.name())? {
                            Some(value) => match safe_standard_ledger_parent(&value) {
                                Some(value) => PartyLedgerMasterFieldObservation::Returned(value),
                                None => PartyLedgerMasterFieldObservation::NotObserved,
                            },
                            None => PartyLedgerMasterFieldObservation::Returned(String::new()),
                        };
                    }
                    b"PARENT" => {
                        validate_only_attributes(&child, &[b"TYPE"])?;
                        skip_standard_ledger_identity_child(
                            reader,
                            child.name().as_ref().to_ascii_uppercase(),
                        )?;
                    }
                    b"LANGUAGENAME.LIST" => skip_standard_ledger_identity_child(
                        reader,
                        child.name().as_ref().to_ascii_uppercase(),
                    )?,
                    _ => anyhow::bail!(
                        "standard ledger identity collection contained an unexpected row field"
                    ),
                }
            }
            Event::End(end) if end.name().as_ref().eq_ignore_ascii_case(&row_name) => break,
            Event::Empty(child)
                if include_ledger_name && child.name().as_ref().eq_ignore_ascii_case(b"PARENT") =>
            {
                validate_only_attributes(&child, &[b"TYPE"])?;
                if parent_seen {
                    anyhow::bail!("standard ledger collection repeated ledger parent");
                }
                parent_seen = true;
                parent = PartyLedgerMasterFieldObservation::Returned(String::new());
            }
            Event::Empty(_) => {
                anyhow::bail!("standard ledger identity collection contained an empty row field")
            }
            Event::Text(text) if !text.decode()?.trim().is_empty() => {
                anyhow::bail!("standard ledger identity collection contained unexpected row text")
            }
            Event::CData(_) | Event::DocType(_) | Event::PI(_) => {
                anyhow::bail!(
                    "standard ledger identity collection contained a forbidden XML construct"
                )
            }
            Event::Eof => {
                anyhow::bail!("standard ledger identity collection row ended before closing")
            }
            _ => {}
        }
    }
    Ok(StandardLedgerIdentityRow {
        company_name: company_name.ok_or_else(|| {
            anyhow::anyhow!("standard ledger identity collection omitted computed company name")
        })?,
        company_guid: company_guid.ok_or_else(|| {
            anyhow::anyhow!("standard ledger identity collection omitted computed company GUID")
        })?,
        ledger_name,
        ledger_guid,
        parent,
    })
}

fn skip_standard_ledger_identity_child(
    reader: &mut Reader<&[u8]>,
    expected_name: Vec<u8>,
) -> anyhow::Result<()> {
    let mut depth = 1_u32;
    loop {
        match reader.read_event()? {
            Event::Start(_) => {
                depth = depth.checked_add(1).ok_or_else(|| {
                    anyhow::anyhow!("standard ledger identity nesting exceeded limits")
                })?
            }
            Event::End(end) => {
                depth = depth.checked_sub(1).ok_or_else(|| {
                    anyhow::anyhow!(
                        "standard ledger identity collection closed an unexpected field"
                    )
                })?;
                if depth == 0 {
                    if !end.name().as_ref().eq_ignore_ascii_case(&expected_name) {
                        anyhow::bail!(
                            "standard ledger identity collection closed an unexpected field"
                        );
                    }
                    return Ok(());
                }
            }
            Event::DocType(_) | Event::PI(_) => {
                anyhow::bail!(
                    "standard ledger identity collection contained a forbidden XML construct"
                )
            }
            Event::Eof => {
                anyhow::bail!("standard ledger identity collection field ended before closing")
            }
            _ => {}
        }
    }
}

/// A ledger name is an **identity** value, not a display string: it is matched
/// against proposals by exact codepoint, and echoed back as the import spelling
/// that has to round-trip to Tally. So the only things it can be refused for are
/// the two that make it unusable as an identity -- absent, or past the bound this
/// parser is willing to hold.
///
/// It deliberately does **not** refuse control or bidi characters. Real books
/// hold ledger names with an embedded newline, and books migrated from other
/// software hold names with C1 bytes baked in by a double-encoding import. When
/// this refused them, one such master failed the entire company's catalog and
/// with it every read that needs one -- presence, a ledger-scoped voucher
/// window, and import validation. A malformed name must not remove a real
/// master, and must not be rewritten either: Tally matches by exact codepoint,
/// so a cleaned-up spelling addresses a ledger that does not exist.
///
/// What it still refuses is the set that makes a name *lie about itself*:
/// bidirectional overrides and zero-width characters, which can render one
/// spelling as another. A newline is ugly in a terminal and the renderer's
/// problem; a right-to-left override is a forged name and this parser's.
fn observed_standard_ledger_name(value: &str) -> Result<String, StandardLedgerCatalogError> {
    if value.trim().is_empty()
        || value.len() > 512
        || value.chars().any(deceptive_display_character)
    {
        return Err(StandardLedgerCatalogError::LedgerNameUnusable);
    }
    Ok(value.to_string())
}

fn standard_ledger_name_comparison_key(value: &str) -> String {
    value.to_lowercase()
}

/// Validates a ledger's `PARENT` without normalising it.
///
/// The emptiness test reads the trimmed view, but the value is retained
/// verbatim. A `PARENT` is a foreign reference to a group `NAME`, matched by
/// exact codepoint, so trimming here would silently resolve a pair that
/// [`group_ancestry`] is built to refuse — and it would do so upstream of the
/// walk, where the walk cannot see it.
fn safe_standard_ledger_parent(value: &str) -> Option<String> {
    if value.trim().is_empty() || value.len() > 1024 || value.chars().any(unsafe_display_character)
    {
        return None;
    }
    Some(value.to_string())
}

fn unsafe_display_character(value: char) -> bool {
    value.is_control() || deceptive_display_character(value)
}

/// The half of [`unsafe_display_character`] that is about *deception* rather
/// than about rendering: characters that reorder or hide the text around them,
/// so that the spelling shown is not the spelling stored.
///
/// Split out because the two halves earn different answers on an observed
/// master name. A control character there is a real, if untidy, name a book
/// genuinely holds; one of these is a name forged to read as another, and no
/// book has a legitimate reason to hold one.
fn deceptive_display_character(value: char) -> bool {
    matches!(
        value,
        // Deliberately NOT a single `U+200B..=U+200F` range. That span holds
        // ZWNJ (U+200C) and ZWJ (U+200D), which are **orthography**, not
        // deception: Devanagari and other Indic scripts need them to control
        // conjunct formation, and this repository's own fixtures are full of
        // Indic ledger names. Refusing them would make a legitimately spelled
        // Hindi or Marathi ledger fail the whole catalog -- the exact failure
        // the newline fix existed to remove.
        //
        // The trade-off is real and worth stating: a codepoint filter cannot
        // know that a ZWJ sits between two Devanagari consonants rather than
        // injected into ASCII, so admitting them re-admits a narrow version of
        // the deception this set exists to stop -- `Alpha<ZWJ> Traders` is
        // byte-distinct from `Alpha Traders` and renders the same. It is
        // bounded rather than closed: the fold and the token index keep the
        // joiner verbatim, so such a name tends to fail exact and token
        // matching instead of quietly aliasing a real master. That is a worse
        // guarantee than refusal and a far better one than breaking every
        // Indic book, which is what refusal actually cost.
        '\u{061C}'
            | '\u{200B}'
            | '\u{200E}'
            | '\u{200F}'
            | '\u{202A}'..='\u{202E}'
            | '\u{2060}'
            | '\u{2066}'..='\u{206F}'
            | '\u{FEFF}'
    )
}

fn set_bootstrap_context_once(
    slot: &mut Option<String>,
    value: String,
    label: &str,
) -> anyhow::Result<()> {
    if slot.replace(value).is_some() {
        anyhow::bail!("standard ledger identity collection repeated computed {label}");
    }
    Ok(())
}
