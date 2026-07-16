use std::{
    collections::{BTreeMap, BTreeSet},
    fmt::Write as _,
    fs,
    path::{Component, Path},
};

use ed25519_dalek::{Signature, Verifier, VerifyingKey};
use serde::{de::DeserializeOwned, Deserialize, Serialize};
use sha2::{Digest, Sha256};
use thiserror::Error;

#[cfg(feature = "bills-native-outstandings-probe-receipt")]
pub mod bills_native_outstandings_probe_receipt;

pub const LIVE_RECEIPT_SCHEMA_VERSION: u16 = 1;
pub const SURFACE_SCHEMA_VERSION: u16 = 1;
pub const SUPPORT_MANIFEST_SCHEMA_VERSION: u16 = 1;
pub const TRUST_MANIFEST_SCHEMA_VERSION: u16 = 1;
pub const ATTESTATION_SCHEMA_VERSION: u16 = 1;
pub const MAX_ARTIFACT_BYTES: usize = 256 * 1024;
pub const MAX_SURFACE_FILES: usize = 96;
pub const MAX_OPERATIONS: usize = 16;
pub const MAX_CLAIMS: usize = 128;
pub const MAX_KEYS: usize = 32;
pub const MAX_MATRIX_MARKDOWN_BYTES: usize = 1024 * 1024;
const MAX_FUTURE_SKEW_MS: i64 = 5 * 60 * 1000;

