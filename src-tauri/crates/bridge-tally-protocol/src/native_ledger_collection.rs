//! Tally's native Ledger collection: ledger source records and party ledger
//! master fields (including GST duty heads), each bound to the pinned company.

use std::collections::{HashMap, HashSet};

use quick_xml::{events::Event, name::QName, Reader};
use serde::{Deserialize, Serialize};

use crate::{
    attr_value, configured_reader, native_ledger_guid_has_company_prefix,
    normalized_standard_company_guid, path_eq, pop_expected_path, read_identifier_text,
    read_optional_text, read_required_text, record_identities_from_values, sha256_hex,
    source_fragment_sha256_from_sanitized, tolerant_xml, validate_only_attributes,
    validated_optional_identifier, DuplicateIdentityEvidence, ExportEvidence, ParsedExport,
    ParsedSourceIdentities, ParsedSourceIdentityKind, ParsedSourceRecord,
    PartyLedgerMasterFieldObservation, TallyLedger,
};

/// Sensitive and compliance ledger-master observations read only by the
/// dedicated party/ledger master path. Ordinary ledger reads never request or
/// retain these values.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize, Serialize)]
pub struct PartyLedgerMasterFields {
    pub income_tax_number: PartyLedgerMasterFieldObservation,
    pub name_on_pan: PartyLedgerMasterFieldObservation,
    pub pin_code: PartyLedgerMasterFieldObservation,
    pub gst_pin_code: PartyLedgerMasterFieldObservation,
    pub msme_registration_number: PartyLedgerMasterFieldObservation,
    pub udyam_registration_number: PartyLedgerMasterFieldObservation,
    pub bank_account_holder_name: PartyLedgerMasterFieldObservation,
    pub bank_details: PartyLedgerMasterFieldObservation,
    pub ifsc_code: PartyLedgerMasterFieldObservation,
    pub email: PartyLedgerMasterFieldObservation,
    pub phone: PartyLedgerMasterFieldObservation,
    pub state: PartyLedgerMasterFieldObservation,
    pub address: PartyLedgerMasterFieldObservation,
    /// Tally's ledger `TAXTYPE`, retained verbatim so callers can distinguish
    /// a GST ledger with no duty head from a ledger that is not GST-classified.
    pub tax_type: PartyLedgerMasterFieldObservation,
    /// Tally's ledger `GSTDUTYHEAD`, classified only against the measured
    /// vocabulary while retaining the source spelling for every returned head.
    pub gst_duty_head: GstDutyHeadObservation,
}

/// The GST duty-head classification observed on one ledger master.
///
/// Tally's vocabulary is deliberately not normalised: for example, the state
/// head is `"State Tax"`, not `"SGST"`. `raw` therefore remains exactly as
/// returned, and any value outside the measured set is surfaced explicitly.
///
/// The measured spellings, the `TAXTYPE` interaction and its four states, the two
/// wire shapes of absence, and the instance scope are recorded in
/// `docs/tally/TALLY_PROTOCOL_REFERENCE.md` §8.3. That section is canonical; this
/// type implements it and should not become a second account of the protocol.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize, Serialize)]
#[serde(tag = "observation", rename_all = "snake_case")]
pub enum GstDutyHeadObservation {
    Recognized {
        raw: String,
        head: GstDutyHead,
    },
    Unrecognized {
        raw: String,
    },
    /// `TAXTYPE` was observed and is not the literal GST tax type; this is
    /// distinct from a GST ledger whose duty-head field was absent.
    NotTaxLedger {
        tax_type: String,
    },
    /// A duty head arrived on a ledger whose observed `TAXTYPE` is NOT GST --
    /// for example `<TAXTYPE>Others</TAXTYPE><GSTDUTYHEAD>CGST</GSTDUTYHEAD>`.
    /// The two fields contradict each other, so neither is asserted: the head is
    /// not recognised and the ledger is not reported as a tax ledger. Both raw
    /// values are retained so a reviewer can see what was actually returned.
    Contradictory {
        tax_type: String,
        raw: String,
    },
    #[default]
    Absent,
}

impl GstDutyHeadObservation {
    pub fn from_observations(
        tax_type: &PartyLedgerMasterFieldObservation,
        duty_head: &PartyLedgerMasterFieldObservation,
    ) -> Self {
        // Tally renders an absent duty head both by omitting the element and as
        // an explicit empty one; both mean the same thing here.
        let head = match duty_head {
            PartyLedgerMasterFieldObservation::Returned(raw) if !raw.is_empty() => Some(raw),
            _ => None,
        };
        // Observed AND not GST. An unobserved or empty TAXTYPE is not evidence
        // that the ledger is non-GST, so it does not contradict a duty head.
        let non_gst = match tax_type {
            PartyLedgerMasterFieldObservation::Returned(value)
                if !value.is_empty() && value != "GST" =>
            {
                Some(value)
            }
            _ => None,
        };

        match (head, non_gst) {
            // A head on a ledger that is explicitly not a GST ledger is a
            // contradictory response. Recognising it releases the contradiction
            // as valid compliance data, which is the one outcome this type was
            // introduced to prevent.
            (Some(raw), Some(tax_type)) => Self::Contradictory {
                tax_type: tax_type.clone(),
                raw: raw.clone(),
            },
            (Some(raw), None) => match raw.as_str() {
                "CGST" => Self::Recognized {
                    raw: raw.clone(),
                    head: GstDutyHead::Cgst,
                },
                "IGST" => Self::Recognized {
                    raw: raw.clone(),
                    head: GstDutyHead::Igst,
                },
                "State Tax" => Self::Recognized {
                    raw: raw.clone(),
                    head: GstDutyHead::StateTax,
                },
                "UT Tax" => Self::Recognized {
                    raw: raw.clone(),
                    head: GstDutyHead::UtTax,
                },
                "Cess" => Self::Recognized {
                    raw: raw.clone(),
                    head: GstDutyHead::Cess,
                },
                _ => Self::Unrecognized { raw: raw.clone() },
            },
            (None, Some(tax_type)) => Self::NotTaxLedger {
                tax_type: tax_type.clone(),
            },
            (None, None) => Self::Absent,
        }
    }
}

