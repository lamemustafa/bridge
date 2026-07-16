//! Dormant parser for one Bridge-owned Party Outstanding observation envelope.
//!
//! Output is deliberately unbound. There is no request artifact, TDL report,
//! production dispatch, canonical adapter, completeness authority, or support
//! promotion. The official Tally import examples inform vocabulary only; they
//! do not establish this read profile on any release.

use std::collections::HashSet;
use std::fmt;

use quick_xml::{events::Event, Reader};
use serde::Deserialize;
use sha2::{Digest, Sha256};

use crate::{decode_tally_text_bytes_limited, TallyTextDecodeError, TallyTextEncoding};

pub const BILLS_OBSERVED_RAW_SCHEMA_V1: &str = "bridge.tally.bills-observed-raw/1";
pub const BILLS_OBSERVED_RAW_PROFILE_V1: &str = "bridge.bills-observed-raw-xml/1";
const RESPONSE_HASH_DOMAIN: &[u8] = b"bridge.tally.bills-observed-raw-response/1\0";
const FRAGMENT_HASH_DOMAIN: &[u8] = b"bridge.tally.bills-observed-raw-fragment/1\0";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BillsObservationLimits {
    pub max_encoded_bytes: usize,
    pub max_decoded_bytes: usize,
    pub max_records: usize,
    pub max_field_bytes: usize,
    pub max_nodes: usize,
    pub max_depth: usize,
    pub max_attributes: usize,
}

impl Default for BillsObservationLimits {
    fn default() -> Self {
        Self {
            max_encoded_bytes: 8 * 1024 * 1024,
            max_decoded_bytes: 8 * 1024 * 1024,
            max_records: 25_000,
            max_field_bytes: 512,
            max_nodes: 250_000,
            max_depth: 4,
            max_attributes: 32,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BillsObservationError {
    InvalidLimits,
    ResponseTooLarge,
    DecodedResponseTooLarge,
    InvalidEncoding,
    MalformedXml,
    ResourceLimitExceeded,
    WrongGrammar,
    DuplicateField,
    ApplicationRejected,
    ProfileMismatch,
    InvalidValue,
    CountMismatch,
    DuplicateObservation,
}

impl BillsObservationError {
    pub const fn safe_code(self) -> &'static str {
        match self {
            Self::InvalidLimits => "bills_observation_limits_invalid",
            Self::ResponseTooLarge => "bills_observation_response_too_large",
            Self::DecodedResponseTooLarge => "bills_observation_decoded_too_large",
            Self::InvalidEncoding => "bills_observation_encoding_invalid",
            Self::MalformedXml => "bills_observation_xml_malformed",
            Self::ResourceLimitExceeded => "bills_observation_resource_limit",
            Self::WrongGrammar => "bills_observation_grammar_invalid",
            Self::DuplicateField => "bills_observation_field_duplicate",
            Self::ApplicationRejected => "bills_observation_application_rejected",
            Self::ProfileMismatch => "bills_observation_profile_mismatch",
            Self::InvalidValue => "bills_observation_value_invalid",
            Self::CountMismatch => "bills_observation_count_mismatch",
            Self::DuplicateObservation => "bills_observation_duplicate",
        }
    }
}

impl fmt::Display for BillsObservationError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.safe_code())
    }
}

