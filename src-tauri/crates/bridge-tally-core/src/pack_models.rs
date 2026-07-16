use crate::{
    CapabilityPackId, ExactDecimal, PackSchemaVersion, ReadWindow, SourceIdentity, TallyError,
};
use serde::{Deserialize, Deserializer, Serialize};
use sha2::{Digest, Sha256};
use std::collections::BTreeSet;

const MAX_CANONICAL_TEXT_BYTES: usize = 512;
const MAX_SOURCE_ID_BYTES: usize = 512;
const MAX_ALTER_ID_BYTES: usize = 128;

/// A structurally valid GSTIN-shaped identifier observed from Tally.
///
/// This type does not claim that the registration exists, is active, belongs to the company, or
/// passed an external GST portal lookup; those are separate evidence states.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(transparent)]
pub struct CanonicalText(String);

impl CanonicalText {
    pub fn parse(value: impl Into<String>) -> Result<Self, TallyError> {
        let value = value.into();
        validate_text(&value, "canonical_text_invalid")?;
        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl<'de> Deserialize<'de> for CanonicalText {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        Self::parse(value).map_err(serde::de::Error::custom)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(transparent)]
pub struct Gstin(String);

impl Gstin {
    pub fn parse(value: impl Into<String>) -> Result<Self, TallyError> {
        let value = value.into();
        let bytes = value.as_bytes();
        let valid = bytes.len() == 15
            && bytes[0..2].iter().all(u8::is_ascii_digit)
            && bytes[2..7].iter().all(u8::is_ascii_uppercase)
            && bytes[7..11].iter().all(u8::is_ascii_digit)
            && bytes[11].is_ascii_uppercase()
            && (bytes[12].is_ascii_uppercase() || bytes[12].is_ascii_digit())
            && bytes[13] == b'Z'
            && (bytes[14].is_ascii_uppercase() || bytes[14].is_ascii_digit());
        if !valid {
            return Err(invalid_data("gstin_invalid"));
        }
        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(transparent)]
pub struct NonNegativeExactDecimal(ExactDecimal);

impl NonNegativeExactDecimal {
    pub fn parse(value: impl Into<String>) -> Result<Self, TallyError> {
        let value = ExactDecimal::parse(value)?;
        if value.as_str().starts_with('-') {
            return Err(invalid_data("negative_exact_decimal"));
        }
        Ok(Self(value))
    }

    pub fn as_exact_decimal(&self) -> &ExactDecimal {
        &self.0
    }
}

impl<'de> Deserialize<'de> for NonNegativeExactDecimal {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        Self::parse(value).map_err(serde::de::Error::custom)
    }
}

impl<'de> Deserialize<'de> for Gstin {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        Self::parse(value).map_err(serde::de::Error::custom)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SourceCountScope {
    /// The source explicitly reported the count for the complete object/query
    /// scope, not merely for the records parsed in this response.
    Complete,
    Window,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct SourceReportedCountEvidence {
    pub object_type: CanonicalText,
    pub query_profile: CanonicalText,
    /// Stable fingerprint of the exact company/filter/window count scope.
    pub source_scope_fingerprint: CanonicalText,
    pub source_count_scope: SourceCountScope,
    pub source_reported_count: u64,
}

/// Identifies which Tally-origin identity field produced a canonical record's
/// source ID. Consumers must preserve the selected field rather than guessing
/// that every identifier is a remote ID.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SourceIdentityKind {
    Guid,
    RemoteId,
    MasterId,
    Fallback,
}

/// SHA-256 of the exact source payload from which the record was parsed.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(transparent)]
pub struct RawSourceSha256(String);

impl RawSourceSha256 {
    pub fn parse(value: impl Into<String>) -> Result<Self, TallyError> {
        let value = value.into();
        if !is_lower_sha256(&value) {
            return Err(invalid_data("raw_source_sha256_invalid"));
        }
        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl<'de> Deserialize<'de> for RawSourceSha256 {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        Self::parse(value).map_err(serde::de::Error::custom)
    }
}

/// Source alter identifier observed with a record, retained as an opaque
/// printable token rather than interpreted as a number.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(transparent)]
pub struct SourceAlterId(String);

impl SourceAlterId {
    pub fn parse(value: impl Into<String>) -> Result<Self, TallyError> {
        let value = value.into();
        if value.is_empty()
            || value.len() > MAX_ALTER_ID_BYTES
            || value.trim() != value
            || !value.bytes().all(|byte| {
                byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-' | b'.' | b':')
            })
        {
            return Err(invalid_data("source_alter_id_invalid"));
        }
        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl<'de> Deserialize<'de> for SourceAlterId {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        Self::parse(value).map_err(serde::de::Error::custom)
    }
}

/// Per-record provenance supplied by the connector. The object type and
/// source ID bind this evidence to exactly one canonical record.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct SourceRecordEvidence {
    pub object_type: CanonicalText,
    pub source_id: SourceRecordId,
    pub identity_kind: SourceIdentityKind,
    #[serde(default)]
    pub observed_identities: ObservedSourceIdentities,
    pub raw_source_sha256: RawSourceSha256,
    pub alter_id: Option<SourceAlterId>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ObservedSourceIdentities {
    pub guid: Option<SourceRecordId>,
    pub remote_id: Option<SourceRecordId>,
    pub master_id: Option<SourceRecordId>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct SourceCountScopeDescriptor {
    pub source_identity: SourceIdentity,
    pub pack: CapabilityPackId,
    pub pack_schema_version: PackSchemaVersion,
    pub object_type: CanonicalText,
    pub query_profile: CanonicalText,
    pub filters_sha256: CanonicalText,
    /// Must be `None` for `Complete` and the exact requested window for
    /// `Window`; the helper rejects the opposite combinations.
    pub window: Option<ReadWindow>,
}

#[derive(Serialize)]
struct SourceCountScopeFingerprintPreimage<'a> {
    contract: &'static str,
    source_identity: &'a SourceIdentity,
    pack: CapabilityPackId,
    pack_schema_version: PackSchemaVersion,
    object_type: &'a CanonicalText,
    query_profile: &'a CanonicalText,
    filters_sha256: &'a CanonicalText,
    window: &'a Option<ReadWindow>,
}

pub fn source_count_scope_fingerprint(
    descriptor: &SourceCountScopeDescriptor,
    scope: SourceCountScope,
) -> Result<CanonicalText, TallyError> {
    validate_scope_descriptor(descriptor, scope)?;
    let preimage = SourceCountScopeFingerprintPreimage {
        contract: "bridge_tally_source_count_scope_v1",
        source_identity: &descriptor.source_identity,
        pack: descriptor.pack,
        pack_schema_version: descriptor.pack_schema_version,
        object_type: &descriptor.object_type,
        query_profile: &descriptor.query_profile,
        filters_sha256: &descriptor.filters_sha256,
        window: &descriptor.window,
    };
    let canonical = serde_json::to_vec(&preimage)
        .map_err(|_| invalid_data("source_count_scope_serialization_failed"))?;
    let digest = Sha256::digest(canonical);
    CanonicalText::parse(hex_lower(&digest))
}

impl SourceReportedCountEvidence {
    pub fn matches_scope_descriptor(
        &self,
        descriptor: &SourceCountScopeDescriptor,
    ) -> Result<bool, TallyError> {
        if self.object_type != descriptor.object_type
            || self.query_profile != descriptor.query_profile
        {
            return Ok(false);
        }
        Ok(self.source_scope_fingerprint
            == source_count_scope_fingerprint(descriptor, self.source_count_scope)?)
    }
}

/// Canonical response envelope for a single requested pack window. Counts and
/// per-record provenance are optional because Bridge must never invent source
/// evidence. Missing record evidence is valid for synthetic/test producers,
/// but production reconciliation must classify that window as partial.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct PackWindow<T> {
    pub batch: T,
    pub source_counts: Option<Vec<SourceReportedCountEvidence>>,
    pub record_evidence: Option<Vec<SourceRecordEvidence>>,
}

impl<T> PackWindow<T> {
    pub fn without_source_count_evidence(batch: T) -> Self {
        Self {
            batch,
            source_counts: None,
            record_evidence: None,
        }
    }

    pub fn validate_source_count_evidence(&self) -> Result<(), TallyError> {
        let Some(source_counts) = &self.source_counts else {
            return Ok(());
        };
        if source_counts.is_empty() {
            return Err(invalid_data("source_count_evidence_empty"));
        }
        let mut scopes = BTreeSet::new();
        for evidence in source_counts {
            let key = (
                evidence.object_type.as_str(),
                evidence.query_profile.as_str(),
                evidence.source_scope_fingerprint.as_str(),
                evidence.source_count_scope,
            );
            if !scopes.insert(key) {
                return Err(invalid_data("source_count_evidence_duplicate_scope"));
            }
        }
        Ok(())
    }

    pub fn validate_record_evidence(&self) -> Result<(), TallyError> {
        let Some(record_evidence) = &self.record_evidence else {
            return Ok(());
        };
        // Empty evidence is meaningful for a proven-empty pack window; the pack-specific
        // binding check still requires it to match an empty canonical record set exactly.
        let mut records = BTreeSet::new();
        for evidence in record_evidence {
            let selected = match evidence.identity_kind {
                SourceIdentityKind::Guid => evidence.observed_identities.guid.as_ref(),
                SourceIdentityKind::RemoteId => evidence.observed_identities.remote_id.as_ref(),
                SourceIdentityKind::MasterId => evidence.observed_identities.master_id.as_ref(),
                SourceIdentityKind::Fallback => {
                    if evidence.observed_identities != ObservedSourceIdentities::default() {
                        return Err(invalid_data("fallback_identity_claimed_native_ids"));
                    }
                    Some(&evidence.source_id)
                }
            };
            if selected != Some(&evidence.source_id) {
                return Err(invalid_data("source_record_primary_identity_mismatch"));
            }
            if !records.insert((evidence.object_type.as_str(), evidence.source_id.as_str())) {
                return Err(invalid_data("source_record_evidence_duplicate_record"));
            }
        }
        Ok(())
    }
}

/// Stable Tally-origin identity or reference. It is intentionally serialized
/// as a string so the wire contract stays compact while construction and
/// deserialization remain fail-closed.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(transparent)]
pub struct SourceRecordId(String);

impl SourceRecordId {
    pub fn parse(value: impl Into<String>) -> Result<Self, TallyError> {
        let value = value.into();
        if value.is_empty()
            || value.len() > MAX_SOURCE_ID_BYTES
            || value.trim() != value
            || value.chars().any(char::is_control)
        {
            return Err(invalid_data("invalid_source_record_id"));
        }
        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl<'de> Deserialize<'de> for SourceRecordId {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        Self::parse(value).map_err(serde::de::Error::custom)
    }
}

/// A validated Gregorian calendar date in Tally's canonical YYYYMMDD form.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(transparent)]
pub struct TallyDate(String);

impl TallyDate {
    pub fn parse(value: impl Into<String>) -> Result<Self, TallyError> {
        let value = value.into();
        if !is_valid_yyyymmdd(&value) {
            return Err(invalid_data("invalid_tally_date"));
        }
        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl<'de> Deserialize<'de> for TallyDate {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        Self::parse(value).map_err(serde::de::Error::custom)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TaxRegistrationOwnerKind {
    Company,
    Ledger,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct TaxRegistrationRecord {
    pub source_id: SourceRecordId,
    pub owner_kind: TaxRegistrationOwnerKind,
    pub owner_source_id: SourceRecordId,
    pub registration_type: CanonicalText,
    pub gstin: Gstin,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct VoucherTaxRecord {
    pub source_id: SourceRecordId,
    pub voucher_source_id: SourceRecordId,
    pub place_of_supply: CanonicalText,
    pub assessable_value: ExactDecimal,
    pub tax_component: CanonicalText,
    pub tax_rate: NonNegativeExactDecimal,
    pub tax_amount: ExactDecimal,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct IndiaTaxBatch {
    pub tax_registrations: Vec<TaxRegistrationRecord>,
    pub voucher_taxes: Vec<VoucherTaxRecord>,
}

impl IndiaTaxBatch {
    pub fn validate(&self) -> Result<(), TallyError> {
        ensure_unique_source_ids(
            self.tax_registrations
                .iter()
                .map(|record| &record.source_id),
            "duplicate_tax_registration_source_id",
        )?;
        ensure_unique_source_ids(
            self.voucher_taxes.iter().map(|record| &record.source_id),
            "duplicate_voucher_tax_source_id",
        )?;
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum BillReferenceKind {
    Advance,
    AgainstReference,
    NewReference,
    OnAccount,
    Unclassified,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct BillReference {
    pub kind: BillReferenceKind,
    pub name: Option<CanonicalText>,
    /// Preserved only when `kind` is `Unclassified`; known values never retain
    /// a second, potentially contradictory representation.
    pub raw_kind: Option<CanonicalText>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(tag = "origin", rename_all = "snake_case")]
pub enum BillAllocationOrigin {
    Voucher {
        voucher_source_id: SourceRecordId,
        party_entry_source_id: SourceRecordId,
    },
    LedgerOpening,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(tag = "origin", rename_all = "snake_case")]
pub enum OutstandingOrigin {
    Voucher {
        voucher_source_id: Option<SourceRecordId>,
    },
    LedgerOpening,
    Unavailable,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DerivedIdentityBasis {
    ParentOrdinal,
    MutableReferenceOrdinal,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Deserialize, Serialize)]
#[serde(tag = "basis", rename_all = "snake_case")]
pub enum CurrencyBasis {
    CompanyBase { currency: CanonicalText },
    ObservedSource { currency: CanonicalText },
    Unspecified,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum OutstandingDirection {
    Receivable,
    Payable,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum BillDueDateEvidence {
    Explicit,
    DerivedFromCreditPeriod,
    DefaultedToBillDate,
    Unavailable,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum BillWiseState {
    EnabledObserved,
    CompanyDisabledObserved,
    PartyDisabledObserved,
    UnsupportedForeignCurrencyLedgerObserved,
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum BillsCoverageState {
    ObservedCompleteScope,
    ObservedPartial,
    Drifted,
    Truncated,
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum FetchBracketState {
    StableObserved,
    ChangedObserved,
    Unavailable,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct BillAllocationRecord {
    pub source_id: SourceRecordId,
    pub identity_basis: DerivedIdentityBasis,
    pub origin: BillAllocationOrigin,
    pub reference: BillReference,
    pub bill_date_yyyymmdd: Option<TallyDate>,
    pub effective_date_yyyymmdd: Option<TallyDate>,
    pub due_date_yyyymmdd: Option<TallyDate>,
    pub due_date_evidence: BillDueDateEvidence,
    pub amount: ExactDecimal,
    pub observed_polarity: Option<crate::LedgerEntryPolarity>,
    pub currency_basis: CurrencyBasis,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct OutstandingObservation {
    pub source_id: SourceRecordId,
    pub identity_basis: DerivedIdentityBasis,
    pub origin: OutstandingOrigin,
    pub reference: BillReference,
    pub bill_date_yyyymmdd: Option<TallyDate>,
    pub effective_date_yyyymmdd: Option<TallyDate>,
    pub due_date_yyyymmdd: Option<TallyDate>,
    pub due_date_evidence: BillDueDateEvidence,
    pub opening_amount: Option<ExactDecimal>,
    pub pending_amount: ExactDecimal,
    pub observed_polarity: Option<crate::LedgerEntryPolarity>,
    pub source_reported_overdue_days: Option<u32>,
    pub currency_basis: CurrencyBasis,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct PartyOutstandingFacts {
    pub source_identity: SourceIdentity,
    pub party_ledger_source_id: SourceRecordId,
    pub report_as_of_yyyymmdd: TallyDate,
    pub direction: OutstandingDirection,
    pub bill_wise_state: BillWiseState,
    pub allocation_coverage: BillsCoverageState,
    pub outstanding_coverage: BillsCoverageState,
    pub fetch_bracket: FetchBracketState,
    pub query_profile: CanonicalText,
    pub source_scope_fingerprint: CanonicalText,
    pub source_reported_allocation_count: u64,
    pub source_reported_outstanding_count: u64,
    pub allocations: Vec<BillAllocationRecord>,
    pub outstanding: Vec<OutstandingObservation>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct BillsAndPaymentsBatch {
    pub parties: Vec<PartyOutstandingFacts>,
}

pub fn derive_bill_allocation_source_id(
    company_fingerprint: &CanonicalText,
    party_ledger_source_id: &SourceRecordId,
    origin_parent_source_id: &SourceRecordId,
    allocation_ordinal: u64,
) -> Result<SourceRecordId, TallyError> {
    if allocation_ordinal == 0 {
        return Err(invalid_data("bill_allocation_ordinal_invalid"));
    }
    let mut digest = Sha256::new();
    digest.update(b"bridge-tally-bill-allocation-id-v1\0");
    digest.update(company_fingerprint.as_str().as_bytes());
    digest.update(b"\0");
    digest.update(party_ledger_source_id.as_str().as_bytes());
    digest.update(b"\0");
    digest.update(origin_parent_source_id.as_str().as_bytes());
    digest.update(b"\0");
    digest.update(allocation_ordinal.to_be_bytes());
    SourceRecordId::parse(format!("bill-allocation:{}", hex_lower(&digest.finalize())))
}

pub fn derive_bill_outstanding_source_id(
    exact_scope_fingerprint: &CanonicalText,
    row_ordinal: u64,
) -> Result<SourceRecordId, TallyError> {
    if row_ordinal == 0 {
        return Err(invalid_data("bill_outstanding_ordinal_invalid"));
    }
    let mut digest = Sha256::new();
    digest.update(b"bridge-tally-outstanding-observation-id-v1\0");
    digest.update(exact_scope_fingerprint.as_str().as_bytes());
    digest.update(b"\0");
    digest.update(row_ordinal.to_be_bytes());
    SourceRecordId::parse(format!(
        "bill-outstanding:{}",
        hex_lower(&digest.finalize())
    ))
}

impl BillsAndPaymentsBatch {
    pub fn validate(&self) -> Result<(), TallyError> {
        ensure_unique_source_ids(
            self.parties
                .iter()
                .map(|record| &record.party_ledger_source_id),
            "duplicate_party_outstanding_scope",
        )?;
        let mut allocation_ids = BTreeSet::new();
        let mut outstanding_ids = BTreeSet::new();
        let expected_source_identity = self.parties.first().map(|party| &party.source_identity);
        for party in &self.parties {
            validate_source_identity(
                &party.source_identity,
                "bills_source_identity_invalid",
                "bills_source_fingerprint_invalid",
            )?;
            if Some(&party.source_identity) != expected_source_identity {
                return Err(invalid_data("bills_mixed_source_identity"));
            }
            validate_sha256(
                party.source_scope_fingerprint.as_str(),
                "bills_source_scope_fingerprint_invalid",
            )?;
            validate_source_count(
                party.allocation_coverage,
                party.source_reported_allocation_count,
                party.allocations.len(),
                "bill_allocation_source_count_invalid",
            )?;
            validate_source_count(
                party.outstanding_coverage,
                party.source_reported_outstanding_count,
                party.outstanding.len(),
                "bill_outstanding_source_count_invalid",
            )?;
            for record in &party.allocations {
                if !allocation_ids.insert(&record.source_id) {
                    return Err(invalid_data("duplicate_bill_allocation_source_id"));
                }
                if record.identity_basis == DerivedIdentityBasis::MutableReferenceOrdinal {
                    return Err(invalid_data("bill_allocation_identity_basis_unsafe"));
                }
                validate_bill_reference(&record.reference)?;
                validate_due_date(record.due_date_yyyymmdd.as_ref(), record.due_date_evidence)?;
            }
            for record in &party.outstanding {
                if !outstanding_ids.insert(&record.source_id) {
                    return Err(invalid_data("duplicate_bill_outstanding_source_id"));
                }
                if record.identity_basis == DerivedIdentityBasis::MutableReferenceOrdinal {
                    return Err(invalid_data("bill_outstanding_identity_basis_unsafe"));
                }
                validate_bill_reference(&record.reference)?;
                validate_due_date(record.due_date_yyyymmdd.as_ref(), record.due_date_evidence)?;
            }
        }
        Ok(())
    }
}

fn validate_source_count(
    coverage: BillsCoverageState,
    source_reported_count: u64,
    parsed_count: usize,
    code: &'static str,
) -> Result<(), TallyError> {
    let parsed_count = u64::try_from(parsed_count).map_err(|_| invalid_data(code))?;
    let valid = match coverage {
        BillsCoverageState::ObservedCompleteScope => source_reported_count == parsed_count,
        BillsCoverageState::ObservedPartial
        | BillsCoverageState::Drifted
        | BillsCoverageState::Truncated
        | BillsCoverageState::Unknown => source_reported_count >= parsed_count,
    };
    if valid {
        Ok(())
    } else {
        Err(invalid_data(code))
    }
}

fn validate_bill_reference(reference: &BillReference) -> Result<(), TallyError> {
    let valid = match reference.kind {
        BillReferenceKind::OnAccount => reference.name.is_none() && reference.raw_kind.is_none(),
        BillReferenceKind::Unclassified => reference.raw_kind.is_some(),
        BillReferenceKind::Advance
        | BillReferenceKind::AgainstReference
        | BillReferenceKind::NewReference => {
            reference.name.is_some() && reference.raw_kind.is_none()
        }
    };
    if valid {
        Ok(())
    } else {
        Err(invalid_data("bill_reference_invalid"))
    }
}

fn validate_due_date(
    due_date: Option<&TallyDate>,
    due_date_evidence: BillDueDateEvidence,
) -> Result<(), TallyError> {
    if matches!(due_date_evidence, BillDueDateEvidence::Unavailable) == due_date.is_none() {
        Ok(())
    } else {
        Err(invalid_data("bill_due_date_evidence_invalid"))
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct StockItemRecord {
    pub source_id: SourceRecordId,
    pub name: CanonicalText,
    pub base_unit: CanonicalText,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct GodownRecord {
    pub source_id: SourceRecordId,
    pub name: CanonicalText,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct InventoryEntryRecord {
    pub source_id: SourceRecordId,
    pub voucher_source_id: SourceRecordId,
    pub stock_item_source_id: SourceRecordId,
    pub godown_source_id: SourceRecordId,
    pub quantity: ExactDecimal,
    pub rate: ExactDecimal,
    pub amount: ExactDecimal,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct InventoryBatch {
    pub stock_items: Vec<StockItemRecord>,
    pub godowns: Vec<GodownRecord>,
    pub inventory_entries: Vec<InventoryEntryRecord>,
}

impl InventoryBatch {
    pub fn validate(&self) -> Result<(), TallyError> {
        ensure_unique_source_ids(
            self.stock_items.iter().map(|record| &record.source_id),
            "duplicate_stock_item_source_id",
        )?;
        ensure_unique_source_ids(
            self.godowns.iter().map(|record| &record.source_id),
            "duplicate_godown_source_id",
        )?;
        ensure_unique_source_ids(
            self.inventory_entries
                .iter()
                .map(|record| &record.source_id),
            "duplicate_inventory_entry_source_id",
        )?;
        Ok(())
    }
}

fn ensure_unique_source_ids<'a>(
    source_ids: impl IntoIterator<Item = &'a SourceRecordId>,
    code: &'static str,
) -> Result<(), TallyError> {
    let mut observed = BTreeSet::new();
    if source_ids
        .into_iter()
        .any(|source_id| !observed.insert(source_id.as_str()))
    {
        return Err(invalid_data(code));
    }
    Ok(())
}

fn validate_text(value: &str, code: &'static str) -> Result<(), TallyError> {
    if value.is_empty()
        || value.len() > MAX_CANONICAL_TEXT_BYTES
        || value.trim() != value
        || value.chars().any(char::is_control)
    {
        return Err(invalid_data(code));
    }
    Ok(())
}

fn invalid_data(code: &'static str) -> TallyError {
    TallyError::InvalidData {
        code: code.to_string(),
    }
}

fn is_lower_sha256(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn validate_scope_descriptor(
    descriptor: &SourceCountScopeDescriptor,
    scope: SourceCountScope,
) -> Result<(), TallyError> {
    let identity = &descriptor.source_identity;
    for value in [
        identity.bridge_source_lineage.as_str(),
        identity.company_guid.as_str(),
        identity.observed_fingerprint.as_str(),
    ] {
        validate_text(value, "source_count_source_identity_invalid")?;
    }
    if descriptor.pack_schema_version.major == 0 {
        return Err(invalid_data("source_count_pack_schema_invalid"));
    }
    validate_sha256(
        descriptor.filters_sha256.as_str(),
        "source_count_filters_sha256_invalid",
    )?;
    match (scope, descriptor.window.as_ref()) {
        (SourceCountScope::Complete, None) | (SourceCountScope::Window, Some(_)) => {}
        _ => return Err(invalid_data("source_count_window_scope_mismatch")),
    }
    if let Some(window) = &descriptor.window {
        TallyDate::parse(window.from_yyyymmdd.clone())?;
        TallyDate::parse(window.to_yyyymmdd.clone())?;
        if window.from_yyyymmdd > window.to_yyyymmdd {
            return Err(invalid_data("source_count_window_reversed"));
        }
    }
    Ok(())
}

fn validate_sha256(value: &str, code: &'static str) -> Result<(), TallyError> {
    if value.len() != 64
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return Err(invalid_data(code));
    }
    Ok(())
}

fn validate_source_identity(
    identity: &SourceIdentity,
    text_code: &'static str,
    fingerprint_code: &'static str,
) -> Result<(), TallyError> {
    validate_text(&identity.bridge_source_lineage, text_code)?;
    validate_text(&identity.company_guid, text_code)?;
    validate_sha256(&identity.observed_fingerprint, fingerprint_code)
}

fn hex_lower(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut output = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        output.push(HEX[(byte >> 4) as usize] as char);
        output.push(HEX[(byte & 0x0f) as usize] as char);
    }
    output
}

fn is_valid_yyyymmdd(value: &str) -> bool {
    if value.len() != 8 || !value.bytes().all(|byte| byte.is_ascii_digit()) {
        return false;
    }
    let year = value[0..4].parse::<u32>().unwrap_or_default();
    let month = value[4..6].parse::<u32>().unwrap_or_default();
    let day = value[6..8].parse::<u32>().unwrap_or_default();
    let leap = year.is_multiple_of(4) && (!year.is_multiple_of(100) || year.is_multiple_of(400));
    let days_in_month = match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 if leap => 29,
        2 => 28,
        _ => return false,
    };
    year != 0 && (1..=days_in_month).contains(&day)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn id(value: &str) -> SourceRecordId {
        SourceRecordId::parse(value).expect("valid synthetic source id")
    }

    fn decimal(value: &str) -> ExactDecimal {
        ExactDecimal::parse(value).expect("valid exact decimal")
    }

    fn text(value: &str) -> CanonicalText {
        CanonicalText::parse(value).expect("valid canonical text")
    }

    fn non_negative(value: &str) -> NonNegativeExactDecimal {
        NonNegativeExactDecimal::parse(value).expect("valid non-negative exact decimal")
    }

    #[test]
    fn canonical_pack_models_round_trip_with_exact_values_and_references() {
        let tax = IndiaTaxBatch {
            tax_registrations: vec![TaxRegistrationRecord {
                source_id: id("tax-registration:1"),
                owner_kind: TaxRegistrationOwnerKind::Ledger,
                owner_source_id: id("ledger:customer"),
                registration_type: text("regular"),
                gstin: Gstin::parse("27ABCDE1234F1Z5").unwrap(),
            }],
            voucher_taxes: vec![VoucherTaxRecord {
                source_id: id("voucher-tax:1"),
                voucher_source_id: id("voucher:1"),
                place_of_supply: text("27"),
                assessable_value: decimal("1000.00"),
                tax_component: text("igst"),
                tax_rate: non_negative("18.00"),
                tax_amount: decimal("180.00"),
            }],
        };
        tax.validate().expect("valid tax batch");
        let encoded = serde_json::to_string(&tax).expect("serialize tax batch");
        let decoded: IndiaTaxBatch = serde_json::from_str(&encoded).expect("deserialize tax batch");
        assert_eq!(decoded, tax);
        assert_eq!(decoded.voucher_taxes[0].tax_amount.as_str(), "180.00");

        let bills = BillsAndPaymentsBatch {
            parties: vec![PartyOutstandingFacts {
                source_identity: SourceIdentity {
                    bridge_source_lineage: "bridge-source:test".to_string(),
                    company_guid: "company-guid:test".to_string(),
                    observed_fingerprint: "b".repeat(64),
                },
                party_ledger_source_id: id("ledger:customer"),
                report_as_of_yyyymmdd: TallyDate::parse("20260228").unwrap(),
                direction: OutstandingDirection::Receivable,
                bill_wise_state: BillWiseState::EnabledObserved,
                allocation_coverage: BillsCoverageState::ObservedCompleteScope,
                outstanding_coverage: BillsCoverageState::ObservedCompleteScope,
                fetch_bracket: FetchBracketState::StableObserved,
                query_profile: text("bills-confidence-v1"),
                source_scope_fingerprint: text(&"a".repeat(64)),
                source_reported_allocation_count: 1,
                source_reported_outstanding_count: 0,
                allocations: vec![BillAllocationRecord {
                    source_id: id("bill-allocation:1"),
                    identity_basis: DerivedIdentityBasis::ParentOrdinal,
                    origin: BillAllocationOrigin::Voucher {
                        voucher_source_id: id("voucher:1"),
                        party_entry_source_id: id("entry:1"),
                    },
                    reference: BillReference {
                        kind: BillReferenceKind::NewReference,
                        name: Some(text("INV-0001")),
                        raw_kind: None,
                    },
                    bill_date_yyyymmdd: Some(TallyDate::parse("20260201").unwrap()),
                    effective_date_yyyymmdd: None,
                    due_date_yyyymmdd: Some(TallyDate::parse("20260228").unwrap()),
                    due_date_evidence: BillDueDateEvidence::Explicit,
                    amount: decimal("-1180.00"),
                    observed_polarity: Some(crate::LedgerEntryPolarity::Debit),
                    currency_basis: CurrencyBasis::CompanyBase {
                        currency: text("company-base"),
                    },
                }],
                outstanding: Vec::new(),
            }],
        };
        bills.validate().expect("valid bills batch");
        assert_eq!(
            serde_json::from_value::<BillsAndPaymentsBatch>(
                serde_json::to_value(&bills).expect("serialize bills batch")
            )
            .expect("deserialize bills batch"),
            bills
        );

        let mut invalid = bills.clone();
        invalid.parties[0].allocations[0].reference = BillReference {
            kind: BillReferenceKind::OnAccount,
            name: Some(text("invented-link")),
            raw_kind: None,
        };
        assert!(invalid.validate().is_err());

        let mut invalid = bills.clone();
        invalid.parties[0].allocations[0].due_date_evidence = BillDueDateEvidence::Unavailable;
        assert!(invalid.validate().is_err());

        let mut invalid = bills.clone();
        invalid.parties[0].allocation_coverage = BillsCoverageState::ObservedPartial;
        invalid.parties[0].source_reported_allocation_count = 0;
        assert!(invalid.validate().is_err());

        let mut invalid = bills.clone();
        invalid.parties[0].source_scope_fingerprint = text("not-a-sha256");
        assert!(invalid.validate().is_err());

        let mut invalid = bills.clone();
        invalid.parties[0].allocations[0].identity_basis =
            DerivedIdentityBasis::MutableReferenceOrdinal;
        assert!(invalid.validate().is_err());

        let inventory = InventoryBatch {
            stock_items: vec![StockItemRecord {
                source_id: id("stock-item:1"),
                name: text("Synthetic Item"),
                base_unit: text("nos"),
            }],
            godowns: vec![GodownRecord {
                source_id: id("godown:1"),
                name: text("Synthetic Location"),
            }],
            inventory_entries: vec![InventoryEntryRecord {
                source_id: id("inventory-entry:1"),
                voucher_source_id: id("voucher:1"),
                stock_item_source_id: id("stock-item:1"),
                godown_source_id: id("godown:1"),
                quantity: decimal("2.000"),
                rate: decimal("500.00"),
                amount: decimal("1000.00"),
            }],
        };
        inventory.validate().expect("valid inventory batch");
        assert_eq!(
            serde_json::from_value::<InventoryBatch>(
                serde_json::to_value(&inventory).expect("serialize inventory batch")
            )
            .expect("deserialize inventory batch"),
            inventory
        );
    }

    #[test]
    fn derived_bill_ids_bind_parent_scope_and_ordinal_not_mutable_values() {
        let company = text(&"c".repeat(64));
        let party = id("ledger:party");
        let parent = id("voucher:1");
        let first = derive_bill_allocation_source_id(&company, &party, &parent, 1).unwrap();
        let same = derive_bill_allocation_source_id(&company, &party, &parent, 1).unwrap();
        let next = derive_bill_allocation_source_id(&company, &party, &parent, 2).unwrap();
        assert_eq!(first, same, "amount and due date are not identity inputs");
        assert_ne!(first, next);
        assert!(derive_bill_allocation_source_id(&company, &party, &parent, 0).is_err());

        let scope = text(&"a".repeat(64));
        assert_ne!(
            derive_bill_outstanding_source_id(&scope, 1).unwrap(),
            derive_bill_outstanding_source_id(&scope, 2).unwrap()
        );
    }

    #[test]
    fn missing_fields_unknown_fields_and_invalid_exact_decimals_fail_deserialization() {
        assert!(serde_json::from_str::<InventoryEntryRecord>(
            r#"{
                "source_id":"entry:1",
                "voucher_source_id":"voucher:1",
                "stock_item_source_id":"item:1",
                "godown_source_id":"godown:1",
                "quantity":"1",
                "rate":"10.00"
            }"#
        )
        .is_err());
        assert!(serde_json::from_str::<InventoryBatch>(
            r#"{"stock_items":[],"godowns":[],"inventory_entries":[],"schema_version":1}"#
        )
        .is_err());
        assert!(serde_json::from_str::<VoucherTaxRecord>(
            r#"{
                "source_id":"tax:1",
                "voucher_source_id":"voucher:1",
                "place_of_supply":"27",
                "assessable_value":"100.00",
                "tax_component":"igst",
                "tax_rate":"NaN",
                "tax_amount":"18.00"
            }"#
        )
        .is_err());
        assert!(serde_json::from_str::<VoucherTaxRecord>(
            r#"{
                "source_id":"tax:1",
                "voucher_source_id":"voucher:1",
                "place_of_supply":"27",
                "assessable_value":"100.00",
                "tax_component":"igst",
                "tax_rate":"-18.00",
                "tax_amount":"18.00"
            }"#
        )
        .is_err());
    }

    #[test]
    fn typed_ids_and_dates_reject_ambiguous_values_at_construction_and_deserialization() {
        for value in ["", " leading", "trailing ", "line\nbreak"] {
            assert!(SourceRecordId::parse(value).is_err(), "accepted {value:?}");
        }
        for value in ["20260229", "20261301", "20260001", "00000101", "2026-01-01"] {
            assert!(TallyDate::parse(value).is_err(), "accepted {value}");
        }
        assert!(TallyDate::parse("20240229").is_ok());
        assert!(serde_json::from_str::<TallyDate>(r#""20260229""#).is_err());
        for value in ["", " leading", "trailing ", "line\nbreak"] {
            assert!(CanonicalText::parse(value).is_err(), "accepted {value:?}");
        }
        for value in ["", "27abcde1234f1z5", "27ABCDE1234F1Z"] {
            assert!(Gstin::parse(value).is_err(), "accepted {value:?}");
        }
    }

    #[test]
    fn semantic_validation_rejects_duplicate_ids_and_unpopulated_required_text() {
        let duplicate = StockItemRecord {
            source_id: id("item:1"),
            name: text("Item"),
            base_unit: text("nos"),
        };
        let inventory = InventoryBatch {
            stock_items: vec![duplicate.clone(), duplicate],
            godowns: Vec::new(),
            inventory_entries: Vec::new(),
        };
        assert!(matches!(
            inventory.validate(),
            Err(TallyError::InvalidData { code }) if code == "duplicate_stock_item_source_id"
        ));

        assert!(serde_json::from_str::<VoucherTaxRecord>(
            r#"{
                "source_id":"tax:1",
                "voucher_source_id":"voucher:1",
                "place_of_supply":"",
                "assessable_value":"100.00",
                "tax_component":"igst",
                "tax_rate":"18.00",
                "tax_amount":"18.00"
            }"#
        )
        .is_err());
    }

    #[test]
    fn source_counts_are_optional_explicit_evidence_never_derived_from_records() {
        let batch = InventoryBatch {
            stock_items: vec![StockItemRecord {
                source_id: id("item:1"),
                name: text("Synthetic Item"),
                base_unit: text("nos"),
            }],
            godowns: Vec::new(),
            inventory_entries: Vec::new(),
        };
        let absent = PackWindow::without_source_count_evidence(batch.clone());
        assert_eq!(absent.source_counts, None);
        absent
            .validate_source_count_evidence()
            .expect("absent source evidence is honest");

        let observed = PackWindow {
            batch,
            source_counts: Some(vec![SourceReportedCountEvidence {
                object_type: text("stock_item"),
                query_profile: text("inventory-v1"),
                source_scope_fingerprint: text("scope-sha256:synthetic"),
                source_count_scope: SourceCountScope::Complete,
                // Deliberately differs from the parsed Vec length. The model
                // stores the source claim without inventing reconciliation.
                source_reported_count: 37,
            }]),
            record_evidence: None,
        };
        observed
            .validate_source_count_evidence()
            .expect("valid explicit evidence");
        assert_eq!(
            observed.source_counts.as_ref().unwrap()[0].source_reported_count,
            37
        );
    }

    #[test]
    fn source_count_fingerprint_binds_exact_versioned_scope() {
        let descriptor = SourceCountScopeDescriptor {
            source_identity: SourceIdentity {
                bridge_source_lineage: "lineage:synthetic".to_string(),
                company_guid: "company:synthetic".to_string(),
                observed_fingerprint: "fingerprint:synthetic".to_string(),
            },
            pack: CapabilityPackId::Inventory,
            pack_schema_version: PackSchemaVersion { major: 1, minor: 0 },
            object_type: text("stock_item"),
            query_profile: text("inventory-v1"),
            filters_sha256: text(&"a".repeat(64)),
            window: None,
        };
        let fingerprint = source_count_scope_fingerprint(&descriptor, SourceCountScope::Complete)
            .expect("valid complete scope fingerprint");
        assert_eq!(fingerprint.as_str().len(), 64);
        assert_eq!(
            fingerprint,
            source_count_scope_fingerprint(&descriptor, SourceCountScope::Complete).unwrap()
        );

        let evidence = SourceReportedCountEvidence {
            object_type: descriptor.object_type.clone(),
            query_profile: descriptor.query_profile.clone(),
            source_scope_fingerprint: fingerprint,
            source_count_scope: SourceCountScope::Complete,
            source_reported_count: 0,
        };
        assert!(evidence.matches_scope_descriptor(&descriptor).unwrap());

        let mut drifted = descriptor.clone();
        drifted.query_profile = text("inventory-v2");
        assert!(!evidence.matches_scope_descriptor(&drifted).unwrap());
        assert!(source_count_scope_fingerprint(&descriptor, SourceCountScope::Window).is_err());
    }

    #[test]
    fn window_count_fingerprint_requires_exact_valid_window() {
        let mut descriptor = SourceCountScopeDescriptor {
            source_identity: SourceIdentity {
                bridge_source_lineage: "lineage:synthetic".to_string(),
                company_guid: "company:synthetic".to_string(),
                observed_fingerprint: "fingerprint:synthetic".to_string(),
            },
            pack: CapabilityPackId::CoreAccounting,
            pack_schema_version: PackSchemaVersion { major: 1, minor: 0 },
            object_type: text("voucher"),
            query_profile: text("voucher-v1"),
            filters_sha256: text(&"b".repeat(64)),
            window: Some(ReadWindow {
                from_yyyymmdd: "20260101".to_string(),
                to_yyyymmdd: "20260131".to_string(),
            }),
        };
        assert!(source_count_scope_fingerprint(&descriptor, SourceCountScope::Window).is_ok());
        assert!(source_count_scope_fingerprint(&descriptor, SourceCountScope::Complete).is_err());

        descriptor.window.as_mut().unwrap().from_yyyymmdd = "20260201".to_string();
        assert!(source_count_scope_fingerprint(&descriptor, SourceCountScope::Window).is_err());
    }

    #[test]
    fn empty_or_duplicate_source_count_evidence_fails_closed() {
        let empty = PackWindow {
            batch: IndiaTaxBatch::default(),
            source_counts: Some(Vec::new()),
            record_evidence: None,
        };
        assert!(matches!(
            empty.validate_source_count_evidence(),
            Err(TallyError::InvalidData { code }) if code == "source_count_evidence_empty"
        ));

        let count = SourceReportedCountEvidence {
            object_type: text("voucher_tax"),
            query_profile: text("tax-v1"),
            source_scope_fingerprint: text("scope-sha256:synthetic"),
            source_count_scope: SourceCountScope::Complete,
            source_reported_count: 2,
        };
        let duplicate = PackWindow {
            batch: IndiaTaxBatch::default(),
            source_counts: Some(vec![count.clone(), count]),
            record_evidence: None,
        };
        assert!(matches!(
            duplicate.validate_source_count_evidence(),
            Err(TallyError::InvalidData { code }) if code == "source_count_evidence_duplicate_scope"
        ));
    }
}