/// The exact GST duty-head vocabulary measured for ledger masters.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum GstDutyHead {
    Cgst,
    Igst,
    StateTax,
    UtTax,
    Cess,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
pub struct PartyLedgerMasterRecord {
    pub ledger: TallyLedger,
    pub fields: PartyLedgerMasterFields,
}

/// Parses the native `List of Ledgers` collection used by ordinary ledger
/// reads. The request is pinned by `render_native_ledger_export_request` to
/// `SVFROMDATE = BOOKSFROM`: an undated request returns `OPENINGBALANCE` as of
/// Tally's loaded display period, so the date is load-bearing and must not be
/// removed (TALLY_PROTOCOL_REFERENCE §5.5). This parser reads the rows only;
/// the as-of date is the request's.
///
/// The collection has no report-envelope company identity. Its row GUIDs bind
/// the response instead: at least one row must carry the requested company
/// GUID prefix. Other prefixes remain counted evidence rather than failures,
/// because the observed books created masters in place but imported masters
/// may legitimately retain foreign GUID prefixes.
pub fn parse_native_ledger_source_records_with_evidence(
    xml: &str,
    expected_company_guid: &str,
) -> anyhow::Result<ParsedExport<ParsedSourceRecord<TallyLedger>>> {
    parse_native_ledger_collection_with_evidence(
        xml,
        expected_company_guid,
        NativeLedgerCollectionCompanyBinding::RowGuidPrefix,
        parse_native_ledger_collection_row,
    )
}

/// Parses the dedicated party/ledger master collection. This is deliberately
/// distinct from ordinary ledger parsing because it retains the sensitive
/// fields that only the workbook renderer consumes.
pub fn parse_native_party_ledger_master_records_with_evidence(
    xml: &str,
    expected_company_guid: &str,
) -> anyhow::Result<ParsedExport<ParsedSourceRecord<PartyLedgerMasterRecord>>> {
    parse_native_ledger_collection_with_evidence(
        xml,
        expected_company_guid,
        NativeLedgerCollectionCompanyBinding::ResponseGuid,
        parse_native_party_ledger_master_collection_row,
    )
}