impl std::error::Error for BillsObservationError {}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BillsObservationBinding {
    UnboundNoRequestArtifact,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BillsCountAuthority {
    ResponseInternalOnly,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ParsedOutstandingDirection {
    Receivable,
    Payable,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ParsedBillWiseState {
    Enabled,
    Disabled,
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ParsedBillOrigin {
    Voucher,
    LedgerOpening,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ParsedBillReferenceKind {
    Advance,
    AgainstReference,
    NewReference,
    OnAccount,
    Unclassified,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ParsedPolarity {
    Debit,
    Credit,
}

#[derive(Clone, PartialEq, Eq)]
pub struct UnboundBillsEvidence {
    profile_id: &'static str,
    binding: BillsObservationBinding,
    count_authority: BillsCountAuthority,
    encoding: TallyTextEncoding,
    encoded_bytes: u64,
    decoded_bytes: u64,
    claimed_company_guid: String,
    claimed_party_ledger: String,
    claimed_from_yyyymmdd: String,
    claimed_to_yyyymmdd: String,
    claimed_as_of_yyyymmdd: String,
    claimed_direction: ParsedOutstandingDirection,
    claimed_query_profile: String,
    claimed_bill_wise_state: ParsedBillWiseState,
    claimed_allocation_count: u64,
    claimed_outstanding_count: u64,
    response_sha256: String,
}

impl fmt::Debug for UnboundBillsEvidence {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("UnboundBillsEvidence")
            .field("profile_id", &self.profile_id)
            .field("binding", &self.binding)
            .field("count_authority", &self.count_authority)
            .field("encoding", &self.encoding)
            .field("encoded_bytes", &self.encoded_bytes)
            .field("decoded_bytes", &self.decoded_bytes)
            .field("claimed_direction", &self.claimed_direction)
            .field("claimed_bill_wise_state", &self.claimed_bill_wise_state)
            .field("claimed_allocation_count", &self.claimed_allocation_count)
            .field("claimed_outstanding_count", &self.claimed_outstanding_count)
            .field("response_sha256", &self.response_sha256)
            .finish_non_exhaustive()
    }
}

impl UnboundBillsEvidence {
    pub const fn binding(&self) -> BillsObservationBinding {
        self.binding
    }
    pub const fn count_authority(&self) -> BillsCountAuthority {
        self.count_authority
    }
    pub fn claimed_company_guid(&self) -> &str {
        &self.claimed_company_guid
    }
    pub fn claimed_party_ledger(&self) -> &str {
        &self.claimed_party_ledger
    }
    pub fn claimed_from_yyyymmdd(&self) -> &str {
        &self.claimed_from_yyyymmdd
    }
    pub fn claimed_to_yyyymmdd(&self) -> &str {
        &self.claimed_to_yyyymmdd
    }
    pub fn claimed_as_of_yyyymmdd(&self) -> &str {
        &self.claimed_as_of_yyyymmdd
    }
    pub const fn claimed_direction(&self) -> ParsedOutstandingDirection {
        self.claimed_direction
    }
    pub fn claimed_query_profile(&self) -> &str {
        &self.claimed_query_profile
    }
    pub const fn claimed_bill_wise_state(&self) -> ParsedBillWiseState {
        self.claimed_bill_wise_state
    }
    pub fn response_sha256(&self) -> &str {
        &self.response_sha256
    }
}

#[derive(Clone, PartialEq, Eq)]
pub struct ObservedRawBillAllocation {
    origin: ParsedBillOrigin,
    voucher_identity: Option<String>,
    party_entry_ordinal: Option<u64>,
    row_ordinal: u64,
    reference_kind: ParsedBillReferenceKind,
    raw_reference_kind: String,
    reference_name: Option<String>,
    bill_date: Option<String>,
    effective_date: Option<String>,
    due_date: Option<String>,
    amount: String,
    polarity: Option<ParsedPolarity>,
    currency: Option<String>,
    decoded_fragment_sha256: String,
}

impl fmt::Debug for ObservedRawBillAllocation {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ObservedRawBillAllocation")
            .field("origin", &self.origin)
            .field("voucher_identity_present", &self.voucher_identity.is_some())
            .field("party_entry_ordinal", &self.party_entry_ordinal)
            .field("row_ordinal", &self.row_ordinal)
            .field("reference_kind", &self.reference_kind)
            .field("reference_name_present", &self.reference_name.is_some())
            .field("bill_date_present", &self.bill_date.is_some())
            .field("effective_date_present", &self.effective_date.is_some())
            .field("due_date_present", &self.due_date.is_some())
            .field("polarity", &self.polarity)
            .field("currency_present", &self.currency.is_some())
            .field("decoded_fragment_sha256", &self.decoded_fragment_sha256)
            .finish()
    }
}

impl ObservedRawBillAllocation {
    pub const fn origin(&self) -> ParsedBillOrigin {
        self.origin
    }
    pub const fn row_ordinal(&self) -> u64 {
        self.row_ordinal
    }
    pub const fn reference_kind(&self) -> ParsedBillReferenceKind {
        self.reference_kind
    }
    pub fn raw_reference_kind(&self) -> &str {
        &self.raw_reference_kind
    }
    pub fn reference_name(&self) -> Option<&str> {
        self.reference_name.as_deref()
    }
    pub fn amount(&self) -> &str {
        &self.amount
    }
    pub fn due_date(&self) -> Option<&str> {
        self.due_date.as_deref()
    }
    pub fn decoded_fragment_sha256(&self) -> &str {
        &self.decoded_fragment_sha256
    }
}

#[derive(Clone, PartialEq, Eq)]
pub struct ObservedRawOutstanding {
    row_ordinal: u64,
    reference_kind: ParsedBillReferenceKind,
    raw_reference_kind: String,
    reference_name: Option<String>,
    bill_date: Option<String>,
    effective_date: Option<String>,
    due_date: Option<String>,
    opening_amount: Option<String>,
    pending_amount: String,
    polarity: Option<ParsedPolarity>,
    currency: Option<String>,
    source_overdue_days: Option<u32>,
    decoded_fragment_sha256: String,
}

impl fmt::Debug for ObservedRawOutstanding {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ObservedRawOutstanding")
            .field("row_ordinal", &self.row_ordinal)
            .field("reference_kind", &self.reference_kind)
            .field("reference_name_present", &self.reference_name.is_some())
            .field("bill_date_present", &self.bill_date.is_some())
            .field("effective_date_present", &self.effective_date.is_some())
            .field("due_date_present", &self.due_date.is_some())
            .field("opening_amount_present", &self.opening_amount.is_some())
            .field("polarity", &self.polarity)
            .field("currency_present", &self.currency.is_some())
            .field("source_overdue_days", &self.source_overdue_days)
            .field("decoded_fragment_sha256", &self.decoded_fragment_sha256)
            .finish()
    }
}

impl ObservedRawOutstanding {
    pub const fn row_ordinal(&self) -> u64 {
        self.row_ordinal
    }
    pub const fn reference_kind(&self) -> ParsedBillReferenceKind {
        self.reference_kind
    }
    pub fn raw_reference_kind(&self) -> &str {
        &self.raw_reference_kind
    }
    pub fn reference_name(&self) -> Option<&str> {
        self.reference_name.as_deref()
    }
    pub fn opening_amount(&self) -> Option<&str> {
        self.opening_amount.as_deref()
    }
    pub fn pending_amount(&self) -> &str {
        &self.pending_amount
    }
    pub fn due_date(&self) -> Option<&str> {
        self.due_date.as_deref()
    }
    pub fn decoded_fragment_sha256(&self) -> &str {
        &self.decoded_fragment_sha256
    }
}

#[derive(Clone, PartialEq, Eq)]
pub struct UnboundPartyOutstandingObservation {
    evidence: UnboundBillsEvidence,
    allocations: Vec<ObservedRawBillAllocation>,
    outstanding: Vec<ObservedRawOutstanding>,
}

impl fmt::Debug for UnboundPartyOutstandingObservation {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("UnboundPartyOutstandingObservation")
            .field("evidence", &self.evidence)
            .field("allocation_count", &self.allocations.len())
            .field("outstanding_count", &self.outstanding.len())
            .finish()
    }
}

impl UnboundPartyOutstandingObservation {
    pub fn evidence(&self) -> &UnboundBillsEvidence {
        &self.evidence
    }
    pub fn allocations(&self) -> &[ObservedRawBillAllocation] {
        &self.allocations
    }
    pub fn outstanding(&self) -> &[ObservedRawOutstanding] {
        &self.outstanding
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawEnvelope {
    #[serde(rename = "HEADER")]
    header: RawHeader,
    #[serde(rename = "BODY")]
    body: RawBody,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawHeader {
    #[serde(rename = "STATUS")]
    status: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawBody {
    #[serde(rename = "BILLSPARTYCONTEXT")]
    context: RawContext,
    #[serde(rename = "BILLALLOCATION", default)]
    allocations: Vec<RawAllocation>,
    #[serde(rename = "BILLOUTSTANDING", default)]
    outstanding: Vec<RawOutstanding>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawContext {
    #[serde(rename = "@SCHEMA")]
    schema: String,
    #[serde(rename = "@PROFILE")]
    profile: String,
    #[serde(rename = "@OBJECTTYPE")]
    object_type: String,
    #[serde(rename = "@COMPANYGUID")]
    company_guid: String,
    #[serde(rename = "@PARTYLEDGER")]
    party_ledger: String,
    #[serde(rename = "@FROMDATE")]
    from_date: String,
    #[serde(rename = "@TODATE")]
    to_date: String,
    #[serde(rename = "@ASOFDATE")]
    as_of_date: String,
    #[serde(rename = "@DIRECTION")]
    direction: String,
    #[serde(rename = "@QUERYPROFILE")]
    query_profile: String,
    #[serde(rename = "@BILLWISESTATE")]
    bill_wise_state: String,
    #[serde(rename = "@ALLOCATIONCOUNT")]
    allocation_count: String,
    #[serde(rename = "@OUTSTANDINGCOUNT")]
    outstanding_count: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawAllocation {
    #[serde(rename = "@ORIGIN")]
    origin: String,
    #[serde(rename = "@VOUCHERIDENTITY", default)]
    voucher_identity: Option<String>,
    #[serde(rename = "@PARTYENTRYORDINAL", default)]
    party_entry_ordinal: Option<String>,
    #[serde(rename = "@ROWORDINAL")]
    row_ordinal: String,
    #[serde(rename = "@REFERENCEKIND")]
    reference_kind: String,
    #[serde(rename = "@REFERENCENAME", default)]
    reference_name: Option<String>,
    #[serde(rename = "@BILLDATE", default)]
    bill_date: Option<String>,
    #[serde(rename = "@EFFECTIVEDATE", default)]
    effective_date: Option<String>,
    #[serde(rename = "@DUEDATE", default)]
    due_date: Option<String>,
    #[serde(rename = "@AMOUNT")]
    amount: String,
    #[serde(rename = "@POLARITY", default)]
    polarity: Option<String>,
    #[serde(rename = "@CURRENCY", default)]
    currency: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawOutstanding {
    #[serde(rename = "@ROWORDINAL")]
    row_ordinal: String,
    #[serde(rename = "@REFERENCEKIND")]
    reference_kind: String,
    #[serde(rename = "@REFERENCENAME", default)]
    reference_name: Option<String>,
    #[serde(rename = "@BILLDATE", default)]
    bill_date: Option<String>,
    #[serde(rename = "@EFFECTIVEDATE", default)]
    effective_date: Option<String>,
    #[serde(rename = "@DUEDATE", default)]
    due_date: Option<String>,
    #[serde(rename = "@OPENINGAMOUNT", default)]
    opening_amount: Option<String>,
    #[serde(rename = "@PENDINGAMOUNT")]
    pending_amount: String,
    #[serde(rename = "@POLARITY", default)]
    polarity: Option<String>,
    #[serde(rename = "@CURRENCY", default)]
    currency: Option<String>,
    #[serde(rename = "@OVERDUEDAYS", default)]
    overdue_days: Option<String>,
}

pub fn parse_unbound_party_outstanding_observation(
    bytes: impl AsRef<[u8]>,
    limits: BillsObservationLimits,
) -> Result<UnboundPartyOutstandingObservation, BillsObservationError> {
    validate_limits(limits)?;
    let encoded = bytes.as_ref();
    let decoded = decode_tally_text_bytes_limited(encoded, limits.max_encoded_bytes)
        .map_err(map_decode_error)?;
    if decoded.text.len() > limits.max_decoded_bytes {
        return Err(BillsObservationError::DecodedResponseTooLarge);
    }
    let fragments = scan_exact_grammar(&decoded.text, limits)?;
    let raw: RawEnvelope =
        quick_xml::de::from_str(&decoded.text).map_err(|_| BillsObservationError::MalformedXml)?;
    if raw.header.status != "1" {
        return Err(BillsObservationError::ApplicationRejected);
    }
    if raw.body.context.schema != BILLS_OBSERVED_RAW_SCHEMA_V1
        || raw.body.context.profile != BILLS_OBSERVED_RAW_PROFILE_V1
        || raw.body.context.object_type != "PARTYOUTSTANDING"
    {
        return Err(BillsObservationError::ProfileMismatch);
    }
    let context = raw.body.context;
    for value in [
        context.company_guid.as_str(),
        context.party_ledger.as_str(),
        context.query_profile.as_str(),
    ] {
        validate_text(value, limits.max_field_bytes)?;
    }
    for value in [
        context.from_date.as_str(),
        context.to_date.as_str(),
        context.as_of_date.as_str(),
    ] {
        validate_date(value)?;
    }
    if context.from_date > context.to_date || context.to_date > context.as_of_date {
        return Err(BillsObservationError::InvalidValue);
    }
    let direction = match context.direction.as_str() {
        "RECEIVABLE" => ParsedOutstandingDirection::Receivable,
        "PAYABLE" => ParsedOutstandingDirection::Payable,
        _ => return Err(BillsObservationError::InvalidValue),
    };
    let bill_wise_state = match context.bill_wise_state.as_str() {
        "ENABLED" => ParsedBillWiseState::Enabled,
        "DISABLED" => ParsedBillWiseState::Disabled,
        "UNKNOWN" => ParsedBillWiseState::Unknown,
        _ => return Err(BillsObservationError::InvalidValue),
    };
    let claimed_allocation_count = parse_u64(&context.allocation_count, true)?;
    let claimed_outstanding_count = parse_u64(&context.outstanding_count, true)?;
    if raw
        .body
        .allocations
        .len()
        .saturating_add(raw.body.outstanding.len())
        > limits.max_records
    {
        return Err(BillsObservationError::ResourceLimitExceeded);
    }
    if claimed_allocation_count != raw.body.allocations.len() as u64
        || claimed_outstanding_count != raw.body.outstanding.len() as u64
        || fragments.len() != raw.body.allocations.len() + raw.body.outstanding.len()
    {
        return Err(BillsObservationError::CountMismatch);
    }

    let mut fragments = fragments.into_iter();
    let mut allocation_keys = HashSet::new();
    let mut allocations = Vec::with_capacity(raw.body.allocations.len());
    for row in raw.body.allocations {
        let origin = match row.origin.as_str() {
            "VOUCHER" => ParsedBillOrigin::Voucher,
            "LEDGEROPENING" => ParsedBillOrigin::LedgerOpening,
            _ => return Err(BillsObservationError::InvalidValue),
        };
        validate_optional_text(&row.voucher_identity, limits.max_field_bytes)?;
        let party_entry_ordinal = row
            .party_entry_ordinal
            .as_deref()
            .map(|value| parse_u64(value, false))
            .transpose()?;
        match origin {
            ParsedBillOrigin::Voucher
                if row.voucher_identity.is_none() || party_entry_ordinal.is_none() =>
            {
                return Err(BillsObservationError::InvalidValue);
            }
            ParsedBillOrigin::LedgerOpening
                if row.voucher_identity.is_some() || party_entry_ordinal.is_some() =>
            {
                return Err(BillsObservationError::InvalidValue);
            }
            _ => {}
        }
        let row_ordinal = parse_u64(&row.row_ordinal, false)?;
        if !allocation_keys.insert((origin, row.voucher_identity.clone(), row_ordinal)) {
            return Err(BillsObservationError::DuplicateObservation);
        }
        validate_text(&row.reference_kind, limits.max_field_bytes)?;
        let reference_kind = parse_reference_kind(&row.reference_kind);
        validate_reference(reference_kind, &row.reference_name, limits.max_field_bytes)?;
        validate_optional_date(&row.bill_date)?;
        validate_optional_date(&row.effective_date)?;
        validate_optional_date(&row.due_date)?;
        validate_decimal(&row.amount)?;
        let polarity = parse_optional_polarity(row.polarity.as_deref())?;
        validate_optional_text(&row.currency, limits.max_field_bytes)?;
        let fragment = fragments
            .next()
            .ok_or(BillsObservationError::WrongGrammar)?;
        allocations.push(ObservedRawBillAllocation {
            origin,
            voucher_identity: row.voucher_identity,
            party_entry_ordinal,
            row_ordinal,
            reference_kind,
            raw_reference_kind: row.reference_kind,
            reference_name: row.reference_name,
            bill_date: row.bill_date,
            effective_date: row.effective_date,
            due_date: row.due_date,
            amount: row.amount,
            polarity,
            currency: row.currency,
            decoded_fragment_sha256: hash_domain(FRAGMENT_HASH_DOMAIN, fragment.as_bytes()),
        });
    }

    let mut outstanding_ordinals = HashSet::new();
    let mut outstanding = Vec::with_capacity(raw.body.outstanding.len());
    for row in raw.body.outstanding {
        let row_ordinal = parse_u64(&row.row_ordinal, false)?;
        if !outstanding_ordinals.insert(row_ordinal) {
            return Err(BillsObservationError::DuplicateObservation);
        }
        validate_text(&row.reference_kind, limits.max_field_bytes)?;
        let reference_kind = parse_reference_kind(&row.reference_kind);
        validate_reference(reference_kind, &row.reference_name, limits.max_field_bytes)?;
        validate_optional_date(&row.bill_date)?;
        validate_optional_date(&row.effective_date)?;
        validate_optional_date(&row.due_date)?;
        if let Some(value) = &row.opening_amount {
            validate_decimal(value)?;
        }
        validate_decimal(&row.pending_amount)?;
        let polarity = parse_optional_polarity(row.polarity.as_deref())?;
        validate_optional_text(&row.currency, limits.max_field_bytes)?;
        let source_overdue_days = row
            .overdue_days
            .as_deref()
            .map(|value| parse_u64(value, true))
            .transpose()?
            .map(|value| u32::try_from(value).map_err(|_| BillsObservationError::InvalidValue))
            .transpose()?;
        let fragment = fragments
            .next()
            .ok_or(BillsObservationError::WrongGrammar)?;
        outstanding.push(ObservedRawOutstanding {
            row_ordinal,
            reference_kind,
            raw_reference_kind: row.reference_kind,
            reference_name: row.reference_name,
            bill_date: row.bill_date,
            effective_date: row.effective_date,
            due_date: row.due_date,
            opening_amount: row.opening_amount,
            pending_amount: row.pending_amount,
            polarity,
            currency: row.currency,
            source_overdue_days,
            decoded_fragment_sha256: hash_domain(FRAGMENT_HASH_DOMAIN, fragment.as_bytes()),
        });
    }

    Ok(UnboundPartyOutstandingObservation {
        evidence: UnboundBillsEvidence {
            profile_id: BILLS_OBSERVED_RAW_PROFILE_V1,
            binding: BillsObservationBinding::UnboundNoRequestArtifact,
            count_authority: BillsCountAuthority::ResponseInternalOnly,
            encoding: decoded.encoding,
            encoded_bytes: encoded.len() as u64,
            decoded_bytes: decoded.text.len() as u64,
            claimed_company_guid: context.company_guid,
            claimed_party_ledger: context.party_ledger,
            claimed_from_yyyymmdd: context.from_date,
            claimed_to_yyyymmdd: context.to_date,
            claimed_as_of_yyyymmdd: context.as_of_date,
            claimed_direction: direction,
            claimed_query_profile: context.query_profile,
            claimed_bill_wise_state: bill_wise_state,
            claimed_allocation_count,
            claimed_outstanding_count,
            response_sha256: hash_domain(RESPONSE_HASH_DOMAIN, encoded),
        },
        allocations,
        outstanding,
    })
}

fn validate_limits(limits: BillsObservationLimits) -> Result<(), BillsObservationError> {
    if limits.max_encoded_bytes == 0
        || limits.max_encoded_bytes > 8 * 1024 * 1024
        || limits.max_decoded_bytes == 0
        || limits.max_decoded_bytes > 8 * 1024 * 1024
        || limits.max_records == 0
        || limits.max_records > 25_000
        || limits.max_field_bytes == 0
        || limits.max_field_bytes > 512
        || limits.max_nodes == 0
        || limits.max_nodes > 250_000
        || !(3..=8).contains(&limits.max_depth)
        || limits.max_attributes == 0
        || limits.max_attributes > 32
    {
        return Err(BillsObservationError::InvalidLimits);
    }
    Ok(())
}

fn map_decode_error(error: TallyTextDecodeError) -> BillsObservationError {
    match error {
        TallyTextDecodeError::TooLarge => BillsObservationError::ResponseTooLarge,
        _ => BillsObservationError::InvalidEncoding,
    }
}

fn parse_reference_kind(value: &str) -> ParsedBillReferenceKind {
    match value {
        "Advance" => ParsedBillReferenceKind::Advance,
        "Agst Ref" => ParsedBillReferenceKind::AgainstReference,
        "New Ref" => ParsedBillReferenceKind::NewReference,
        "On Account" => ParsedBillReferenceKind::OnAccount,
        _ => ParsedBillReferenceKind::Unclassified,
    }
}

fn validate_reference(
    kind: ParsedBillReferenceKind,
    name: &Option<String>,
    limit: usize,
) -> Result<(), BillsObservationError> {
    validate_optional_text(name, limit)?;
    let valid = match kind {
        ParsedBillReferenceKind::OnAccount => name.is_none(),
        ParsedBillReferenceKind::Advance
        | ParsedBillReferenceKind::AgainstReference
        | ParsedBillReferenceKind::NewReference => name.is_some(),
        ParsedBillReferenceKind::Unclassified => true,
    };
    if valid {
        Ok(())
    } else {
        Err(BillsObservationError::InvalidValue)
    }
}

fn parse_optional_polarity(
    value: Option<&str>,
) -> Result<Option<ParsedPolarity>, BillsObservationError> {
    value
        .map(|value| match value {
            "DEBIT" => Ok(ParsedPolarity::Debit),
            "CREDIT" => Ok(ParsedPolarity::Credit),
            _ => Err(BillsObservationError::InvalidValue),
        })
        .transpose()
}

fn validate_optional_text(
    value: &Option<String>,
    limit: usize,
) -> Result<(), BillsObservationError> {
    if let Some(value) = value {
        validate_text(value, limit)?;
    }
    Ok(())
}

fn validate_text(value: &str, limit: usize) -> Result<(), BillsObservationError> {
    if value.is_empty()
        || value.len() > limit
        || value.trim() != value
        || value.chars().any(char::is_control)
    {
        Err(BillsObservationError::InvalidValue)
    } else {
        Ok(())
    }
}

fn validate_optional_date(value: &Option<String>) -> Result<(), BillsObservationError> {
    if let Some(value) = value {
        validate_date(value)?;
    }
    Ok(())
}

fn validate_date(value: &str) -> Result<(), BillsObservationError> {
    if value.len() != 8 || !value.bytes().all(|byte| byte.is_ascii_digit()) {
        return Err(BillsObservationError::InvalidValue);
    }
    let year = value[0..4]
        .parse::<u32>()
        .map_err(|_| BillsObservationError::InvalidValue)?;
    let month = value[4..6]
        .parse::<u32>()
        .map_err(|_| BillsObservationError::InvalidValue)?;
    let day = value[6..8]
        .parse::<u32>()
        .map_err(|_| BillsObservationError::InvalidValue)?;
    let leap = year % 4 == 0 && (year % 100 != 0 || year % 400 == 0);
    let maximum = match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 if leap => 29,
        2 => 28,
        _ => return Err(BillsObservationError::InvalidValue),
    };
    if year == 0 || day == 0 || day > maximum {
        return Err(BillsObservationError::InvalidValue);
    }
    Ok(())
}

fn validate_decimal(value: &str) -> Result<(), BillsObservationError> {
    if value.is_empty() || value.len() > 256 || value.starts_with('+') {
        return Err(BillsObservationError::InvalidValue);
    }
    let digits = value.strip_prefix('-').unwrap_or(value);
    let mut parts = digits.split('.');
    let whole = parts.next().unwrap_or_default();
    let fraction = parts.next();
    if parts.next().is_some()
        || whole.is_empty()
        || !whole.bytes().all(|byte| byte.is_ascii_digit())
        || fraction
            .is_some_and(|part| part.is_empty() || !part.bytes().all(|byte| byte.is_ascii_digit()))
    {
        return Err(BillsObservationError::InvalidValue);
    }
    Ok(())
}

fn parse_u64(value: &str, allow_zero: bool) -> Result<u64, BillsObservationError> {
    if value.is_empty()
        || !value.bytes().all(|byte| byte.is_ascii_digit())
        || (value.len() > 1 && value.starts_with('0'))
    {
        return Err(BillsObservationError::InvalidValue);
    }
    let parsed = value
        .parse::<u64>()
        .map_err(|_| BillsObservationError::InvalidValue)?;
    if !allow_zero && parsed == 0 {
        return Err(BillsObservationError::InvalidValue);
    }
    Ok(parsed)
}

fn hash_domain(domain: &[u8], bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(domain);
    hasher.update(bytes);
    hasher
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RowKind {
    Allocation,
    Outstanding,
}

fn scan_exact_grammar(
    xml: &str,
    limits: BillsObservationLimits,
) -> Result<Vec<String>, BillsObservationError> {
    let mut reader = Reader::from_str(xml);
    reader.config_mut().trim_text(false);
    let mut stack = Vec::<String>::new();
    let mut nodes = 0usize;
    let mut root_closed = false;
    let mut header_status_seen = false;
    let mut context_seen = false;
    let mut body_stage = 0_u8;
    let mut fragments = Vec::new();
    loop {
        let before = reader.buffer_position() as usize;
        let event = reader
            .read_event()
            .map_err(|_| BillsObservationError::MalformedXml)?;
        nodes = nodes.saturating_add(1);
        if nodes > limits.max_nodes {
            return Err(BillsObservationError::ResourceLimitExceeded);
        }
        match event {
            Event::Start(start) => {
                if root_closed {
                    return Err(BillsObservationError::WrongGrammar);
                }
                let name = event_name(start.name().as_ref())?;
                validate_attributes(&start, &name, limits)?;
                let path = stack.iter().map(String::as_str).collect::<Vec<_>>();
                match (path.as_slice(), name.as_str()) {
                    ([], "ENVELOPE") => {}
                    (["ENVELOPE"], "HEADER") => {}
                    (["ENVELOPE", "HEADER"], "STATUS") if !header_status_seen => {
                        header_status_seen = true;
                    }
                    (["ENVELOPE"], "BODY") if header_status_seen => {}
                    _ => return Err(BillsObservationError::WrongGrammar),
                }
                stack.push(name);
                if stack.len() > limits.max_depth {
                    return Err(BillsObservationError::ResourceLimitExceeded);
                }
            }
            Event::Empty(empty) => {
                if root_closed || stack.as_slice() != ["ENVELOPE", "BODY"] {
                    return Err(BillsObservationError::WrongGrammar);
                }
                let name = event_name(empty.name().as_ref())?;
                validate_attributes(&empty, &name, limits)?;
                let row_kind = match name.as_str() {
                    "BILLSPARTYCONTEXT" if !context_seen && body_stage == 0 => {
                        context_seen = true;
                        body_stage = 1;
                        None
                    }
                    "BILLALLOCATION" if context_seen && body_stage <= 1 => {
                        body_stage = 1;
                        Some(RowKind::Allocation)
                    }
                    "BILLOUTSTANDING" if context_seen => {
                        body_stage = 2;
                        Some(RowKind::Outstanding)
                    }
                    _ => return Err(BillsObservationError::WrongGrammar),
                };
                if row_kind.is_some() {
                    if fragments.len() >= limits.max_records {
                        return Err(BillsObservationError::ResourceLimitExceeded);
                    }
                    let after = reader.buffer_position() as usize;
                    fragments.push(
                        xml.get(before..after)
                            .ok_or(BillsObservationError::MalformedXml)?
                            .to_string(),
                    );
                }
            }
            Event::End(end) => {
                let name = event_name(end.name().as_ref())?;
                if stack.last().map(String::as_str) != Some(name.as_str()) {
                    return Err(BillsObservationError::WrongGrammar);
                }
                stack.pop();
                if name == "ENVELOPE" {
                    root_closed = true;
                }
            }
            Event::Text(text) => {
                let decoded = text
                    .decode()
                    .map_err(|_| BillsObservationError::MalformedXml)?;
                if decoded.len() > limits.max_field_bytes {
                    return Err(BillsObservationError::ResourceLimitExceeded);
                }
                if stack.last().map(String::as_str) != Some("STATUS")
                    && !decoded.chars().all(char::is_whitespace)
                {
                    return Err(BillsObservationError::WrongGrammar);
                }
            }
            Event::Decl(_) if nodes == 1 && stack.is_empty() => {}
            Event::Eof => break,
            Event::Decl(_)
            | Event::Comment(_)
            | Event::CData(_)
            | Event::PI(_)
            | Event::DocType(_)
            | Event::GeneralRef(_) => return Err(BillsObservationError::WrongGrammar),
        }
    }
    if !root_closed || !stack.is_empty() || !header_status_seen || !context_seen {
        return Err(BillsObservationError::WrongGrammar);
    }
    Ok(fragments)
}

fn event_name(bytes: &[u8]) -> Result<String, BillsObservationError> {
    let name = std::str::from_utf8(bytes).map_err(|_| BillsObservationError::MalformedXml)?;
    if !name.bytes().all(|byte| byte.is_ascii_uppercase()) {
        return Err(BillsObservationError::WrongGrammar);
    }
    Ok(name.to_string())
}

fn validate_attributes(
    start: &quick_xml::events::BytesStart<'_>,
    element: &str,
    limits: BillsObservationLimits,
) -> Result<(), BillsObservationError> {
    let allowed: &[&str] = match element {
        "BILLSPARTYCONTEXT" => &[
            "SCHEMA",
            "PROFILE",
            "OBJECTTYPE",
            "COMPANYGUID",
            "PARTYLEDGER",
            "FROMDATE",
            "TODATE",
            "ASOFDATE",
            "DIRECTION",
            "QUERYPROFILE",
            "BILLWISESTATE",
            "ALLOCATIONCOUNT",
            "OUTSTANDINGCOUNT",
        ],
        "BILLALLOCATION" => &[
            "ORIGIN",
            "VOUCHERIDENTITY",
            "PARTYENTRYORDINAL",
            "ROWORDINAL",
            "REFERENCEKIND",
            "REFERENCENAME",
            "BILLDATE",
            "EFFECTIVEDATE",
            "DUEDATE",
            "AMOUNT",
            "POLARITY",
            "CURRENCY",
        ],
        "BILLOUTSTANDING" => &[
            "ROWORDINAL",
            "REFERENCEKIND",
            "REFERENCENAME",
            "BILLDATE",
            "EFFECTIVEDATE",
            "DUEDATE",
            "OPENINGAMOUNT",
            "PENDINGAMOUNT",
            "POLARITY",
            "CURRENCY",
            "OVERDUEDAYS",
        ],
        _ => &[],
    };
    let mut seen = HashSet::new();
    let mut count = 0usize;
    for attribute in start.attributes().with_checks(false) {
        let attribute = attribute.map_err(|_| BillsObservationError::MalformedXml)?;
        count = count.saturating_add(1);
        if count > limits.max_attributes {
            return Err(BillsObservationError::ResourceLimitExceeded);
        }
        let key = std::str::from_utf8(attribute.key.as_ref())
            .map_err(|_| BillsObservationError::MalformedXml)?;
        if key != key.to_ascii_uppercase() || !seen.insert(key.to_string()) {
            return Err(BillsObservationError::DuplicateField);
        }
        if !allowed.contains(&key) {
            return Err(BillsObservationError::WrongGrammar);
        }
        if attribute.value.len() > limits.max_field_bytes {
            return Err(BillsObservationError::ResourceLimitExceeded);
        }
    }
    if allowed.is_empty() && count != 0 {
        return Err(BillsObservationError::WrongGrammar);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn envelope(allocation: &str, outstanding: &str, counts: (&str, &str)) -> String {
        format!(
            "<ENVELOPE><HEADER><STATUS>1</STATUS></HEADER><BODY><BILLSPARTYCONTEXT SCHEMA=\"{BILLS_OBSERVED_RAW_SCHEMA_V1}\" PROFILE=\"{BILLS_OBSERVED_RAW_PROFILE_V1}\" OBJECTTYPE=\"PARTYOUTSTANDING\" COMPANYGUID=\"synthetic-company-guid\" PARTYLEDGER=\"Synthetic Party\" FROMDATE=\"20260101\" TODATE=\"20260731\" ASOFDATE=\"20260731\" DIRECTION=\"RECEIVABLE\" QUERYPROFILE=\"bills-confidence-v1\" BILLWISESTATE=\"ENABLED\" ALLOCATIONCOUNT=\"{}\" OUTSTANDINGCOUNT=\"{}\"/>{allocation}{outstanding}</BODY></ENVELOPE>",
            counts.0, counts.1
        )
    }

    fn voucher_allocation(kind: &str, name: Option<&str>, amount: &str) -> String {
        let name = name.map_or(String::new(), |value| format!(" REFERENCENAME=\"{value}\""));
        format!(
            "<BILLALLOCATION ORIGIN=\"VOUCHER\" VOUCHERIDENTITY=\"synthetic-voucher-1\" PARTYENTRYORDINAL=\"1\" ROWORDINAL=\"1\" REFERENCEKIND=\"{kind}\"{name} BILLDATE=\"20260701\" DUEDATE=\"20260731\" AMOUNT=\"{amount}\" POLARITY=\"DEBIT\" CURRENCY=\"company-base\"/>"
        )
    }

    fn outstanding(kind: &str, name: Option<&str>, pending: &str) -> String {
        let name = name.map_or(String::new(), |value| format!(" REFERENCENAME=\"{value}\""));
        format!(
            "<BILLOUTSTANDING ROWORDINAL=\"1\" REFERENCEKIND=\"{kind}\"{name} BILLDATE=\"20260701\" DUEDATE=\"20260731\" OPENINGAMOUNT=\"-1000\" PENDINGAMOUNT=\"{pending}\" POLARITY=\"DEBIT\" CURRENCY=\"company-base\" OVERDUEDAYS=\"0\"/>"
        )
    }

    #[test]
    fn parses_unbound_exact_scope_opening_optional_dates_and_on_account() {
        let xml = envelope(
            "<BILLALLOCATION ORIGIN=\"LEDGEROPENING\" ROWORDINAL=\"1\" REFERENCEKIND=\"On Account\" AMOUNT=\"-50.00\" POLARITY=\"DEBIT\" CURRENCY=\"company-base\"/>",
            &outstanding("On Account", None, "-50"),
            ("1", "1"),
        );
        let parsed = parse_unbound_party_outstanding_observation(
            xml.as_bytes(),
            BillsObservationLimits::default(),
        )
        .unwrap();
        assert_eq!(
            parsed.evidence().binding(),
            BillsObservationBinding::UnboundNoRequestArtifact
        );
        assert_eq!(
            parsed.allocations()[0].origin(),
            ParsedBillOrigin::LedgerOpening
        );
        assert_eq!(
            parsed.allocations()[0].reference_kind(),
            ParsedBillReferenceKind::OnAccount
        );
        assert!(parsed.allocations()[0].reference_name().is_none());
        assert_eq!(parsed.outstanding()[0].pending_amount(), "-50");
    }

    #[test]
    fn preserves_unclassified_reference_without_coercing_it() {
        let xml = envelope(
            &voucher_allocation("Future Ref", Some("SYNTHETIC-1"), "-100"),
            &outstanding("Future Ref", Some("SYNTHETIC-1"), "-100"),
            ("1", "1"),
        );
        let parsed = parse_unbound_party_outstanding_observation(
            xml.as_bytes(),
            BillsObservationLimits::default(),
        )
        .unwrap();
        assert_eq!(
            parsed.allocations()[0].reference_kind(),
            ParsedBillReferenceKind::Unclassified
        );
        assert_eq!(parsed.allocations()[0].raw_reference_kind(), "Future Ref");
    }

    #[test]
    fn exact_reference_types_partial_amounts_and_utf16_are_preserved() {
        for kind in ["Advance", "Agst Ref", "New Ref"] {
            let xml = envelope(
                &voucher_allocation(kind, Some("SYNTHETIC-1"), "-500.000"),
                &outstanding(kind, Some("SYNTHETIC-1"), "-500"),
                ("1", "1"),
            );
            let mut encoded = vec![0xff, 0xfe];
            encoded.extend(
                xml.encode_utf16()
                    .flat_map(u16::to_le_bytes)
                    .collect::<Vec<_>>(),
            );
            let parsed = parse_unbound_party_outstanding_observation(
                encoded,
                BillsObservationLimits::default(),
            )
            .unwrap();
            assert_eq!(parsed.allocations()[0].amount(), "-500.000");
            assert_eq!(parsed.outstanding()[0].opening_amount(), Some("-1000"));
        }
    }

    #[test]
    fn counts_scope_reference_rules_and_grammar_fail_closed() {
        let valid_allocation = voucher_allocation("New Ref", Some("SYNTHETIC-1"), "-100");
        let valid_outstanding = outstanding("New Ref", Some("SYNTHETIC-1"), "-100");
        for xml in [
            envelope(&valid_allocation, &valid_outstanding, ("2", "1")),
            envelope(
                &voucher_allocation("", Some("SYNTHETIC-1"), "-100"),
                &valid_outstanding,
                ("1", "1"),
            ),
            envelope(
                &voucher_allocation(" New Ref ", Some("SYNTHETIC-1"), "-100"),
                &valid_outstanding,
                ("1", "1"),
            ),
            envelope(
                &voucher_allocation("New Ref", None, "-100"),
                &valid_outstanding,
                ("1", "1"),
            ),
            envelope(
                &valid_allocation,
                "<BILLOUTSTANDING ROWORDINAL=\"1\" REFERENCEKIND=\"New Ref\" REFERENCENAME=\"SYNTHETIC-1\" PENDINGAMOUNT=\"1e2\"/>",
                ("1", "1"),
            ),
            envelope(
                &format!("{valid_allocation}{valid_allocation}"),
                &valid_outstanding,
                ("2", "1"),
            ),
            envelope(
                &valid_allocation,
                &valid_outstanding.replace("/>", " UNKNOWN=\"x\"/>"),
                ("1", "1"),
            ),
        ] {
            assert!(parse_unbound_party_outstanding_observation(
                xml.as_bytes(),
                BillsObservationLimits::default(),
            )
            .is_err());
        }
    }

    #[test]
    fn resource_limits_and_errors_never_echo_sensitive_values() {
        let sentinel = "SENSITIVE-PARTY-AND-BILL";
        let xml = envelope(
            &voucher_allocation("New Ref", Some(sentinel), "-100"),
            &outstanding("New Ref", Some(sentinel), "-100"),
            ("1", "1"),
        );
        let limits = BillsObservationLimits {
            max_field_bytes: 8,
            ..BillsObservationLimits::default()
        };
        let error =
            parse_unbound_party_outstanding_observation(xml.as_bytes(), limits).unwrap_err();
        assert!(!format!("{error:?} {error}").contains(sentinel));

        let limits = BillsObservationLimits {
            max_records: 1,
            ..BillsObservationLimits::default()
        };
        assert_eq!(
            parse_unbound_party_outstanding_observation(xml.as_bytes(), limits).unwrap_err(),
            BillsObservationError::ResourceLimitExceeded
        );
    }
}
