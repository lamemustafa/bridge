//! Dormant parser for one Bridge-owned India Tax observation envelope.
//!
//! The parsed values are deliberately unbound: there is no request artifact,
//! TDL report, runtime dispatch, canonical adapter, or completeness authority.

use std::{collections::HashSet, fmt};

use quick_xml::{events::Event, Reader};
use serde::Deserialize;
use sha2::{Digest, Sha256};

use crate::{decode_tally_text_bytes_limited, TallyTextDecodeError, TallyTextEncoding};

pub const INDIA_TAX_OBSERVED_RAW_SCHEMA_V1: &str = "bridge.tally.india-tax-observed-raw/1";
pub const INDIA_TAX_OBSERVED_RAW_PROFILE_V1: &str = "bridge.india-tax-observed-raw-xml/1";

const RESPONSE_HASH_DOMAIN: &[u8] = b"bridge.tally.india-tax-observed-raw-response/1\0";
const FRAGMENT_HASH_DOMAIN: &[u8] = b"bridge.tally.india-tax-observed-raw-fragment/1\0";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct IndiaTaxObservationLimits {
    pub max_encoded_bytes: usize,
    pub max_decoded_bytes: usize,
    pub max_records: usize,
    pub max_field_bytes: usize,
    pub max_nodes: usize,
    pub max_depth: usize,
    pub max_attributes: usize,
}

impl Default for IndiaTaxObservationLimits {
    fn default() -> Self {
        Self {
            max_encoded_bytes: 8 * 1024 * 1024,
            max_decoded_bytes: 8 * 1024 * 1024,
            max_records: 25_000,
            max_field_bytes: 512,
            max_nodes: 250_000,
            max_depth: 8,
            max_attributes: 16,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IndiaTaxObservationError {
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
    MissingIdentity,
    CountMismatch,
    DuplicateObservation,
}

impl IndiaTaxObservationError {
    pub const fn safe_code(self) -> &'static str {
        match self {
            Self::InvalidLimits => "india_tax_observation_limits_invalid",
            Self::ResponseTooLarge => "india_tax_observation_response_too_large",
            Self::DecodedResponseTooLarge => "india_tax_observation_decoded_too_large",
            Self::InvalidEncoding => "india_tax_observation_encoding_invalid",
            Self::MalformedXml => "india_tax_observation_xml_malformed",
            Self::ResourceLimitExceeded => "india_tax_observation_resource_limit",
            Self::WrongGrammar => "india_tax_observation_grammar_invalid",
            Self::DuplicateField => "india_tax_observation_field_duplicate",
            Self::ApplicationRejected => "india_tax_observation_application_rejected",
            Self::ProfileMismatch => "india_tax_observation_profile_mismatch",
            Self::InvalidValue => "india_tax_observation_value_invalid",
            Self::MissingIdentity => "india_tax_observation_identity_missing",
            Self::CountMismatch => "india_tax_observation_count_mismatch",
            Self::DuplicateObservation => "india_tax_observation_duplicate",
        }
    }
}

impl fmt::Display for IndiaTaxObservationError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.safe_code())
    }
}

impl std::error::Error for IndiaTaxObservationError {}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IndiaTaxObservationBinding {
    UnboundNoRequestArtifact,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IndiaTaxCountAuthority {
    ResponseInternalOnly,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ObservedTaxOwnerKind {
    Company,
    Ledger,
}

#[derive(Clone, PartialEq, Eq, Hash)]
pub struct ObservedIdentityCandidates {
    guid: Option<String>,
    remote_id: Option<String>,
    master_id: Option<String>,
}

impl fmt::Debug for ObservedIdentityCandidates {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ObservedIdentityCandidates")
            .field("guid_present", &self.guid.is_some())
            .field("remote_id_present", &self.remote_id.is_some())
            .field("master_id_present", &self.master_id.is_some())
            .finish()
    }
}

impl ObservedIdentityCandidates {
    pub fn guid(&self) -> Option<&str> {
        self.guid.as_deref()
    }

    pub fn remote_id(&self) -> Option<&str> {
        self.remote_id.as_deref()
    }