fn parse_native_ledger_collection_with_evidence<T>(
    xml: &str,
    expected_company_guid: &str,
    company_binding: NativeLedgerCollectionCompanyBinding,
    parse_row: impl Fn(
        &mut Reader<&[u8]>,
        &quick_xml::events::BytesStart<'_>,
    ) -> anyhow::Result<NativeLedgerCollectionRow<T>>,
) -> anyhow::Result<ParsedExport<ParsedSourceRecord<T>>> {
    let sanitized = tolerant_xml::sanitize_invalid_numeric_references_with_provenance(xml);
    let mut reader = configured_reader(sanitized.as_str());
    let mut path = Vec::<Vec<u8>>::new();
    let mut status_seen = false;
    let mut collection_seen = false;
    let mut records = Vec::new();
    let mut identities = HashMap::<String, u64>::new();
    let mut company_guid_prefix_match_count = 0_u64;
    let mut company_guid_prefix_mismatch_count = 0_u64;

    loop {
        let record_start = reader.buffer_position() as usize;
        match reader.read_event()? {
            Event::Start(element) => {
                let name = element.name().as_ref().to_ascii_uppercase();
                if path.is_empty() && name != b"ENVELOPE" {
                    anyhow::bail!("native ledger collection root was not ENVELOPE");
                }
                if path_eq(&path, &[b"ENVELOPE", b"HEADER"]) && name == b"STATUS" {
                    if status_seen || read_required_text(&mut reader, element.name())? != "1" {
                        anyhow::bail!("native ledger collection did not report success");
                    }
                    status_seen = true;
                    continue;
                }
                if path_eq(&path, &[b"ENVELOPE", b"BODY", b"DATA"]) && name == b"COLLECTION" {
                    collection_seen = true;
                }
                if path_eq(&path, &[b"ENVELOPE", b"BODY", b"DATA", b"COLLECTION"])
                    && name == b"LEDGER"
                {
                    let NativeLedgerCollectionRow {
                        record,
                        identities: identities_for_row,
                        alter_id,
                        response_company_guid,
                    } = parse_row(&mut reader, &element)?;
                    if matches!(
                        company_binding,
                        NativeLedgerCollectionCompanyBinding::ResponseGuid
                    ) {
                        let response_company_guid = response_company_guid.ok_or_else(|| {
                            anyhow::anyhow!(
                                "native ledger collection omitted response company GUID"
                            )
                        })?;
                        if !response_company_guid.eq_ignore_ascii_case(expected_company_guid) {
                            anyhow::bail!(
                                "native ledger collection did not confirm the selected company"
                            );
                        }
                    }
                    let guid = identities_for_row
                        .guid
                        .as_deref()
                        .ok_or_else(|| anyhow::anyhow!("native ledger row omitted GUID"))?;
                    if native_ledger_guid_has_company_prefix(guid, expected_company_guid) {
                        company_guid_prefix_match_count = company_guid_prefix_match_count
                            .checked_add(1)
                            .ok_or_else(|| {
                                anyhow::anyhow!("native ledger prefix count overflow")
                            })?;
                    } else {
                        company_guid_prefix_mismatch_count = company_guid_prefix_mismatch_count
                            .checked_add(1)
                            .ok_or_else(|| {
                                anyhow::anyhow!("native ledger prefix count overflow")
                            })?;
                    }
                    record_identities_from_values("LEDGER", &identities_for_row, &mut identities)?;
                    let record_end = reader.buffer_position() as usize;
                    records.push(ParsedSourceRecord {
                        record,
                        source_id: Some(guid.to_owned()),
                        identity_kind: Some(ParsedSourceIdentityKind::Guid),
                        identities: identities_for_row,
                        alter_id,
                        raw_source_sha256: source_fragment_sha256_from_sanitized(
                            &sanitized,
                            record_start,
                            record_end,
                        )?,
                    });
                    continue;
                }
                path.push(name);
            }
            Event::Empty(element) => {
                let name = element.name().as_ref().to_ascii_uppercase();
                if path_eq(&path, &[b"ENVELOPE", b"BODY", b"DATA"]) && name == b"COLLECTION" {
                    collection_seen = true;
                } else if path_eq(&path, &[b"ENVELOPE", b"BODY", b"DATA", b"COLLECTION"])
                    && name == b"LEDGER"
                {
                    anyhow::bail!("native ledger collection contained an empty ledger row");
                }
            }
            Event::End(element) => pop_expected_path(&mut path, element.name().as_ref())?,
            Event::Eof => break,
            _ => {}
        }
    }
    if !path.is_empty() {
        anyhow::bail!("native ledger collection ended before its root closed");
    }
    if !status_seen {
        anyhow::bail!("native ledger collection did not report success");
    }
    if !collection_seen {
        anyhow::bail!("native ledger collection omitted BODY/DATA/COLLECTION");
    }
    if matches!(
        company_binding,
        NativeLedgerCollectionCompanyBinding::RowGuidPrefix
    ) && company_guid_prefix_match_count == 0
    {
        anyhow::bail!("native ledger collection did not bind to the requested company");
    }
    let mut duplicate_identities = identities
        .into_iter()
        .filter(|(_, occurrences)| *occurrences > 1)
        .map(|(identity, occurrences)| DuplicateIdentityEvidence {
            identity_sha256: sha256_hex(identity.as_bytes()),
            occurrences,
        })
        .collect::<Vec<_>>();
    duplicate_identities.sort_by(|left, right| left.identity_sha256.cmp(&right.identity_sha256));
    let source_record_count = u64::try_from(records.len())
        .map_err(|_| anyhow::anyhow!("native ledger collection exceeded supported record count"))?;
    Ok(ParsedExport {
        records,
        evidence: ExportEvidence {
            observed_record_count: Some(source_record_count),
            identified_record_count: source_record_count,
            duplicate_identities,
            company_guid_prefix_match_count,
            company_guid_prefix_mismatch_count,
            ..ExportEvidence::default()
        },
    })
}

#[derive(Clone, Copy)]
enum NativeLedgerCollectionCompanyBinding {
    /// Ordinary collection readers predate the response-bound compute and
    /// retain their established row-identity contract.
    RowGuidPrefix,
    /// The dedicated party/ledger export must prove which selected company
    /// answered, independently of imported ledger-object GUIDs.
    ResponseGuid,
}

struct NativeLedgerCollectionRow<T> {
    record: T,
    identities: ParsedSourceIdentities,
    alter_id: Option<String>,
    response_company_guid: Option<String>,
}

fn parse_native_ledger_collection_row(
    reader: &mut Reader<&[u8]>,
    element: &quick_xml::events::BytesStart<'_>,
) -> anyhow::Result<NativeLedgerCollectionRow<TallyLedger>> {
    let ParsedNativeLedgerCollectionRow {
        ledger,
        identities,
        alter_id,
        response_company_guid,
        ..
    } = parse_native_ledger_collection_row_with_master_fields(reader, element, false)?;
    Ok(NativeLedgerCollectionRow {
        record: ledger,
        identities,
        alter_id,
        response_company_guid,
    })
}

