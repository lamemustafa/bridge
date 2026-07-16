use async_trait::async_trait;
use serde::{Deserialize, Deserializer, Serialize};
use std::collections::{BTreeMap, BTreeSet};

pub mod bills_reconciliation;
mod exact_arithmetic;
mod pack_models;
pub mod reconciliation;
pub mod report_tie_out;
pub mod transport_qualification;

pub use pack_models::*;

pub mod destination;

pub const PROOF_CONTRACT_VERSION: u16 = 3;
const MAX_EXACT_DECIMAL_BYTES: usize = 256;
pub const CORE_ACCOUNTING_SCHEMA_VERSION: PackSchemaVersion =
    PackSchemaVersion { major: 3, minor: 0 };
pub const BILLS_AND_PAYMENTS_SCHEMA_VERSION: PackSchemaVersion =
    PackSchemaVersion { major: 2, minor: 0 };

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CapabilityPackId {
    CoreAccounting,
    IndiaTax,
    BillsAndPayments,
    Inventory,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TransportId {
    XmlHttp,
    JsonEx,
    TdlCompanion,
    Odbc,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CapabilityFeatureId {
    EndpointReachability,
    LoadedCompanies,
    StableCompanyIdentity,
    EncodingBehaviour,
    PracticalResponseLimit,
    CompanyRead,
    LedgerRead,
    VoucherRead,
    SelectedLedgerRead,
    SelectedVoucherWindowRead,
    Write,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
pub struct PackSchemaVersion {
    pub major: u16,
    pub minor: u16,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
pub struct SourceIdentity {
    pub bridge_source_lineage: String,
    pub company_guid: String,
    pub observed_fingerprint: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
pub struct CompanyRef {
    pub identity: SourceIdentity,
    pub display_name: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
pub struct ReadWindow {
    pub from_yyyymmdd: String,
    pub to_yyyymmdd: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
pub struct RequestContext {
    pub run_id: String,
    pub company: CompanyRef,
    pub pack: CapabilityPackId,
    pub schema_version: PackSchemaVersion,
    pub window: ReadWindow,
    pub query_profile: CanonicalText,
    pub filters_sha256: CanonicalText,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CapabilityState {
    Supported,
    Unsupported,
    Unknown,
    NotConfigured,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum EvidenceConfidence {
    Documented,
    Observed,
    Inferred,
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
pub struct CapabilityEvidence {
    pub state: CapabilityState,
    pub confidence: EvidenceConfidence,
    pub safe_reason_code: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
pub struct CapabilityProfile {
    pub profile_version: u16,
    pub product: String,
    pub release: Option<String>,
    pub mode: Option<String>,
    pub transports: BTreeMap<TransportId, CapabilityEvidence>,
    #[serde(default)]
    pub features: BTreeMap<CapabilityFeatureId, CapabilityEvidence>,
    pub packs: BTreeMap<CapabilityPackId, CapabilityEvidence>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
pub struct Gap {
    pub pack: CapabilityPackId,
    pub field_or_invariant: String,
    pub state: CapabilityState,
    pub safe_reason_code: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(transparent)]
pub struct ExactDecimal(String);

impl ExactDecimal {
    pub fn parse(value: impl Into<String>) -> Result<Self, TallyError> {
        let value = value.into();
        let bytes = value.as_bytes();
        let body = bytes.strip_prefix(b"-").unwrap_or(bytes);
        let mut sections = body.split(|byte| *byte == b'.');
        let whole = sections.next().unwrap_or_default();
        let fractional = sections.next();
        let valid = bytes.len() <= MAX_EXACT_DECIMAL_BYTES
            && !whole.is_empty()
            && whole.iter().all(u8::is_ascii_digit)
            && fractional
                .is_none_or(|part| !part.is_empty() && part.iter().all(u8::is_ascii_digit))
            && sections.next().is_none();
        if !valid {
            return Err(TallyError::InvalidData {
                code: "invalid_exact_decimal".to_string(),
            });
        }
        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl<'de> Deserialize<'de> for ExactDecimal {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        Self::parse(value).map_err(serde::de::Error::custom)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
pub struct GroupRecord {
    pub source_id: String,
    pub name: String,
    pub parent_source_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
pub struct LedgerRecord {
    pub source_id: String,
    pub name: String,
    pub parent_source_id: Option<String>,
    pub opening_balance: Option<ExactDecimal>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
pub struct VoucherTypeRecord {
    pub source_id: String,
    pub name: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
pub struct VoucherRecord {
    pub source_id: String,
    pub date_yyyymmdd: String,
    pub voucher_type_source_id: String,
    pub voucher_number: Option<String>,
    pub cancelled: bool,
    /// Source-observed Tally optional-voucher state. Optional vouchers do not
    /// affect ordinary books unless an explicitly selected scenario includes
    /// them; Bridge's Core profile is bound to ordinary books.
    pub optional: bool,
}

/// Tally's `ISDEEMEDPOSITIVE` accounting polarity. Tally represents debits
/// with `Yes` and credits with `No`; the signed amount must independently
/// agree with this observed flag.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum LedgerEntryPolarity {
    Debit,
    Credit,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
pub struct LedgerEntryRecord {
    pub source_id: String,
    pub voucher_source_id: String,
    pub ledger_source_id: String,
    pub amount: ExactDecimal,
    pub polarity: LedgerEntryPolarity,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Deserialize, Serialize)]
pub struct CoreAccountingBatch {
    pub groups: Vec<GroupRecord>,
    pub ledgers: Vec<LedgerRecord>,
    pub voucher_types: Vec<VoucherTypeRecord>,
    pub vouchers: Vec<VoucherRecord>,
    pub ledger_entries: Vec<LedgerEntryRecord>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(tag = "pack", content = "batch", rename_all = "snake_case")]
pub enum PackBatch {
    CoreAccounting(CoreAccountingBatch),
    IndiaTax(IndiaTaxBatch),
    BillsAndPayments(BillsAndPaymentsBatch),
    Inventory(InventoryBatch),
}

pub type CanonicalPackWindow = PackWindow<PackBatch>;

impl PackWindow<PackBatch> {
    /// Verifies that supplied provenance is a one-to-one cover of every
    /// canonical record in the returned batch. Absence is allowed so tests and
    /// synthetic producers remain representable; callers must treat it as an
    /// explicit provenance gap rather than manufacturing evidence.
    pub fn validate_record_evidence_binding(&self) -> Result<(), TallyError> {
        self.validate_record_evidence()?;
        let Some(record_evidence) = &self.record_evidence else {
            return Ok(());
        };

        let expected = canonical_record_keys(&self.batch);
        let supplied = record_evidence
            .iter()
            .map(|evidence| {
                (
                    evidence.object_type.as_str().to_string(),
                    evidence.source_id.as_str().to_string(),
                )
            })
            .collect::<BTreeSet<_>>();
        if supplied != expected {
            return Err(TallyError::InvalidData {
                code: "source_record_evidence_binding_mismatch".to_string(),
            });
        }
        Ok(())
    }
}

fn canonical_record_keys(batch: &PackBatch) -> BTreeSet<(String, String)> {
    let mut keys = BTreeSet::new();
    macro_rules! insert_records {
        ($records:expr, $object_type:literal) => {
            keys.extend($records.iter().map(|record| {
                (
                    $object_type.to_string(),
                    record.source_id.as_str().to_string(),
                )
            }));
        };
    }
    macro_rules! insert_core_records {
        ($records:expr, $object_type:literal) => {
            keys.extend(
                $records
                    .iter()
                    .map(|record| ($object_type.to_string(), record.source_id.clone())),
            );
        };
    }

    match batch {
        PackBatch::CoreAccounting(core) => {
            insert_core_records!(core.groups, "group");
            insert_core_records!(core.ledgers, "ledger");
            insert_core_records!(core.voucher_types, "voucher_type");
            insert_core_records!(core.vouchers, "voucher");
            insert_core_records!(core.ledger_entries, "ledger_entry");
        }
        PackBatch::IndiaTax(tax) => {
            insert_records!(tax.tax_registrations, "tax_registration");
            insert_records!(tax.voucher_taxes, "voucher_tax");
        }
        PackBatch::BillsAndPayments(bills) => {
            for party in &bills.parties {
                keys.insert((
                    "party_outstanding".to_string(),
                    party.party_ledger_source_id.as_str().to_string(),
                ));
                insert_records!(party.allocations, "bill_allocation");
                insert_records!(party.outstanding, "bill_outstanding");
            }
        }
        PackBatch::Inventory(inventory) => {
            insert_records!(inventory.stock_items, "stock_item");
            insert_records!(inventory.godowns, "godown");
            insert_records!(inventory.inventory_entries, "inventory_entry");
        }
    }
    keys
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RunOutcome {
    Completed,
    Failed,
    Cancelled,
    OutcomeUnknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum VerificationState {
    Verified,
    Partial,
    Unverified,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Freshness {
    Fresh,
    Stale,
    NeverVerified,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
pub struct ProofManifest {
    pub proof_contract_version: u16,
    pub run_id: String,
    pub source_identity: SourceIdentity,
    pub pack: CapabilityPackId,
    pub pack_schema_version: PackSchemaVersion,
    pub outcome: RunOutcome,
    pub verification: VerificationState,
    pub freshness: Freshness,
    pub started_at_unix_ms: i64,
    pub completed_at_unix_ms: Option<i64>,
    pub record_counts: BTreeMap<String, u64>,
    pub snapshot_sha256: Option<String>,
    pub gaps: Vec<Gap>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
pub struct ProbeResult {
    pub reachable: bool,
    pub profile: CapabilityProfile,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
pub struct DeliverySession {
    pub delivery_id: String,
    pub accepted_pack_versions: BTreeMap<CapabilityPackId, PackSchemaVersion>,
    pub max_batch_bytes: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
pub struct DeliveryReceipt {
    pub delivery_id: String,
    pub receipt_id: String,
    pub content_sha256: String,
    pub committed: bool,
}

#[derive(Debug, thiserror::Error)]
pub enum TallyError {
    #[error("Tally is not reachable")]
    Unreachable,
    #[error("Tally returned an invalid protocol response ({code})")]
    Protocol { code: String },
    #[error("Tally data failed validation ({code})")]
    InvalidData { code: String },
    #[error("Capability is unavailable ({code})")]
    Unsupported { code: String },
    #[error("Tally read response exceeded the bounded limit ({scope:?})")]
    ReadResponseTooLarge { scope: ReadResponseScope },
    #[error("Operation was cancelled")]
    Cancelled,
    #[error("The outcome of the write could not be proven")]
    OutcomeUnknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReadResponseScope {
    VoucherWindow,
}

#[async_trait]
pub trait TallyConnector: Send + Sync {
    async fn probe(&self) -> Result<ProbeResult, TallyError>;
    /// Performs a new source observation rather than returning capability
    /// evidence cached by an earlier probe. Connectors that cannot make that
    /// guarantee must leave the check unavailable.
    async fn probe_fresh(&self) -> Result<ProbeResult, TallyError> {
        Err(TallyError::Unsupported {
            code: "fresh_capability_probe_not_supported".to_string(),
        })
    }
    async fn discover_companies(&self) -> Result<Vec<CompanyRef>, TallyError>;
    async fn read_pack_window(
        &self,
        context: &RequestContext,
    ) -> Result<CanonicalPackWindow, TallyError>;
    async fn read_core_period_balance_report(
        &self,
        _context: &RequestContext,
    ) -> Result<report_tie_out::LedgerPeriodBalanceReport, TallyError> {
        Err(TallyError::Unsupported {
            code: "core_period_balance_report_not_supported".to_string(),
        })
    }
}

#[async_trait]
pub trait DestinationAdapter: Send + Sync {
    async fn supported_packs(
        &self,
    ) -> Result<BTreeMap<CapabilityPackId, PackSchemaVersion>, TallyError>;
    async fn begin_delivery(&self, proof: &ProofManifest) -> Result<DeliverySession, TallyError>;
    async fn deliver_batch(
        &self,
        session: &DeliverySession,
        batch: &PackBatch,
        content_sha256: &str,
        idempotency_key: &str,
    ) -> Result<DeliveryReceipt, TallyError>;
    async fn finalize_delivery(
        &self,
        session: &DeliverySession,
        proof: &ProofManifest,
    ) -> Result<DeliveryReceipt, TallyError>;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exact_decimals_round_trip_without_float_conversion() {
        for value in ["0", "0.00", "12345678901234567890.0001", "-1180.00"] {
            let parsed = ExactDecimal::parse(value).expect("valid exact decimal");
            assert_eq!(parsed.as_str(), value);
        }
    }

    #[test]
    fn exact_decimals_reject_ambiguous_or_non_numeric_values() {
        for value in ["", "-", ".1", "1.", "+1", "1,000", "NaN", "1.2.3"] {
            assert!(
                ExactDecimal::parse(value).is_err(),
                "unexpectedly accepted {value}"
            );
        }
        assert!(ExactDecimal::parse("9".repeat(MAX_EXACT_DECIMAL_BYTES + 1)).is_err());
    }

    #[test]
    fn legacy_capability_profiles_deserialize_without_inventing_feature_evidence() {
        let profile: CapabilityProfile = serde_json::from_str(
            r#"{
                "profile_version": 1,
                "product": "Unknown",
                "release": null,
                "mode": null,
                "transports": {},
                "packs": {}
            }"#,
        )
        .expect("deserialize legacy profile");

        assert_eq!(profile.profile_version, 1);
        assert!(profile.features.is_empty());
    }

    fn record_evidence(object_type: &str, source_id: &str) -> SourceRecordEvidence {
        let source_id = SourceRecordId::parse(source_id).unwrap();
        SourceRecordEvidence {
            object_type: CanonicalText::parse(object_type).unwrap(),
            source_id: source_id.clone(),
            identity_kind: SourceIdentityKind::Guid,
            observed_identities: ObservedSourceIdentities {
                guid: Some(source_id),
                ..Default::default()
            },
            raw_source_sha256: RawSourceSha256::parse("a".repeat(64)).unwrap(),
            alter_id: Some(SourceAlterId::parse("alter:42").unwrap()),
        }
    }

    #[test]
    fn record_provenance_must_bind_one_to_one_to_canonical_records() {
        let batch = PackBatch::Inventory(InventoryBatch {
            stock_items: vec![StockItemRecord {
                source_id: SourceRecordId::parse("stock:1").unwrap(),
                name: CanonicalText::parse("Synthetic Item").unwrap(),
                base_unit: CanonicalText::parse("nos").unwrap(),
            }],
            godowns: Vec::new(),
            inventory_entries: Vec::new(),
        });
        let valid = CanonicalPackWindow {
            batch: batch.clone(),
            source_counts: None,
            record_evidence: Some(vec![record_evidence("stock_item", "stock:1")]),
        };
        valid.validate_record_evidence_binding().unwrap();

        let missing = CanonicalPackWindow {
            batch: batch.clone(),
            source_counts: None,
            record_evidence: Some(vec![record_evidence("godown", "stock:1")]),
        };
        assert!(matches!(
            missing.validate_record_evidence_binding(),
            Err(TallyError::InvalidData { code })
                if code == "source_record_evidence_binding_mismatch"
        ));

        let duplicate = CanonicalPackWindow {
            batch,
            source_counts: None,
            record_evidence: Some(vec![
                record_evidence("stock_item", "stock:1"),
                record_evidence("stock_item", "stock:1"),
            ]),
        };
        assert!(matches!(
            duplicate.validate_record_evidence_binding(),
            Err(TallyError::InvalidData { code })
                if code == "source_record_evidence_duplicate_record"
        ));
    }

    #[test]
    fn record_provenance_rejects_noncanonical_hashes_and_alter_ids() {
        assert!(RawSourceSha256::parse("A".repeat(64)).is_err());
        assert!(RawSourceSha256::parse("a".repeat(63)).is_err());
        assert!(SourceAlterId::parse("contains whitespace").is_err());
        assert!(SourceAlterId::parse("alter:42").is_ok());
    }
}