#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum CompatibilityError {
    #[error("compatibility artifact was invalid ({code})")]
    Invalid { code: &'static str },
    #[error("compatibility support gate failed ({code})")]
    Gate { code: &'static str },
}

fn invalid(code: &'static str) -> CompatibilityError {
    CompatibilityError::Invalid { code }
}

fn gate(code: &'static str) -> CompatibilityError {
    CompatibilityError::Gate { code }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProductFamily {
    TallyPrime,
    TallyErp9,
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TallyMode {
    Education,
    Licensed,
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EvidenceAuthority {
    OfficialDocumentation,
    BridgeConfiguration,
    BridgeObservation,
    EndpointClaim,
    UserAttestation,
    Inference,
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EvidenceConfidence {
    Observed,
    Attested,
    Inferred,
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProfileValue<T> {
    pub value: T,
    pub authority: EvidenceAuthority,
    pub confidence: EvidenceConfidence,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Platform {
    Windows,
    Macos,
    Linux,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Architecture {
    X86_64,
    Aarch64,
    Other,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LoopbackFamily {
    LocalhostAlias,
    Ipv4,
    Ipv6,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TransportProfile {
    XmlHttp,
    JsonExShadow,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReadProfileId {
    XmlCompanyEnumerationV1,
    XmlSyntheticFixtureMarkerV1,
    XmlLedgerReadV1,
    XmlVoucherEmptyRangeV1,
    XmlVoucherPopulatedRangeV1,
    XmlEducationModeProbeV1,
    JsonExSemanticShadowV1,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OperationOutcome {
    Passed,
    Failed,
    Unsupported,
    Inconclusive,
    NotAttempted,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ApplicationStatus {
    Success,
    Failure,
    Unrecognized,
    NotApplicable,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TextEncoding {
    Utf8,
    Utf8Bom,
    Utf16Le,
    Utf16Be,
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OdbcState {
    Disabled,
    Enabled,
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CompanyLoadState {
    None,
    One,
    Multiple,
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LocaleProfile {
    EnglishIndia,
    Other,
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DatasetTier {
    SyntheticSmall,
    SyntheticMedium,
    SyntheticLarge,
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SizeBucket {
    Zero,
    Bytes1To4096,
    Bytes4097To65536,
    Bytes65537To1048576,
    Over1048576,
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CountBucket {
    Zero,
    One,
    TwoToFive,
    SixToTwenty,
    OverTwenty,
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OperationEvidence {
    pub profile: ReadProfileId,
    pub template_sha256: String,
    pub outcome: OperationOutcome,
    pub application_status: ApplicationStatus,
    pub encoding: TextEncoding,
    pub response_size: SizeBucket,
    pub record_count: CountBucket,
    pub safe_reason_code: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LiveReadAuthority {
    pub live_endpoint_response_observed: bool,
    pub read_only: bool,
    pub writes_attempted: bool,
    pub raw_customer_data_retained: bool,
    pub responder_authenticity_established: bool,
    pub accounting_correctness_established: bool,
    pub source_completeness_established: bool,
    pub source_atomicity_established: bool,
    pub performance_budget_established: bool,
    pub tauri_runtime_observed: bool,
    pub support_claim_eligible: bool,
}

impl LiveReadAuthority {
    pub fn observation_only() -> Self {
        Self {
            live_endpoint_response_observed: true,
            read_only: true,
            writes_attempted: false,
            raw_customer_data_retained: false,
            responder_authenticity_established: false,
            accounting_correctness_established: false,
            source_completeness_established: false,
            source_atomicity_established: false,
            performance_budget_established: false,
            tauri_runtime_observed: false,
            support_claim_eligible: false,
        }
    }

    pub fn attempt_only() -> Self {
        Self {
            live_endpoint_response_observed: false,
            ..Self::observation_only()
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LiveCompatibilityReceipt {
    pub schema_version: u16,
    pub observed_at_unix_ms: i64,
    pub bridge_commit_sha: String,
    pub working_tree_dirty: bool,
    pub compatibility_surface_sha256: String,
    pub executable_sha256: String,
    pub cargo_lock_sha256: String,
    pub platform: Platform,
    pub architecture: Architecture,
    pub endpoint_family: LoopbackFamily,
    pub transport: TransportProfile,
    pub product: ProfileValue<ProductFamily>,
    pub release: ProfileValue<String>,
    pub mode: ProfileValue<TallyMode>,
    pub odbc_state: ProfileValue<OdbcState>,
    pub locale: ProfileValue<LocaleProfile>,
    pub dataset_tier: ProfileValue<DatasetTier>,
    pub fixture_manifest_sha256: String,
    pub fixture_marker_verified: bool,
    pub no_customer_data: ProfileValue<bool>,
    pub loaded_company_count: CountBucket,
    pub operations: Vec<OperationEvidence>,
    pub authority: LiveReadAuthority,
    pub receipt_sha256: String,
}

impl LiveCompatibilityReceipt {
    pub fn seal(mut self) -> Result<Self, CompatibilityError> {
        self.receipt_sha256.clear();
        self.validate_shape(false)?;
        self.receipt_sha256 = checksum(b"bridge.tally.live-read-qualification/1\0", &self)?;
        self.validate()?;
        Ok(self)
    }

    pub fn validate(&self) -> Result<(), CompatibilityError> {
        self.validate_shape(true)?;
        let mut unsigned = self.clone();
        let supplied = std::mem::take(&mut unsigned.receipt_sha256);
        let expected = checksum(b"bridge.tally.live-read-qualification/1\0", &unsigned)?;
        if supplied != expected {
            return Err(invalid("receipt_checksum_mismatch"));
        }
        Ok(())
    }

    fn validate_shape(&self, require_checksum: bool) -> Result<(), CompatibilityError> {
        if self.schema_version != LIVE_RECEIPT_SCHEMA_VERSION {
            return Err(invalid("receipt_schema_unsupported"));
        }
        if self.observed_at_unix_ms <= 0 {
            return Err(invalid("receipt_time_invalid"));
        }
        validate_commit(&self.bridge_commit_sha)?;
        for digest in [
            &self.compatibility_surface_sha256,
            &self.executable_sha256,
            &self.cargo_lock_sha256,
            &self.fixture_manifest_sha256,
        ] {
            validate_sha256(digest)?;
        }
        if require_checksum {
            validate_sha256(&self.receipt_sha256)?;
        } else if !self.receipt_sha256.is_empty() {
            return Err(invalid("receipt_checksum_must_start_empty"));
        }
        validate_profile_values(
            &self.product,
            &self.release,
            &self.mode,
            &self.odbc_state,
            &self.locale,
            &self.dataset_tier,
            &self.no_customer_data,
        )?;
        if self.operations.is_empty() || self.operations.len() > MAX_OPERATIONS {
            return Err(invalid("operation_count_invalid"));
        }
        let mut previous = None;
        for operation in &self.operations {
            if previous.is_some_and(|value| value >= operation.profile) {
                return Err(invalid("operations_not_unique_sorted"));
            }
            previous = Some(operation.profile);
            validate_operation(operation)?;
        }
        self.operations
            .iter()
            .find(|operation| operation.profile == ReadProfileId::XmlSyntheticFixtureMarkerV1)
            .ok_or_else(|| invalid("fixture_marker_operation_missing"))?;
        if !self
            .operations
            .iter()
            .any(|operation| operation.profile == ReadProfileId::XmlCompanyEnumerationV1)
        {
            return Err(invalid("company_enumeration_operation_missing"));
        }
        if self.fixture_marker_verified {
            let required = [
                ReadProfileId::XmlCompanyEnumerationV1,
                ReadProfileId::XmlSyntheticFixtureMarkerV1,
            ];
            if required.iter().any(|profile| {
                !self.operations.iter().any(|operation| {
                    operation.profile == *profile
                        && operation.outcome == OperationOutcome::Passed
                        && operation.application_status == ApplicationStatus::Success
                })
            }) {
                return Err(invalid("fixture_marker_contract_not_passed"));
            }
        }
        if self.authority != LiveReadAuthority::observation_only()
            && self.authority != LiveReadAuthority::attempt_only()
        {
            return Err(invalid("receipt_authority_invalid"));
        }
        if !self.fixture_marker_verified {
            for operation in &self.operations {
                if matches!(
                    operation.profile,
                    ReadProfileId::XmlLedgerReadV1
                        | ReadProfileId::XmlVoucherEmptyRangeV1
                        | ReadProfileId::XmlVoucherPopulatedRangeV1
                ) && operation.outcome != OperationOutcome::NotAttempted
                {
                    return Err(invalid("company_read_without_fixture_marker"));
                }
            }
        }
        Ok(())
    }

    pub fn to_pretty_json(&self) -> Result<Vec<u8>, CompatibilityError> {
        self.validate()?;
        let bytes = serde_json::to_vec_pretty(self).map_err(|_| invalid("serialization_failed"))?;
        if bytes.len() > MAX_ARTIFACT_BYTES {
            return Err(invalid("artifact_too_large"));
        }
        Ok(bytes)
    }

    pub fn from_json(bytes: &[u8]) -> Result<Self, CompatibilityError> {
        let receipt: Self = parse_bounded_json(bytes)?;
        receipt.validate()?;
        Ok(receipt)
    }
}

fn validate_profile_values(
    product: &ProfileValue<ProductFamily>,
    release: &ProfileValue<String>,
    mode: &ProfileValue<TallyMode>,
    odbc_state: &ProfileValue<OdbcState>,
    locale: &ProfileValue<LocaleProfile>,
    dataset_tier: &ProfileValue<DatasetTier>,
    no_customer_data: &ProfileValue<bool>,
) -> Result<(), CompatibilityError> {
    let product_ok = match product.value {
        ProductFamily::Unknown => {
            product.authority == EvidenceAuthority::Unknown
                && product.confidence == EvidenceConfidence::Unknown
        }
        _ => {
            product.authority == EvidenceAuthority::UserAttestation
                && product.confidence == EvidenceConfidence::Attested
        }
    };
    if !product_ok {
        return Err(invalid("product_authority_invalid"));
    }
    if release.value == "unknown" {
        if release.authority != EvidenceAuthority::Unknown
            || release.confidence != EvidenceConfidence::Unknown
        {
            return Err(invalid("release_authority_invalid"));
        }
    } else {
        validate_label(&release.value)?;
        if release.authority != EvidenceAuthority::UserAttestation
            || release.confidence != EvidenceConfidence::Attested
        {
            return Err(invalid("release_authority_invalid"));
        }
    }
    let mode_ok = match mode.value {
        TallyMode::Unknown => {
            mode.authority == EvidenceAuthority::Unknown
                && mode.confidence == EvidenceConfidence::Unknown
        }
        _ => matches!(
            (mode.authority, mode.confidence),
            (
                EvidenceAuthority::UserAttestation,
                EvidenceConfidence::Attested
            ) | (
                EvidenceAuthority::EndpointClaim,
                EvidenceConfidence::Observed
            )
        ),
    };
    if !mode_ok {
        return Err(invalid("mode_authority_invalid"));
    }
    for (is_unknown, authority, confidence, code) in [
        (
            odbc_state.value == OdbcState::Unknown,
            odbc_state.authority,
            odbc_state.confidence,
            "odbc_authority_invalid",
        ),
        (
            locale.value == LocaleProfile::Unknown,
            locale.authority,
            locale.confidence,
            "locale_authority_invalid",
        ),
    ] {
        let valid = if is_unknown {
            authority == EvidenceAuthority::Unknown && confidence == EvidenceConfidence::Unknown
        } else {
            authority == EvidenceAuthority::UserAttestation
                && confidence == EvidenceConfidence::Attested
        };
        if !valid {
            return Err(invalid(code));
        }
    }
    let dataset_valid = if dataset_tier.value == DatasetTier::Unknown {
        dataset_tier.authority == EvidenceAuthority::Unknown
            && dataset_tier.confidence == EvidenceConfidence::Unknown
    } else {
        dataset_tier.authority == EvidenceAuthority::BridgeConfiguration
            && dataset_tier.confidence == EvidenceConfidence::Attested
    };
    if !dataset_valid {
        return Err(invalid("dataset_authority_invalid"));
    }
    if no_customer_data.authority != EvidenceAuthority::UserAttestation
        || no_customer_data.confidence != EvidenceConfidence::Attested
    {
        return Err(invalid("customer_data_authority_invalid"));
    }
    Ok(())
}

fn validate_operation(operation: &OperationEvidence) -> Result<(), CompatibilityError> {
    validate_sha256(&operation.template_sha256)?;
    match operation.outcome {
        OperationOutcome::Passed => {
            if operation.safe_reason_code.is_some()
                || operation.application_status != ApplicationStatus::Success
            {
                return Err(invalid("passed_operation_invalid"));
            }
        }
        OperationOutcome::NotAttempted => {
            if operation.application_status != ApplicationStatus::NotApplicable
                || operation.encoding != TextEncoding::Unknown
                || operation.response_size != SizeBucket::Zero
                || operation.record_count != CountBucket::Unknown
            {
                return Err(invalid("not_attempted_operation_invalid"));
            }
            let reason = operation
                .safe_reason_code
                .as_deref()
                .ok_or_else(|| invalid("operation_reason_missing"))?;
            validate_safe_code(reason)?;
        }
        OperationOutcome::Unsupported => {
            return Err(invalid("unsupported_operation_signature_unavailable"));
        }
        OperationOutcome::Failed | OperationOutcome::Inconclusive => {
            let reason = operation
                .safe_reason_code
                .as_deref()
                .ok_or_else(|| invalid("operation_reason_missing"))?;
            validate_safe_code(reason)?;
        }
    }
    Ok(())
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SurfaceFile {
    pub path: String,
    pub sha256: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CompatibilitySurfaceManifest {
    pub schema_version: u16,
    pub files: Vec<SurfaceFile>,
    pub manifest_sha256: String,
}

impl CompatibilitySurfaceManifest {
    pub fn seal(mut self) -> Result<Self, CompatibilityError> {
        self.manifest_sha256.clear();
        self.validate_shape(false)?;
        self.manifest_sha256 = checksum(b"bridge.tally.compatibility-surface/1\0", &self)?;
        self.validate()?;
        Ok(self)
    }

    pub fn validate(&self) -> Result<(), CompatibilityError> {
        self.validate_shape(true)?;
        let mut unsigned = self.clone();
        let supplied = std::mem::take(&mut unsigned.manifest_sha256);
        let expected = checksum(b"bridge.tally.compatibility-surface/1\0", &unsigned)?;
        if supplied != expected {
            return Err(invalid("surface_checksum_mismatch"));
        }
        Ok(())
    }

    fn validate_shape(&self, require_checksum: bool) -> Result<(), CompatibilityError> {
        if self.schema_version != SURFACE_SCHEMA_VERSION {
            return Err(invalid("surface_schema_unsupported"));
        }
        if self.files.is_empty() || self.files.len() > MAX_SURFACE_FILES {
            return Err(invalid("surface_file_count_invalid"));
        }
        let mut previous: Option<&str> = None;
        for file in &self.files {
            validate_relative_path(&file.path)?;
            validate_sha256(&file.sha256)?;
            if previous.is_some_and(|value| value >= file.path.as_str()) {
                return Err(invalid("surface_files_not_unique_sorted"));
            }
            previous = Some(&file.path);
        }
        if require_checksum {
            validate_sha256(&self.manifest_sha256)?;
        } else if !self.manifest_sha256.is_empty() {
            return Err(invalid("surface_checksum_must_start_empty"));
        }
        Ok(())
    }

    pub fn validate_files(&self, repository_root: &Path) -> Result<(), CompatibilityError> {
        self.validate()?;
        for file in &self.files {
            let bytes = fs::read(repository_root.join(&file.path))
                .map_err(|_| invalid("surface_file_unavailable"))?;
            if sha256_bytes(&bytes) != file.sha256 {
                return Err(invalid("surface_file_changed"));
            }
        }
        Ok(())
    }

    pub fn from_json(bytes: &[u8]) -> Result<Self, CompatibilityError> {
        let value: Self = parse_bounded_json(bytes)?;
        value.validate()?;
        Ok(value)
    }

    pub fn to_pretty_json(&self) -> Result<Vec<u8>, CompatibilityError> {
        self.validate()?;
        let bytes = serde_json::to_vec_pretty(self).map_err(|_| invalid("serialization_failed"))?;
        if bytes.len() > MAX_ARTIFACT_BYTES {
            return Err(invalid("artifact_too_large"));
        }
        Ok(bytes)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TrustedEvidenceKey {
    pub key_id: String,
    pub public_key_hex: String,
    pub valid_from_unix_ms: i64,
    pub valid_until_unix_ms: i64,
    pub revoked_at_unix_ms: Option<i64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TrustedEvidenceKeys {
    pub schema_version: u16,
    pub keys: Vec<TrustedEvidenceKey>,
}

impl TrustedEvidenceKeys {
    pub fn validate(&self) -> Result<(), CompatibilityError> {
        if self.schema_version != TRUST_MANIFEST_SCHEMA_VERSION || self.keys.len() > MAX_KEYS {
            return Err(invalid("trust_manifest_invalid"));
        }
        let mut ids = BTreeSet::new();
        for key in &self.keys {
            validate_slug(&key.key_id)?;
            let bytes =
                hex::decode(&key.public_key_hex).map_err(|_| invalid("public_key_invalid"))?;
            let _: [u8; 32] = bytes
                .try_into()
                .map_err(|_| invalid("public_key_invalid"))?;
            if key.valid_from_unix_ms <= 0 || key.valid_until_unix_ms <= key.valid_from_unix_ms {
                return Err(invalid("key_validity_invalid"));
            }
            if key
                .revoked_at_unix_ms
                .is_some_and(|value| value < key.valid_from_unix_ms)
            {
                return Err(invalid("key_revocation_invalid"));
            }
            if !ids.insert(&key.key_id) {
                return Err(invalid("duplicate_key_id"));
            }
        }
        Ok(())
    }

    pub fn from_json(bytes: &[u8]) -> Result<Self, CompatibilityError> {
        let value: Self = parse_bounded_json(bytes)?;
        value.validate()?;
        Ok(value)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReviewedEvidenceAttestation {
    pub schema_version: u16,
    pub evidence_id: String,
    pub receipt_sha256: String,
    pub compatibility_surface_sha256: String,
    pub reviewed_at_unix_ms: i64,
    pub expires_at_unix_ms: i64,
    pub review_commit_sha: String,
    pub review_url: String,
    pub key_id: String,
    pub signature_hex: String,
}

impl ReviewedEvidenceAttestation {
    fn signing_bytes(&self) -> Result<Vec<u8>, CompatibilityError> {
        #[derive(Serialize)]
        struct Signed<'a> {
            domain: &'static str,
            schema_version: u16,
            evidence_id: &'a str,
            receipt_sha256: &'a str,
            compatibility_surface_sha256: &'a str,
            reviewed_at_unix_ms: i64,
            expires_at_unix_ms: i64,
            review_commit_sha: &'a str,
            review_url: &'a str,
            key_id: &'a str,
        }
        serde_json::to_vec(&Signed {
            domain: "bridge.tally.reviewed-live-evidence/1",
            schema_version: self.schema_version,
            evidence_id: &self.evidence_id,
            receipt_sha256: &self.receipt_sha256,
            compatibility_surface_sha256: &self.compatibility_surface_sha256,
            reviewed_at_unix_ms: self.reviewed_at_unix_ms,
            expires_at_unix_ms: self.expires_at_unix_ms,
            review_commit_sha: &self.review_commit_sha,
            review_url: &self.review_url,
            key_id: &self.key_id,
        })
        .map_err(|_| invalid("attestation_serialization_failed"))
    }

    pub fn validate_shape(&self) -> Result<(), CompatibilityError> {
        if self.schema_version != ATTESTATION_SCHEMA_VERSION {
            return Err(invalid("attestation_schema_unsupported"));
        }
        validate_slug(&self.evidence_id)?;
        validate_slug(&self.key_id)?;
        validate_sha256(&self.receipt_sha256)?;
        validate_sha256(&self.compatibility_surface_sha256)?;
        validate_commit(&self.review_commit_sha)?;
        if self.reviewed_at_unix_ms <= 0 || self.expires_at_unix_ms <= self.reviewed_at_unix_ms {
            return Err(invalid("attestation_time_invalid"));
        }
        if !self
            .review_url
            .starts_with("https://github.com/lamemustafa/bridge/")
            || self.review_url.len() > 256
            || self.review_url.chars().any(char::is_control)
        {
            return Err(invalid("review_url_invalid"));
        }
        let signature = hex::decode(&self.signature_hex)
            .map_err(|_| invalid("attestation_signature_invalid"))?;
        let _: [u8; 64] = signature
            .try_into()
            .map_err(|_| invalid("attestation_signature_invalid"))?;
        Ok(())
    }

    pub fn verify(
        &self,
        trust: &TrustedEvidenceKeys,
        now_unix_ms: i64,
    ) -> Result<(), CompatibilityError> {
        self.validate_shape()?;
        trust.validate()?;
        let key = trust
            .keys
            .iter()
            .find(|key| key.key_id == self.key_id)
            .ok_or_else(|| gate("attestation_key_untrusted"))?;
        if now_unix_ms < key.valid_from_unix_ms
            || now_unix_ms > key.valid_until_unix_ms
            || key
                .revoked_at_unix_ms
                .is_some_and(|revoked| now_unix_ms >= revoked)
        {
            return Err(gate("attestation_key_inactive"));
        }
        if self.reviewed_at_unix_ms < key.valid_from_unix_ms
            || self.reviewed_at_unix_ms > key.valid_until_unix_ms
            || key
                .revoked_at_unix_ms
                .is_some_and(|revoked| self.reviewed_at_unix_ms >= revoked)
        {
            return Err(gate("attestation_review_key_inactive"));
        }
        if self.reviewed_at_unix_ms > now_unix_ms.saturating_add(MAX_FUTURE_SKEW_MS)
            || now_unix_ms > self.expires_at_unix_ms
        {
            return Err(gate("attestation_not_current"));
        }
        let public: [u8; 32] = hex::decode(&key.public_key_hex)
            .map_err(|_| invalid("public_key_invalid"))?
            .try_into()
            .map_err(|_| invalid("public_key_invalid"))?;
        let verifier =
            VerifyingKey::from_bytes(&public).map_err(|_| invalid("public_key_invalid"))?;
        let signature: [u8; 64] = hex::decode(&self.signature_hex)
            .map_err(|_| invalid("attestation_signature_invalid"))?
            .try_into()
            .map_err(|_| invalid("attestation_signature_invalid"))?;
        verifier
            .verify(&self.signing_bytes()?, &Signature::from_bytes(&signature))
            .map_err(|_| gate("attestation_signature_unverified"))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ClaimLevel {
    Unknown,
    Observed,
    Supported,
    Unsupported,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SupportClaim {
    pub claim_id: String,
    pub level: ClaimLevel,
    pub promotion_eligible: bool,
    pub product: ProductFamily,
    pub release: String,
    pub mode: TallyMode,
    pub platform: Platform,
    pub architecture: Architecture,
    pub transport: TransportProfile,
    pub endpoint_family: LoopbackFamily,
    pub odbc_state: OdbcState,
    pub company_state: CompanyLoadState,
    pub locale: LocaleProfile,
    pub encoding: TextEncoding,
    pub dataset_tier: DatasetTier,
    pub fixture_manifest_sha256: Option<String>,
    pub required_profiles: Vec<ReadProfileId>,
    pub max_evidence_age_days: u16,
    pub evidence_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SupportClaimsManifest {
    pub schema_version: u16,
    pub bridge_commit_sha: String,
    pub compatibility_surface_sha256: String,
    pub claims: Vec<SupportClaim>,
}

impl SupportClaimsManifest {
    pub fn validate(&self) -> Result<(), CompatibilityError> {
        if self.schema_version != SUPPORT_MANIFEST_SCHEMA_VERSION
            || self.claims.is_empty()
            || self.claims.len() > MAX_CLAIMS
        {
            return Err(invalid("support_manifest_invalid"));
        }
        validate_commit(&self.bridge_commit_sha)?;
        validate_sha256(&self.compatibility_surface_sha256)?;
        let mut ids = BTreeSet::new();
        let mut previous_id: Option<&str> = None;
        for claim in &self.claims {
            validate_claim(claim)?;
            if previous_id.is_some_and(|value| value >= claim.claim_id.as_str()) {
                return Err(invalid("claims_not_unique_sorted"));
            }
            previous_id = Some(&claim.claim_id);
            if !ids.insert(&claim.claim_id) {
                return Err(invalid("duplicate_claim_id"));
            }
        }
        Ok(())
    }

    pub fn from_json(bytes: &[u8]) -> Result<Self, CompatibilityError> {
        let value: Self = parse_bounded_json(bytes)?;
        value.validate()?;
        Ok(value)
    }
}

fn validate_claim(claim: &SupportClaim) -> Result<(), CompatibilityError> {
    validate_slug(&claim.claim_id)?;
    validate_label(&claim.release)?;
    if let Some(fixture) = &claim.fixture_manifest_sha256 {
        validate_sha256(fixture)?;
    }
    if !(1..=365).contains(&claim.max_evidence_age_days) {
        return Err(invalid("claim_age_invalid"));
    }
    let mut previous = None;
    for profile in &claim.required_profiles {
        if previous.is_some_and(|value| value >= *profile) {
            return Err(invalid("claim_profiles_not_unique_sorted"));
        }
        previous = Some(*profile);
    }
    match claim.level {
        ClaimLevel::Unknown => {
            if claim.evidence_id.is_some() {
                return Err(invalid("unknown_claim_has_evidence"));
            }
        }
        ClaimLevel::Observed | ClaimLevel::Supported | ClaimLevel::Unsupported => {
            if claim.product == ProductFamily::Unknown
                || claim.mode == TallyMode::Unknown
                || claim.release == "unknown"
                || claim.odbc_state == OdbcState::Unknown
                || claim.company_state == CompanyLoadState::Unknown
                || claim.locale == LocaleProfile::Unknown
                || claim.encoding == TextEncoding::Unknown
                || claim.dataset_tier == DatasetTier::Unknown
                || claim.fixture_manifest_sha256.is_none()
                || claim.required_profiles.is_empty()
            {
                return Err(invalid("positive_claim_scope_incomplete"));
            }
            validate_exact_release(&claim.release)?;
            validate_sha256(
                claim
                    .fixture_manifest_sha256
                    .as_deref()
                    .ok_or_else(|| invalid("positive_claim_fixture_missing"))?,
            )?;
            validate_slug(
                claim
                    .evidence_id
                    .as_deref()
                    .ok_or_else(|| invalid("positive_claim_missing_evidence"))?,
            )?;
        }
    }
    match claim.level {
        ClaimLevel::Observed | ClaimLevel::Supported if !claim.promotion_eligible => {
            return Err(invalid("positive_claim_not_promotion_eligible"));
        }
        ClaimLevel::Unsupported if claim.promotion_eligible => {
            return Err(invalid("unsupported_claim_promotion_eligible"));
        }
        _ => {}
    }
    match claim.transport {
        TransportProfile::XmlHttp => {
            if claim
                .required_profiles
                .contains(&ReadProfileId::JsonExSemanticShadowV1)
            {
                return Err(invalid("xml_claim_contains_jsonex_profile"));
            }
        }
        TransportProfile::JsonExShadow => {
            if claim
                .required_profiles
                .iter()
                .any(|profile| *profile != ReadProfileId::JsonExSemanticShadowV1)
            {
                return Err(invalid("jsonex_claim_contains_xml_profile"));
            }
            if claim.promotion_eligible || claim.level != ClaimLevel::Unknown {
                return Err(invalid("jsonex_claim_not_qualifiable"));
            }
        }
    }
    if claim.level == ClaimLevel::Supported {
        if claim.transport != TransportProfile::XmlHttp {
            return Err(invalid("supported_claim_transport_invalid"));
        }
        let required: BTreeSet<_> = [
            ReadProfileId::XmlCompanyEnumerationV1,
            ReadProfileId::XmlSyntheticFixtureMarkerV1,
            ReadProfileId::XmlLedgerReadV1,
            ReadProfileId::XmlVoucherEmptyRangeV1,
            ReadProfileId::XmlVoucherPopulatedRangeV1,
        ]
        .into_iter()
        .collect();
        if !required.is_subset(&claim.required_profiles.iter().copied().collect()) {
            return Err(invalid("supported_claim_missing_core_profiles"));
        }
    }
    Ok(())
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GateReport {
    pub unknown_claims: usize,
    pub evidenced_claims: usize,
}

pub fn enforce_support_gate(
    manifest: &SupportClaimsManifest,
    surface: &CompatibilitySurfaceManifest,
    trust: &TrustedEvidenceKeys,
    receipts: &[LiveCompatibilityReceipt],
    attestations: &[ReviewedEvidenceAttestation],
    repository_root: &Path,
    now_unix_ms: i64,
) -> Result<GateReport, CompatibilityError> {
    manifest.validate()?;
    surface.validate_files(repository_root)?;
    trust.validate()?;
    if manifest.compatibility_surface_sha256 != surface.manifest_sha256 {
        return Err(gate("support_surface_mismatch"));
    }
    let receipt_by_checksum = receipts
        .iter()
        .map(|receipt| {
            receipt.validate()?;
            Ok((receipt.receipt_sha256.as_str(), receipt))
        })
        .collect::<Result<BTreeMap<_, _>, CompatibilityError>>()?;
    let attestation_by_id = attestations
        .iter()
        .map(|attestation| {
            attestation.validate_shape()?;
            Ok((attestation.evidence_id.as_str(), attestation))
        })
        .collect::<Result<BTreeMap<_, _>, CompatibilityError>>()?;
    if receipt_by_checksum.len() != receipts.len() || attestation_by_id.len() != attestations.len()
    {
        return Err(gate("duplicate_evidence"));
    }

    let mut report = GateReport {
        unknown_claims: 0,
        evidenced_claims: 0,
    };
    for claim in &manifest.claims {
        if claim.level == ClaimLevel::Unknown {
            report.unknown_claims += 1;
            continue;
        }
        report.evidenced_claims += 1;
        let evidence_id = claim
            .evidence_id
            .as_deref()
            .ok_or_else(|| gate("evidence_missing"))?;
        let attestation = attestation_by_id
            .get(evidence_id)
            .copied()
            .ok_or_else(|| gate("attestation_missing"))?;
        attestation.verify(trust, now_unix_ms)?;
        if attestation.compatibility_surface_sha256 != surface.manifest_sha256
            || attestation.review_commit_sha != manifest.bridge_commit_sha
        {
            return Err(gate("attestation_scope_mismatch"));
        }
        let max_age_ms = i64::from(claim.max_evidence_age_days) * 24 * 60 * 60 * 1000;
        if now_unix_ms.saturating_sub(attestation.reviewed_at_unix_ms) > max_age_ms {
            return Err(gate("reviewed_evidence_stale"));
        }
        let receipt = receipt_by_checksum
            .get(attestation.receipt_sha256.as_str())
            .copied()
            .ok_or_else(|| gate("receipt_missing"))?;
        validate_receipt_for_claim(receipt, claim, manifest, surface, attestation, now_unix_ms)?;
    }
    Ok(report)
}

fn validate_receipt_for_claim(
    receipt: &LiveCompatibilityReceipt,
    claim: &SupportClaim,
    manifest: &SupportClaimsManifest,
    surface: &CompatibilitySurfaceManifest,
    attestation: &ReviewedEvidenceAttestation,
    now_unix_ms: i64,
) -> Result<(), CompatibilityError> {
    let max_age_ms = i64::from(claim.max_evidence_age_days) * 24 * 60 * 60 * 1000;
    if receipt.working_tree_dirty
        || receipt.bridge_commit_sha != manifest.bridge_commit_sha
        || receipt.compatibility_surface_sha256 != surface.manifest_sha256
        || receipt.receipt_sha256 != attestation.receipt_sha256
        || receipt.observed_at_unix_ms > attestation.reviewed_at_unix_ms
        || receipt.product.value != claim.product
        || receipt.release.value != claim.release
        || receipt.mode.value != claim.mode
        || receipt.platform != claim.platform
        || receipt.architecture != claim.architecture
        || receipt.transport != claim.transport
        || receipt.endpoint_family != claim.endpoint_family
        || receipt.odbc_state.value != claim.odbc_state
        || company_state(receipt.loaded_company_count) != claim.company_state
        || receipt.locale.value != claim.locale
        || receipt.dataset_tier.value != claim.dataset_tier
        || Some(receipt.fixture_manifest_sha256.as_str())
            != claim.fixture_manifest_sha256.as_deref()
        || !receipt.operations.iter().all(|operation| {
            operation.outcome == OperationOutcome::NotAttempted
                || operation.encoding == claim.encoding
        })
        || receipt.observed_at_unix_ms > now_unix_ms.saturating_add(MAX_FUTURE_SKEW_MS)
        || now_unix_ms.saturating_sub(receipt.observed_at_unix_ms) > max_age_ms
        || attestation
            .reviewed_at_unix_ms
            .saturating_sub(receipt.observed_at_unix_ms)
            > max_age_ms
        || !receipt.no_customer_data.value
        || !receipt.authority.live_endpoint_response_observed
    {
        return Err(gate("receipt_claim_scope_mismatch"));
    }
    if receipt.product.authority != EvidenceAuthority::UserAttestation
        || receipt.product.confidence != EvidenceConfidence::Attested
        || receipt.release.authority != EvidenceAuthority::UserAttestation
        || receipt.release.confidence != EvidenceConfidence::Attested
    {
        return Err(gate("receipt_profile_authority_insufficient"));
    }
    if receipt.dataset_tier.authority != EvidenceAuthority::BridgeConfiguration
        || receipt.dataset_tier.confidence != EvidenceConfidence::Attested
        || receipt.no_customer_data.authority != EvidenceAuthority::UserAttestation
        || receipt.no_customer_data.confidence != EvidenceConfidence::Attested
    {
        return Err(gate("receipt_dataset_authority_insufficient"));
    }
    let operations: BTreeMap<_, _> = receipt
        .operations
        .iter()
        .map(|operation| (operation.profile, operation))
        .collect();
    match claim.level {
        ClaimLevel::Observed | ClaimLevel::Supported => {
            if !receipt.fixture_marker_verified {
                return Err(gate("fixture_marker_contract_not_verified"));
            }
            for profile in &claim.required_profiles {
                let operation = operations
                    .get(profile)
                    .ok_or_else(|| gate("required_operation_missing"))?;
                if operation.outcome != OperationOutcome::Passed
                    || operation.application_status != ApplicationStatus::Success
                {
                    return Err(gate("required_operation_not_passed"));
                }
            }
        }
        ClaimLevel::Unsupported => {
            if !receipt.fixture_marker_verified {
                return Err(gate("fixture_marker_contract_not_verified"));
            }
            return Err(gate("unsupported_claim_signature_unavailable"));
        }
        ClaimLevel::Unknown => return Err(gate("unknown_claim_reached_evidence_validation")),
    }
    Ok(())
}

fn company_state(count: CountBucket) -> CompanyLoadState {
    match count {
        CountBucket::Zero => CompanyLoadState::None,
        CountBucket::One => CompanyLoadState::One,
        CountBucket::TwoToFive | CountBucket::SixToTwenty | CountBucket::OverTwenty => {
            CompanyLoadState::Multiple
        }
        CountBucket::Unknown => CompanyLoadState::Unknown,
    }
}

pub fn parse_artifact<T: DeserializeOwned>(bytes: &[u8]) -> Result<T, CompatibilityError> {
    parse_bounded_json(bytes)
}

fn parse_bounded_json<T: DeserializeOwned>(bytes: &[u8]) -> Result<T, CompatibilityError> {
    if bytes.is_empty() || bytes.len() > MAX_ARTIFACT_BYTES {
        return Err(invalid("artifact_size_invalid"));
    }
    serde_json::from_slice(bytes).map_err(|_| invalid("artifact_json_invalid"))
}

fn checksum<T: Serialize>(domain: &[u8], value: &T) -> Result<String, CompatibilityError> {
    let bytes = serde_json::to_vec(value).map_err(|_| invalid("serialization_failed"))?;
    let mut digest = Sha256::new();
    digest.update(domain);
    digest.update(bytes);
    Ok(hex::encode(digest.finalize()))
}

fn sha256_bytes(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}

fn validate_sha256(value: &str) -> Result<(), CompatibilityError> {
    if value.len() != 64
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return Err(invalid("sha256_invalid"));
    }
    Ok(())
}

fn validate_commit(value: &str) -> Result<(), CompatibilityError> {
    if !matches!(value.len(), 40 | 64)
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return Err(invalid("commit_invalid"));
    }
    Ok(())
}

fn validate_slug(value: &str) -> Result<(), CompatibilityError> {
    if value.is_empty()
        || value.len() > 96
        || !value.bytes().all(|byte| {
            byte.is_ascii_lowercase() || byte.is_ascii_digit() || matches!(byte, b'-' | b'_')
        })
    {
        return Err(invalid("slug_invalid"));
    }
    Ok(())
}

fn validate_safe_code(value: &str) -> Result<(), CompatibilityError> {
    validate_slug(value)
}

fn validate_label(value: &str) -> Result<(), CompatibilityError> {
    if value.is_empty()
        || value.len() > 64
        || value.trim() != value
        || value.chars().any(char::is_control)
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'-' | b'_' | b' '))
    {
        return Err(invalid("label_invalid"));
    }
    Ok(())
}

fn validate_exact_release(value: &str) -> Result<(), CompatibilityError> {
    let normalized = value.to_ascii_lowercase();
    if matches!(normalized.as_str(), "unknown" | "latest")
        || normalized.contains('*')
        || normalized.ends_with(".x")
    {
        return Err(invalid("exact_release_required"));
    }
    Ok(())
}

fn validate_relative_path(value: &str) -> Result<(), CompatibilityError> {
    if value.is_empty() || value.len() > 240 || value.contains('\\') {
        return Err(invalid("surface_path_invalid"));
    }
    let path = Path::new(value);
    if path.is_absolute()
        || path
            .components()
            .any(|part| !matches!(part, Component::Normal(_)))
    {
        return Err(invalid("surface_path_invalid"));
    }
    Ok(())
}

pub fn sha256_file(path: &Path) -> Result<String, CompatibilityError> {
    let bytes = fs::read(path).map_err(|_| invalid("file_unavailable"))?;
    Ok(sha256_bytes(&bytes))
}

pub fn now_unix_ms() -> Result<i64, CompatibilityError> {
    let elapsed = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_err(|_| invalid("system_clock_invalid"))?;
    i64::try_from(elapsed.as_millis()).map_err(|_| invalid("system_clock_invalid"))
}

pub fn safe_error_code(error: &CompatibilityError) -> &'static str {
    match error {
        CompatibilityError::Invalid { code } | CompatibilityError::Gate { code } => code,
    }
}

pub fn format_gate_success(report: &GateReport) -> String {
    let mut output = String::new();
    write!(
        &mut output,
        "compatibility_gate_passed:unknown_claims={}:evidenced_claims={}",
        report.unknown_claims, report.evidenced_claims
    )
    .expect("writing to a String cannot fail");
    output
}

pub fn render_claim_matrix(manifest: &SupportClaimsManifest) -> Result<String, CompatibilityError> {
    manifest.validate()?;
    let mut output = String::from(
        "<!-- BEGIN GENERATED TALLY COMPATIBILITY CLAIMS -->\n\
| Exact cell | Product / release / mode | Host | Transport / loopback / ODBC | Data profile | Claim | Promotion eligible | Evidence |\n\
| --- | --- | --- | --- | --- | --- | --- | --- |\n",
    );
    for claim in &manifest.claims {
        let evidence = claim.evidence_id.as_deref().unwrap_or("missing");
        writeln!(
            &mut output,
            "| `{}` | `{}` / `{}` / `{}` | `{}` / `{}` | `{}` / `{}` / `{}` | `{}` / `{}` / `{}` / `{}` | `{}` | `{}` | `{}` |",
            claim.claim_id,
            enum_label(&claim.product)?,
            claim.release,
            enum_label(&claim.mode)?,
            enum_label(&claim.platform)?,
            enum_label(&claim.architecture)?,
            enum_label(&claim.transport)?,
            enum_label(&claim.endpoint_family)?,
            enum_label(&claim.odbc_state)?,
            enum_label(&claim.company_state)?,
            enum_label(&claim.locale)?,
            enum_label(&claim.encoding)?,
            enum_label(&claim.dataset_tier)?,
            enum_label(&claim.level)?,
            claim.promotion_eligible,
            evidence,
        )
        .map_err(|_| invalid("matrix_render_failed"))?;
    }
    output.push_str("<!-- END GENERATED TALLY COMPATIBILITY CLAIMS -->");
    Ok(output)
}

pub fn verify_claim_matrix_markdown(
    manifest: &SupportClaimsManifest,
    markdown: &[u8],
) -> Result<(), CompatibilityError> {
    if markdown.is_empty() || markdown.len() > MAX_MATRIX_MARKDOWN_BYTES {
        return Err(invalid("matrix_markdown_size_invalid"));
    }
    let text = std::str::from_utf8(markdown).map_err(|_| invalid("matrix_markdown_invalid"))?;
    let expected = render_claim_matrix(manifest)?;
    if text
        .matches("<!-- BEGIN GENERATED TALLY COMPATIBILITY CLAIMS -->")
        .count()
        != 1
        || text
            .matches("<!-- END GENERATED TALLY COMPATIBILITY CLAIMS -->")
            .count()
            != 1
        || !text.contains(&expected)
    {
        return Err(invalid("matrix_markdown_drift"));
    }
    Ok(())
}

fn enum_label<T: Serialize>(value: &T) -> Result<String, CompatibilityError> {
    let serialized = serde_json::to_string(value).map_err(|_| invalid("serialization_failed"))?;
    serialized
        .strip_prefix('"')
        .and_then(|value| value.strip_suffix('"'))
        .map(str::to_owned)
        .ok_or_else(|| invalid("matrix_render_failed"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use ed25519_dalek::{Signer, SigningKey};

    const SHA: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
    const COMMIT: &str = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";
    const NOW: i64 = 1_800_000_000_000;

    fn profile<T>(value: T) -> ProfileValue<T> {
        ProfileValue {
            value,
            authority: EvidenceAuthority::UserAttestation,
            confidence: EvidenceConfidence::Attested,
        }
    }

    fn configured<T>(value: T) -> ProfileValue<T> {
        ProfileValue {
            value,
            authority: EvidenceAuthority::BridgeConfiguration,
            confidence: EvidenceConfidence::Attested,
        }
    }

    fn operation(profile: ReadProfileId) -> OperationEvidence {
        OperationEvidence {
            profile,
            template_sha256: SHA.to_string(),
            outcome: OperationOutcome::Passed,
            application_status: ApplicationStatus::Success,
            encoding: TextEncoding::Utf8,
            response_size: SizeBucket::Bytes1To4096,
            record_count: CountBucket::One,
            safe_reason_code: None,
        }
    }

    fn receipt(surface: &str) -> LiveCompatibilityReceipt {
        LiveCompatibilityReceipt {
            schema_version: LIVE_RECEIPT_SCHEMA_VERSION,
            observed_at_unix_ms: NOW - 10_000,
            bridge_commit_sha: COMMIT.to_string(),
            working_tree_dirty: false,
            compatibility_surface_sha256: surface.to_string(),
            executable_sha256: SHA.to_string(),
            cargo_lock_sha256: SHA.to_string(),
            platform: Platform::Windows,
            architecture: Architecture::X86_64,
            endpoint_family: LoopbackFamily::Ipv4,
            transport: TransportProfile::XmlHttp,
            product: profile(ProductFamily::TallyPrime),
            release: profile("7.1".to_string()),
            mode: profile(TallyMode::Education),
            odbc_state: profile(OdbcState::Disabled),
            locale: profile(LocaleProfile::EnglishIndia),
            dataset_tier: configured(DatasetTier::SyntheticSmall),
            fixture_manifest_sha256: SHA.to_string(),
            fixture_marker_verified: true,
            no_customer_data: profile(true),
            loaded_company_count: CountBucket::One,
            operations: [
                ReadProfileId::XmlCompanyEnumerationV1,
                ReadProfileId::XmlSyntheticFixtureMarkerV1,
                ReadProfileId::XmlLedgerReadV1,
                ReadProfileId::XmlVoucherEmptyRangeV1,
                ReadProfileId::XmlVoucherPopulatedRangeV1,
            ]
            .into_iter()
            .map(operation)
            .collect(),
            authority: LiveReadAuthority::observation_only(),
            receipt_sha256: String::new(),
        }
        .seal()
        .unwrap()
    }

    fn trust(signing: &SigningKey) -> TrustedEvidenceKeys {
        TrustedEvidenceKeys {
            schema_version: TRUST_MANIFEST_SCHEMA_VERSION,
            keys: vec![TrustedEvidenceKey {
                key_id: "release-evidence-1".to_string(),
                public_key_hex: hex::encode(signing.verifying_key().to_bytes()),
                valid_from_unix_ms: NOW - 100_000,
                valid_until_unix_ms: NOW + 100_000,
                revoked_at_unix_ms: None,
            }],
        }
    }

    fn attestation(
        receipt: &LiveCompatibilityReceipt,
        surface: &CompatibilitySurfaceManifest,
        signing: &SigningKey,
    ) -> ReviewedEvidenceAttestation {
        let mut value = ReviewedEvidenceAttestation {
            schema_version: ATTESTATION_SCHEMA_VERSION,
            evidence_id: "evidence-1".to_string(),
            receipt_sha256: receipt.receipt_sha256.clone(),
            compatibility_surface_sha256: surface.manifest_sha256.clone(),
            reviewed_at_unix_ms: NOW - 1_000,
            expires_at_unix_ms: NOW + 50_000,
            review_commit_sha: COMMIT.to_string(),
            review_url: "https://github.com/lamemustafa/bridge/pull/1".to_string(),
            key_id: "release-evidence-1".to_string(),
            signature_hex: "00".repeat(64),
        };
        value.signature_hex = hex::encode(signing.sign(&value.signing_bytes().unwrap()).to_bytes());
        value
    }

    fn unsupported_manifest(
        surface: &CompatibilitySurfaceManifest,
        required_profile: ReadProfileId,
    ) -> SupportClaimsManifest {
        SupportClaimsManifest {
            schema_version: SUPPORT_MANIFEST_SCHEMA_VERSION,
            bridge_commit_sha: COMMIT.to_string(),
            compatibility_surface_sha256: surface.manifest_sha256.clone(),
            claims: vec![SupportClaim {
                claim_id: "unsupported-exact-scope".to_string(),
                level: ClaimLevel::Unsupported,
                promotion_eligible: false,
                product: ProductFamily::TallyPrime,
                release: "7.1".to_string(),
                mode: TallyMode::Education,
                platform: Platform::Windows,
                architecture: Architecture::X86_64,
                transport: TransportProfile::XmlHttp,
                endpoint_family: LoopbackFamily::Ipv4,
                odbc_state: OdbcState::Disabled,
                company_state: CompanyLoadState::One,
                locale: LocaleProfile::EnglishIndia,
                encoding: TextEncoding::Utf8,
                dataset_tier: DatasetTier::SyntheticSmall,
                fixture_manifest_sha256: Some(SHA.to_string()),
                required_profiles: vec![required_profile],
                max_evidence_age_days: 30,
                evidence_id: Some("evidence-1".to_string()),
            }],
        }
    }

    fn not_attempted_operation(profile: ReadProfileId) -> OperationEvidence {
        OperationEvidence {
            profile,
            template_sha256: SHA.to_string(),
            outcome: OperationOutcome::NotAttempted,
            application_status: ApplicationStatus::NotApplicable,
            encoding: TextEncoding::Unknown,
            response_size: SizeBucket::Zero,
            record_count: CountBucket::Unknown,
            safe_reason_code: Some("fixture_not_verified".to_string()),
        }
    }

    #[test]
    fn receipt_round_trip_is_bounded_private_and_checksum_bound() {
        let receipt = receipt(SHA);
        let bytes = receipt.to_pretty_json().unwrap();
        assert!(bytes.len() < MAX_ARTIFACT_BYTES);
        assert_eq!(
            LiveCompatibilityReceipt::from_json(&bytes).unwrap(),
            receipt
        );
        let mut tampered = receipt.clone();
        tampered.loaded_company_count = CountBucket::TwoToFive;
        assert_eq!(
            tampered.validate().unwrap_err(),
            invalid("receipt_checksum_mismatch")
        );
        let text = String::from_utf8(bytes).unwrap();
        for forbidden in [
            "company_name",
            "company_guid",
            "endpoint_port",
            "raw_xml",
            "amount",
        ] {
            assert!(!text.contains(forbidden));
        }
    }

    #[test]
    fn receipt_cannot_claim_support_authenticity_or_writes() {
        for mutate in [
            |value: &mut LiveCompatibilityReceipt| value.authority.support_claim_eligible = true,
            |value: &mut LiveCompatibilityReceipt| value.authority.writes_attempted = true,
            |value: &mut LiveCompatibilityReceipt| {
                value.authority.responder_authenticity_established = true
            },
        ] {
            let mut value = receipt(SHA);
            value.receipt_sha256.clear();
            mutate(&mut value);
            assert_eq!(
                value.seal().unwrap_err(),
                invalid("receipt_authority_invalid")
            );
        }
        let mut value = receipt(SHA);
        value.receipt_sha256.clear();
        value.authority.tauri_runtime_observed = true;
        assert_eq!(
            value.seal().unwrap_err(),
            invalid("receipt_authority_invalid")
        );
    }

    #[test]
    fn unsuccessful_attempt_is_receipted_without_live_or_fixture_claims() {
        let not_attempted = |profile| OperationEvidence {
            profile,
            template_sha256: SHA.to_string(),
            outcome: OperationOutcome::NotAttempted,
            application_status: ApplicationStatus::NotApplicable,
            encoding: TextEncoding::Unknown,
            response_size: SizeBucket::Zero,
            record_count: CountBucket::Unknown,
            safe_reason_code: Some("fixture_not_verified".to_string()),
        };
        let mut value = receipt(SHA);
        value.receipt_sha256.clear();
        value.fixture_marker_verified = false;
        value.loaded_company_count = CountBucket::Zero;
        value.authority = LiveReadAuthority::attempt_only();
        value.operations = vec![
            OperationEvidence {
                profile: ReadProfileId::XmlCompanyEnumerationV1,
                template_sha256: SHA.to_string(),
                outcome: OperationOutcome::Failed,
                application_status: ApplicationStatus::NotApplicable,
                encoding: TextEncoding::Unknown,
                response_size: SizeBucket::Zero,
                record_count: CountBucket::Zero,
                safe_reason_code: Some("endpoint_unreachable".to_string()),
            },
            not_attempted(ReadProfileId::XmlSyntheticFixtureMarkerV1),
            not_attempted(ReadProfileId::XmlLedgerReadV1),
            not_attempted(ReadProfileId::XmlVoucherEmptyRangeV1),
            not_attempted(ReadProfileId::XmlVoucherPopulatedRangeV1),
        ];
        let sealed = value.seal().unwrap();
        assert!(!sealed.authority.live_endpoint_response_observed);
        assert!(!sealed.fixture_marker_verified);

        let mut illegal = sealed;
        illegal.receipt_sha256.clear();
        illegal.operations[2].outcome = OperationOutcome::Failed;
        illegal.operations[2].safe_reason_code = Some("read_failed".to_string());
        assert_eq!(
            illegal.seal().unwrap_err(),
            invalid("company_read_without_fixture_marker")
        );
    }

    #[test]
    fn unknown_claims_pass_without_live_or_trusted_evidence() {
        let temp = tempfile::tempdir().unwrap();
        fs::write(temp.path().join("surface.txt"), b"surface").unwrap();
        let file_sha = sha256_file(&temp.path().join("surface.txt")).unwrap();
        let surface = CompatibilitySurfaceManifest {
            schema_version: SURFACE_SCHEMA_VERSION,
            files: vec![SurfaceFile {
                path: "surface.txt".to_string(),
                sha256: file_sha,
            }],
            manifest_sha256: String::new(),
        }
        .seal()
        .unwrap();
        let manifest = SupportClaimsManifest {
            schema_version: SUPPORT_MANIFEST_SCHEMA_VERSION,
            bridge_commit_sha: COMMIT.to_string(),
            compatibility_surface_sha256: surface.manifest_sha256.clone(),
            claims: vec![SupportClaim {
                claim_id: "tally-prime-7-1-windows-education".to_string(),
                level: ClaimLevel::Unknown,
                promotion_eligible: true,
                product: ProductFamily::TallyPrime,
                release: "7.1".to_string(),
                mode: TallyMode::Education,
                platform: Platform::Windows,
                architecture: Architecture::X86_64,
                transport: TransportProfile::XmlHttp,
                endpoint_family: LoopbackFamily::Ipv4,
                odbc_state: OdbcState::Disabled,
                company_state: CompanyLoadState::One,
                locale: LocaleProfile::EnglishIndia,
                encoding: TextEncoding::Utf8,
                dataset_tier: DatasetTier::SyntheticSmall,
                fixture_manifest_sha256: None,
                required_profiles: Vec::new(),
                max_evidence_age_days: 30,
                evidence_id: None,
            }],
        };
        let report = enforce_support_gate(
            &manifest,
            &surface,
            &TrustedEvidenceKeys {
                schema_version: TRUST_MANIFEST_SCHEMA_VERSION,
                keys: Vec::new(),
            },
            &[],
            &[],
            temp.path(),
            NOW,
        )
        .unwrap();
        assert_eq!(report.unknown_claims, 1);
        assert_eq!(report.evidenced_claims, 0);
    }

    #[test]
    fn positive_claim_requires_fresh_signed_exact_scope_evidence() {
        let temp = tempfile::tempdir().unwrap();
        fs::write(temp.path().join("surface.txt"), b"surface").unwrap();
        let surface = CompatibilitySurfaceManifest {
            schema_version: SURFACE_SCHEMA_VERSION,
            files: vec![SurfaceFile {
                path: "surface.txt".to_string(),
                sha256: sha256_file(&temp.path().join("surface.txt")).unwrap(),
            }],
            manifest_sha256: String::new(),
        }
        .seal()
        .unwrap();
        let receipt = receipt(&surface.manifest_sha256);
        let signing = SigningKey::from_bytes(&[7_u8; 32]);
        let trust = TrustedEvidenceKeys {
            schema_version: TRUST_MANIFEST_SCHEMA_VERSION,
            keys: vec![TrustedEvidenceKey {
                key_id: "release-evidence-1".to_string(),
                public_key_hex: hex::encode(signing.verifying_key().to_bytes()),
                valid_from_unix_ms: NOW - 100_000,
                valid_until_unix_ms: NOW + 100_000,
                revoked_at_unix_ms: None,
            }],
        };
        let mut attestation = ReviewedEvidenceAttestation {
            schema_version: ATTESTATION_SCHEMA_VERSION,
            evidence_id: "evidence-1".to_string(),
            receipt_sha256: receipt.receipt_sha256.clone(),
            compatibility_surface_sha256: surface.manifest_sha256.clone(),
            reviewed_at_unix_ms: NOW - 1_000,
            expires_at_unix_ms: NOW + 50_000,
            review_commit_sha: COMMIT.to_string(),
            review_url: "https://github.com/lamemustafa/bridge/pull/1".to_string(),
            key_id: "release-evidence-1".to_string(),
            signature_hex: "00".repeat(64),
        };
        attestation.signature_hex = hex::encode(
            signing
                .sign(&attestation.signing_bytes().unwrap())
                .to_bytes(),
        );
        let profiles = receipt
            .operations
            .iter()
            .map(|value| value.profile)
            .collect();
        let manifest = SupportClaimsManifest {
            schema_version: SUPPORT_MANIFEST_SCHEMA_VERSION,
            bridge_commit_sha: COMMIT.to_string(),
            compatibility_surface_sha256: surface.manifest_sha256.clone(),
            claims: vec![SupportClaim {
                claim_id: "supported-exact-scope".to_string(),
                level: ClaimLevel::Supported,
                promotion_eligible: true,
                product: ProductFamily::TallyPrime,
                release: "7.1".to_string(),
                mode: TallyMode::Education,
                platform: Platform::Windows,
                architecture: Architecture::X86_64,
                transport: TransportProfile::XmlHttp,
                endpoint_family: LoopbackFamily::Ipv4,
                odbc_state: OdbcState::Disabled,
                company_state: CompanyLoadState::One,
                locale: LocaleProfile::EnglishIndia,
                encoding: TextEncoding::Utf8,
                dataset_tier: DatasetTier::SyntheticSmall,
                fixture_manifest_sha256: Some(SHA.to_string()),
                required_profiles: profiles,
                max_evidence_age_days: 30,
                evidence_id: Some("evidence-1".to_string()),
            }],
        };
        assert!(enforce_support_gate(
            &manifest,
            &surface,
            &trust,
            std::slice::from_ref(&receipt),
            std::slice::from_ref(&attestation),
            temp.path(),
            NOW,
        )
        .is_ok());

        let mut review_time_invalid = trust.clone();
        review_time_invalid.keys[0].valid_from_unix_ms = NOW - 500;
        assert_eq!(
            enforce_support_gate(
                &manifest,
                &surface,
                &review_time_invalid,
                std::slice::from_ref(&receipt),
                std::slice::from_ref(&attestation),
                temp.path(),
                NOW,
            )
            .unwrap_err(),
            gate("attestation_review_key_inactive")
        );

        let mut wildcard = manifest.clone();
        wildcard.claims[0].release = "latest".to_string();
        assert_eq!(
            wildcard.validate().unwrap_err(),
            invalid("exact_release_required")
        );

        let mut revoked = trust.clone();
        revoked.keys[0].revoked_at_unix_ms = Some(NOW - 1);
        assert_eq!(
            enforce_support_gate(
                &manifest,
                &surface,
                &revoked,
                &[receipt],
                &[attestation],
                temp.path(),
                NOW,
            )
            .unwrap_err(),
            gate("attestation_key_inactive")
        );
    }

    #[test]
    fn unsupported_claims_remain_disabled_without_a_profile_specific_signature() {
        let temp = tempfile::tempdir().unwrap();
        fs::write(temp.path().join("surface.txt"), b"surface").unwrap();
        let surface = CompatibilitySurfaceManifest {
            schema_version: SURFACE_SCHEMA_VERSION,
            files: vec![SurfaceFile {
                path: "surface.txt".to_string(),
                sha256: sha256_file(&temp.path().join("surface.txt")).unwrap(),
            }],
            manifest_sha256: String::new(),
        }
        .seal()
        .unwrap();
        let signing = SigningKey::from_bytes(&[9_u8; 32]);
        let trust = trust(&signing);

        let mut invented_unsupported = receipt(&surface.manifest_sha256);
        invented_unsupported.receipt_sha256.clear();
        let ledger = invented_unsupported
            .operations
            .iter_mut()
            .find(|operation| operation.profile == ReadProfileId::XmlLedgerReadV1)
            .unwrap();
        ledger.outcome = OperationOutcome::Unsupported;
        ledger.application_status = ApplicationStatus::Failure;
        ledger.safe_reason_code = Some("tally_export_rejected".to_string());
        assert_eq!(
            invented_unsupported.seal().unwrap_err(),
            invalid("unsupported_operation_signature_unavailable")
        );

        let mut later_failure = receipt(&surface.manifest_sha256);
        later_failure.receipt_sha256.clear();
        let ledger = later_failure
            .operations
            .iter_mut()
            .find(|operation| operation.profile == ReadProfileId::XmlLedgerReadV1)
            .unwrap();
        ledger.outcome = OperationOutcome::Failed;
        ledger.application_status = ApplicationStatus::Failure;
        ledger.safe_reason_code = Some("tally_export_rejected".to_string());
        for profile in [
            ReadProfileId::XmlVoucherEmptyRangeV1,
            ReadProfileId::XmlVoucherPopulatedRangeV1,
        ] {
            let operation = later_failure
                .operations
                .iter_mut()
                .find(|operation| operation.profile == profile)
                .unwrap();
            *operation = not_attempted_operation(profile);
        }
        let later_failure = later_failure.seal().unwrap();
        let later_attestation = attestation(&later_failure, &surface, &signing);
        let ledger_manifest = unsupported_manifest(&surface, ReadProfileId::XmlLedgerReadV1);
        assert_eq!(
            enforce_support_gate(
                &ledger_manifest,
                &surface,
                &trust,
                std::slice::from_ref(&later_failure),
                std::slice::from_ref(&later_attestation),
                temp.path(),
                NOW,
            )
            .unwrap_err(),
            gate("unsupported_claim_signature_unavailable")
        );

        for (reason, application_status, transport_failure) in [
            (
                "ledger_fixture_or_context_invalid",
                ApplicationStatus::Success,
                false,
            ),
            (
                "ledger_response_malformed",
                ApplicationStatus::Unrecognized,
                false,
            ),
            (
                "transport_connection_reset",
                ApplicationStatus::NotApplicable,
                true,
            ),
        ] {
            let mut non_authoritative = receipt(&surface.manifest_sha256);
            non_authoritative.receipt_sha256.clear();
            let ledger = non_authoritative
                .operations
                .iter_mut()
                .find(|operation| operation.profile == ReadProfileId::XmlLedgerReadV1)
                .unwrap();
            ledger.outcome = OperationOutcome::Failed;
            ledger.application_status = application_status;
            ledger.safe_reason_code = Some(reason.to_string());
            if transport_failure {
                ledger.encoding = TextEncoding::Unknown;
                ledger.response_size = SizeBucket::Zero;
            }
            for profile in [
                ReadProfileId::XmlVoucherEmptyRangeV1,
                ReadProfileId::XmlVoucherPopulatedRangeV1,
            ] {
                let operation = non_authoritative
                    .operations
                    .iter_mut()
                    .find(|operation| operation.profile == profile)
                    .unwrap();
                *operation = not_attempted_operation(profile);
            }
            let non_authoritative = non_authoritative.seal().unwrap();
            let non_authoritative_attestation = attestation(&non_authoritative, &surface, &signing);
            assert_eq!(
                enforce_support_gate(
                    &ledger_manifest,
                    &surface,
                    &trust,
                    std::slice::from_ref(&non_authoritative),
                    std::slice::from_ref(&non_authoritative_attestation),
                    temp.path(),
                    NOW,
                )
                .unwrap_err(),
                if transport_failure {
                    gate("receipt_claim_scope_mismatch")
                } else {
                    gate("unsupported_claim_signature_unavailable")
                },
                "non-authoritative failure was accepted: {reason}"
            );
        }

        let mut wrong_fixture = later_failure.clone();
        wrong_fixture.receipt_sha256.clear();
        wrong_fixture.fixture_manifest_sha256 = "c".repeat(64);
        let wrong_fixture = wrong_fixture.seal().unwrap();
        let wrong_attestation = attestation(&wrong_fixture, &surface, &signing);
        assert_eq!(
            enforce_support_gate(
                &ledger_manifest,
                &surface,
                &trust,
                std::slice::from_ref(&wrong_fixture),
                std::slice::from_ref(&wrong_attestation),
                temp.path(),
                NOW,
            )
            .unwrap_err(),
            gate("receipt_claim_scope_mismatch")
        );

        let mut missing_fixture = receipt(&surface.manifest_sha256);
        missing_fixture.receipt_sha256.clear();
        missing_fixture.fixture_marker_verified = false;
        for profile in [
            ReadProfileId::XmlSyntheticFixtureMarkerV1,
            ReadProfileId::XmlLedgerReadV1,
            ReadProfileId::XmlVoucherEmptyRangeV1,
            ReadProfileId::XmlVoucherPopulatedRangeV1,
        ] {
            let operation = missing_fixture
                .operations
                .iter_mut()
                .find(|operation| operation.profile == profile)
                .unwrap();
            *operation = not_attempted_operation(profile);
        }
        let missing_fixture = missing_fixture.seal().unwrap();
        let missing_attestation = attestation(&missing_fixture, &surface, &signing);
        assert_eq!(
            enforce_support_gate(
                &ledger_manifest,
                &surface,
                &trust,
                std::slice::from_ref(&missing_fixture),
                std::slice::from_ref(&missing_attestation),
                temp.path(),
                NOW,
            )
            .unwrap_err(),
            gate("fixture_marker_contract_not_verified")
        );

        let mut marker_failure = receipt(&surface.manifest_sha256);
        marker_failure.receipt_sha256.clear();
        marker_failure.fixture_marker_verified = false;
        let marker = marker_failure
            .operations
            .iter_mut()
            .find(|operation| operation.profile == ReadProfileId::XmlSyntheticFixtureMarkerV1)
            .unwrap();
        marker.outcome = OperationOutcome::Failed;
        marker.application_status = ApplicationStatus::Failure;
        marker.safe_reason_code = Some("synthetic_fixture_unverified".to_string());
        for profile in [
            ReadProfileId::XmlLedgerReadV1,
            ReadProfileId::XmlVoucherEmptyRangeV1,
            ReadProfileId::XmlVoucherPopulatedRangeV1,
        ] {
            let operation = marker_failure
                .operations
                .iter_mut()
                .find(|operation| operation.profile == profile)
                .unwrap();
            *operation = not_attempted_operation(profile);
        }
        let marker_failure = marker_failure.seal().unwrap();
        let marker_attestation = attestation(&marker_failure, &surface, &signing);
        let marker_manifest =
            unsupported_manifest(&surface, ReadProfileId::XmlSyntheticFixtureMarkerV1);
        assert_eq!(
            enforce_support_gate(
                &marker_manifest,
                &surface,
                &trust,
                std::slice::from_ref(&marker_failure),
                std::slice::from_ref(&marker_attestation),
                temp.path(),
                NOW,
            )
            .unwrap_err(),
            gate("fixture_marker_contract_not_verified")
        );
    }

    #[test]
    fn surface_manifest_detects_compatibility_drift() {
        let temp = tempfile::tempdir().unwrap();
        fs::write(temp.path().join("surface.txt"), b"before").unwrap();
        let surface = CompatibilitySurfaceManifest {
            schema_version: SURFACE_SCHEMA_VERSION,
            files: vec![SurfaceFile {
                path: "surface.txt".to_string(),
                sha256: sha256_file(&temp.path().join("surface.txt")).unwrap(),
            }],
            manifest_sha256: String::new(),
        }
        .seal()
        .unwrap();
        surface.validate_files(temp.path()).unwrap();
        fs::write(temp.path().join("surface.txt"), b"after").unwrap();
        assert_eq!(
            surface.validate_files(temp.path()).unwrap_err(),
            invalid("surface_file_changed")
        );
    }

    #[test]
    fn rendered_claim_matrix_is_deterministic_and_drift_checked() {
        let manifest = SupportClaimsManifest {
            schema_version: SUPPORT_MANIFEST_SCHEMA_VERSION,
            bridge_commit_sha: COMMIT.to_string(),
            compatibility_surface_sha256: SHA.to_string(),
            claims: vec![SupportClaim {
                claim_id: "prime-7-1-windows-education-xml-one-company".to_string(),
                level: ClaimLevel::Unknown,
                promotion_eligible: true,
                product: ProductFamily::TallyPrime,
                release: "7.1".to_string(),
                mode: TallyMode::Education,
                platform: Platform::Windows,
                architecture: Architecture::X86_64,
                transport: TransportProfile::XmlHttp,
                endpoint_family: LoopbackFamily::Ipv4,
                odbc_state: OdbcState::Disabled,
                company_state: CompanyLoadState::One,
                locale: LocaleProfile::EnglishIndia,
                encoding: TextEncoding::Utf8,
                dataset_tier: DatasetTier::SyntheticSmall,
                fixture_manifest_sha256: None,
                required_profiles: Vec::new(),
                max_evidence_age_days: 180,
                evidence_id: None,
            }],
        };
        let rendered = render_claim_matrix(&manifest).unwrap();
        assert!(rendered.contains("`unknown` | `true` | `missing`"));
        let document = format!("# Matrix\n\n{rendered}\n");
        verify_claim_matrix_markdown(&manifest, document.as_bytes()).unwrap();
        assert_eq!(
            verify_claim_matrix_markdown(&manifest, b"# Matrix\n").unwrap_err(),
            invalid("matrix_markdown_drift")
        );

        let mut unsupported = manifest.clone();
        unsupported.claims[0].level = ClaimLevel::Unsupported;
        unsupported.claims[0].promotion_eligible = false;
        unsupported.claims[0].fixture_manifest_sha256 = Some(SHA.to_string());
        unsupported.claims[0].required_profiles = vec![ReadProfileId::XmlCompanyEnumerationV1];
        unsupported.claims[0].evidence_id = Some("observed-failure-1".to_string());
        assert!(unsupported.validate().is_ok());

        let mut non_promotable_positive = unsupported.clone();
        non_promotable_positive.claims[0].level = ClaimLevel::Observed;
        assert_eq!(
            non_promotable_positive.validate().unwrap_err(),
            invalid("positive_claim_not_promotion_eligible")
        );

        let mut mixed_transport = manifest;
        mixed_transport.claims[0].transport = TransportProfile::JsonExShadow;
        mixed_transport.claims[0].promotion_eligible = false;
        mixed_transport.claims[0].required_profiles = vec![ReadProfileId::XmlCompanyEnumerationV1];
        assert_eq!(
            mixed_transport.validate().unwrap_err(),
            invalid("jsonex_claim_contains_xml_profile")
        );
    }
}