fn parse_native_party_ledger_master_collection_row(
    reader: &mut Reader<&[u8]>,
    element: &quick_xml::events::BytesStart<'_>,
) -> anyhow::Result<NativeLedgerCollectionRow<PartyLedgerMasterRecord>> {
    let ParsedNativeLedgerCollectionRow {
        ledger,
        fields,
        identities,
        alter_id,
        response_company_guid,
    } = parse_native_ledger_collection_row_with_master_fields(reader, element, true)?;
    Ok(NativeLedgerCollectionRow {
        record: PartyLedgerMasterRecord { ledger, fields },
        identities,
        alter_id,
        response_company_guid,
    })
}

fn parse_native_ledger_collection_row_with_master_fields(
    reader: &mut Reader<&[u8]>,
    element: &quick_xml::events::BytesStart<'_>,
    retain_master_fields: bool,
) -> anyhow::Result<ParsedNativeLedgerCollectionRow> {
    validate_only_attributes(element, &[b"NAME", b"RESERVEDNAME"])?;
    let name = attr_value(reader, element, b"NAME")
        .ok_or_else(|| anyhow::anyhow!("native ledger row omitted NAME"))?;
    let mut ledger = TallyLedger {
        name,
        parent: PartyLedgerMasterFieldObservation::NotObserved,
        party_gstin: PartyLedgerMasterFieldObservation::NotObserved,
        opening_balance: None,
    };
    let mut identities = ParsedSourceIdentities::default();
    let mut alter_id = None;
    let mut parent_seen = false;
    let mut gstin_seen = false;
    let mut guid_seen = false;
    let mut remote_id_seen = false;
    let mut master_id_seen = false;
    let mut alter_id_seen = false;
    let mut opening_balance_seen = false;
    let mut response_company_guid = None;
    let mut response_company_guid_seen = false;
    let mut master_fields = PartyLedgerMasterFields::default();
    let mut master_fields_seen = HashSet::new();
    let mut gst_duty_head = PartyLedgerMasterFieldObservation::NotObserved;
    loop {
        match reader.read_event()? {
            Event::Start(child) => match child.name().as_ref().to_ascii_uppercase().as_slice() {
                b"GUID" => {
                    validate_only_attributes(&child, &[b"TYPE"])?;
                    if std::mem::replace(&mut guid_seen, true) {
                        anyhow::bail!("native ledger row repeated GUID");
                    }
                    identities.guid = validated_optional_identifier(Some(read_required_text(
                        reader,
                        child.name(),
                    )?))?
                    .map(|guid| guid.to_ascii_lowercase());
                }
                b"REMOTEID" => {
                    validate_only_attributes(&child, &[b"TYPE"])?;
                    if std::mem::replace(&mut remote_id_seen, true) {
                        anyhow::bail!("native ledger row repeated REMOTEID");
                    }
                    identities.remote_id =
                        validated_optional_identifier(read_optional_text(reader, child.name())?)?;
                }
                b"MASTERID" => {
                    validate_only_attributes(&child, &[b"TYPE"])?;
                    if std::mem::replace(&mut master_id_seen, true) {
                        anyhow::bail!("native ledger row repeated MASTERID");
                    }
                    identities.master_id = validated_optional_identifier(Some(
                        read_required_text(reader, child.name())?,
                    ))?;
                }
                b"ALTERID" => {
                    validate_only_attributes(&child, &[b"TYPE"])?;
                    if std::mem::replace(&mut alter_id_seen, true) {
                        anyhow::bail!("native ledger row repeated ALTERID");
                    }
                    alter_id = validated_optional_identifier(Some(read_required_text(
                        reader,
                        child.name(),
                    )?))?;
                }
                b"PARENT" => {
                    validate_only_attributes(&child, &[b"TYPE"])?;
                    if std::mem::replace(&mut parent_seen, true) {
                        anyhow::bail!("native ledger row repeated PARENT");
                    }
                    ledger.parent = PartyLedgerMasterFieldObservation::Returned(
                        read_identifier_text(reader, child.name())?.unwrap_or_default(),
                    );
                }
                b"PARTYGSTIN" => {
                    validate_only_attributes(&child, &[b"TYPE"])?;
                    if std::mem::replace(&mut gstin_seen, true) {
                        anyhow::bail!("native ledger row repeated PARTYGSTIN");
                    }
                    ledger.party_gstin = PartyLedgerMasterFieldObservation::Returned(
                        read_optional_text(reader, child.name())?.unwrap_or_default(),
                    );
                }
                b"OPENINGBALANCE" => {
                    validate_only_attributes(&child, &[b"TYPE"])?;
                    if std::mem::replace(&mut opening_balance_seen, true) {
                        anyhow::bail!("native ledger row repeated OPENINGBALANCE");
                    }
                    let opening_balance = read_required_text(reader, child.name())?;
                    // Every captured row carries this field. Its absence is
                    // unmeasured, so fail closed rather than silently turning
                    // a missing debtor/creditor balance into zero.
                    bridge_tally_primitives::ExactDecimal::parse(opening_balance.clone())?;
                    ledger.opening_balance = Some(opening_balance);
                }
                b"BRIDGECOMPANYGUID" => {
                    validate_only_attributes(&child, &[b"TYPE"])?;
                    if std::mem::replace(&mut response_company_guid_seen, true) {
                        anyhow::bail!("native ledger row repeated response company GUID");
                    }
                    response_company_guid = Some(normalized_standard_company_guid(
                        &read_required_text(reader, child.name())?,
                    )?);
                }
                b"INCOMETAXNUMBER" => retain_party_ledger_master_field(
                    reader,
                    &child,
                    retain_master_fields,
                    &mut master_fields_seen,
                    &mut master_fields.income_tax_number,
                )?,
                b"NAMEONPAN" => retain_party_ledger_master_field(
                    reader,
                    &child,
                    retain_master_fields,
                    &mut master_fields_seen,
                    &mut master_fields.name_on_pan,
                )?,
                b"LEDPINCODE" => retain_party_ledger_master_field(
                    reader,
                    &child,
                    retain_master_fields,
                    &mut master_fields_seen,
                    &mut master_fields.pin_code,
                )?,
                b"LEDGSTPINCODE" => retain_party_ledger_master_field(
                    reader,
                    &child,
                    retain_master_fields,
                    &mut master_fields_seen,
                    &mut master_fields.gst_pin_code,
                )?,
                b"MSMEREGNUMBER" => retain_party_ledger_master_field(
                    reader,
                    &child,
                    retain_master_fields,
                    &mut master_fields_seen,
                    &mut master_fields.msme_registration_number,
                )?,
                b"LEDUDYAMREGNUMBER" => retain_party_ledger_master_field(
                    reader,
                    &child,
                    retain_master_fields,
                    &mut master_fields_seen,
                    &mut master_fields.udyam_registration_number,
                )?,
                b"BANKACCHOLDERNAME" => retain_party_ledger_master_field(
                    reader,
                    &child,
                    retain_master_fields,
                    &mut master_fields_seen,
                    &mut master_fields.bank_account_holder_name,
                )?,
                b"BANKDETAILS" => retain_party_ledger_master_field(
                    reader,
                    &child,
                    retain_master_fields,
                    &mut master_fields_seen,
                    &mut master_fields.bank_details,
                )?,
                b"IFSCODE" => retain_party_ledger_master_field(
                    reader,
                    &child,
                    retain_master_fields,
                    &mut master_fields_seen,
                    &mut master_fields.ifsc_code,
                )?,
                b"EMAIL" => retain_party_ledger_master_field(
                    reader,
                    &child,
                    retain_master_fields,
                    &mut master_fields_seen,
                    &mut master_fields.email,
                )?,
                b"LEDGERPHONE" => retain_party_ledger_master_field(
                    reader,
                    &child,
                    retain_master_fields,
                    &mut master_fields_seen,
                    &mut master_fields.phone,
                )?,
                b"STATENAME" => retain_party_ledger_master_field(
                    reader,
                    &child,
                    retain_master_fields,
                    &mut master_fields_seen,
                    &mut master_fields.state,
                )?,
                b"LEDADDRESS.LIST" => retain_party_ledger_master_field(
                    reader,
                    &child,
                    retain_master_fields,
                    &mut master_fields_seen,
                    &mut master_fields.address,
                )?,
                b"TAXTYPE" => retain_party_ledger_master_scalar(
                    reader,
                    &child,
                    retain_master_fields,
                    &mut master_fields_seen,
                    &mut master_fields.tax_type,
                )?,
                b"GSTDUTYHEAD" => retain_party_ledger_master_scalar(
                    reader,
                    &child,
                    retain_master_fields,
                    &mut master_fields_seen,
                    &mut gst_duty_head,
                )?,
                _ => {
                    let child_name = child.name().as_ref().to_vec();
                    reader.read_to_end(QName(&child_name).to_owned())?;
                }
            },
            Event::Empty(child) => match child.name().as_ref().to_ascii_uppercase().as_slice() {
                b"PARENT" => {
                    validate_only_attributes(&child, &[b"TYPE"])?;
                    if std::mem::replace(&mut parent_seen, true) {
                        anyhow::bail!("native ledger row repeated PARENT");
                    }
                    ledger.parent = PartyLedgerMasterFieldObservation::Returned(String::new());
                }
                b"PARTYGSTIN" => {
                    validate_only_attributes(&child, &[b"TYPE"])?;
                    if std::mem::replace(&mut gstin_seen, true) {
                        anyhow::bail!("native ledger row repeated PARTYGSTIN");
                    }
                    ledger.party_gstin = PartyLedgerMasterFieldObservation::Returned(String::new());
                }
                b"GUID" | b"MASTERID" | b"ALTERID" | b"OPENINGBALANCE" | b"BRIDGECOMPANYGUID" => {
                    anyhow::bail!("native ledger row omitted a required field");
                }
                b"REMOTEID" => {
                    validate_only_attributes(&child, &[b"TYPE"])?;
                    if std::mem::replace(&mut remote_id_seen, true) {
                        anyhow::bail!("native ledger row repeated REMOTEID");
                    }
                }
                b"INCOMETAXNUMBER" => retain_empty_party_ledger_master_field(
                    &child,
                    retain_master_fields,
                    &mut master_fields_seen,
                    &mut master_fields.income_tax_number,
                )?,
                b"NAMEONPAN" => retain_empty_party_ledger_master_field(
                    &child,
                    retain_master_fields,
                    &mut master_fields_seen,
                    &mut master_fields.name_on_pan,
                )?,
                b"LEDPINCODE" => retain_empty_party_ledger_master_field(
                    &child,
                    retain_master_fields,
                    &mut master_fields_seen,
                    &mut master_fields.pin_code,
                )?,
                b"LEDGSTPINCODE" => retain_empty_party_ledger_master_field(
                    &child,
                    retain_master_fields,
                    &mut master_fields_seen,
                    &mut master_fields.gst_pin_code,
                )?,
                b"MSMEREGNUMBER" => retain_empty_party_ledger_master_field(
                    &child,
                    retain_master_fields,
                    &mut master_fields_seen,
                    &mut master_fields.msme_registration_number,
                )?,
                b"LEDUDYAMREGNUMBER" => retain_empty_party_ledger_master_field(
                    &child,
                    retain_master_fields,
                    &mut master_fields_seen,
                    &mut master_fields.udyam_registration_number,
                )?,
                b"BANKACCHOLDERNAME" => retain_empty_party_ledger_master_field(
                    &child,
                    retain_master_fields,
                    &mut master_fields_seen,
                    &mut master_fields.bank_account_holder_name,
                )?,
                b"BANKDETAILS" => retain_empty_party_ledger_master_field(
                    &child,
                    retain_master_fields,
                    &mut master_fields_seen,
                    &mut master_fields.bank_details,
                )?,
                b"IFSCODE" => retain_empty_party_ledger_master_field(
                    &child,
                    retain_master_fields,
                    &mut master_fields_seen,
                    &mut master_fields.ifsc_code,
                )?,
                b"EMAIL" => retain_empty_party_ledger_master_field(
                    &child,
                    retain_master_fields,
                    &mut master_fields_seen,
                    &mut master_fields.email,
                )?,
                b"LEDGERPHONE" => retain_empty_party_ledger_master_field(
                    &child,
                    retain_master_fields,
                    &mut master_fields_seen,
                    &mut master_fields.phone,
                )?,
                b"STATENAME" => retain_empty_party_ledger_master_field(
                    &child,
                    retain_master_fields,
                    &mut master_fields_seen,
                    &mut master_fields.state,
                )?,
                b"LEDADDRESS.LIST" => retain_empty_party_ledger_master_field(
                    &child,
                    retain_master_fields,
                    &mut master_fields_seen,
                    &mut master_fields.address,
                )?,
                b"TAXTYPE" => retain_empty_party_ledger_master_field(
                    &child,
                    retain_master_fields,
                    &mut master_fields_seen,
                    &mut master_fields.tax_type,
                )?,
                b"GSTDUTYHEAD" => retain_empty_party_ledger_master_field(
                    &child,
                    retain_master_fields,
                    &mut master_fields_seen,
                    &mut gst_duty_head,
                )?,
                _ => {}
            },
            Event::End(end) if end.name().as_ref().eq_ignore_ascii_case(b"LEDGER") => break,
            Event::Text(text) if !text.decode()?.trim().is_empty() => {
                anyhow::bail!("native ledger row contained unexpected text");
            }
            Event::Eof => anyhow::bail!("native ledger row ended before LEDGER closed"),
            _ => {}
        }
    }
    if !guid_seen || identities.guid.is_none() {
        anyhow::bail!("native ledger row omitted GUID");
    }
    if !master_id_seen || identities.master_id.is_none() {
        anyhow::bail!("native ledger row omitted MASTERID");
    }
    if !alter_id_seen || alter_id.is_none() {
        anyhow::bail!("native ledger row omitted ALTERID");
    }
    if !opening_balance_seen || ledger.opening_balance.is_none() {
        anyhow::bail!("native ledger row omitted OPENINGBALANCE");
    }
    // Unlike GUID/MASTERID/ALTERID/OPENINGBALANCE, an absent PARENT is not simply invalid --
    // downstream (`build_core_window`) reads `ledger.parent == None` as "this ledger is at the
    // tree root", which is also the correct reading of an explicitly EMPTY PARENT (Tally does
    // send those for genuinely root-parented ledgers). So the requirement here is narrower than
    // "must be non-empty": only that the field was OBSERVED at all. `parent_seen` is set by both
    // the `Event::Start` and `Event::Empty` arms above, but never by omission, so by the time we
    // reach here `ledger.parent == None` can only mean "explicitly empty" -- never "never sent" --
    // which is exactly the distinction the reserved-root handling depends on.
    if !parent_seen {
        anyhow::bail!("native ledger row omitted PARENT");
    }
    master_fields.gst_duty_head =
        GstDutyHeadObservation::from_observations(&master_fields.tax_type, &gst_duty_head);
    Ok(ParsedNativeLedgerCollectionRow {
        ledger,
        fields: master_fields,
        identities,
        alter_id,
        response_company_guid,
    })
}

