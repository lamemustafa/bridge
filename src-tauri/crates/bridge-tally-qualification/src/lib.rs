//! Synthetic, parser-only qualification receipts for Bridge's Tally integration.
//!
//! This crate cannot observe Tally and its receipts cannot establish Tally
//! compatibility or performance. Payload generation happens before a fresh
//! worker process measures the Bridge parser.

use std::io::Read as _;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tally_protocol_simulator::{GeneratedWindow, VoucherCorpusSpec};
use thiserror::Error;

pub const RECEIPT_SCHEMA: &str = "bridge.tally.synthetic-qualification/1";
pub const GENERATOR_SCHEMA: &str = "bridge.tally.synthetic-vouchers/1";
pub const MAX_RECEIPT_BYTES: usize = 256 * 1024;
pub const RESPONSE_LIMIT_BYTES: u64 = 32 * 1024 * 1024;

#[derive(Debug, PartialEq, Eq)]
pub enum BoundedBodyRead {
    Accepted(Vec<u8>),
    SizeLimit,
}

/// Reads at most the qualification contract's inclusive 32 MiB encoded-body
/// limit plus one detection byte. This is not yet shared with the production
/// HTTP runtime, so receipts keep `runtime_cap_binding` false.
pub fn read_qualification_body<R: std::io::Read>(
    reader: R,
    declared_bytes: Option<u64>,
) -> std::io::Result<BoundedBodyRead> {
    if declared_bytes.is_some_and(|bytes| bytes > RESPONSE_LIMIT_BYTES) {
        return Ok(BoundedBodyRead::SizeLimit);
    }
    let capacity = declared_bytes
        .unwrap_or(RESPONSE_LIMIT_BYTES + 1)
        .min(RESPONSE_LIMIT_BYTES + 1) as usize;
    let mut body = Vec::with_capacity(capacity);
    reader
        .take(RESPONSE_LIMIT_BYTES + 1)
        .read_to_end(&mut body)?;
    if body.len() as u64 > RESPONSE_LIMIT_BYTES {
        Ok(BoundedBodyRead::SizeLimit)
    } else {
        Ok(BoundedBodyRead::Accepted(body))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Scenario {
    CiSmoke,
    Small1k,
    Medium50k,
    Large500k,
    DeepVoucher,
}

impl Scenario {
    pub fn corpus(self, seed: u64) -> VoucherCorpusSpec {
        match self {
            Self::CiSmoke => corpus(50, 50, 2, 16, 0, seed),
            Self::Small1k => corpus(1_000, 1_000, 2, 16, 0, seed),
            Self::Medium50k => corpus(50_000, 1_000, 2, 16, 0, seed),
            Self::Large500k => corpus(500_000, 1_000, 2, 16, 0, seed),
            Self::DeepVoucher => corpus(1, 1, 256, 256, 256, seed),
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "ci-smoke" => Some(Self::CiSmoke),
            "small-1k" => Some(Self::Small1k),
            "medium-50k" => Some(Self::Medium50k),
            "large-500k" => Some(Self::Large500k),
            "deep-voucher" => Some(Self::DeepVoucher),
            _ => None,
        }
    }

    pub fn worker_timeout(self) -> std::time::Duration {
        match self {
            Self::CiSmoke => std::time::Duration::from_secs(60),
            Self::Small1k | Self::DeepVoucher => std::time::Duration::from_secs(120),
            Self::Medium50k => std::time::Duration::from_secs(600),
            Self::Large500k => std::time::Duration::from_secs(1_800),
        }
    }
}

fn corpus(
    total_records: u64,
    records_per_window: u32,
    entries_per_voucher: u16,
    text_width: u16,
    nesting_depth: u16,
    seed: u64,
) -> VoucherCorpusSpec {
    VoucherCorpusSpec {
        total_records,
        records_per_window,
        entries_per_voucher,
        text_width,
        nesting_depth,
        seed,
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WindowEvidence {
    pub ordinal: u32,
    pub first_record: u64,
    pub records: u32,
    pub ledger_entries: u64,
    pub wire_bytes: u64,
    pub sha256: String,
    pub expected_semantic_sha256: String,
}

impl From<GeneratedWindow> for WindowEvidence {
    fn from(value: GeneratedWindow) -> Self {
        Self {
            ordinal: value.window_index,
            first_record: value.first_record,
            records: value.record_count,
            ledger_entries: value.ledger_entry_count,
            wire_bytes: value.wire_bytes,
            sha256: value.sha256,
            expected_semantic_sha256: value.expected_semantic_sha256,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CorpusEvidence {
    pub generator_schema: String,
    pub scenario: Scenario,
    pub seed: u64,
    pub encoding: String,
    pub requested_records: u64,
    pub emitted_records: u64,
    pub windowed_corpus: bool,
    pub entries_per_voucher: u16,
    pub text_width: u16,
    pub nesting_depth: u16,
    pub window_limit_records: u32,
    pub response_limit_bytes: u64,
    pub total_wire_bytes: u64,
    pub manifest_sha256: String,
    pub expected_output_sha256: String,
    pub windows: Vec<WindowEvidence>,
}

impl CorpusEvidence {
    pub fn from_windows(
        scenario: Scenario,
        spec: VoucherCorpusSpec,
        windows: Vec<WindowEvidence>,
    ) -> Result<Self, QualificationError> {
        spec.validate()
            .map_err(|_| QualificationError::InvalidCorpus)?;
        let emitted_records = windows.iter().try_fold(0_u64, |sum, window| {
            sum.checked_add(u64::from(window.records))
                .ok_or(QualificationError::InvalidCorpus)
        })?;
        let total_wire_bytes = windows.iter().try_fold(0_u64, |sum, window| {
            sum.checked_add(window.wire_bytes)
                .ok_or(QualificationError::InvalidCorpus)
        })?;
        let manifest_sha256 = manifest_sha256(&windows)?;
        let expected_output_sha256 = semantic_output_sha256(&windows)?;
        let evidence = Self {
            generator_schema: GENERATOR_SCHEMA.to_owned(),
            scenario,
            seed: spec.seed,
            encoding: "utf8_no_bom".to_owned(),
            requested_records: spec.total_records,
            emitted_records,
            windowed_corpus: windows.len() > 1,
            entries_per_voucher: spec.entries_per_voucher,
            text_width: spec.text_width,
            nesting_depth: spec.nesting_depth,
            window_limit_records: spec.records_per_window,
            response_limit_bytes: RESPONSE_LIMIT_BYTES,
            total_wire_bytes,
            manifest_sha256,
            expected_output_sha256,
            windows,
        };
        evidence.validate()?;
        Ok(evidence)
    }

    pub fn validate(&self) -> Result<(), QualificationError> {
        if self.generator_schema != GENERATOR_SCHEMA
            || self.encoding != "utf8_no_bom"
            || self.requested_records == 0
            || self.requested_records != self.emitted_records
            || self.response_limit_bytes != RESPONSE_LIMIT_BYTES
            || self.windows.is_empty()
            || self.windowed_corpus != (self.windows.len() > 1)
        {
            return Err(QualificationError::InvalidCorpus);
        }
        let expected = self.scenario.corpus(self.seed);
        if self.requested_records != expected.total_records
            || self.window_limit_records != expected.records_per_window
            || self.entries_per_voucher != expected.entries_per_voucher
            || self.text_width != expected.text_width
            || self.nesting_depth != expected.nesting_depth
            || self.windows.len()
                != expected
                    .window_count()
                    .map_err(|_| QualificationError::InvalidCorpus)? as usize
        {
            return Err(QualificationError::InvalidCorpus);
        }
        let mut expected_first = 0_u64;
        let mut emitted = 0_u64;
        let mut wire_bytes = 0_u64;
        for (ordinal, window) in self.windows.iter().enumerate() {
            let (_, regenerated) = tally_protocol_simulator::generate_voucher_window(
                std::io::sink(),
                expected
                    .window(ordinal as u32)
                    .map_err(|_| QualificationError::InvalidCorpus)?,
            )
            .map_err(|_| QualificationError::InvalidCorpus)?;
            if WindowEvidence::from(regenerated) != *window {
                return Err(QualificationError::InvalidCorpus);
            }
            if window.ordinal != ordinal as u32
                || window.first_record != expected_first
                || window.records == 0
                || window.records > self.window_limit_records
                || window.wire_bytes == 0
                || window.wire_bytes > self.response_limit_bytes
                || window.ledger_entries
                    != u64::from(window.records)
                        .checked_mul(u64::from(self.entries_per_voucher))
                        .ok_or(QualificationError::InvalidCorpus)?
                || !is_sha256(&window.sha256)
                || !is_sha256(&window.expected_semantic_sha256)
            {
                return Err(QualificationError::InvalidCorpus);
            }
            expected_first = expected_first
                .checked_add(u64::from(window.records))
                .ok_or(QualificationError::InvalidCorpus)?;
            emitted = emitted
                .checked_add(u64::from(window.records))
                .ok_or(QualificationError::InvalidCorpus)?;
            wire_bytes = wire_bytes
                .checked_add(window.wire_bytes)
                .ok_or(QualificationError::InvalidCorpus)?;
        }
        if emitted != self.emitted_records
            || wire_bytes != self.total_wire_bytes
            || manifest_sha256(&self.windows)? != self.manifest_sha256
            || semantic_output_sha256(&self.windows)? != self.expected_output_sha256
        {
            return Err(QualificationError::InvalidCorpus);
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EnvironmentEvidence {
    build_embedded_bridge_commit: Option<String>,
    executable_sha256: String,
    cargo_lock_sha256: String,
    rustc_version: String,
    target_triple: String,
    target_os: String,
    target_arch: String,
    build_profile: String,
    enabled_features: Vec<String>,
}

impl EnvironmentEvidence {
    pub fn current() -> Result<Self, QualificationError> {
        let executable =
            std::env::current_exe().map_err(|_| QualificationError::InvalidEnvironment)?;
        let executable_bytes =
            std::fs::read(executable).map_err(|_| QualificationError::InvalidEnvironment)?;
        Ok(Self {
            build_embedded_bridge_commit: option_env!("BRIDGE_QUALIFICATION_COMMIT")
                .map(str::to_owned),
            executable_sha256: hex::encode(Sha256::digest(executable_bytes)),
            cargo_lock_sha256: hex::encode(Sha256::digest(include_bytes!("../../../Cargo.lock"))),
            rustc_version: env!("BRIDGE_QUALIFICATION_RUSTC_VERSION").to_owned(),
            target_triple: env!("BRIDGE_QUALIFICATION_TARGET").to_owned(),
            target_os: std::env::consts::OS.to_owned(),
            target_arch: std::env::consts::ARCH.to_owned(),
            build_profile: env!("BRIDGE_QUALIFICATION_PROFILE").to_owned(),
            enabled_features: Vec::new(),
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MemoryEvidence {
    pub lifetime_peak_resident_bytes: Option<u64>,
    pub baseline_lifetime_peak_resident_bytes: Option<u64>,
    pub lifetime_peak_delta_bytes: Option<u64>,
    pub method: Option<String>,
    pub unavailable_reason: Option<String>,
}

impl MemoryEvidence {
    pub fn unavailable(reason: &str) -> Self {
        Self {
            lifetime_peak_resident_bytes: None,
            baseline_lifetime_peak_resident_bytes: None,
            lifetime_peak_delta_bytes: None,
            method: None,
            unavailable_reason: Some(reason.to_owned()),
        }
    }

    pub fn validate(&self) -> bool {
        match (
            self.lifetime_peak_resident_bytes,
            self.baseline_lifetime_peak_resident_bytes,
            self.lifetime_peak_delta_bytes,
            self.method.as_deref(),
            self.unavailable_reason.as_deref(),
        ) {
            (Some(peak), Some(base), Some(delta), Some(method), None) => {
                peak >= base
                    && peak - base == delta
                    && matches!(
                        method,
                        "windows_get_process_memory_info_peak_working_set_bytes"
                            | "macos_getrusage_ru_maxrss_kib_normalized_to_bytes"
                            | "unix_getrusage_ru_maxrss_kib_normalized_to_bytes"
                    )
            }
            (None, None, None, None, Some(reason)) => {
                matches!(
                    reason,
                    "platform_peak_resident_measurement_unavailable" | "unsupported_test_platform"
                )
            }
            _ => false,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SampleEvidence {
    pub ordinal: u16,
    pub elapsed_ns: u64,
    pub parsed_records: u64,
    pub parsed_ledger_entries: u64,
    pub output_sha256: String,
    pub outcome: String,
    pub memory: MemoryEvidence,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SummaryEvidence {
    pub sample_count: u16,
    pub min_elapsed_ns: u64,
    pub median_elapsed_ns: u64,
    pub max_elapsed_ns: u64,
    pub p95_elapsed_ns: Option<u64>,
    pub quantile_method: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct QualificationReceipt {
    schema: String,
    authority: String,
    operation: String,
    execution_target: String,
    live_tally_observed: bool,
    establishes_tally_support: bool,
    establishes_tally_capability: bool,
    establishes_accounting_correctness: bool,
    establishes_performance_budget: bool,
    runtime_cap_binding: bool,
    budget_verdict: String,
    integrity: String,
    environment: EnvironmentEvidence,
    corpus: CorpusEvidence,
    samples: Vec<SampleEvidence>,
    summary: SummaryEvidence,
    receipt_sha256: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct UncheckedQualificationReceipt {
    schema: String,
    authority: String,
    operation: String,
    execution_target: String,
    live_tally_observed: bool,
    establishes_tally_support: bool,
    establishes_tally_capability: bool,
    establishes_accounting_correctness: bool,
    establishes_performance_budget: bool,
    runtime_cap_binding: bool,
    budget_verdict: String,
    integrity: String,
    environment: EnvironmentEvidence,
    corpus: CorpusEvidence,
    samples: Vec<SampleEvidence>,
    summary: SummaryEvidence,
    receipt_sha256: String,
}

impl<'de> Deserialize<'de> for QualificationReceipt {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let unchecked = UncheckedQualificationReceipt::deserialize(deserializer)?;
        let receipt = Self {
            schema: unchecked.schema,
            authority: unchecked.authority,
            operation: unchecked.operation,
            execution_target: unchecked.execution_target,
            live_tally_observed: unchecked.live_tally_observed,
            establishes_tally_support: unchecked.establishes_tally_support,
            establishes_tally_capability: unchecked.establishes_tally_capability,
            establishes_accounting_correctness: unchecked.establishes_accounting_correctness,
            establishes_performance_budget: unchecked.establishes_performance_budget,
            runtime_cap_binding: unchecked.runtime_cap_binding,
            budget_verdict: unchecked.budget_verdict,
            integrity: unchecked.integrity,
            environment: unchecked.environment,
            corpus: unchecked.corpus,
            samples: unchecked.samples,
            summary: unchecked.summary,
            receipt_sha256: unchecked.receipt_sha256,
        };
        receipt.validate().map_err(serde::de::Error::custom)?;
        Ok(receipt)
    }
}

#[derive(Serialize)]
struct UnsignedReceipt<'a> {
    schema: &'a str,
    authority: &'a str,
    operation: &'a str,
    execution_target: &'a str,
    live_tally_observed: bool,
    establishes_tally_support: bool,
    establishes_tally_capability: bool,
    establishes_accounting_correctness: bool,
    establishes_performance_budget: bool,
    runtime_cap_binding: bool,
    budget_verdict: &'a str,
    integrity: &'a str,
    environment: &'a EnvironmentEvidence,
    corpus: &'a CorpusEvidence,
    samples: &'a [SampleEvidence],
    summary: &'a SummaryEvidence,
}

impl QualificationReceipt {
    pub fn corpus(&self) -> &CorpusEvidence {
        &self.corpus
    }

    pub fn samples(&self) -> &[SampleEvidence] {
        &self.samples
    }

    pub fn receipt_sha256(&self) -> &str {
        &self.receipt_sha256
    }
    pub fn build(
        environment: EnvironmentEvidence,
        corpus: CorpusEvidence,
        samples: Vec<SampleEvidence>,
    ) -> Result<Self, QualificationError> {
        let summary = summarize(&samples)?;
        let mut receipt = Self {
            schema: RECEIPT_SCHEMA.to_owned(),
            authority: "repository_synthetic_bridge_parser_qualification".to_owned(),
            operation: "qualification_worker_pipeline".to_owned(),
            execution_target: "file_read_digest_decode_voucher_parse_and_semantic_digest"
                .to_owned(),
            live_tally_observed: false,
            establishes_tally_support: false,
            establishes_tally_capability: false,
            establishes_accounting_correctness: false,
            establishes_performance_budget: false,
            runtime_cap_binding: false,
            budget_verdict: "not_evaluated_no_baseline".to_owned(),
            integrity: "checksum_only_not_authenticated".to_owned(),
            environment,
            corpus,
            samples,
            summary,
            receipt_sha256: String::new(),
        };
        receipt.receipt_sha256 = receipt.calculate_sha256()?;
        receipt.validate()?;
        Ok(receipt)
    }

    pub fn validate(&self) -> Result<(), QualificationError> {
        if self.schema != RECEIPT_SCHEMA
            || self.authority != "repository_synthetic_bridge_parser_qualification"
            || self.operation != "qualification_worker_pipeline"
            || self.execution_target != "file_read_digest_decode_voucher_parse_and_semantic_digest"
            || self.live_tally_observed
            || self.establishes_tally_support
            || self.establishes_tally_capability
            || self.establishes_accounting_correctness
            || self.establishes_performance_budget
            || self.runtime_cap_binding
            || self.budget_verdict != "not_evaluated_no_baseline"
            || self.integrity != "checksum_only_not_authenticated"
            || self.samples.is_empty()
            || self.samples.len() > 25
        {
            return Err(QualificationError::InvalidReceipt);
        }
        self.corpus.validate()?;
        if self
            .environment
            .build_embedded_bridge_commit
            .as_deref()
            .is_some_and(|value| !is_commit(value))
            || !is_sha256(&self.environment.executable_sha256)
            || !is_sha256(&self.environment.cargo_lock_sha256)
            || !self.environment.rustc_version.starts_with("rustc ")
            || self.environment.rustc_version.len() > 128
            || !self.environment.rustc_version.bytes().all(|byte| {
                byte.is_ascii_alphanumeric()
                    || matches!(byte, b' ' | b'.' | b'-' | b'_' | b'(' | b')' | b'+')
            })
            || self.environment.target_triple.len() > 96
            || !self
                .environment
                .target_triple
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'))
            || !self.environment.enabled_features.is_empty()
            || !matches!(
                self.environment.target_os.as_str(),
                "windows" | "macos" | "linux"
            )
            || !matches!(
                self.environment.target_arch.as_str(),
                "x86" | "x86_64" | "arm" | "aarch64"
            )
            || self.environment.build_profile.is_empty()
            || self.environment.build_profile.len() > 64
            || !self
                .environment
                .build_profile
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
        {
            return Err(QualificationError::InvalidEnvironment);
        }
        let expected_entries = self
            .corpus
            .emitted_records
            .checked_mul(u64::from(self.corpus.entries_per_voucher))
            .ok_or(QualificationError::InvalidReceipt)?;
        let expected_output = &self.corpus.expected_output_sha256;
        for (ordinal, sample) in self.samples.iter().enumerate() {
            if sample.ordinal != ordinal as u16
                || sample.elapsed_ns == 0
                || sample.parsed_records != self.corpus.emitted_records
                || sample.parsed_ledger_entries != expected_entries
                || sample.outcome != "passed"
                || !is_sha256(&sample.output_sha256)
                || sample.output_sha256 != *expected_output
                || !sample.memory.validate()
                || !memory_method_matches_os(&sample.memory, &self.environment.target_os)
            {
                return Err(QualificationError::InvalidReceipt);
            }
        }
        if summarize(&self.samples)? != self.summary
            || self.calculate_sha256()? != self.receipt_sha256
        {
            return Err(QualificationError::InvalidReceipt);
        }
        let encoded = serde_json::to_vec(self).map_err(|_| QualificationError::Serialization)?;
        if encoded.len() > MAX_RECEIPT_BYTES {
            return Err(QualificationError::ReceiptTooLarge);
        }
        Ok(())
    }

    pub fn to_pretty_json(&self) -> Result<Vec<u8>, QualificationError> {
        self.validate()?;
        serde_json::to_vec_pretty(self).map_err(|_| QualificationError::Serialization)
    }

    pub fn from_json_limited<R: std::io::Read>(reader: R) -> Result<Self, QualificationError> {
        let mut bytes = Vec::with_capacity(MAX_RECEIPT_BYTES + 1);
        reader
            .take((MAX_RECEIPT_BYTES + 1) as u64)
            .read_to_end(&mut bytes)
            .map_err(|_| QualificationError::Serialization)?;
        if bytes.len() > MAX_RECEIPT_BYTES {
            return Err(QualificationError::ReceiptTooLarge);
        }
        serde_json::from_slice(&bytes).map_err(|_| QualificationError::InvalidReceipt)
    }

    fn calculate_sha256(&self) -> Result<String, QualificationError> {
        let unsigned = UnsignedReceipt {
            schema: &self.schema,
            authority: &self.authority,
            operation: &self.operation,
            execution_target: &self.execution_target,
            live_tally_observed: self.live_tally_observed,
            establishes_tally_support: self.establishes_tally_support,
            establishes_tally_capability: self.establishes_tally_capability,
            establishes_accounting_correctness: self.establishes_accounting_correctness,
            establishes_performance_budget: self.establishes_performance_budget,
            runtime_cap_binding: self.runtime_cap_binding,
            budget_verdict: &self.budget_verdict,
            integrity: &self.integrity,
            environment: &self.environment,
            corpus: &self.corpus,
            samples: &self.samples,
            summary: &self.summary,
        };
        let encoded =
            serde_json::to_vec(&unsigned).map_err(|_| QualificationError::Serialization)?;
        let mut digest = Sha256::new();
        digest.update(b"bridge.tally.synthetic-qualification-receipt/1\0");
        digest.update(encoded);
        Ok(hex::encode(digest.finalize()))
    }
}

fn summarize(samples: &[SampleEvidence]) -> Result<SummaryEvidence, QualificationError> {
    if samples.is_empty() || samples.len() > 25 {
        return Err(QualificationError::InvalidReceipt);
    }
    let mut elapsed = samples
        .iter()
        .map(|sample| sample.elapsed_ns)
        .collect::<Vec<_>>();
    elapsed.sort_unstable();
    let middle = elapsed.len() / 2;
    let median = if elapsed.len() % 2 == 0 {
        elapsed[middle - 1]
            .checked_add(elapsed[middle])
            .ok_or(QualificationError::InvalidReceipt)?
            / 2
    } else {
        elapsed[middle]
    };
    let (p95, method) = if elapsed.len() >= 20 {
        let rank = (95 * elapsed.len()).div_ceil(100).saturating_sub(1);
        (Some(elapsed[rank]), Some("nearest_rank".to_owned()))
    } else {
        (None, None)
    };
    Ok(SummaryEvidence {
        sample_count: elapsed.len() as u16,
        min_elapsed_ns: elapsed[0],
        median_elapsed_ns: median,
        max_elapsed_ns: *elapsed.last().expect("samples are not empty"),
        p95_elapsed_ns: p95,
        quantile_method: method,
    })
}

fn manifest_sha256(windows: &[WindowEvidence]) -> Result<String, QualificationError> {
    let encoded = serde_json::to_vec(windows).map_err(|_| QualificationError::Serialization)?;
    let mut digest = Sha256::new();
    digest.update(b"bridge.tally.synthetic-corpus-manifest/1\0");
    digest.update(encoded);
    Ok(hex::encode(digest.finalize()))
}

pub fn window_manifest_sha256(windows: &[WindowEvidence]) -> Result<String, QualificationError> {
    manifest_sha256(windows)
}

fn semantic_output_sha256(windows: &[WindowEvidence]) -> Result<String, QualificationError> {
    let mut digest = Sha256::new();
    digest.update(b"bridge.tally.synthetic-corpus-semantics/1\0");
    for window in windows {
        if !is_sha256(&window.expected_semantic_sha256) {
            return Err(QualificationError::InvalidCorpus);
        }
        digest.update(window.expected_semantic_sha256.as_bytes());
    }
    Ok(hex::encode(digest.finalize()))
}

fn is_sha256(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn memory_method_matches_os(memory: &MemoryEvidence, target_os: &str) -> bool {
    match memory.method.as_deref() {
        None => memory.unavailable_reason.is_some(),
        Some("windows_get_process_memory_info_peak_working_set_bytes") => target_os == "windows",
        Some("macos_getrusage_ru_maxrss_kib_normalized_to_bytes") => target_os == "macos",
        Some("unix_getrusage_ru_maxrss_kib_normalized_to_bytes") => target_os == "linux",
        Some(_) => false,
    }
}

fn is_commit(value: &str) -> bool {
    matches!(value.len(), 40 | 64) && is_lower_hex(value)
}

fn is_lower_hex(value: &str) -> bool {
    value
        .bytes()
        .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

#[derive(Debug, Error)]
pub enum QualificationError {
    #[error("synthetic corpus evidence was invalid")]
    InvalidCorpus,
    #[error("qualification environment evidence was invalid")]
    InvalidEnvironment,
    #[error("qualification receipt was invalid")]
    InvalidReceipt,
    #[error("qualification receipt exceeded its size limit")]
    ReceiptTooLarge,
    #[error("qualification evidence serialization failed")]
    Serialization,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn corpus_evidence() -> CorpusEvidence {
        let scenario = Scenario::CiSmoke;
        let spec = scenario.corpus(7);
        let windows = (0..spec.window_count().unwrap())
            .map(|index| {
                let (_, generated) = tally_protocol_simulator::generate_voucher_window(
                    Vec::new(),
                    spec.window(index).unwrap(),
                )
                .unwrap();
                generated.into()
            })
            .collect();
        CorpusEvidence::from_windows(scenario, spec, windows).unwrap()
    }

    fn sample(ordinal: u16, elapsed_ns: u64) -> SampleEvidence {
        SampleEvidence {
            ordinal,
            elapsed_ns,
            parsed_records: 50,
            parsed_ledger_entries: 100,
            output_sha256: corpus_evidence().expected_output_sha256,
            outcome: "passed".to_owned(),
            memory: MemoryEvidence::unavailable("unsupported_test_platform"),
        }
    }

    #[test]
    fn receipt_is_checksum_bound_and_cannot_claim_live_tally() {
        let receipt = QualificationReceipt::build(
            EnvironmentEvidence::current().unwrap(),
            corpus_evidence(),
            vec![sample(0, 10), sample(1, 20), sample(2, 30)],
        )
        .unwrap();
        assert!(!receipt.live_tally_observed);
        assert!(!receipt.establishes_tally_support);
        assert!(!receipt.establishes_performance_budget);
        receipt.validate().unwrap();

        let mut tampered = receipt;
        tampered.samples[0].parsed_records = 49;
        assert!(tampered.validate().is_err());
    }

    #[test]
    fn p95_requires_twenty_samples_and_uses_nearest_rank() {
        let few = (0..19)
            .map(|index| sample(index, u64::from(index + 1)))
            .collect::<Vec<_>>();
        assert_eq!(summarize(&few).unwrap().p95_elapsed_ns, None);
        let enough = (0..20)
            .map(|index| sample(index, u64::from(index + 1)))
            .collect::<Vec<_>>();
        let summary = summarize(&enough).unwrap();
        assert_eq!(summary.p95_elapsed_ns, Some(19));
        assert_eq!(summary.quantile_method.as_deref(), Some("nearest_rank"));
    }

    #[test]
    fn receipt_fields_do_not_accept_paths_or_arbitrary_features() {
        let receipt = QualificationReceipt::build(
            EnvironmentEvidence::current().unwrap(),
            corpus_evidence(),
            vec![sample(0, 1)],
        )
        .unwrap();
        let text = String::from_utf8(receipt.to_pretty_json().unwrap()).unwrap();
        for forbidden in ["C:\\Users\\", "/Users/", "@example", "GSTIN", "PAN"] {
            assert!(!text.contains(forbidden));
        }
    }

    #[test]
    fn manifest_rejects_reordering_and_response_limit_overflow() {
        let valid = corpus_evidence();
        let mut reordered = valid.clone();
        reordered.windows[0].ordinal = 1;
        assert!(reordered.validate().is_err());
        let mut oversized = valid;
        oversized.windows[0].wire_bytes = RESPONSE_LIMIT_BYTES + 1;
        assert!(oversized.validate().is_err());

        let mut wrong_scenario = corpus_evidence();
        wrong_scenario.scenario = Scenario::Small1k;
        assert!(wrong_scenario.validate().is_err());

        let mut wrong_entries = corpus_evidence();
        wrong_entries.windows[0].ledger_entries -= 1;
        assert!(wrong_entries.validate().is_err());

        let mut forged_semantics = corpus_evidence();
        forged_semantics.windows[0].expected_semantic_sha256 = "f".repeat(64);
        forged_semantics.expected_output_sha256 =
            semantic_output_sha256(&forged_semantics.windows).unwrap();
        assert!(forged_semantics.validate().is_err());
    }

    #[test]
    fn receipt_deserialization_rejects_unknown_fields() {
        let receipt = QualificationReceipt::build(
            EnvironmentEvidence::current().unwrap(),
            corpus_evidence(),
            vec![sample(0, 1)],
        )
        .unwrap();
        let encoded = receipt.to_pretty_json().unwrap();
        QualificationReceipt::from_json_limited(std::io::Cursor::new(encoded)).unwrap();
        let mut value = serde_json::to_value(receipt).unwrap();
        value["establishes_tally_support"] = serde_json::json!(true);
        assert!(serde_json::from_value::<QualificationReceipt>(value.clone()).is_err());
        value["establishes_tally_support"] = serde_json::json!(false);
        value.as_object_mut().unwrap().insert(
            "local_path".to_owned(),
            serde_json::json!("C:\\Users\\person"),
        );
        assert!(serde_json::from_value::<QualificationReceipt>(value).is_err());
        assert!(matches!(
            QualificationReceipt::from_json_limited(
                std::io::repeat(0x20).take((MAX_RECEIPT_BYTES + 1) as u64)
            ),
            Err(QualificationError::ReceiptTooLarge)
        ));
    }

    #[test]
    fn memory_method_must_match_the_receipt_os() {
        let wrong_method = if std::env::consts::OS == "windows" {
            "macos_getrusage_ru_maxrss_kib_normalized_to_bytes"
        } else {
            "windows_get_process_memory_info_peak_working_set_bytes"
        };
        let mut evidence = sample(0, 1);
        evidence.memory = MemoryEvidence {
            lifetime_peak_resident_bytes: Some(2),
            baseline_lifetime_peak_resident_bytes: Some(1),
            lifetime_peak_delta_bytes: Some(1),
            method: Some(wrong_method.to_owned()),
            unavailable_reason: None,
        };
        assert!(QualificationReceipt::build(
            EnvironmentEvidence::current().unwrap(),
            corpus_evidence(),
            vec![evidence],
        )
        .is_err());
    }

    #[test]
    fn qualification_body_limit_is_inclusive_and_detects_one_extra_byte() {
        let exact = read_qualification_body(
            std::io::repeat(0x58).take(RESPONSE_LIMIT_BYTES),
            Some(RESPONSE_LIMIT_BYTES),
        )
        .unwrap();
        assert!(matches!(
            exact,
            BoundedBodyRead::Accepted(body) if body.len() as u64 == RESPONSE_LIMIT_BYTES
        ));

        let overflow =
            read_qualification_body(std::io::repeat(0x58).take(RESPONSE_LIMIT_BYTES + 1), None)
                .unwrap();
        assert_eq!(overflow, BoundedBodyRead::SizeLimit);
    }

    #[test]
    fn declared_overflow_is_rejected_before_reading() {
        struct PanicReader;
        impl std::io::Read for PanicReader {
            fn read(&mut self, _: &mut [u8]) -> std::io::Result<usize> {
                panic!("declared overflow must not read the body")
            }
        }
        assert_eq!(
            read_qualification_body(PanicReader, Some(RESPONSE_LIMIT_BYTES + 1)).unwrap(),
            BoundedBodyRead::SizeLimit
        );
    }
}