    pub fn master_id(&self) -> Option<&str> {
        self.master_id.as_deref()
    }
}

#[derive(Clone, PartialEq, Eq)]
pub struct ObservedRawTaxRegistration {
    owner_kind: ObservedTaxOwnerKind,
    owner_identities: ObservedIdentityCandidates,
    owner_alter_id: Option<String>,
    registration_type: String,
    gstin: String,
    raw_fragment_sha256: String,
}

impl fmt::Debug for ObservedRawTaxRegistration {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ObservedRawTaxRegistration")
            .field("owner_kind", &self.owner_kind)
            .field("owner_identities", &self.owner_identities)
            .field("owner_alter_id_present", &self.owner_alter_id.is_some())
            .field(
                "registration_type_present",
                &!self.registration_type.is_empty(),
            )
            .field("gstin_present", &!self.gstin.is_empty())
            .field("raw_fragment_sha256", &self.raw_fragment_sha256)
            .finish()
    }
}

impl ObservedRawTaxRegistration {
    pub const fn owner_kind(&self) -> ObservedTaxOwnerKind {
        self.owner_kind
    }
    pub fn owner_identities(&self) -> &ObservedIdentityCandidates {
        &self.owner_identities
    }
    pub fn owner_alter_id(&self) -> Option<&str> {
        self.owner_alter_id.as_deref()
    }
    pub fn registration_type(&self) -> &str {
        &self.registration_type
    }
    pub fn gstin(&self) -> &str {
        &self.gstin
    }
    pub fn raw_fragment_sha256(&self) -> &str {
        &self.raw_fragment_sha256
    }
}

#[derive(Clone, PartialEq, Eq)]
pub struct ObservedRawVoucherTax {
    voucher_identities: ObservedIdentityCandidates,
    voucher_alter_id: Option<String>,
    tax_row_ordinal: u64,
    place_of_supply: String,
    assessable_value: String,
    tax_component: String,
    tax_rate: String,
    tax_amount: String,
    raw_fragment_sha256: String,
}

impl fmt::Debug for ObservedRawVoucherTax {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ObservedRawVoucherTax")
            .field("voucher_identities", &self.voucher_identities)
            .field("voucher_alter_id_present", &self.voucher_alter_id.is_some())
            .field("tax_row_ordinal", &self.tax_row_ordinal)
            .field("raw_fragment_sha256", &self.raw_fragment_sha256)
            .finish()
    }
}

impl ObservedRawVoucherTax {
    pub fn voucher_identities(&self) -> &ObservedIdentityCandidates {
        &self.voucher_identities
    }
    pub fn voucher_alter_id(&self) -> Option<&str> {
        self.voucher_alter_id.as_deref()
    }
    pub const fn tax_row_ordinal(&self) -> u64 {
        self.tax_row_ordinal
    }
    pub fn place_of_supply(&self) -> &str {
        &self.place_of_supply
    }
    pub fn assessable_value(&self) -> &str {
        &self.assessable_value
    }
    pub fn tax_component(&self) -> &str {
        &self.tax_component
    }
    pub fn tax_rate(&self) -> &str {
        &self.tax_rate
    }
    pub fn tax_amount(&self) -> &str {
        &self.tax_amount
    }
    pub fn raw_fragment_sha256(&self) -> &str {
        &self.raw_fragment_sha256
    }
}

#[derive(Clone, PartialEq, Eq)]
pub struct UnboundIndiaTaxEvidence {
    profile_id: &'static str,
    binding: IndiaTaxObservationBinding,
    count_authority: IndiaTaxCountAuthority,
    encoding: TallyTextEncoding,
    encoded_bytes: u64,
    decoded_bytes: u64,
    claimed_company_guid: String,
    claimed_from_yyyymmdd: String,
    claimed_to_yyyymmdd: String,
    claimed_registration_count: u64,
    claimed_voucher_tax_count: u64,
    response_sha256: String,
}

impl fmt::Debug for UnboundIndiaTaxEvidence {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("UnboundIndiaTaxEvidence")
            .field("profile_id", &self.profile_id)
            .field("binding", &self.binding)
            .field("count_authority", &self.count_authority)
            .field("encoding", &self.encoding)
            .field("encoded_bytes", &self.encoded_bytes)
            .field("decoded_bytes", &self.decoded_bytes)
            .field(
                "claimed_registration_count",
                &self.claimed_registration_count,
            )
            .field("claimed_voucher_tax_count", &self.claimed_voucher_tax_count)
            .field("response_sha256", &self.response_sha256)
            .finish()
    }
}