struct ParsedNativeLedgerCollectionRow {
    ledger: TallyLedger,
    fields: PartyLedgerMasterFields,
    identities: ParsedSourceIdentities,
    alter_id: Option<String>,
    response_company_guid: Option<String>,
}

/// Read a scalar that must be a scalar: any child element is a malformed response.
///
/// `read_flattened_optional_text` counts nesting depth and keeps collecting text, so
/// `<GSTDUTYHEAD><VALUE>CGST</VALUE></GSTDUTYHEAD>` flattens to `CGST` and is released
/// as a recognised duty head. For a field that only ever carries a scalar, and whose
/// value drives a classification, an unexpected shape has to fail at the boundary
/// rather than become compliance data.
///
/// Used for `TAXTYPE` and `GSTDUTYHEAD`, the two inputs to
/// [`GstDutyHeadObservation::from_observations`]. The other retained master fields keep
/// the flattening reader: they are recorded, not classified, and narrowing them is a
/// separate decision from this one.
fn read_scalar_rejecting_nested_markup(
    reader: &mut Reader<&[u8]>,
    name: QName<'_>,
) -> anyhow::Result<Option<String>> {
    let expected = name.as_ref().to_ascii_uppercase();
    with_untrimmed_text(reader, |reader| {
        let mut current = String::new();
        loop {
            match reader.read_event()? {
                Event::Start(child) | Event::Empty(child) => {
                    let child = String::from_utf8_lossy(child.name().as_ref()).to_ascii_uppercase();
                    anyhow::bail!("party/ledger master scalar contained nested markup <{child}>");
                }
                Event::Text(text) => {
                    let decoded = text.decode()?;
                    let value = quick_xml::escape::unescape(&decoded)?;
                    current.push_str(&value);
                }
                Event::GeneralRef(reference) => {
                    current.push_str(&resolve_party_ledger_master_reference(reference)?);
                }
                Event::CData(text) => {
                    current.push_str(&text.decode()?);
                }
                Event::End(end) => {
                    if end.name().as_ref().to_ascii_uppercase() != expected {
                        anyhow::bail!("party/ledger master field closed unexpectedly");
                    }
                    break;
                }
                Event::Eof => anyhow::bail!("party/ledger master field ended before it closed"),
                _ => {}
            }
        }
        let trimmed = current.trim();
        Ok((!trimmed.is_empty()).then(|| trimmed.to_owned()))
    })
}

