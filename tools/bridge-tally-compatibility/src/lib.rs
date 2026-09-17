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
/// Capacity deliberately reserved for one small cohesive surface change.
pub const RESERVED_SURFACE_FILES: usize = 15;
/// Bounded high enough for the additive Tally safety-migration, Trial Balance, selected-ledger evidence, and endpoint-reconnect
/// helper surfaces while still rejecting an unexpectedly
/// broad attestation surface.
/// Every file under the Tally migration and report directories is required by a
/// directory rule; `src/` and the protocol crates remain judgment-pinned
/// because their mixed-purpose directories do not have that invariant. The
/// reserved capacity covers a small cohesive feature (source, tests, docs
/// and manifest) but makes further unreviewed additions an explicit
/// compatibility-surface decision.
///
/// **Raised eleven times, the first three by branches that did not see each
/// other.** 210 to 211 on master for `src-tauri/src/agent_ledgers.rs`, 211 to
/// 212 for `src-tauri/crates/bridge-tally-core/src/master_binding.rs`, 212 to
/// 216 in a single commit for the voucher-presence engine, its adapter, its
/// admission-contract assertion, and `agent_catalog.rs` -- the last of those
/// taking the slot a paragraph below had already reserved for it by name, which
/// is why the four pins arrive as one raise and not two -- 216 to 217 for
/// `.github/workflows/dependency-security-scheduled.yml`, 217 to 218 for
/// `src-tauri/src/agent_import_identity.rs`, 218 to 232 for fourteen files
/// named individually below, 232 to 238 for six more, and 238 to 239 for
/// `src-tauri/src/agent_import_amend.rs`, 239 to 251 for the
/// bank-statement parser's twelve files, and, after the lowering to 249
/// described below, 249 to 262 for the thirteen test modules of
/// `src-tauri/src/reports`, and 262 to 266 for the four modules carved out of
/// the `bridge-tally-protocol` crate root. Each reason stands; a
/// merge that
/// keeps a raise but loses its pin would pass the gate with behavior silently
/// outside the evidence boundary, which is the failure this constant exists to
/// make loud.
///
/// `master_binding.rs` decides `validate_masters` results and, through them,
/// import admission. Left unpinned, an edit confined to the matcher would leave
/// the surface digest unchanged and let existing evidence attest behaviour it
/// never covered. That is the deliberate decision the paragraph above requires,
/// and it is one file for one named reason — not headroom.
///
/// The slot this paragraph once reserved for `agent_catalog.rs` has been taken
/// by it, as intended. The raise to 217 binds
/// `dependency-security-scheduled.yml`, and the named reason is different in
/// kind from the ones above: it is the only workflow that runs unattended on a
/// schedule holding `issues: write`, and the only one of the five whose sibling
/// is pinned while it is not. Left unpinned, an edit that widened its
/// permissions or pointed its audit at a different lockfile would leave the
/// surface digest unchanged. Every other raise here bound a file that decides
/// what Bridge admits; this one binds a file that decides what Bridge is
/// allowed to do to its own repository while nobody is watching.
///
/// The raise to 218 binds `agent_import_identity.rs`. The marker derivation it
/// holds is shared by the import writer and the presence reader: the writer
/// stamps a marker into a voucher's narration, and the reader identifies that
/// voucher by it. Left unpinned, an edit to that one derivation would change
/// both halves at once and leave the surface digest unchanged, so a receipt
/// would attest an identity rule the evidence never covered. It is one file for
/// one named reason — not headroom.
///
/// The raise to 232 binds fourteen files at once, which reads like headroom and
/// is not: each is named here with its own reason, and none was chosen to fill
/// space. They were found together (bridge#416) by looking for unpinned
/// production modules declared by pinned ones, then keeping only those whose
/// own body holds a rule about what Bridge posts or prepares for posting, or
/// what may leave the machine. An edit confined to any of them would leave the
/// surface digest unchanged. Each reason says what the file holds, not that it
/// holds all of a guarantee: several guarantees here are shared with pinned
/// files, and a reason that claimed the whole of one would be false.
///
/// What Bridge posts, or prepares for posting:
/// - `tally/approved_import.rs` -- the operator approval dialog, and which
///   choice counts as consent (the named post button, or Yes on Windows).
/// - `agent_import_post.rs` -- the MCP post handler, which admits only a
///   single saved Journal batch, and its part of the refusal to post one batch
///   twice; `agent_import.rs` holds the admission lock and journal append.
/// - `agent_import_ledger.rs` -- the import journal replay: whether a batch was
///   dispatched, derived from its dispatch-intent records, and the refusal of a
///   second dispatch intent for one batch.
/// - `agent_company.rs` -- finding the loaded company whose GUID matches the
///   request and refusing when none or several do; import admission and the
///   company-scoped MCP read tools call it.
/// - `agent_import_cash_bank.rs` -- the reserved-group tables deciding which
///   ledgers may sit on the cash/bank side of a Payment, Receipt or Contra in
///   an import file Bridge builds.
/// - `bridge-tally-protocol/src/group_ancestry.rs` -- the ancestry walk under
///   those tables; its other callers were already pinned and it was not.
/// - `agent_import_persistence.rs` -- whether an earlier import publication has
///   settled, checked every time the import admission lock is taken.
/// - `tally/runtime_control.rs` -- the read retry loop: the attempt limits,
///   including the single-attempt policy, and which failures may repeat a
///   request.
/// - `endpoint_coordination.rs` -- the advisory per-user, per-port lease the
///   shipped post path takes before dispatch, so two of one OS user's Bridge
///   processes cannot both hold it while posting to one Tally port.
///
/// What leaves the machine, and the record of it:
/// - `documents.rs` -- which storage URLs customer documents may be uploaded
///   to, and the file checks made before an upload.
/// - `axal.rs` -- which AXAL API origins may receive credentialed requests,
///   and that its API client follows no redirects.
/// - `agent_protocol.rs` -- the MCP response loop, which records an egress
///   receipt for a tool response before writing it and decides what is sent
///   when recording fails.
/// - `agent_egress.rs` -- the egress log: a failed append is truncated back, or
///   reported as `egress_record_rollback_failed` when that fails, and a torn
///   final row is refused rather than read as evidence.
/// - `agent_delivery.rs` -- the egress receipt record: the fields it carries,
///   the response hash it commits to, and that only a persisted preparation
///   yields a write-completion token.
///
/// The raise to 238 binds six more files from the same bridge#416 search. Five
/// decide what a read tells a caller, or when a sync may move past rows. The
/// sixth, `agent_receipt_fields.rs`, belongs with the egress record above:
/// bridge#416 listed it as borderline rather than with the fourteen, and it
/// describes the response of any tool, not only a read. Same rule: each reason
/// says what the file holds.
///
/// What a read reports, or lets a sync skip:
/// - `agent_movement.rs` -- the ledger movement tool, and the voucher predicate
///   it applies after checking the whole window: cancelled, optional and
///   entryless vouchers are left out of the movement figures.
/// - `agent_movement_math.rs` -- one ledger's movement row: when an opening was
///   observed, closing is opening plus debit plus credit; when none was, closing
///   is left empty and the row is marked `partial` with
///   `opening_balance_not_observed`.
/// - `agent_outstandings.rs` -- the outstandings tool's receivable and payable
///   totals over open bills, and its ageing buckets (0-30, 31-60, 61-90, over 90
///   days, and unaged).
/// - `agent_change_parse.rs` -- `checkpoint_advanceable`: true when a page was
///   not truncated and its highest returned alter id, or the requested
///   checkpoint when it returned none, reaches the company's high-water mark.
/// - `agent_changes.rs` -- the changed-since tool, which applies that predicate
///   to vouchers and masters separately, reports `checkpoint_advanceable` only
///   when both hold, chooses each axis's next alter id from its own result, and
///   refuses a checkpoint past the company snapshot.
///
/// And the egress record:
/// - `agent_receipt_fields.rs` -- `released_fields`, which the egress receipt in
///   `agent_delivery.rs` uses to describe a released tool response by its JSON
///   key paths rather than its values.
///
/// The raise to 239 binds `agent_import_amend.rs`. It decides whether a build
/// may reuse an earlier batch's wire identity, which turns an import file from
/// one Tally creates into one Tally applies over vouchers already in the book:
/// the refusal when any build of that batch was posted natively, and the
/// compare-and-swap that admits an amendment only while each voucher is still
/// as a build of that batch wrote it. Left unpinned, an edit confined to it
/// would leave the surface digest unchanged while changing what Bridge prepares
/// to overwrite. It is one file for one named reason — not headroom.
///
/// The raise to 251 binds twelve files for the bank-statement parser
/// (`parse_bank_statement`), found by the same rule: each holds a rule about
/// what Bridge prepares for posting, or what may leave the machine.
///
/// What may leave the machine:
/// - `agent_bank_statement.rs` -- the tool keeps every statement row in a
///   private local file and returns only a counterparty summary with every name
///   marked for `mask_parties`; it reads the PDF password from an owner-only
///   file and never returns or writes it. It also decides which local file
///   `build_import_xml` may build from by `proposals_id`: one this tool
///   published, unchanged since, by digest.
/// - `bridge-bank-statement/src/pdf.rs` -- the refusal of a password PDFium
///   cannot carry, without which `pdfium-render` panics with the password's
///   bytes in the message; and the refusal of a rotated page.
///
/// What Bridge prepares for posting:
/// - `bridge-bank-statement/Cargo.toml` -- binds the PDFium API version the
///   parser loads (`pdfium_7881`), beside the pinned manifests of the other
///   crates.
/// - `bridge-bank-statement/src/pipeline.rs` -- no proposal exists until the
///   statement binds to the account and its balance chain and totals reproduce.
/// - `bridge-bank-statement/src/money.rs` -- the balance replay and control
///   totals that decide whether a statement is proven at all.
/// - `bridge-bank-statement/src/parse.rs` -- which account a statement belongs
///   to, and which printed lines become rows.
/// - `bridge-bank-statement/src/bank.rs` -- which counterparty a row names,
///   and so which mapping row, and ledger, it reaches.
/// - `bridge-bank-statement/src/geometry.rs` -- the wrap rule that keeps a
///   12-digit bank reference intact in the narration.
/// - `bridge-bank-statement/src/text.rs` -- the mapping key that decides which
///   statement spellings reach one ledger, and the loose fold behind the
///   suspense flag and the self-cancelling-voucher refusal.
/// - `bridge-bank-statement/src/mapping.rs` -- the refusal of an ambiguous
///   mapping, a sentinel party, and a Contra without a ledger.
/// - `bridge-bank-statement/src/date.rs` -- the voucher date read from the
///   statement, refusing an impossible one.
/// - `bridge-bank-statement/src/proposals.rs` -- each proposal's voucher type,
///   legs and sides, the suspense fallback, and the refusal of a row Bridge's
///   builder could not accept.
///
/// Not pinned: `bbox.rs`, which reads `pdftotext` captures for tests and is
/// on no production path, and `refusal.rs` and `lib.rs`, which hold no rule.
///
/// Lowered to 249 when `db/migrations/mod.rs` and `tally/xml_builder.rs` were
/// deleted. Neither was reachable from any binary, test or feature: the first
/// ran a legacy schema no caller opened, the second named import actions no
/// builder used.
///
/// The raise to 262 binds thirteen test files because a directory rule requires
/// it, not because of what they decide. Each `src-tauri/src/reports`
/// file kept its tests in an inline `#[cfg(test)]` module, so editing a test
/// re-hashed a production report. Those modules now live beside their parents
/// as `<stem>_tests.rs`, and `REQUIRED_SURFACE_DIRECTORIES` requires every file
/// under that directory to be pinned, test files included. The test files the
/// same extraction moved out of pinned parents in other directories were left
/// unpinned, following bridge#416. Here the directory rule outranks that, and
/// exempting `_tests.rs` from the rule would
/// change what the gate can miss, not merely what it reports. Nothing else is
/// bound by this raise:
/// - `bulk_party_statement_tests.rs`, `outstandings_working_paper_tests.rs`,
///   `outstandings_working_paper_store_tests.rs`,
///   `outstandings_working_paper_xlsx_tests.rs`, `party_ledger_master_tests.rs`,
///   `party_ledger_master_xlsx_tests.rs`, `party_statement_tests.rs`,
///   `party_statement_pdf_tests.rs`, `party_statement_xlsx_tests.rs`,
///   `schedule_iii_tests.rs`, `trial_balance_tests.rs`,
///   `trial_balance_store_tests.rs` and `trial_balance_xlsx_tests.rs` -- the
///   tests formerly inline in the report file of the same stem.
///
/// The raise by four binds the modules carved out of the pinned
/// `bridge-tally-protocol` crate root, so the split does not shrink the seal by
/// what it moved:
/// - `text_encoding.rs` -- how every Tally response body becomes text: the
///   declared-versus-observed encoding check and bounded, strict UTF-8/UTF-16
///   decoding.
/// - `import_outcome.rs` -- the import response's application status and its
///   created, altered, deleted, ignored, errors, cancelled and exceptions
///   counters, from which Bridge reports what a post did.
/// - `standard_ledger_catalog.rs` -- which standard ledgers Bridge reports as
///   existing and which company a catalogue belongs to; it refuses a ledger name
///   carrying a bidirectional override or one of the listed invisible characters
///   that would render one spelling as another, while admitting the zero-width
///   joiners Indic ledger names need.
/// - `native_ledger_collection.rs` -- ledger source records and party ledger
///   master fields from the native Ledger collection, bound to the pinned
///   company: a ledger read fails when no row's GUID carries the company prefix,
///   counting rather than dropping individual foreign prefixes, and a party
///   ledger master read fails as a whole unless the response names the pinned
///   company GUID.
///
/// Not pinned, and deliberately: files feature-gated out of every shipped build
/// (`agent_lab.rs`, `jsonex*.rs`, `india_tax_observation.rs`), operator filing
/// labels (`client_groups.rs`, `client_group_label_migration.rs` and the
/// `commands/all_clients.rs` commands over them, none of which reads Tally),
/// dead or declaration-only modules, and `observability.rs`. Its count
/// bucketing is a real privacy reduction, and the `tally_telemetry_preview`
/// command returns what it builds, but nothing in the frontend calls that
/// command and nothing sends its result off the machine. It becomes a candidate
/// when something does. bridge#416 records the reasoning for the rest.
///
/// Also not pinned: the four frontend modules bridge#468 moved out of the pinned
/// `src/main.tsx` and `src/MirrorProofScreen.tsx`, where they had been copied.
/// `tally-mirror-contract.ts` holds only types, which are erased at runtime.
/// `display-format.ts`, `tally-capability-evidence.tsx` and
/// `tally-command-error.tsx` present identifiers, capability labels and command
/// errors. The error notice's classification comes from the pinned
/// `tally-error-copy.ts`. None of them decides which book a report or drawer is
/// attributed to, or what Bridge posts or lets leave the machine.
pub const MAX_SURFACE_FILES: usize = 266;
pub const MAX_OPERATIONS: usize = 16;
pub const MAX_CLAIMS: usize = 128;
pub const MAX_KEYS: usize = 32;
pub const MAX_MATRIX_MARKDOWN_BYTES: usize = 1024 * 1024;
const MAX_FUTURE_SKEW_MS: i64 = 5 * 60 * 1000;
const REQUIRED_SURFACE_DIRECTORIES: [&str; 2] =
    ["src-tauri/src/db/migrations", "src-tauri/src/reports"];