impl UnboundIndiaTaxEvidence {
    pub const fn profile_id(&self) -> &'static str {
        self.profile_id
    }
    pub const fn binding(&self) -> IndiaTaxObservationBinding {
        self.binding
    }
    pub const fn count_authority(&self) -> IndiaTaxCountAuthority {
        self.count_authority
    }
    pub const fn encoding(&self) -> TallyTextEncoding {
        self.encoding
    }
    pub const fn encoded_bytes(&self) -> u64 {
        self.encoded_bytes
    }
    pub const fn decoded_bytes(&self) -> u64 {
        self.decoded_bytes
    }
    pub fn claimed_company_guid(&self) -> &str {
        &self.claimed_company_guid
    }
    pub fn claimed_from_yyyymmdd(&self) -> &str {
        &self.claimed_from_yyyymmdd
    }
    pub fn claimed_to_yyyymmdd(&self) -> &str {
        &self.claimed_to_yyyymmdd
    }
    pub const fn claimed_registration_count(&self) -> u64 {
        self.claimed_registration_count
    }
    pub const fn claimed_voucher_tax_count(&self) -> u64 {
        self.claimed_voucher_tax_count
    }
    pub fn response_sha256(&self) -> &str {
        &self.response_sha256
    }
}

#[derive(Clone, PartialEq, Eq)]
pub struct UnboundIndiaTaxObservation {
    tax_registrations: Vec<ObservedRawTaxRegistration>,
    voucher_taxes: Vec<ObservedRawVoucherTax>,
    evidence: UnboundIndiaTaxEvidence,
}

impl fmt::Debug for UnboundIndiaTaxObservation {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("UnboundIndiaTaxObservation")
            .field("tax_registration_count", &self.tax_registrations.len())
            .field("voucher_tax_count", &self.voucher_taxes.len())
            .field("evidence", &self.evidence)
            .finish()
    }
}