/// Retain a classification-driving field, refusing nested markup. See
/// [`read_scalar_rejecting_nested_markup`].
fn retain_party_ledger_master_scalar(
    reader: &mut Reader<&[u8]>,
    element: &quick_xml::events::BytesStart<'_>,
    retain: bool,
    seen: &mut HashSet<Vec<u8>>,
    target: &mut PartyLedgerMasterFieldObservation,
) -> anyhow::Result<()> {
    validate_only_attributes(element, &[b"TYPE"])?;
    let name = element.name();
    let key = name.as_ref().to_ascii_uppercase();
    if !seen.insert(key) {
        anyhow::bail!("native ledger row repeated a party/ledger master field");
    }
    let value = read_scalar_rejecting_nested_markup(reader, name)?;
    if retain {
        *target = PartyLedgerMasterFieldObservation::Returned(value.unwrap_or_default());
    }
    Ok(())
}

fn retain_party_ledger_master_field(
    reader: &mut Reader<&[u8]>,
    element: &quick_xml::events::BytesStart<'_>,
    retain: bool,
    seen: &mut HashSet<Vec<u8>>,
    target: &mut PartyLedgerMasterFieldObservation,
) -> anyhow::Result<()> {
    validate_only_attributes(element, &[b"TYPE"])?;
    let name = element.name();
    let key = name.as_ref().to_ascii_uppercase();
    if !seen.insert(key) {
        anyhow::bail!("native ledger row repeated a party/ledger master field");
    }
    let value = read_flattened_optional_text(reader, name)?;
    if retain {
        *target = PartyLedgerMasterFieldObservation::Returned(value.unwrap_or_default());
    }
    Ok(())
}