/// Compatibility evidence binds the selected-ledger constructor and the native
/// lifecycle implementation, error fallback, and frontend admission points, rather than
/// trusting only their callers.
///
/// `agent_ledgers.rs` renders the agent ledger reads. It is here rather than left as a
/// judgment pin because a judgment pin can be dropped during a conflict resolution and
/// the gate still returns `compatibility_gate_passed` -- measured, by deleting this very
/// entry and resealing. A required path cannot be dropped silently, and
/// `gate_rejects_each_omitted_required_lifecycle_path` iterates this list, so adding it
/// here is what covers its omission.
const REQUIRED_SURFACE_FILES: [&str; 7] = [
    "src-tauri/src/agent_catalog.rs",
    "src-tauri/src/agent_desktop_journal.rs",
    "src-tauri/src/agent_ledgers.rs",
    "src-tauri/src/source_draft/lifecycle.rs",
    "src/JournalPostingScreen.tsx",
    "src/ErrorBoundary.tsx",
    "src/NativeLifecycleController.tsx",
];

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
    TallyPrimeEditLog,
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
        self.validate_required_directory_coverage(repository_root)?;
        for file in &self.files {
            let bytes = fs::read(repository_root.join(&file.path))
                .map_err(|_| invalid("surface_file_unavailable"))?;
            if sha256_bytes(&bytes) != file.sha256 {
                return Err(invalid("surface_file_changed"));
            }
        }
        Ok(())
    }

    fn validate_required_directory_coverage(
        &self,
        repository_root: &Path,
    ) -> Result<(), CompatibilityError> {
        let sealed_paths = self
            .files
            .iter()
            .map(|file| file.path.as_str())
            .collect::<BTreeSet<_>>();
        let mut required_paths = BTreeSet::new();
        for directory in REQUIRED_SURFACE_DIRECTORIES {
            let directory_path = repository_root.join(directory);
            let directory_type = fs::symlink_metadata(&directory_path)
                .map_err(|_| invalid("surface_required_directory_unavailable"))?
                .file_type();
            if !directory_type.is_dir() {
                return Err(invalid("surface_required_directory_not_directory"));
            }
            collect_required_surface_files(
                repository_root,
                Path::new(directory),
                &mut required_paths,
            )?;
        }
        required_paths.extend(
            REQUIRED_SURFACE_FILES
                .iter()
                .map(|path| (*path).to_string()),
        );
        if required_paths
            .iter()
            .any(|path| !sealed_paths.contains(path.as_str()))
        {
            return Err(invalid("surface_required_directory_file_unpinned"));
        }
        Ok(())
    }

    /// Refreshes only existing surface-file digests. The returned manifest intentionally retains
    /// the old manifest checksum so that `seal-surface` remains the explicit attestation step.
    pub fn rehash_files(
        &self,
        repository_root: &Path,
    ) -> Result<(Self, usize), CompatibilityError> {
        self.validate()?;
        let mut rehashed = self.clone();
        let mut changed = 0;
        for file in &mut rehashed.files {
            let digest = sha256_file(&repository_root.join(&file.path))
                .map_err(|_| invalid("surface_file_unavailable"))?;
            if file.sha256 != digest {
                file.sha256 = digest;
                changed += 1;
            }
        }
        Ok((rehashed, changed))
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

fn collect_required_surface_files(
    repository_root: &Path,
    relative_directory: &Path,
    paths: &mut BTreeSet<String>,
) -> Result<(), CompatibilityError> {
    for entry in fs::read_dir(repository_root.join(relative_directory))
        .map_err(|_| invalid("surface_required_directory_unavailable"))?
    {
        let entry = entry.map_err(|_| invalid("surface_required_directory_unavailable"))?;
        let entry_type = entry
            .file_type()
            .map_err(|_| invalid("surface_required_directory_unavailable"))?;
        let path = entry.path();
        if entry_type.is_dir() {
            collect_required_surface_files(
                repository_root,
                path.strip_prefix(repository_root)
                    .map_err(|_| invalid("surface_required_directory_unavailable"))?,
                paths,
            )?;
        } else if entry_type.is_file() {
            let relative_path = path
                .strip_prefix(repository_root)
                .map_err(|_| invalid("surface_required_directory_unavailable"))?;
            paths.insert(normalise_surface_path(relative_path)?);
        } else {
            return Err(invalid("surface_required_directory_entry_unsupported"));
        }
    }
    Ok(())
}

fn normalise_surface_path(path: &Path) -> Result<String, CompatibilityError> {
    let mut components = Vec::new();
    for component in path.components() {
        match component {
            Component::Normal(component) => components.push(
                component
                    .to_str()
                    .ok_or_else(|| invalid("surface_required_directory_unavailable"))?,
            ),
            Component::CurDir
            | Component::ParentDir
            | Component::RootDir
            | Component::Prefix(_) => {
                return Err(invalid("surface_required_directory_unavailable"));
            }
        }
    }
    if components.is_empty() {
        return Err(invalid("surface_required_directory_unavailable"));
    }
    Ok(components.join("/"))
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

    pub fn repoint_surface(
        mut self,
        surface: &CompatibilitySurfaceManifest,
    ) -> Result<Self, CompatibilityError> {
        self.validate()?;
        surface.validate()?;
        self.compatibility_surface_sha256 = surface.manifest_sha256.clone();
        self.validate()?;
        Ok(self)
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
    if matches!(claim.level, ClaimLevel::Observed | ClaimLevel::Supported)
        && claim.mode == TallyMode::Education
    {
        return Err(invalid("education_positive_claim_forbidden"));
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
#[path = "lib_tests.rs"]
mod tests;