impl UnboundIndiaTaxObservation {
    pub fn tax_registrations(&self) -> &[ObservedRawTaxRegistration] {
        &self.tax_registrations
    }
    pub fn voucher_taxes(&self) -> &[ObservedRawVoucherTax] {
        &self.voucher_taxes
    }
    pub fn evidence(&self) -> &UnboundIndiaTaxEvidence {
        &self.evidence
    }
    pub const fn canonicalization_eligible(&self) -> bool {
        false
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawEnvelope {
    #[serde(rename = "HEADER")]
    header: RawHeader,
    #[serde(rename = "BODY")]
    body: RawBody,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawHeader {
    #[serde(rename = "STATUS")]
    status: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawBody {
    #[serde(rename = "INDIATAXCONTEXT")]
    context: RawContext,
    #[serde(rename = "TAXREGISTRATION", default)]
    registrations: Vec<RawRegistration>,
    #[serde(rename = "VOUCHERTAX", default)]
    voucher_taxes: Vec<RawVoucherTax>,
}

#[derive(Deserialize)]
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
    #[serde(rename = "@FROMDATE")]
    from_date: String,
    #[serde(rename = "@TODATE")]
    to_date: String,
    #[serde(rename = "@TAXREGISTRATIONCOUNT")]
    registration_count: String,
    #[serde(rename = "@VOUCHERTAXCOUNT")]
    voucher_tax_count: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawRegistration {
    #[serde(rename = "@OWNERKIND")]
    owner_kind: String,
    #[serde(rename = "@OWNERGUID", default)]
    owner_guid: Option<String>,
    #[serde(rename = "@OWNERREMOTEID", default)]
    owner_remote_id: Option<String>,
    #[serde(rename = "@OWNERMASTERID", default)]
    owner_master_id: Option<String>,
    #[serde(rename = "@OWNERALTERID", default)]
    owner_alter_id: Option<String>,
    #[serde(rename = "REGISTRATIONTYPE")]
    registration_type: String,
    #[serde(rename = "GSTIN")]
    gstin: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawVoucherTax {
    #[serde(rename = "@VOUCHERGUID", default)]
    voucher_guid: Option<String>,
    #[serde(rename = "@VOUCHERREMOTEID", default)]
    voucher_remote_id: Option<String>,
    #[serde(rename = "@VOUCHERMASTERID", default)]
    voucher_master_id: Option<String>,
    #[serde(rename = "@VOUCHERALTERID", default)]
    voucher_alter_id: Option<String>,
    #[serde(rename = "@TAXROWORDINAL")]
    tax_row_ordinal: String,
    #[serde(rename = "PLACEOFSUPPLY")]
    place_of_supply: String,
    #[serde(rename = "ASSESSABLEVALUE")]
    assessable_value: String,
    #[serde(rename = "TAXCOMPONENT")]
    tax_component: String,
    #[serde(rename = "TAXRATE")]
    tax_rate: String,
    #[serde(rename = "TAXAMOUNT")]
    tax_amount: String,
}

pub fn parse_unbound_india_tax_observation(
    bytes: impl AsRef<[u8]>,
    limits: IndiaTaxObservationLimits,
) -> Result<UnboundIndiaTaxObservation, IndiaTaxObservationError> {
    validate_limits(limits)?;
    let encoded = bytes.as_ref();
    let decoded = decode_tally_text_bytes_limited(encoded, limits.max_encoded_bytes)
        .map_err(map_decode_error)?;
    if decoded.text.len() > limits.max_decoded_bytes {
        return Err(IndiaTaxObservationError::DecodedResponseTooLarge);
    }
    let fragments = scan_exact_grammar(&decoded.text, limits)?;
    let raw: RawEnvelope = quick_xml::de::from_str(&decoded.text)
        .map_err(|_| IndiaTaxObservationError::MalformedXml)?;
    if raw.header.status != "1" {
        return Err(IndiaTaxObservationError::ApplicationRejected);
    }
    if raw.body.context.schema != INDIA_TAX_OBSERVED_RAW_SCHEMA_V1
        || raw.body.context.profile != INDIA_TAX_OBSERVED_RAW_PROFILE_V1
        || raw.body.context.object_type != "INDIATAXOBSERVATION"
    {
        return Err(IndiaTaxObservationError::ProfileMismatch);
    }
    validate_text(
        &raw.body.context.company_guid,
        limits.max_field_bytes,
        false,
    )?;
    validate_date(&raw.body.context.from_date)?;
    validate_date(&raw.body.context.to_date)?;
    if raw.body.context.from_date > raw.body.context.to_date {
        return Err(IndiaTaxObservationError::InvalidValue);
    }
    let claimed_registration_count = parse_u64(&raw.body.context.registration_count, true)?;
    let claimed_voucher_tax_count = parse_u64(&raw.body.context.voucher_tax_count, true)?;
    if raw
        .body
        .registrations
        .len()
        .saturating_add(raw.body.voucher_taxes.len())
        > limits.max_records
    {
        return Err(IndiaTaxObservationError::ResourceLimitExceeded);
    }
    if claimed_registration_count != raw.body.registrations.len() as u64
        || claimed_voucher_tax_count != raw.body.voucher_taxes.len() as u64
    {
        return Err(IndiaTaxObservationError::CountMismatch);
    }
    if fragments.len() != raw.body.registrations.len() + raw.body.voucher_taxes.len() {
        return Err(IndiaTaxObservationError::WrongGrammar);
    }

    let mut fragment_iter = fragments.into_iter();
    let mut registration_keys = HashSet::new();
    let mut tax_registrations = Vec::with_capacity(raw.body.registrations.len());
    for registration in raw.body.registrations {
        let owner_kind = match registration.owner_kind.as_str() {
            "COMPANY" => ObservedTaxOwnerKind::Company,
            "LEDGER" => ObservedTaxOwnerKind::Ledger,
            _ => return Err(IndiaTaxObservationError::InvalidValue),
        };
        validate_optional_text(&registration.owner_guid, limits.max_field_bytes)?;
        validate_optional_text(&registration.owner_remote_id, limits.max_field_bytes)?;
        validate_optional_text(&registration.owner_master_id, limits.max_field_bytes)?;
        validate_optional_text(&registration.owner_alter_id, limits.max_field_bytes)?;
        let identities = ObservedIdentityCandidates {
            guid: registration.owner_guid,
            remote_id: registration.owner_remote_id,
            master_id: registration.owner_master_id,
        };
        match owner_kind {
            ObservedTaxOwnerKind::Company
                if identities.guid.as_deref() != Some(raw.body.context.company_guid.as_str())
                    || identities.remote_id.is_some()
                    || identities.master_id.is_some() =>
            {
                return Err(IndiaTaxObservationError::MissingIdentity)
            }
            ObservedTaxOwnerKind::Ledger
                if identities.guid.is_none()
                    && identities.remote_id.is_none()
                    && identities.master_id.is_none() =>
            {
                return Err(IndiaTaxObservationError::MissingIdentity)
            }
            _ => {}
        }
        validate_text(
            &registration.registration_type,
            limits.max_field_bytes,
            false,
        )?;
        validate_gstin_shape(&registration.gstin, limits.max_field_bytes)?;
        let key = (
            owner_kind,
            identities.clone(),
            registration.registration_type.clone(),
            registration.gstin.clone(),
        );
        if !registration_keys.insert(key) {
            return Err(IndiaTaxObservationError::DuplicateObservation);
        }
        let fragment = fragment_iter
            .next()
            .ok_or(IndiaTaxObservationError::WrongGrammar)?;
        tax_registrations.push(ObservedRawTaxRegistration {
            owner_kind,
            owner_identities: identities,
            owner_alter_id: registration.owner_alter_id,
            registration_type: registration.registration_type,
            gstin: registration.gstin,
            raw_fragment_sha256: hash_domain(FRAGMENT_HASH_DOMAIN, fragment.as_bytes()),
        });
    }

    let mut voucher_keys = HashSet::new();
    let mut voucher_taxes = Vec::with_capacity(raw.body.voucher_taxes.len());
    for tax in raw.body.voucher_taxes {
        validate_optional_text(&tax.voucher_guid, limits.max_field_bytes)?;
        validate_optional_text(&tax.voucher_remote_id, limits.max_field_bytes)?;
        validate_optional_text(&tax.voucher_master_id, limits.max_field_bytes)?;
        validate_optional_text(&tax.voucher_alter_id, limits.max_field_bytes)?;
        let identities = ObservedIdentityCandidates {
            guid: tax.voucher_guid,
            remote_id: tax.voucher_remote_id,
            master_id: tax.voucher_master_id,
        };
        if identities.guid.is_none()
            && identities.remote_id.is_none()
            && identities.master_id.is_none()
        {
            return Err(IndiaTaxObservationError::MissingIdentity);
        }
        let ordinal = parse_u64(&tax.tax_row_ordinal, false)?;
        validate_text(&tax.place_of_supply, limits.max_field_bytes, false)?;
        validate_text(&tax.tax_component, limits.max_field_bytes, false)?;
        validate_decimal(&tax.assessable_value, limits.max_field_bytes, true)?;
        validate_decimal(&tax.tax_rate, limits.max_field_bytes, false)?;
        validate_decimal(&tax.tax_amount, limits.max_field_bytes, true)?;
        if !voucher_keys.insert((identities.clone(), ordinal)) {
            return Err(IndiaTaxObservationError::DuplicateObservation);
        }
        let fragment = fragment_iter
            .next()
            .ok_or(IndiaTaxObservationError::WrongGrammar)?;
        voucher_taxes.push(ObservedRawVoucherTax {
            voucher_identities: identities,
            voucher_alter_id: tax.voucher_alter_id,
            tax_row_ordinal: ordinal,
            place_of_supply: tax.place_of_supply,
            assessable_value: tax.assessable_value,
            tax_component: tax.tax_component,
            tax_rate: tax.tax_rate,
            tax_amount: tax.tax_amount,
            raw_fragment_sha256: hash_domain(FRAGMENT_HASH_DOMAIN, fragment.as_bytes()),
        });
    }

    Ok(UnboundIndiaTaxObservation {
        tax_registrations,
        voucher_taxes,
        evidence: UnboundIndiaTaxEvidence {
            profile_id: INDIA_TAX_OBSERVED_RAW_PROFILE_V1,
            binding: IndiaTaxObservationBinding::UnboundNoRequestArtifact,
            count_authority: IndiaTaxCountAuthority::ResponseInternalOnly,
            encoding: decoded.encoding,
            encoded_bytes: encoded.len() as u64,
            decoded_bytes: decoded.text.len() as u64,
            claimed_company_guid: raw.body.context.company_guid,
            claimed_from_yyyymmdd: raw.body.context.from_date,
            claimed_to_yyyymmdd: raw.body.context.to_date,
            claimed_registration_count,
            claimed_voucher_tax_count,
            response_sha256: hash_domain(RESPONSE_HASH_DOMAIN, decoded.text.as_bytes()),
        },
    })
}

fn validate_limits(limits: IndiaTaxObservationLimits) -> Result<(), IndiaTaxObservationError> {
    if limits.max_encoded_bytes == 0
        || limits.max_decoded_bytes == 0
        || limits.max_field_bytes == 0
        || limits.max_nodes == 0
        || limits.max_depth < 4
        || limits.max_attributes == 0
    {
        return Err(IndiaTaxObservationError::InvalidLimits);
    }
    Ok(())
}

fn map_decode_error(error: TallyTextDecodeError) -> IndiaTaxObservationError {
    match error {
        TallyTextDecodeError::TooLarge => IndiaTaxObservationError::ResponseTooLarge,
        _ => IndiaTaxObservationError::InvalidEncoding,
    }
}

fn validate_optional_text(
    value: &Option<String>,
    limit: usize,
) -> Result<(), IndiaTaxObservationError> {
    if let Some(value) = value {
        validate_text(value, limit, false)?;
    }
    Ok(())
}

fn validate_text(
    value: &str,
    limit: usize,
    allow_empty: bool,
) -> Result<(), IndiaTaxObservationError> {
    if value.len() > limit
        || (!allow_empty && value.is_empty())
        || value.chars().any(|character| character.is_control())
    {
        return Err(IndiaTaxObservationError::InvalidValue);
    }
    Ok(())
}

fn validate_gstin_shape(value: &str, limit: usize) -> Result<(), IndiaTaxObservationError> {
    validate_text(value, limit, false)?;
    if value.len() != 15
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_uppercase() || byte.is_ascii_digit())
    {
        return Err(IndiaTaxObservationError::InvalidValue);
    }
    Ok(())
}

fn parse_u64(value: &str, allow_zero: bool) -> Result<u64, IndiaTaxObservationError> {
    if value.is_empty() || !value.bytes().all(|byte| byte.is_ascii_digit()) {
        return Err(IndiaTaxObservationError::InvalidValue);
    }
    let parsed = value
        .parse::<u64>()
        .map_err(|_| IndiaTaxObservationError::InvalidValue)?;
    if !allow_zero && parsed == 0 {
        return Err(IndiaTaxObservationError::InvalidValue);
    }
    Ok(parsed)
}

fn validate_date(value: &str) -> Result<(), IndiaTaxObservationError> {
    if value.len() != 8 || !value.bytes().all(|byte| byte.is_ascii_digit()) {
        return Err(IndiaTaxObservationError::InvalidValue);
    }
    let year = value[0..4]
        .parse::<u32>()
        .map_err(|_| IndiaTaxObservationError::InvalidValue)?;
    let month = value[4..6]
        .parse::<u32>()
        .map_err(|_| IndiaTaxObservationError::InvalidValue)?;
    let day = value[6..8]
        .parse::<u32>()
        .map_err(|_| IndiaTaxObservationError::InvalidValue)?;
    let leap = year % 4 == 0 && (year % 100 != 0 || year % 400 == 0);
    let max_day = match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 if leap => 29,
        2 => 28,
        _ => return Err(IndiaTaxObservationError::InvalidValue),
    };
    if day == 0 || day > max_day {
        return Err(IndiaTaxObservationError::InvalidValue);
    }
    Ok(())
}

fn validate_decimal(
    value: &str,
    limit: usize,
    signed: bool,
) -> Result<(), IndiaTaxObservationError> {
    validate_text(value, limit, false)?;
    let digits = if signed {
        value.strip_prefix('-').unwrap_or(value)
    } else {
        value
    };
    if digits.is_empty() || value.starts_with('+') || (!signed && value.starts_with('-')) {
        return Err(IndiaTaxObservationError::InvalidValue);
    }
    let mut parts = digits.split('.');
    let whole = parts.next().unwrap_or_default();
    let fraction = parts.next();
    if parts.next().is_some()
        || whole.is_empty()
        || !whole.bytes().all(|byte| byte.is_ascii_digit())
        || (whole.len() > 1 && whole.starts_with('0'))
        || fraction
            .is_some_and(|part| part.is_empty() || !part.bytes().all(|byte| byte.is_ascii_digit()))
    {
        return Err(IndiaTaxObservationError::InvalidValue);
    }
    Ok(())
}

fn hash_domain(domain: &[u8], bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(domain);
    hasher.update(bytes);
    let digest = hasher.finalize();
    let mut encoded = String::with_capacity(digest.len() * 2);
    for byte in digest {
        use std::fmt::Write as _;
        let _ = write!(encoded, "{byte:02x}");
    }
    encoded
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum RowKind {
    Registration,
    VoucherTax,
}

fn scan_exact_grammar(
    xml: &str,
    limits: IndiaTaxObservationLimits,
) -> Result<Vec<String>, IndiaTaxObservationError> {
    let mut reader = Reader::from_str(xml);
    reader.config_mut().trim_text(false);
    let mut stack = Vec::<String>::new();
    let mut nodes = 0usize;
    let mut fragments = Vec::new();
    let mut row_start = None::<(RowKind, usize)>;
    let mut body_stage = 0u8;
    let mut header_status_seen = false;
    let mut context_seen = false;
    let mut current_child_index = 0usize;
    let mut root_closed = false;
    loop {
        let before = reader.buffer_position() as usize;
        let event = reader
            .read_event()
            .map_err(|_| IndiaTaxObservationError::MalformedXml)?;
        nodes = nodes.saturating_add(1);
        if nodes > limits.max_nodes {
            return Err(IndiaTaxObservationError::ResourceLimitExceeded);
        }
        match event {
            Event::Start(start) => {
                if root_closed {
                    return Err(IndiaTaxObservationError::WrongGrammar);
                }
                let name = event_name(start.name().as_ref())?;
                validate_attributes(&start, &name, limits.max_attributes, limits.max_field_bytes)?;
                validate_start(
                    &stack,
                    &name,
                    &mut body_stage,
                    &mut header_status_seen,
                    &mut context_seen,
                    &mut current_child_index,
                )?;
                if name == "TAXREGISTRATION" {
                    row_start = Some((RowKind::Registration, before));
                }
                if name == "VOUCHERTAX" {
                    row_start = Some((RowKind::VoucherTax, before));
                }
                stack.push(name);
                if stack.len() > limits.max_depth {
                    return Err(IndiaTaxObservationError::ResourceLimitExceeded);
                }
            }
            Event::Empty(empty) => {
                if root_closed {
                    return Err(IndiaTaxObservationError::WrongGrammar);
                }
                let name = event_name(empty.name().as_ref())?;
                validate_attributes(&empty, &name, limits.max_attributes, limits.max_field_bytes)?;
                if stack.as_slice() != ["ENVELOPE", "BODY"]
                    || name != "INDIATAXCONTEXT"
                    || context_seen
                    || body_stage != 0
                {
                    return Err(IndiaTaxObservationError::WrongGrammar);
                }
                context_seen = true;
                body_stage = 1;
            }
            Event::End(end) => {
                let name = event_name(end.name().as_ref())?;
                if stack.last().map(String::as_str) != Some(name.as_str()) {
                    return Err(IndiaTaxObservationError::WrongGrammar);
                }
                if name == "TAXREGISTRATION" || name == "VOUCHERTAX" {
                    let expected = if name == "TAXREGISTRATION" {
                        RowKind::Registration
                    } else {
                        RowKind::VoucherTax
                    };
                    let (kind, start) = row_start
                        .take()
                        .ok_or(IndiaTaxObservationError::WrongGrammar)?;
                    if kind != expected {
                        return Err(IndiaTaxObservationError::WrongGrammar);
                    }
                    let after = reader.buffer_position() as usize;
                    fragments.push(
                        xml.get(start..after)
                            .ok_or(IndiaTaxObservationError::MalformedXml)?
                            .to_owned(),
                    );
                    current_child_index = 0;
                }
                stack.pop();
                if name == "ENVELOPE" {
                    root_closed = true;
                }
            }
            Event::Text(text) => {
                let decoded = text
                    .decode()
                    .map_err(|_| IndiaTaxObservationError::MalformedXml)?;
                if decoded.len() > limits.max_field_bytes {
                    return Err(IndiaTaxObservationError::ResourceLimitExceeded);
                }
                let leaf = stack.last().map(String::as_str);
                let allowed_leaf = matches!(
                    leaf,
                    Some(
                        "STATUS"
                            | "REGISTRATIONTYPE"
                            | "GSTIN"
                            | "PLACEOFSUPPLY"
                            | "ASSESSABLEVALUE"
                            | "TAXCOMPONENT"
                            | "TAXRATE"
                            | "TAXAMOUNT"
                    )
                );
                if !allowed_leaf && !decoded.chars().all(char::is_whitespace) {
                    return Err(IndiaTaxObservationError::WrongGrammar);
                }
            }
            Event::Decl(_) => {
                if nodes != 1 || !stack.is_empty() {
                    return Err(IndiaTaxObservationError::WrongGrammar);
                }
            }
            Event::Eof => break,
            Event::Comment(_)
            | Event::CData(_)
            | Event::PI(_)
            | Event::DocType(_)
            | Event::GeneralRef(_) => return Err(IndiaTaxObservationError::WrongGrammar),
        }
    }
    if !root_closed
        || !stack.is_empty()
        || !header_status_seen
        || !context_seen
        || row_start.is_some()
    {
        return Err(IndiaTaxObservationError::WrongGrammar);
    }
    Ok(fragments)
}

fn event_name(bytes: &[u8]) -> Result<String, IndiaTaxObservationError> {
    let name = std::str::from_utf8(bytes).map_err(|_| IndiaTaxObservationError::MalformedXml)?;
    if !name.bytes().all(|byte| byte.is_ascii_uppercase()) {
        return Err(IndiaTaxObservationError::WrongGrammar);
    }
    Ok(name.to_owned())
}

fn validate_start(
    stack: &[String],
    name: &str,
    body_stage: &mut u8,
    header_status_seen: &mut bool,
    context_seen: &mut bool,
    child_index: &mut usize,
) -> Result<(), IndiaTaxObservationError> {
    let path = stack.iter().map(String::as_str).collect::<Vec<_>>();
    match (path.as_slice(), name) {
        ([], "ENVELOPE") => {}
        (["ENVELOPE"], "HEADER") => {}
        (["ENVELOPE", "HEADER"], "STATUS") if !*header_status_seen => *header_status_seen = true,
        (["ENVELOPE"], "BODY") if *header_status_seen => {}
        (["ENVELOPE", "BODY"], "INDIATAXCONTEXT") if !*context_seen && *body_stage == 0 => {
            *context_seen = true;
            *body_stage = 1;
        }
        (["ENVELOPE", "BODY"], "TAXREGISTRATION") if *context_seen && *body_stage <= 1 => {
            *body_stage = 1;
            *child_index = 0;
        }
        (["ENVELOPE", "BODY"], "VOUCHERTAX") if *context_seen => {
            *body_stage = 2;
            *child_index = 0;
        }
        (["ENVELOPE", "BODY", "TAXREGISTRATION"], child) => {
            let expected = ["REGISTRATIONTYPE", "GSTIN"];
            if expected.get(*child_index) != Some(&child) {
                return Err(IndiaTaxObservationError::WrongGrammar);
            }
            *child_index += 1;
        }
        (["ENVELOPE", "BODY", "VOUCHERTAX"], child) => {
            let expected = [
                "PLACEOFSUPPLY",
                "ASSESSABLEVALUE",
                "TAXCOMPONENT",
                "TAXRATE",
                "TAXAMOUNT",
            ];
            if expected.get(*child_index) != Some(&child) {
                return Err(IndiaTaxObservationError::WrongGrammar);
            }
            *child_index += 1;
        }
        _ => return Err(IndiaTaxObservationError::WrongGrammar),
    }
    Ok(())
}

fn validate_attributes(
    start: &quick_xml::events::BytesStart<'_>,
    element: &str,
    max_attributes: usize,
    max_field_bytes: usize,
) -> Result<(), IndiaTaxObservationError> {
    let allowed: &[&str] = match element {
        "INDIATAXCONTEXT" => &[
            "SCHEMA",
            "PROFILE",
            "OBJECTTYPE",
            "COMPANYGUID",
            "FROMDATE",
            "TODATE",
            "TAXREGISTRATIONCOUNT",
            "VOUCHERTAXCOUNT",
        ],
        "TAXREGISTRATION" => &[
            "OWNERKIND",
            "OWNERGUID",
            "OWNERREMOTEID",
            "OWNERMASTERID",
            "OWNERALTERID",
        ],
        "VOUCHERTAX" => &[
            "VOUCHERGUID",
            "VOUCHERREMOTEID",
            "VOUCHERMASTERID",
            "VOUCHERALTERID",
            "TAXROWORDINAL",
        ],
        _ => &[],
    };
    let mut seen = HashSet::new();
    let mut count = 0usize;
    for attribute in start.attributes().with_checks(false) {
        let attribute = attribute.map_err(|_| IndiaTaxObservationError::MalformedXml)?;
        count += 1;
        if count > max_attributes {
            return Err(IndiaTaxObservationError::ResourceLimitExceeded);
        }
        let key = std::str::from_utf8(attribute.key.as_ref())
            .map_err(|_| IndiaTaxObservationError::MalformedXml)?;
        let folded = key.to_ascii_uppercase();
        if key != folded || !seen.insert(folded.clone()) {
            return Err(IndiaTaxObservationError::DuplicateField);
        }
        if !allowed.contains(&folded.as_str()) {
            return Err(IndiaTaxObservationError::WrongGrammar);
        }
        if attribute.value.len() > max_field_bytes {
            return Err(IndiaTaxObservationError::ResourceLimitExceeded);
        }
    }
    if allowed.is_empty() && count != 0 {
        return Err(IndiaTaxObservationError::WrongGrammar);
    }
    Ok(())
}