fn retain_empty_party_ledger_master_field(
    element: &quick_xml::events::BytesStart<'_>,
    retain: bool,
    seen: &mut HashSet<Vec<u8>>,
    target: &mut PartyLedgerMasterFieldObservation,
) -> anyhow::Result<()> {
    validate_only_attributes(element, &[b"TYPE"])?;
    let key = element.name().as_ref().to_ascii_uppercase();
    if !seen.insert(key) {
        anyhow::bail!("native ledger row repeated a party/ledger master field");
    }
    if retain {
        *target = PartyLedgerMasterFieldObservation::Returned(String::new());
    }
    Ok(())
}

fn read_flattened_optional_text(
    reader: &mut Reader<&[u8]>,
    name: QName<'_>,
) -> anyhow::Result<Option<String>> {
    let expected = name.as_ref().to_ascii_uppercase();
    with_untrimmed_text(reader, |reader| {
        let mut nested_depth = 0_usize;
        let mut parts = Vec::new();
        let mut current = String::new();
        loop {
            match reader.read_event()? {
                Event::Start(_) => {
                    flush_flattened_part(&mut current, &mut parts);
                    nested_depth = nested_depth.saturating_add(1);
                }
                Event::Empty(_) => flush_flattened_part(&mut current, &mut parts),
                Event::Text(text) => {
                    let decoded = text.decode()?;
                    let value = quick_xml::escape::unescape(&decoded)?;
                    current.push_str(&value);
                }
                Event::GeneralRef(reference) => {
                    current.push_str(&resolve_party_ledger_master_reference(reference)?);
                }
                Event::CData(text) => {
                    current.push_str(&text.decode()?);
                }
                Event::End(end) => {
                    if nested_depth == 0 {
                        if end.name().as_ref().to_ascii_uppercase() != expected {
                            anyhow::bail!("party/ledger master field closed unexpectedly");
                        }
                        flush_flattened_part(&mut current, &mut parts);
                        break;
                    }
                    flush_flattened_part(&mut current, &mut parts);
                    nested_depth = nested_depth.saturating_sub(1);
                }
                Event::Eof => anyhow::bail!("party/ledger master field ended before it closed"),
                _ => {}
            }
        }
        Ok((!parts.is_empty()).then(|| parts.join("\n")))
    })
}

/// Trims `current` and, if non-empty, moves it onto `parts` as one flattened
/// line; always leaves `current` empty afterward.
///
/// Called at every element boundary (`Start`, `Empty`, and `End` of a nested
/// child) in [`read_flattened_optional_text`], so a value is split into
/// separate lines only where genuine nested markup separates it -- the
/// repeated `<LEDADDRESS>` children of `LEDADDRESS.LIST` being the real case.
/// It is never called merely because quick_xml delivered one line's text as
/// more than one event: a `GeneralRef` or a `CData` section splits a `Text`
/// run into several events without introducing a new line, so those pieces
/// accumulate in `current` and are trimmed and pushed together, once, here.
fn flush_flattened_part(current: &mut String, parts: &mut Vec<String>) {
    let trimmed = current.trim();
    if !trimmed.is_empty() {
        parts.push(trimmed.to_owned());
    }
    current.clear();
}

/// Resolves one `GeneralRef` event (`&amp;`, `&#8377;`, `&#x20B9;`, ...) into
/// its character, the same way [`read_optional_text`] and
/// [`read_identifier_text`] resolve a reference embedded in an ordinary
/// `Text` event: by reforming `&<ref>;` and unescaping it with
/// `quick_xml::escape::unescape`, which resolves both the five predefined
/// XML entities and legal numeric character references.
///
/// quick_xml 0.41 delivers an entity or numeric character reference as its
/// own event, separate from any surrounding `Text`/`CData` for the same
/// logical run of text. A catch-all match arm with no `GeneralRef` case
/// silently drops it -- the bug this function's callers close. By the time a
/// `GeneralRef` reaches either caller,
/// `tolerant_xml::sanitize_invalid_numeric_references_with_provenance` (see
/// `parse_native_ledger_collection_with_evidence`) has already rewritten any
/// reference to a code point XML 1.0 forbids into literal marker text
/// (`TALLY_PROTOCOL_REFERENCE.md` section 1.1(d)), so a `GeneralRef` seen
/// here is always either a predefined named entity or a legal numeric
/// reference; `unescape` fails closed on anything else.
fn resolve_party_ledger_master_reference(
    reference: quick_xml::events::BytesRef<'_>,
) -> anyhow::Result<String> {
    let decoded = reference.decode()?;
    Ok(quick_xml::escape::unescape(&format!("&{decoded};"))?.into_owned())
}

/// Runs `body` with the reader's automatic text trimming disabled, restoring
/// whatever was configured before returning -- on every exit path, including
/// an error propagated by `body`.
///
/// The whole document is parsed with `trim_text(true)` (`configured_reader`),
/// which quick_xml applies independently to *every* `Text` event it emits --
/// including the fragments immediately before and after a `GeneralRef` or a
/// `CData` section. Concatenating those fragments without disabling this
/// first would silently lose whitespace that sat next to the split, e.g. the
/// spaces in `RAM &amp; SONS` (`RAM` and `SONS` each arrive already trimmed).
/// Reading untrimmed and trimming only the fully reassembled value (done by
/// [`read_flattened_optional_text`] and [`read_scalar_rejecting_nested_markup`])
/// is the same approach the agent-facing read-back parsers already use for
/// the same reason (see `trim_text(false)` in `agent_lab.rs`,
/// `agent_voucher_parse.rs`, `agent_company_checkpoint.rs`, and
/// `source_draft_xml.rs`).
fn with_untrimmed_text<T>(
    reader: &mut Reader<&[u8]>,
    body: impl FnOnce(&mut Reader<&[u8]>) -> anyhow::Result<T>,
) -> anyhow::Result<T> {
    let config = reader.config_mut();
    let trim_text_start = std::mem::replace(&mut config.trim_text_start, false);
    let trim_text_end = std::mem::replace(&mut config.trim_text_end, false);
    let result = body(reader);
    let config = reader.config_mut();
    config.trim_text_start = trim_text_start;
    config.trim_text_end = trim_text_end;
    result
}

#[cfg(test)]
#[path = "native_ledger_collection_tests.rs"]
mod tests;
