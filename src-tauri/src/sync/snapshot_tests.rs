use std::collections::VecDeque;
use std::sync::Mutex;

use bridge_tally_core::{
    CanonicalPackWindow, CapabilityEvidence, CapabilityProfile, CoreAccountingBatch,
    EvidenceConfidence, GroupRecord, ObservedSourceIdentities, PackBatch, ProbeResult,
    RawSourceSha256, SourceIdentity, SourceIdentityKind, SourceRecordEvidence, SourceRecordId,
    TransportId,
};
use bridge_tally_protocol::parse_native_ledger_source_records_with_evidence;
use bridge_tally_transport::{TransportPolicy, XML_REQUEST_MAX_BYTES};
use sqlx::sqlite::{SqliteConnectOptions, SqlitePoolOptions};
use tally_protocol_simulator::{
    Fixture, ResponseFraming, ScenarioPlan, SequenceSimulator, WireEncoding,
};

use crate::db::tally_mirror::{
    CapabilityItemInput, CapabilityKind, CapabilitySnapshotInput, CompanyInput, Confidence,
    RunOutcome, SourceIdentityInput, VerificationState,
};
use crate::tally::{
    company_source_identity, connector::simulator_test_lock, RuntimeTallyConnector, TallyConfig,
    TallyRuntime,
};

use super::*;

fn fake_profile() -> CapabilityProfile {
    CapabilityProfile {
        profile_version: 1,
        product: "TallyPrime".to_string(),
        release: None,
        license_tier: None,
        mode: Some("Education".to_string()),
        transports: BTreeMap::from([(
            TransportId::XmlHttp,
            CapabilityEvidence {
                state: CapabilityState::Supported,
                confidence: EvidenceConfidence::Observed,
                safe_reason_code: None,
            },
        )]),
        features: BTreeMap::new(),
        packs: BTreeMap::from([(
            CapabilityPackId::CoreAccounting,
            CapabilityEvidence {
                state: CapabilityState::Unknown,
                confidence: EvidenceConfidence::Observed,
                safe_reason_code: Some("sealed_profile_executed".to_string()),
            },
        )]),
    }
}

struct FakeConnector {
    batch: Mutex<VecDeque<Result<CanonicalPackWindow, TallyError>>>,
    company: CompanyRef,
    requests: Mutex<Vec<ReadWindow>>,
}

struct RuntimeCancelledStabilityConnector {
    inner: FakeConnector,
    request_count: Mutex<usize>,
}

struct RuntimeCancelledReportConnector {
    inner: FakeConnector,
}

struct RuntimeCancelledEndProbeConnector {
    inner: FakeConnector,
}

struct HeartbeatCountingStore {
    inner: SqliteSnapshotStateStore,
    heartbeats: Mutex<usize>,
}

struct RuntimeReadOnlyConnector {
    inner: RuntimeTallyConnector,
    company: CompanyRef,
}

struct FailAfterFirstSplitStore {
    inner: SqliteSnapshotStateStore,
    failed: AtomicBool,
}

struct FailAfterFirstCommitPendingStore {
    inner: SqliteSnapshotStateStore,
    failed: AtomicBool,
}

struct FailBeforeFirstCompletedWindowSaveStore {
    inner: SqliteSnapshotStateStore,
    failed: AtomicBool,
}

struct FailAfterFirstStagingSaveStore {
    inner: SqliteSnapshotStateStore,
    failed: AtomicBool,
}

struct FailBeforeSecondStagingHeartbeatStore {
    inner: SqliteSnapshotStateStore,
    staging_heartbeats: Mutex<usize>,
}

struct FailAfterClockRollbackAbandonmentStore {
    inner: SqliteSnapshotStateStore,
    failed: AtomicBool,
}

struct FailAfterClockRollbackBeginStore {
    inner: SqliteSnapshotStateStore,
    failed: AtomicBool,
}

struct SaveMetricsStore {
    inner: SqliteSnapshotStateStore,
    saves: Mutex<usize>,
    max_state_json_bytes: Mutex<usize>,
}

struct ReportConnector {
    inner: FakeConnector,
}

struct AmbiguousCompanyConnector {
    inner: FakeConnector,
}

#[async_trait]
impl SnapshotStateStore for HeartbeatCountingStore {
    async fn load(&self, resume_key: &str) -> Result<Option<DurableSnapshotState>, SnapshotError> {
        self.inner.load(resume_key).await
    }

    async fn save(&self, state: &mut DurableSnapshotState) -> Result<(), SnapshotError> {
        self.inner.save(state).await
    }

    async fn heartbeat(&self, state: &DurableSnapshotState) -> Result<(), SnapshotError> {
        *self.heartbeats.lock().unwrap() += 1;
        self.inner.heartbeat(state).await
    }
}

#[async_trait]
impl SnapshotStateStore for FailAfterFirstSplitStore {
    async fn load(&self, resume_key: &str) -> Result<Option<DurableSnapshotState>, SnapshotError> {
        self.inner.load(resume_key).await
    }

    async fn save(&self, state: &mut DurableSnapshotState) -> Result<(), SnapshotError> {
        self.inner.save(state).await?;
        if state
            .windows
            .values()
            .any(|window| window.phase == WindowPhase::Split)
            && !self.failed.swap(true, Ordering::AcqRel)
        {
            return Err(SnapshotError::StateInvariant(
                "injected_crash_after_split_commit",
            ));
        }
        Ok(())
    }

    async fn heartbeat(&self, state: &DurableSnapshotState) -> Result<(), SnapshotError> {
        self.inner.heartbeat(state).await
    }
}

#[async_trait]
impl SnapshotStateStore for FailAfterFirstCommitPendingStore {
    async fn load(&self, resume_key: &str) -> Result<Option<DurableSnapshotState>, SnapshotError> {
        self.inner.load(resume_key).await
    }

    async fn save(&self, state: &mut DurableSnapshotState) -> Result<(), SnapshotError> {
        self.inner.save(state).await?;
        if state.progress.phase == SnapshotPhase::CommitPending
            && !self.failed.swap(true, Ordering::AcqRel)
        {
            return Err(SnapshotError::StateInvariant(
                "injected_crash_after_commit_pending",
            ));
        }
        Ok(())
    }

    async fn heartbeat(&self, state: &DurableSnapshotState) -> Result<(), SnapshotError> {
        self.inner.heartbeat(state).await
    }
}

#[async_trait]
impl SnapshotStateStore for FailBeforeFirstCompletedWindowSaveStore {
    async fn load(&self, resume_key: &str) -> Result<Option<DurableSnapshotState>, SnapshotError> {
        self.inner.load(resume_key).await
    }

    async fn save(&self, state: &mut DurableSnapshotState) -> Result<(), SnapshotError> {
        if state
            .windows
            .values()
            .any(|window| window.phase == WindowPhase::Complete && window.stage_receipt.is_some())
            && !self.failed.swap(true, Ordering::AcqRel)
        {
            return Err(SnapshotError::StateInvariant(
                "injected_crash_after_window_attempt_completion",
            ));
        }
        self.inner.save(state).await
    }

    async fn heartbeat(&self, state: &DurableSnapshotState) -> Result<(), SnapshotError> {
        self.inner.heartbeat(state).await
    }
}

#[async_trait]
impl SnapshotStateStore for FailAfterFirstStagingSaveStore {
    async fn load(&self, resume_key: &str) -> Result<Option<DurableSnapshotState>, SnapshotError> {
        self.inner.load(resume_key).await
    }

    async fn save(&self, state: &mut DurableSnapshotState) -> Result<(), SnapshotError> {
        self.inner.save(state).await?;
        if state
            .windows
            .values()
            .any(|window| window.phase == WindowPhase::Staging && window.stage_attempt.is_some())
            && !self.failed.swap(true, Ordering::AcqRel)
        {
            return Err(SnapshotError::StateInvariant(
                "injected_crash_after_window_attempt_staging",
            ));
        }
        Ok(())
    }

    async fn heartbeat(&self, state: &DurableSnapshotState) -> Result<(), SnapshotError> {
        self.inner.heartbeat(state).await
    }
}

#[async_trait]
impl SnapshotStateStore for FailBeforeSecondStagingHeartbeatStore {
    async fn load(&self, resume_key: &str) -> Result<Option<DurableSnapshotState>, SnapshotError> {
        self.inner.load(resume_key).await
    }

    async fn save(&self, state: &mut DurableSnapshotState) -> Result<(), SnapshotError> {
        self.inner.save(state).await
    }

    async fn heartbeat(&self, state: &DurableSnapshotState) -> Result<(), SnapshotError> {
        if state
            .windows
            .values()
            .any(|window| window.phase == WindowPhase::Staging)
        {
            let mut heartbeats = self.staging_heartbeats.lock().unwrap();
            *heartbeats += 1;
            if *heartbeats == 2 {
                return Err(SnapshotError::StateInvariant(
                    "injected_crash_after_partial_window_staging",
                ));
            }
        }
        self.inner.heartbeat(state).await
    }
}

#[async_trait]
impl SnapshotStateStore for FailAfterClockRollbackAbandonmentStore {
    async fn load(&self, resume_key: &str) -> Result<Option<DurableSnapshotState>, SnapshotError> {
        self.inner.load(resume_key).await
    }

    async fn save(&self, state: &mut DurableSnapshotState) -> Result<(), SnapshotError> {
        if state.gap_codes.contains("local_clock_moved_backwards")
            && state
                .windows
                .values()
                .all(|window| window.stage_attempt.is_none())
            && !self.failed.swap(true, Ordering::AcqRel)
        {
            // The repository abandonment has already committed, but its returned evidence
            // has not reached the durable run state. This models a lost acknowledgement.
            return Err(SnapshotError::StateInvariant(
                "injected_crash_after_window_abandonment",
            ));
        }
        self.inner.save(state).await
    }

    async fn heartbeat(&self, state: &DurableSnapshotState) -> Result<(), SnapshotError> {
        self.inner.heartbeat(state).await
    }
}

#[async_trait]
impl SnapshotStateStore for FailAfterClockRollbackBeginStore {
    async fn load(&self, resume_key: &str) -> Result<Option<DurableSnapshotState>, SnapshotError> {
        self.inner.load(resume_key).await
    }

    async fn save(&self, state: &mut DurableSnapshotState) -> Result<(), SnapshotError> {
        if state.gap_codes.contains("local_clock_moved_backwards")
            && state.windows.values().any(|window| {
                window.phase == WindowPhase::Staging && window.stage_attempt.is_some()
            })
            && !self.failed.swap(true, Ordering::AcqRel)
        {
            return Err(SnapshotError::StateInvariant(
                "injected_crash_after_window_attempt_begin",
            ));
        }
        self.inner.save(state).await
    }

    async fn heartbeat(&self, state: &DurableSnapshotState) -> Result<(), SnapshotError> {
        self.inner.heartbeat(state).await
    }
}

#[async_trait]
impl SnapshotStateStore for SaveMetricsStore {
    async fn load(&self, resume_key: &str) -> Result<Option<DurableSnapshotState>, SnapshotError> {
        self.inner.load(resume_key).await
    }

    async fn save(&self, state: &mut DurableSnapshotState) -> Result<(), SnapshotError> {
        let state_json = serde_json::to_vec(state).map_err(|_| SnapshotError::Serialization)?;
        {
            *self.saves.lock().unwrap() += 1;
            let mut max_bytes = self.max_state_json_bytes.lock().unwrap();
            *max_bytes = (*max_bytes).max(state_json.len());
        }
        self.inner.save(state).await
    }

    async fn heartbeat(&self, state: &DurableSnapshotState) -> Result<(), SnapshotError> {
        self.inner.heartbeat(state).await
    }
}

#[async_trait]
impl TallyConnector for FakeConnector {
    async fn probe(&self) -> Result<ProbeResult, TallyError> {
        Ok(ProbeResult {
            reachable: true,
            profile: fake_profile(),
        })
    }

    async fn probe_fresh(&self) -> Result<ProbeResult, TallyError> {
        Ok(ProbeResult {
            reachable: true,
            profile: fake_profile(),
        })
    }

    async fn discover_companies(&self) -> Result<Vec<CompanyRef>, TallyError> {
        Ok(vec![self.company.clone()])
    }

    async fn read_pack_window(
        &self,
        context: &RequestContext,
    ) -> Result<CanonicalPackWindow, TallyError> {
        self.requests.lock().unwrap().push(context.window.clone());
        self.batch.lock().unwrap().pop_front().unwrap_or_else(|| {
            Ok(CanonicalPackWindow::without_source_count_evidence(
                PackBatch::CoreAccounting(CoreAccountingBatch::default()),
            ))
        })
    }
}

#[async_trait]
impl TallyConnector for AmbiguousCompanyConnector {
    async fn probe(&self) -> Result<ProbeResult, TallyError> {
        self.inner.probe().await
    }

    async fn probe_fresh(&self) -> Result<ProbeResult, TallyError> {
        self.inner.probe_fresh().await
    }

    async fn discover_companies(&self) -> Result<Vec<CompanyRef>, TallyError> {
        Ok(vec![self.inner.company.clone(), self.inner.company.clone()])
    }

    async fn read_pack_window(
        &self,
        context: &RequestContext,
    ) -> Result<CanonicalPackWindow, TallyError> {
        self.inner.read_pack_window(context).await
    }
}

#[async_trait]
impl TallyConnector for ReportConnector {
    async fn probe(&self) -> Result<ProbeResult, TallyError> {
        self.inner.probe().await
    }

    async fn probe_fresh(&self) -> Result<ProbeResult, TallyError> {
        self.inner.probe_fresh().await
    }

    async fn discover_companies(&self) -> Result<Vec<CompanyRef>, TallyError> {
        self.inner.discover_companies().await
    }

    async fn read_pack_window(
        &self,
        context: &RequestContext,
    ) -> Result<CanonicalPackWindow, TallyError> {
        self.inner.read_pack_window(context).await
    }

    async fn read_core_period_balance_report(
        &self,
        context: &RequestContext,
    ) -> Result<bridge_tally_core::report_tie_out::LedgerPeriodBalanceReport, TallyError> {
        Ok(
            bridge_tally_core::report_tie_out::LedgerPeriodBalanceReport {
                source_identity: context.company.identity.clone(),
                window: context.window.clone(),
                ordinary_books_scope_observed: true,
                source_reported_count: 0,
                balances: Vec::new(),
            },
        )
    }
}

fn core_groups(count: usize) -> CanonicalPackWindow {
    CanonicalPackWindow::without_source_count_evidence(PackBatch::CoreAccounting(
        CoreAccountingBatch {
            groups: (0..count)
                .map(|index| GroupRecord {
                    source_id: format!("group-{index:06}"),
                    name: format!("Synthetic Group {index:06}"),
                    parent_source_id: None,
                })
                .collect(),
            ..CoreAccountingBatch::default()
        },
    ))
}

fn core_groups_with_provenance(count: usize) -> CanonicalPackWindow {
    let mut window = core_groups(count);
    window.record_evidence = Some(
        (0..count)
            .map(|index| {
                let source_id = SourceRecordId::parse(format!("group-{index:06}"))
                    .expect("synthetic source id");
                SourceRecordEvidence {
                    object_type: CanonicalText::parse("group").expect("object type"),
                    source_id: source_id.clone(),
                    identity_kind: SourceIdentityKind::Guid,
                    observed_identities: ObservedSourceIdentities {
                        guid: Some(source_id.clone()),
                        ..ObservedSourceIdentities::default()
                    },
                    raw_source_sha256: RawSourceSha256::parse(sha256_bytes(
                        source_id.as_str().as_bytes(),
                    ))
                    .expect("synthetic raw hash"),
                    alter_id: None,
                }
            })
            .collect(),
    );
    window
}

fn core_groups_with_foreign_master_text_diagnostic() -> CanonicalPackWindow {
    let mut window = core_groups(1);
    let PackBatch::CoreAccounting(core) = &mut window.batch else {
        panic!("core fixture");
    };
    core.foreign_master_text_diagnostics
        .push(bridge_tally_core::ForeignMasterTextDiagnostic {
            object_type: "group".to_string(),
            source_id: "group-000000".to_string(),
            stored_name: "Synthetic ill-rendering name".to_string(),
            likely_intended_spelling: None,
        });
    window
}

#[async_trait]
impl TallyConnector for RuntimeCancelledStabilityConnector {
    async fn probe(&self) -> Result<ProbeResult, TallyError> {
        self.inner.probe().await
    }

    async fn probe_fresh(&self) -> Result<ProbeResult, TallyError> {
        self.inner.probe_fresh().await
    }

    async fn discover_companies(&self) -> Result<Vec<CompanyRef>, TallyError> {
        self.inner.discover_companies().await
    }

    async fn read_pack_window(
        &self,
        context: &RequestContext,
    ) -> Result<CanonicalPackWindow, TallyError> {
        let response = self.inner.read_pack_window(context).await;
        let mut request_count = self.request_count.lock().unwrap();
        *request_count += 1;
        if *request_count == 2 {
            return Err(TallyError::Cancelled);
        }
        response
    }
}

#[async_trait]
impl TallyConnector for RuntimeCancelledReportConnector {
    async fn probe(&self) -> Result<ProbeResult, TallyError> {
        self.inner.probe().await
    }

    async fn probe_fresh(&self) -> Result<ProbeResult, TallyError> {
        self.inner.probe_fresh().await
    }

    async fn discover_companies(&self) -> Result<Vec<CompanyRef>, TallyError> {
        self.inner.discover_companies().await
    }

    async fn read_pack_window(
        &self,
        context: &RequestContext,
    ) -> Result<CanonicalPackWindow, TallyError> {
        self.inner.read_pack_window(context).await
    }

    async fn read_core_period_balance_report(
        &self,
        _context: &RequestContext,
    ) -> Result<bridge_tally_core::report_tie_out::LedgerPeriodBalanceReport, TallyError> {
        Err(TallyError::Cancelled)
    }
}

#[async_trait]
impl TallyConnector for RuntimeCancelledEndProbeConnector {
    async fn probe(&self) -> Result<ProbeResult, TallyError> {
        self.inner.probe().await
    }

    async fn probe_fresh(&self) -> Result<ProbeResult, TallyError> {
        Err(TallyError::Cancelled)
    }

    async fn discover_companies(&self) -> Result<Vec<CompanyRef>, TallyError> {
        self.inner.discover_companies().await
    }

    async fn read_pack_window(
        &self,
        context: &RequestContext,
    ) -> Result<CanonicalPackWindow, TallyError> {
        self.inner.read_pack_window(context).await
    }
}

#[async_trait]
impl TallyConnector for RuntimeReadOnlyConnector {
    async fn probe(&self) -> Result<ProbeResult, TallyError> {
        let profile = fake_profile();
        self.inner.observe_snapshot_profile(&profile)?;
        Ok(ProbeResult {
            reachable: true,
            profile,
        })
    }

    async fn probe_fresh(&self) -> Result<ProbeResult, TallyError> {
        self.probe().await
    }

    async fn discover_companies(&self) -> Result<Vec<CompanyRef>, TallyError> {
        Ok(vec![self.company.clone()])
    }

    async fn read_pack_window(
        &self,
        context: &RequestContext,
    ) -> Result<CanonicalPackWindow, TallyError> {
        self.inner.read_pack_window(context).await
    }
}

pub(crate) async fn setup() -> (
    SqlitePool,
    TallyMirrorRepository,
    SqliteSnapshotStateStore,
    SnapshotPlan,
) {
    let pool = SqlitePoolOptions::new()
        .max_connections(1)
        .after_connect(|connection, _| {
            Box::pin(async move {
                sqlx::query("PRAGMA foreign_keys = ON")
                    .execute(connection)
                    .await?;
                Ok(())
            })
        })
        .connect("sqlite::memory:")
        .await
        .unwrap();
    let mirror = TallyMirrorRepository::new(pool.clone());
    mirror.migrate().await.unwrap();
    let store =
        SqliteSnapshotStateStore::for_worker(pool.clone(), "synthetic-test-worker".to_string())
            .unwrap();
    store.migrate().await.unwrap();
    let capability = mirror
        .save_capability_snapshot(CapabilitySnapshotInput {
            canonical_origin: "http://127.0.0.1:9000".to_string(),
            observed_at_unix_ms: 1_000,
            profile_version: 1,
            product: "TallyPrime".to_string(),
            release: None,
            license_tier: None,
            mode: Some("Education".to_string()),
            mode_confidence: Confidence::Observed,
            items: vec![CapabilityItemInput {
                kind: CapabilityKind::Pack,
                key: "core_accounting".to_string(),
                state: crate::db::tally_mirror::CapabilityState::Supported,
                confidence: Confidence::Observed,
                safe_reason_code: None,
            }],
        })
        .await
        .unwrap();
    let company_identity = SourceIdentity {
        bridge_source_lineage: "lineage".to_string(),
        company_guid: "company-guid".to_string(),
        observed_fingerprint: "fingerprint".to_string(),
    };
    let company = mirror
        .upsert_company(CompanyInput {
            endpoint_id: capability.endpoint_id.clone(),
            display_name: "Synthetic Company".to_string(),
            identity: SourceIdentityInput {
                guid: Some(company_identity.company_guid.clone()),
                ..SourceIdentityInput::default()
            },
            observed_at_unix_ms: 1_000,
        })
        .await
        .unwrap();
    let range = ReadWindow {
        from_yyyymmdd: "20260701".to_string(),
        to_yyyymmdd: "20260731".to_string(),
    };
    let root_window = PlannedWindow::deterministic(CapabilityPackId::CoreAccounting, range);
    let capability_canary_window = PlannedWindow::deterministic(
        CapabilityPackId::CoreAccounting,
        ReadWindow {
            from_yyyymmdd: "20260701".to_string(),
            to_yyyymmdd: "20260701".to_string(),
        },
    );
    let plan = SnapshotPlan {
        resume_key: "resume-1".to_string(),
        run_id: "run-1".to_string(),
        capability_snapshot_id: capability.id,
        mirror_company_id: company.id,
        company: CompanyRef {
            identity: company_identity,
            display_name: "Synthetic Company".to_string(),
        },
        pack: CapabilityPackId::CoreAccounting,
        pack_schema_version: PackSchemaVersion { major: 1, minor: 0 },
        capability_profile_version: 1,
        capability_profile_sha256: capability_profile_sha256(&fake_profile()).unwrap(),
        source_product: "TallyPrime".to_string(),
        source_transport: "xml_http".to_string(),
        source_release: None,
        source_mode: Some("Education".to_string()),
        external_references: ExternalReferenceCatalog::Unavailable,
        windows: vec![root_window],
        adaptive_window_policy: Some(AdaptiveWindowPolicy::bounded_default()),
        capability_canary_window: Some(capability_canary_window),
        started_at_unix_ms: 2_000,
        freshness_target_seconds: 300,
    };
    (pool, mirror, store, plan)
}

async fn setup_file_backed_lease_store() -> (
    tempfile::TempDir,
    SqlitePool,
    SqliteSnapshotStateStore,
    SnapshotPlan,
) {
    let (_, _, _, plan) = setup().await;
    let directory = tempfile::tempdir().unwrap();
    let options = SqliteConnectOptions::new()
        .filename(directory.path().join("snapshot-lease.sqlite3"))
        .create_if_missing(true);
    let pool = SqlitePoolOptions::new()
        .max_connections(2)
        .after_connect(|connection, _| {
            Box::pin(async move {
                sqlx::query("PRAGMA foreign_keys = ON")
                    .execute(connection)
                    .await?;
                Ok(())
            })
        })
        .connect_with(options)
        .await
        .unwrap();
    let mirror = TallyMirrorRepository::new(pool.clone());
    mirror.migrate().await.unwrap();
    let store =
        SqliteSnapshotStateStore::for_worker(pool.clone(), "synthetic-file-worker".to_string())
            .unwrap();
    store.migrate().await.unwrap();
    (directory, pool, store, plan)
}

#[test]
fn adaptive_midpoint_split_is_calendar_exact_and_deterministic() {
    let leap = ReadWindow {
        from_yyyymmdd: "20240228".to_string(),
        to_yyyymmdd: "20240302".to_string(),
    };
    let (left, right) = midpoint_split(&leap).expect("split leap window");
    assert_eq!(
        (left.from_yyyymmdd.as_str(), left.to_yyyymmdd.as_str()),
        ("20240228", "20240229")
    );
    assert_eq!(
        (right.from_yyyymmdd.as_str(), right.to_yyyymmdd.as_str()),
        ("20240301", "20240302")
    );

    let parent = PlannedWindow::deterministic(CapabilityPackId::CoreAccounting, leap);
    assert_eq!(
        PlannedWindow::adaptive_child(&parent, left.clone()),
        PlannedWindow::adaptive_child(&parent, left)
    );
    assert!(midpoint_split(&ReadWindow {
        from_yyyymmdd: "20240229".to_string(),
        to_yyyymmdd: "20240229".to_string(),
    })
    .is_none());
}

#[tokio::test]
async fn adaptive_graph_tampering_fails_closed() {
    let (_, _, _, plan) = setup().await;
    let mut state = DurableSnapshotState::new(&plan, Freshness::NeverVerified).unwrap();
    let root_id = plan.windows[0].id.clone();
    state.windows.get_mut(&root_id).unwrap().phase = WindowPhase::Extracting;
    assert!(matches!(
        split_leaf(&mut state, &plan, &root_id).unwrap(),
        SplitLeafResult::Created
    ));
    state.validate_invariants().expect("valid split graph");

    let split = state.windows[&root_id].split.clone().unwrap();
    let mut missing_child = state.clone();
    missing_child.windows.remove(&split.left_window_id);
    assert!(matches!(
        missing_child.validate_invariants(),
        Err(SnapshotError::CorruptState)
    ));

    let mut drifted_child = state;
    drifted_child
        .windows
        .get_mut(&split.right_window_id)
        .unwrap()
        .planned
        .range
        .from_yyyymmdd = "20260716".to_string();
    assert!(matches!(
        drifted_child.validate_invariants(),
        Err(SnapshotError::CorruptState)
    ));
}

#[tokio::test]
async fn oversized_voucher_window_persists_deterministic_children_before_dispatch() {
    let (_, mirror, store, plan) = setup().await;
    let original_plan_sha256 = plan.fingerprint().unwrap();
    let root = plan.windows[0].clone();
    let connector = FakeConnector {
        batch: Mutex::new(VecDeque::from([Err(TallyError::ReadResponseTooLarge {
            scope: ReadResponseScope::VoucherWindow,
        })])),
        company: plan.company.clone(),
        requests: Mutex::new(Vec::new()),
    };
    let result = FullSnapshotEngine::new(&mirror, &store, &connector)
        .run(&plan, &AtomicCancellation::default())
        .await
        .expect("adaptively split oversized window");

    assert_eq!(plan.fingerprint().unwrap(), original_plan_sha256);
    let root_progress = result.state.windows.get(&root.id).expect("root progress");
    assert_eq!(root_progress.phase, WindowPhase::Split);
    let leaves = result.state.executable_leaves();
    assert_eq!(leaves.len(), 2);
    assert_eq!(leaves[0].range.from_yyyymmdd, "20260701");
    assert_eq!(leaves[0].range.to_yyyymmdd, "20260716");
    assert_eq!(leaves[1].range.from_yyyymmdd, "20260717");
    assert_eq!(leaves[1].range.to_yyyymmdd, "20260731");
    assert_eq!(result.state.progress.total_windows, 2);
    assert!(result
        .state
        .warning_codes
        .contains(&WarningCode::AdaptiveWindowSplit));
    let requests = connector.requests.lock().unwrap();
    assert_eq!(
        requests
            .iter()
            .filter(|range| **range == root.range)
            .count(),
        1
    );
}

#[test]
fn durable_v2_core_query_plan_cannot_resume_under_the_v3_profile() {
    let mut current = SnapshotPlan {
        resume_key: "resume-profile-upgrade".to_string(),
        run_id: "run-profile-upgrade".to_string(),
        capability_snapshot_id: "capability-profile-upgrade".to_string(),
        mirror_company_id: "company-profile-upgrade".to_string(),
        company: CompanyRef {
            identity: SourceIdentity {
                bridge_source_lineage: "lineage".to_string(),
                company_guid: "company-guid".to_string(),
                observed_fingerprint: "fingerprint".to_string(),
            },
            display_name: "Synthetic Company".to_string(),
        },
        pack: CapabilityPackId::CoreAccounting,
        pack_schema_version: PackSchemaVersion { major: 1, minor: 0 },
        capability_profile_version: 1,
        capability_profile_sha256: "0".repeat(64),
        source_product: "TallyPrime".to_string(),
        source_transport: "xml_http".to_string(),
        source_release: None,
        source_mode: Some("Education".to_string()),
        external_references: ExternalReferenceCatalog::Unavailable,
        windows: vec![PlannedWindow::deterministic(
            CapabilityPackId::CoreAccounting,
            ReadWindow {
                from_yyyymmdd: "20260701".to_string(),
                to_yyyymmdd: "20260701".to_string(),
            },
        )],
        adaptive_window_policy: Some(AdaptiveWindowPolicy::bounded_default()),
        capability_canary_window: Some(PlannedWindow::deterministic(
            CapabilityPackId::CoreAccounting,
            ReadWindow {
                from_yyyymmdd: "20260701".to_string(),
                to_yyyymmdd: "20260701".to_string(),
            },
        )),
        started_at_unix_ms: 2_000,
        freshness_target_seconds: 60,
    };
    let mut durable = DurableSnapshotState::new(&current, Freshness::NeverVerified)
        .expect("the current v3 plan is constructible");

    // This emulates the immutable plan embedded in a durable state saved
    // before the opening-balance query change. Constructing it through
    // today's API would (correctly) reject it before persistence, so the
    // test installs the historical bytes' semantic content directly.
    let legacy_profile = CanonicalText::parse("core_accounting_v2").unwrap();
    current.windows[0].query_profile = legacy_profile.clone();
    current
        .capability_canary_window
        .as_mut()
        .expect("canary")
        .query_profile = legacy_profile;
    durable.plan_sha256 = current.fingerprint().unwrap();
    durable.plan = Some(current.clone());
    durable
        .windows
        .get_mut(&current.windows[0].id)
        .unwrap()
        .planned = current.windows[0].clone();

    let expected_current = durable
        .plan
        .as_ref()
        .expect("historic plan installed")
        .clone();
    let mut upgraded = expected_current.clone();
    upgraded.windows[0] = PlannedWindow::deterministic(
        CapabilityPackId::CoreAccounting,
        upgraded.windows[0].range.clone(),
    );
    upgraded.capability_canary_window = Some(PlannedWindow::deterministic(
        CapabilityPackId::CoreAccounting,
        upgraded
            .capability_canary_window
            .as_ref()
            .expect("historic canary")
            .range
            .clone(),
    ));

    assert!(matches!(
        durable.assert_resumable_with(&upgraded),
        Err(SnapshotError::ResumePlanMismatch)
    ));
}

#[tokio::test]
async fn redacted_proof_export_accepts_adaptive_window_split_warning() {
    let (_, mirror, store, plan) = setup().await;
    let root_id = plan.windows[0].id.clone();
    let connector = FakeConnector {
        batch: Mutex::new(VecDeque::from([Err(TallyError::ReadResponseTooLarge {
            scope: ReadResponseScope::VoucherWindow,
        })])),
        company: plan.company.clone(),
        requests: Mutex::new(Vec::new()),
    };
    let result = FullSnapshotEngine::new(&mirror, &store, &connector)
        .run(&plan, &AtomicCancellation::default())
        .await
        .expect("complete adaptive split snapshot");

    assert_eq!(result.state.windows[&root_id].phase, WindowPhase::Split);
    assert!(result
        .state
        .warning_codes
        .contains(&WarningCode::AdaptiveWindowSplit));
    let proof_id = result
        .receipt
        .proof_id
        .as_deref()
        .expect("completed snapshot has proof id");
    mirror
        .redacted_proof_export(
            &plan.mirror_company_id,
            proof_id,
            result.proof.completed_at_unix_ms.unwrap(),
        )
        .await
        .expect("adaptive split warning must export");
    assert!(mirror
        .local_reconciliation_mismatches(
            &plan.mirror_company_id,
            proof_id,
            result.proof.completed_at_unix_ms.unwrap(),
        )
        .await
        .expect("adaptive split warning must permit mismatch drill-down")
        .is_empty());
}

#[tokio::test]
async fn simulator_transport_limit_splits_only_the_voucher_window_before_child_dispatch() {
    const TEST_RESPONSE_LIMIT: usize = 4 * 1024;
    let _simulator_guard = simulator_test_lock().lock().await;
    let (_, mirror, store, mut plan) = setup().await;
    plan.resume_key = "resume-transport-adaptive-split".to_string();
    plan.run_id = "run-transport-adaptive-split".to_string();
    plan.pack_schema_version = bridge_tally_core::CORE_ACCOUNTING_SCHEMA_VERSION;
    let company_guid = plan.company.identity.company_guid.clone();
    let company_extent = || {
        format!(
            r#"<ENVELOPE><HEADER><STATUS>1</STATUS></HEADER><BODY><DATA><COLLECTION><COMPANY NAME="Synthetic Company"><LASTVOUCHERDATE TYPE="Date">20260731</LASTVOUCHERDATE><BOOKSFROM TYPE="Date">20260401</BOOKSFROM><NAME TYPE="String">Synthetic Company</NAME><GUID TYPE="String">{company_guid}</GUID><COMPANYNUMBER TYPE="Number">100001</COMPANYNUMBER><ALTMSTID TYPE="Number">1</ALTMSTID></COMPANY></COLLECTION></DATA></BODY></ENVELOPE>"#
        )
    };
    let company_list = || {
        format!(
            r#"<ENVELOPE><HEADER><VERSION>1</VERSION><STATUS>1</STATUS></HEADER><BODY><DATA><COLLECTION><COMPANY NAME="Synthetic Company"><GUID TYPE="String">{company_guid}</GUID><COMPANYNUMBER TYPE="Number">100001</COMPANYNUMBER><BOOKSFROM TYPE="Date">20260401</BOOKSFROM></COMPANY></COLLECTION></DATA></BODY></ENVELOPE>"#
        )
    };
    let native_groups = || {
        format!(
            r#"<ENVELOPE><HEADER><STATUS>1</STATUS></HEADER><BODY><DATA><COLLECTION><GROUP NAME="Primary"><GUID TYPE="String">{company_guid}-00000000</GUID><PARENT TYPE="String">Primary</PARENT><ALTERID TYPE="Number">1</ALTERID><MASTERID TYPE="Number">0</MASTERID></GROUP></COLLECTION></DATA></BODY></ENVELOPE>"#
        )
    };
    let native_ledgers = || {
        format!(
            r#"<ENVELOPE><HEADER><STATUS>1</STATUS></HEADER><BODY><DATA><COLLECTION><LEDGER NAME="Synthetic Ledger"><GUID TYPE="String">{company_guid}-00000001</GUID><PARENT TYPE="String">Primary</PARENT><ALTERID TYPE="Number">1</ALTERID><MASTERID TYPE="Number">1</MASTERID><OPENINGBALANCE TYPE="Amount">0.00</OPENINGBALANCE></LEDGER></COLLECTION></DATA></BODY></ENVELOPE>"#
        )
    };
    let native_voucher_types = || {
        format!(
            r#"<ENVELOPE><HEADER><STATUS>1</STATUS></HEADER><BODY><DATA><COLLECTION><VOUCHERTYPE NAME="Synthetic Voucher Type"><GUID TYPE="String">{company_guid}-00000002</GUID><PARENT TYPE="String">Synthetic Voucher Type</PARENT><ALTERID TYPE="Number">1</ALTERID><MASTERID TYPE="Number">2</MASTERID></VOUCHERTYPE></COLLECTION></DATA></BODY></ENVELOPE>"#
        )
    };
    assert!(
        parse_native_ledger_source_records_with_evidence(&native_ledgers(), &company_guid).is_ok(),
        "the synthetic native-ledger response must satisfy the dispatched parser"
    );
    let simulator = SequenceSimulator::spawn(vec![
        ScenarioPlan::new(Fixture::SyntheticXml(company_list()))
            .with_encoding(WireEncoding::Utf16Le),
        ScenarioPlan::new(Fixture::SyntheticXml(company_extent()))
            .with_encoding(WireEncoding::Utf16Le),
        ScenarioPlan::new(Fixture::SyntheticXml(company_extent()))
            .with_encoding(WireEncoding::Utf16Le),
        ScenarioPlan::new(Fixture::SyntheticXml(native_groups()))
            .with_encoding(WireEncoding::Utf16Le),
        ScenarioPlan::new(Fixture::SyntheticXml(native_groups()))
            .with_encoding(WireEncoding::Utf16Le),
        ScenarioPlan::new(Fixture::SyntheticXml(company_extent()))
            .with_encoding(WireEncoding::Utf16Le),
        ScenarioPlan::new(Fixture::SyntheticXml(company_extent()))
            .with_encoding(WireEncoding::Utf16Le),
        ScenarioPlan::new(Fixture::SyntheticXml(company_extent()))
            .with_encoding(WireEncoding::Utf16Le),
        ScenarioPlan::new(Fixture::SyntheticXml(company_extent()))
            .with_encoding(WireEncoding::Utf16Le),
        ScenarioPlan::new(Fixture::SyntheticXml(native_ledgers()))
            .with_encoding(WireEncoding::Utf16Le),
        ScenarioPlan::new(Fixture::SyntheticXml(native_ledgers()))
            .with_encoding(WireEncoding::Utf16Le),
        ScenarioPlan::new(Fixture::SyntheticXml(company_extent()))
            .with_encoding(WireEncoding::Utf16Le),
        ScenarioPlan::new(Fixture::SyntheticXml(company_extent()))
            .with_encoding(WireEncoding::Utf16Le),
        ScenarioPlan::new(Fixture::SyntheticXml(native_voucher_types()))
            .with_encoding(WireEncoding::Utf16Le),
        // The voucher export is bracketed by paired, GUID-verified book
        // extent reads. Keep these distinct from the intentionally
        // oversized voucher response below so the test exercises the
        // voucher-only adaptive split contract.
        ScenarioPlan::new(Fixture::SyntheticXml(company_extent()))
            .with_encoding(WireEncoding::Utf16Le),
        ScenarioPlan::new(Fixture::SyntheticXml(company_extent()))
            .with_encoding(WireEncoding::Utf16Le),
        ScenarioPlan::new(Fixture::Oversized {
            minimum_bytes: TEST_RESPONSE_LIMIT + 1,
        })
        .with_encoding(WireEncoding::Utf16Le)
        .with_framing(ResponseFraming::ConnectionClose),
        ScenarioPlan::new(Fixture::SyntheticXml(company_list()))
            .with_encoding(WireEncoding::Utf16Le),
    ])
    .unwrap();
    let config = TallyConfig {
        host: "127.0.0.1".to_string(),
        port: simulator.address().port(),
    };
    plan.company.identity = company_source_identity(
        &format!("tally_xml_http:http://127.0.0.1:{}", config.port),
        &company_guid,
        "100001",
        "Synthetic Company",
        "20260401",
    );
    let listed = bridge_tally_protocol::parse_companies_from_collection(&company_list())
        .expect("synthetic CompanyListV2 response parses");
    assert_eq!(listed.len(), 1);
    assert_eq!(
        company_source_identity(
            &plan.company.identity.bridge_source_lineage,
            listed[0].guid.as_deref().expect("company GUID"),
            listed[0].company_number.as_deref().expect("company number"),
            &listed[0].name,
            listed[0].books_from.as_deref().expect("books from"),
        ),
        plan.company.identity,
    );
    let runtime = TallyRuntime::with_transport_policy(TransportPolicy {
        request_timeout: std::time::Duration::from_secs(30),
        status_response_max_bytes: TEST_RESPONSE_LIMIT,
        xml_request_max_bytes: XML_REQUEST_MAX_BYTES,
        xml_response_max_bytes: TEST_RESPONSE_LIMIT,
    });
    let root = plan.windows[0].clone();
    let canary_context = RequestContext {
        run_id: plan.run_id.clone(),
        company: plan.company.clone(),
        pack: plan.pack,
        schema_version: plan.pack_schema_version,
        window: root.range.clone(),
        query_profile: root.query_profile.clone(),
        filters_sha256: root.filters_sha256.clone(),
    };
    let connector = RuntimeReadOnlyConnector {
        inner: RuntimeTallyConnector::new(runtime, config, plan.company.clone(), canary_context)
            .unwrap(),
        company: plan.company.clone(),
    };
    let crash_store = FailAfterFirstSplitStore {
        inner: store.clone(),
        failed: AtomicBool::new(false),
    };

    let result = FullSnapshotEngine::new(&mirror, &crash_store, &connector)
        .run(&plan, &AtomicCancellation::default())
        .await;
    let error = result.expect_err("stop after the split graph is durably persisted");
    assert!(matches!(
        error,
        SnapshotError::StateInvariant("injected_crash_after_split_commit")
    ));
    let requests = simulator.finish().unwrap();
    let persisted = store.load(&plan.resume_key).await.unwrap().unwrap();
    assert_eq!(persisted.windows.len(), 3);
    assert!(persisted.windows[&root.id].split.is_some());
    assert_eq!(
        persisted
            .windows
            .values()
            .filter(|window| window.parent_window_id.as_deref() == Some(root.id.as_str()))
            .count(),
        2
    );
    assert_eq!(requests.len(), 18);
    assert!(requests.iter().all(|request| request.request_processed));
}

#[tokio::test]
async fn adaptive_windowing_recursively_splits_in_deterministic_date_order() {
    let (_, mirror, store, plan) = setup().await;
    let connector = FakeConnector {
        batch: Mutex::new(VecDeque::from([
            Err(TallyError::ReadResponseTooLarge {
                scope: ReadResponseScope::VoucherWindow,
            }),
            Err(TallyError::ReadResponseTooLarge {
                scope: ReadResponseScope::VoucherWindow,
            }),
        ])),
        company: plan.company.clone(),
        requests: Mutex::new(Vec::new()),
    };
    let result = FullSnapshotEngine::new(&mirror, &store, &connector)
        .run(&plan, &AtomicCancellation::default())
        .await
        .expect("recursively split oversized left child");
    let ranges = result
        .state
        .executable_leaves()
        .into_iter()
        .map(|window| (window.range.from_yyyymmdd, window.range.to_yyyymmdd))
        .collect::<Vec<_>>();
    assert_eq!(
        ranges,
        vec![
            ("20260701".to_string(), "20260708".to_string()),
            ("20260709".to_string(), "20260716".to_string()),
            ("20260717".to_string(), "20260731".to_string()),
        ]
    );
    assert_eq!(result.state.progress.total_windows, 3);
}

#[tokio::test]
async fn adaptive_leaf_limit_fails_closed_before_mutating_the_graph() {
    let (_, mirror, store, mut plan) = setup().await;
    plan.resume_key = "resume-adaptive-limit".to_string();
    plan.run_id = "run-adaptive-limit".to_string();
    plan.adaptive_window_policy
        .as_mut()
        .unwrap()
        .maximum_leaf_windows = 1;
    let root_id = plan.windows[0].id.clone();
    let connector = FakeConnector {
        batch: Mutex::new(VecDeque::from([Err(TallyError::ReadResponseTooLarge {
            scope: ReadResponseScope::VoucherWindow,
        })])),
        company: plan.company.clone(),
        requests: Mutex::new(Vec::new()),
    };
    let result = FullSnapshotEngine::new(&mirror, &store, &connector)
        .run(&plan, &AtomicCancellation::default())
        .await
        .expect("leaf limit is a terminal truthful result");
    assert_eq!(result.state.windows.len(), 1);
    assert!(result.state.windows[&root_id].split.is_none());
    assert!(result
        .state
        .gap_codes
        .contains("adaptive_window_limit_reached"));
    assert!(result
        .proof
        .gaps
        .iter()
        .any(|gap| gap.safe_reason_code == "adaptive_window_limit_reached"));
}

#[tokio::test]
async fn adaptive_total_node_ceiling_is_checked_before_graph_mutation() {
    let (_, _, _, mut plan) = setup().await;
    let broad_range = ReadWindow {
        from_yyyymmdd: "00010101".to_string(),
        to_yyyymmdd: "99991231".to_string(),
    };
    plan.windows = vec![PlannedWindow::deterministic(
        CapabilityPackId::CoreAccounting,
        broad_range,
    )];
    plan.capability_canary_window = Some(PlannedWindow::deterministic(
        CapabilityPackId::CoreAccounting,
        ReadWindow {
            from_yyyymmdd: "00010101".to_string(),
            to_yyyymmdd: "00010101".to_string(),
        },
    ));
    let mut state = DurableSnapshotState::new(&plan, Freshness::NeverVerified).unwrap();
    while state.windows.len() < MAX_SNAPSHOT_WINDOWS - 1 {
        let candidate = state
            .executable_leaves()
            .into_iter()
            .max_by_key(|window| {
                let from = parse_yyyymmdd(&window.range.from_yyyymmdd).unwrap();
                let to = parse_yyyymmdd(&window.range.to_yyyymmdd).unwrap();
                (to - from).num_days()
            })
            .unwrap();
        state.windows.get_mut(&candidate.id).unwrap().phase = WindowPhase::Extracting;
        assert!(matches!(
            split_leaf(&mut state, &plan, &candidate.id).unwrap(),
            SplitLeafResult::Created
        ));
    }
    assert_eq!(state.windows.len(), 1_023);
    state
        .validate_invariants()
        .expect("near-limit graph is valid");
    let candidate = state
        .executable_leaves()
        .into_iter()
        .max_by_key(|window| {
            let from = parse_yyyymmdd(&window.range.from_yyyymmdd).unwrap();
            let to = parse_yyyymmdd(&window.range.to_yyyymmdd).unwrap();
            (to - from).num_days()
        })
        .unwrap();
    state.windows.get_mut(&candidate.id).unwrap().phase = WindowPhase::Extracting;
    assert!(matches!(
        split_leaf(&mut state, &plan, &candidate.id).unwrap(),
        SplitLeafResult::LeafLimitReached
    ));
    assert_eq!(state.windows.len(), 1_023);
}

#[tokio::test]
async fn one_day_overflow_fails_once_and_preserves_previous_checkpoint() {
    let (_, mirror, store, mut plan) = setup().await;
    let one_day = ReadWindow {
        from_yyyymmdd: "20260701".to_string(),
        to_yyyymmdd: "20260701".to_string(),
    };
    plan.resume_key = "resume-one-day-overflow".to_string();
    plan.run_id = "run-one-day-overflow".to_string();
    plan.windows = vec![PlannedWindow::deterministic(
        CapabilityPackId::CoreAccounting,
        one_day.clone(),
    )];
    plan.capability_canary_window = Some(PlannedWindow::deterministic(
        CapabilityPackId::CoreAccounting,
        one_day,
    ));
    let previous = seed_verified_checkpoint(&mirror, &plan).await;
    let freshness_before = mirror
        .freshness(
            &plan.mirror_company_id,
            pack_code(plan.pack),
            Utc::now().timestamp_millis(),
        )
        .await
        .unwrap();
    let mut state =
        DurableSnapshotState::new(&plan, core_freshness(freshness_before.state)).unwrap();
    state.checkpoint_before = freshness_before.checkpoint_token.clone();
    state.gap_codes.insert("earlier_safe_gap".to_string());
    state.warning_codes.insert(WarningCode::AdaptiveWindowSplit);
    store.save(&mut state).await.unwrap();
    let connector = FakeConnector {
        batch: Mutex::new(VecDeque::from([Err(TallyError::ReadResponseTooLarge {
            scope: ReadResponseScope::VoucherWindow,
        })])),
        company: plan.company.clone(),
        requests: Mutex::new(Vec::new()),
    };
    let result = FullSnapshotEngine::new(&mirror, &store, &connector)
        .run(&plan, &AtomicCancellation::default())
        .await
        .expect("one-day overflow is a truthful terminal result");

    assert_eq!(result.state.progress.phase, SnapshotPhase::Failed);
    assert!(!result.receipt.checkpoint_advanced);
    assert!(result
        .state
        .gap_codes
        .contains("minimum_window_response_too_large"));
    let status = crate::sync::coordinator::status_from_state_for_test(result.state.clone(), false);
    assert_eq!(status.phase, SnapshotPhase::Failed);
    assert_eq!((status.completed_windows, status.total_windows), (0, 1));
    assert!(status
        .gap_codes
        .contains(&"minimum_window_response_too_large".to_string()));
    assert!(result
        .proof
        .gaps
        .iter()
        .any(|gap| gap.safe_reason_code == "minimum_window_response_too_large"));
    assert!(result
        .proof
        .gaps
        .iter()
        .any(|gap| gap.safe_reason_code == "earlier_safe_gap"));
    let ledger_receipt = mirror
        .historical_commit_receipt_for_batch(
            result.state.batch_id.as_deref().unwrap(),
            &plan.run_id,
        )
        .await
        .unwrap();
    assert_eq!(
        ledger_receipt.facts.gap_codes,
        vec!["earlier_safe_gap", "minimum_window_response_too_large"]
    );
    assert_eq!(
        ledger_receipt.facts.warning_codes,
        vec!["adaptive_window_split"]
    );
    assert_eq!(connector.requests.lock().unwrap().len(), 1);
    let freshness_after = mirror
        .freshness(
            &plan.mirror_company_id,
            pack_code(plan.pack),
            Utc::now().timestamp_millis(),
        )
        .await
        .unwrap();
    assert_eq!(freshness_after.checkpoint_token, Some(previous));
    assert_eq!(freshness_after.proof_id, freshness_before.proof_id);
    assert_eq!(
        freshness_after.verified_at_unix_ms,
        freshness_before.verified_at_unix_ms
    );
}

#[tokio::test]
async fn terminal_gap_and_warning_sets_survive_commit_pending_crash_resume() {
    let (_, mirror, store, mut plan) = setup().await;
    let one_day = ReadWindow {
        from_yyyymmdd: "20260701".to_string(),
        to_yyyymmdd: "20260701".to_string(),
    };
    plan.resume_key = "resume-terminal-commit-pending".to_string();
    plan.run_id = "run-terminal-commit-pending".to_string();
    plan.windows = vec![PlannedWindow::deterministic(
        CapabilityPackId::CoreAccounting,
        one_day.clone(),
    )];
    plan.capability_canary_window = Some(PlannedWindow::deterministic(
        CapabilityPackId::CoreAccounting,
        one_day,
    ));
    let mut state = DurableSnapshotState::new(&plan, Freshness::NeverVerified).unwrap();
    state.gap_codes.insert("earlier_safe_gap".to_string());
    state.warning_codes.insert(WarningCode::AdaptiveWindowSplit);
    store.save(&mut state).await.unwrap();

    let connector = FakeConnector {
        batch: Mutex::new(VecDeque::from([Err(TallyError::ReadResponseTooLarge {
            scope: ReadResponseScope::VoucherWindow,
        })])),
        company: plan.company.clone(),
        requests: Mutex::new(Vec::new()),
    };
    let crash_store = FailAfterFirstCommitPendingStore {
        inner: store.clone(),
        failed: AtomicBool::new(false),
    };
    let error = FullSnapshotEngine::new(&mirror, &crash_store, &connector)
        .run(&plan, &AtomicCancellation::default())
        .await
        .expect_err("inject crash after durable CommitPending save");
    assert!(matches!(
        error,
        SnapshotError::StateInvariant("injected_crash_after_commit_pending")
    ));
    let pending = store.load(&plan.resume_key).await.unwrap().unwrap();
    assert_eq!(pending.progress.phase, SnapshotPhase::CommitPending);
    assert!(pending.gap_codes.contains("earlier_safe_gap"));
    assert!(pending
        .gap_codes
        .contains("minimum_window_response_too_large"));
    assert!(pending
        .warning_codes
        .contains(&WarningCode::AdaptiveWindowSplit));

    let resumed = FullSnapshotEngine::new(&mirror, &store, &connector)
        .run(&plan, &AtomicCancellation::default())
        .await
        .expect("resume reconstructs and commits the identical terminal decision");
    assert_eq!(resumed.state.progress.phase, SnapshotPhase::Failed);
    assert_eq!(connector.requests.lock().unwrap().len(), 1);
    assert_eq!(
        resumed
            .proof
            .gaps
            .iter()
            .map(|gap| gap.safe_reason_code.as_str())
            .collect::<Vec<_>>(),
        vec!["earlier_safe_gap", "minimum_window_response_too_large"]
    );
    let receipt = mirror
        .historical_commit_receipt_for_batch(
            resumed.state.batch_id.as_deref().unwrap(),
            &plan.run_id,
        )
        .await
        .unwrap();
    assert_eq!(
        receipt.facts.gap_codes,
        vec!["earlier_safe_gap", "minimum_window_response_too_large"]
    );
    assert_eq!(receipt.facts.warning_codes, vec!["adaptive_window_split"]);
}

#[tokio::test]
async fn resume_after_split_commit_never_refetches_the_parent() {
    let (_, mirror, store, plan) = setup().await;
    let root = plan.windows[0].clone();
    let connector = FakeConnector {
        batch: Mutex::new(VecDeque::from([Err(TallyError::ReadResponseTooLarge {
            scope: ReadResponseScope::VoucherWindow,
        })])),
        company: plan.company.clone(),
        requests: Mutex::new(Vec::new()),
    };
    let crash_store = FailAfterFirstSplitStore {
        inner: store.clone(),
        failed: AtomicBool::new(false),
    };
    let error = FullSnapshotEngine::new(&mirror, &crash_store, &connector)
        .run(&plan, &AtomicCancellation::default())
        .await
        .expect_err("inject crash after durable split commit");
    assert!(matches!(
        error,
        SnapshotError::StateInvariant("injected_crash_after_split_commit")
    ));
    let persisted = store
        .load(&plan.resume_key)
        .await
        .unwrap()
        .expect("load split graph after crash");
    assert_eq!(
        persisted.windows.get(&root.id).unwrap().phase,
        WindowPhase::Split
    );

    let resumed = FullSnapshotEngine::new(&mirror, &store, &connector)
        .run(&plan, &AtomicCancellation::default())
        .await
        .expect("resume persisted children");
    assert!(resumed.state.progress.phase.is_terminal());
    assert_eq!(
        connector
            .requests
            .lock()
            .unwrap()
            .iter()
            .filter(|range| *range == &root.range)
            .count(),
        1
    );
}

#[tokio::test]
async fn report_mismatch_aliases_never_persist_raw_source_ids() {
    let (_, _, _, plan) = setup().await;
    let window = &plan.windows[0];
    let raw = "00000000-0000-4000-8000-000000000777";
    let alias = scoped_mismatch_record_alias(
        &plan.company.identity.observed_fingerprint,
        &plan.run_id,
        &window.id,
        raw,
    );
    assert!(alias.starts_with("rid:"));
    assert_eq!(alias.len(), 68);
    assert!(!alias.contains(raw));
    assert_eq!(
        alias,
        scoped_mismatch_record_alias(
            &plan.company.identity.observed_fingerprint,
            &plan.run_id,
            &window.id,
            raw,
        )
    );

    let mut another_run = plan.clone();
    another_run.run_id = "run-2".to_string();
    assert_ne!(
        alias,
        scoped_mismatch_record_alias(
            &another_run.company.identity.observed_fingerprint,
            &another_run.run_id,
            &window.id,
            raw,
        )
    );

    let durable = serde_json::to_string(&ReconciliationMismatch {
        safe_reason_code: "period_report_movement_mismatch".to_string(),
        safe_record_ids: vec![alias],
    })
    .unwrap();
    assert!(!durable.contains(raw));
}

async fn seed_verified_checkpoint(mirror: &TallyMirrorRepository, plan: &SnapshotPlan) -> String {
    let batch_id = mirror
        .begin_batch(BeginBatchInput {
            run_id: "seed-run".to_string(),
            capability_snapshot_id: plan.capability_snapshot_id.clone(),
            company_id: plan.mirror_company_id.clone(),
            pack_id: pack_code(plan.pack).to_string(),
            pack_schema_major: plan.pack_schema_version.major,
            pack_schema_minor: plan.pack_schema_version.minor,
            source_transport: plan.source_transport.clone(),
            source_release: None,
            requested_from_yyyymmdd: Some("20260601".to_string()),
            requested_to_yyyymmdd: Some("20260630".to_string()),
            started_at_unix_ms: 1_000,
        })
        .await
        .unwrap();
    let token = "full:seed".to_string();
    mirror
        .commit_batch(CommitBatchInput::test_only(CommitBatchParts {
            batch_id,
            proof_contract_version: 1,
            outcome: RunOutcome::Completed,
            verification: VerificationState::Verified,
            completed_at_unix_ms: 1_500,
            record_counts_sha256: None,
            snapshot_sha256: Some("a".repeat(64)),
            expected_checkpoint_before: None,
            checkpoint_after: Some(token.clone()),
            freshness_target_seconds: 300,
            gap_codes: Vec::new(),
            warning_codes: Vec::new(),
        }))
        .await
        .unwrap();
    token
}

#[tokio::test]
async fn stale_commit_pending_checkpoint_terminalizes_and_closes_staging_batch() {
    let (pool, mirror, store, plan) = setup().await;
    let checkpoint_before = seed_verified_checkpoint(&mirror, &plan).await;
    let batch_id = mirror
        .begin_batch(BeginBatchInput {
            run_id: plan.run_id.clone(),
            capability_snapshot_id: plan.capability_snapshot_id.clone(),
            company_id: plan.mirror_company_id.clone(),
            pack_id: pack_code(plan.pack).to_string(),
            pack_schema_major: plan.pack_schema_version.major,
            pack_schema_minor: plan.pack_schema_version.minor,
            source_transport: plan.source_transport.clone(),
            source_release: None,
            requested_from_yyyymmdd: Some("20260701".to_string()),
            requested_to_yyyymmdd: Some("20260731".to_string()),
            started_at_unix_ms: plan.started_at_unix_ms,
        })
        .await
        .unwrap();
    let lost_ack_attempt = mirror
        .begin_snapshot_window_attempt(BeginSnapshotWindowAttemptInput {
            batch_id: batch_id.clone(),
            window_id: plan.windows[0].id.clone(),
            started_at_unix_ms: plan.started_at_unix_ms + 1,
        })
        .await
        .unwrap()
        .attempt;
    let freshness_before = mirror
        .freshness(
            &plan.mirror_company_id,
            pack_code(plan.pack),
            plan.started_at_unix_ms,
        )
        .await
        .unwrap();
    let mut pending =
        DurableSnapshotState::new(&plan, core_freshness(freshness_before.state)).unwrap();
    pending.batch_id = Some(batch_id.clone());
    pending.checkpoint_before = Some(checkpoint_before.clone());
    pending.gap_codes.insert("earlier_safe_gap".to_string());
    pending.set_phase(SnapshotPhase::CommitPending, None);
    pending.pending_commit = Some(PendingCommit {
        kind: PendingDecisionKind::Reconciled,
        completed_at_unix_ms: plan.started_at_unix_ms + 100,
        safe_reason_code: None,
        intended_checkpoint: Some("full:losing-run".to_string()),
        // The stale decision is deliberately replaced before receipt verification.
        expected_receipt_facts_sha256: Some("a".repeat(64)),
        reconciled_proof: None,
    });
    store.save(&mut pending).await.unwrap();

    let winning_batch = mirror
        .begin_batch(BeginBatchInput {
            run_id: "checkpoint-winning-run".to_string(),
            capability_snapshot_id: plan.capability_snapshot_id.clone(),
            company_id: plan.mirror_company_id.clone(),
            pack_id: pack_code(plan.pack).to_string(),
            pack_schema_major: plan.pack_schema_version.major,
            pack_schema_minor: plan.pack_schema_version.minor,
            source_transport: plan.source_transport.clone(),
            source_release: None,
            requested_from_yyyymmdd: None,
            requested_to_yyyymmdd: None,
            started_at_unix_ms: plan.started_at_unix_ms + 200,
        })
        .await
        .unwrap();
    mirror
        .commit_batch(CommitBatchInput::test_only(CommitBatchParts {
            batch_id: winning_batch,
            proof_contract_version: 1,
            outcome: RunOutcome::Completed,
            verification: VerificationState::Verified,
            completed_at_unix_ms: plan.started_at_unix_ms + 300,
            record_counts_sha256: None,
            snapshot_sha256: Some("b".repeat(64)),
            expected_checkpoint_before: Some(checkpoint_before.clone()),
            checkpoint_after: Some("full:winning-run".to_string()),
            freshness_target_seconds: 300,
            gap_codes: Vec::new(),
            warning_codes: Vec::new(),
        }))
        .await
        .unwrap();

    let connector = FakeConnector {
        batch: Mutex::new(VecDeque::new()),
        company: plan.company.clone(),
        requests: Mutex::new(Vec::new()),
    };
    let result = FullSnapshotEngine::new(&mirror, &store, &connector)
        .run(&plan, &AtomicCancellation::default())
        .await
        .expect("a losing checkpoint CAS becomes a durable terminal proof");

    assert_eq!(result.state.progress.phase, SnapshotPhase::Failed);
    assert!(!result.receipt.checkpoint_advanced);
    assert!(result
        .proof
        .gaps
        .iter()
        .any(|gap| gap.safe_reason_code == "snapshot_checkpoint_changed"));
    assert!(result
        .proof
        .gaps
        .iter()
        .any(|gap| gap.safe_reason_code == "earlier_safe_gap"));
    assert!(connector.requests.lock().unwrap().is_empty());
    let ledger_receipt = mirror
        .historical_commit_receipt_for_batch(&batch_id, &plan.run_id)
        .await
        .unwrap();
    assert_eq!(
        ledger_receipt.facts.checkpoint_before,
        Some(checkpoint_before)
    );
    assert_eq!(ledger_receipt.facts.checkpoint_after, None);
    assert_eq!(
        sqlx::query_scalar::<_, String>(
            "SELECT state FROM tally_observation_batches WHERE id = ?1",
        )
        .bind(&batch_id)
        .fetch_one(&pool)
        .await
        .unwrap(),
        "failed"
    );
    assert_eq!(
        sqlx::query_scalar::<_, String>(
            "SELECT state FROM tally_snapshot_window_attempts WHERE id = ?1",
        )
        .bind(&lost_ack_attempt.attempt_id)
        .fetch_one(&pool)
        .await
        .unwrap(),
        "abandoned"
    );
    let persisted = store.load(&plan.resume_key).await.unwrap().unwrap();
    assert_eq!(persisted.progress.phase, SnapshotPhase::Failed);
    assert!(persisted.pending_commit.is_none());
    assert_eq!(
        mirror
            .freshness(
                &plan.mirror_company_id,
                pack_code(plan.pack),
                plan.started_at_unix_ms + 400,
            )
            .await
            .unwrap()
            .checkpoint_token
            .as_deref(),
        Some("full:winning-run")
    );
}

#[tokio::test]
async fn immediate_checkpoint_cas_loss_terminalizes_instead_of_returning_resumable_error() {
    let (pool, mirror, store, plan) = setup().await;
    let checkpoint_before = seed_verified_checkpoint(&mirror, &plan).await;
    let batch_id = mirror
        .begin_batch(BeginBatchInput {
            run_id: plan.run_id.clone(),
            capability_snapshot_id: plan.capability_snapshot_id.clone(),
            company_id: plan.mirror_company_id.clone(),
            pack_id: pack_code(plan.pack).to_string(),
            pack_schema_major: plan.pack_schema_version.major,
            pack_schema_minor: plan.pack_schema_version.minor,
            source_transport: plan.source_transport.clone(),
            source_release: None,
            requested_from_yyyymmdd: None,
            requested_to_yyyymmdd: None,
            started_at_unix_ms: plan.started_at_unix_ms,
        })
        .await
        .unwrap();
    let lost_ack_attempt = mirror
        .begin_snapshot_window_attempt(BeginSnapshotWindowAttemptInput {
            batch_id: batch_id.clone(),
            window_id: plan.windows[0].id.clone(),
            started_at_unix_ms: plan.started_at_unix_ms + 1,
        })
        .await
        .unwrap()
        .attempt;
    let mut state = DurableSnapshotState::new(&plan, Freshness::Fresh).unwrap();
    state.batch_id = Some(batch_id.clone());
    state.checkpoint_before = Some(checkpoint_before.clone());

    let winning_batch = mirror
        .begin_batch(BeginBatchInput {
            run_id: "immediate-checkpoint-winner".to_string(),
            capability_snapshot_id: plan.capability_snapshot_id.clone(),
            company_id: plan.mirror_company_id.clone(),
            pack_id: pack_code(plan.pack).to_string(),
            pack_schema_major: plan.pack_schema_version.major,
            pack_schema_minor: plan.pack_schema_version.minor,
            source_transport: plan.source_transport.clone(),
            source_release: None,
            requested_from_yyyymmdd: None,
            requested_to_yyyymmdd: None,
            started_at_unix_ms: plan.started_at_unix_ms + 100,
        })
        .await
        .unwrap();
    mirror
        .commit_batch(CommitBatchInput::test_only(CommitBatchParts {
            batch_id: winning_batch,
            proof_contract_version: 1,
            outcome: RunOutcome::Completed,
            verification: VerificationState::Verified,
            completed_at_unix_ms: plan.started_at_unix_ms + 200,
            record_counts_sha256: None,
            snapshot_sha256: Some("d".repeat(64)),
            expected_checkpoint_before: Some(checkpoint_before),
            checkpoint_after: Some("full:immediate-winner".to_string()),
            freshness_target_seconds: 300,
            gap_codes: Vec::new(),
            warning_codes: Vec::new(),
        }))
        .await
        .unwrap();

    let connector = FakeConnector {
        batch: Mutex::new(VecDeque::new()),
        company: plan.company.clone(),
        requests: Mutex::new(Vec::new()),
    };
    let engine = FullSnapshotEngine::new(&mirror, &store, &connector);
    let cleanup = engine
        .close_open_attempts_before_decision(&mut state)
        .await
        .expect("proof preparation closes the lost-ack attempt");
    assert!(!cleanup.gap_changed);
    assert!(
        cleanup.completed_at_floor >= Some(plan.started_at_unix_ms + 1),
        "the reconciled decision must be built after the orphan completion floor"
    );
    let record_counts = BTreeMap::new();
    let losing_decision = ReconciliationDecision {
        proof: ProofManifest {
            proof_contract_version: 3,
            run_id: plan.run_id.clone(),
            source_identity: plan.company.identity.clone(),
            pack: plan.pack,
            pack_schema_version: plan.pack_schema_version,
            outcome: bridge_tally_core::RunOutcome::Completed,
            verification: bridge_tally_core::VerificationState::Verified,
            freshness: Freshness::Fresh,
            started_at_unix_ms: plan.started_at_unix_ms,
            completed_at_unix_ms: Some(plan.started_at_unix_ms + 250),
            record_counts: record_counts.clone(),
            snapshot_sha256: Some("e".repeat(64)),
            gaps: Vec::new(),
        },
        mirror_commit: CommitBatchInput::test_only(CommitBatchParts {
            batch_id: batch_id.clone(),
            proof_contract_version: 3,
            outcome: RunOutcome::Completed,
            verification: VerificationState::Verified,
            completed_at_unix_ms: plan.started_at_unix_ms + 250,
            record_counts_sha256: Some(proof_record_counts_sha256(&record_counts)),
            snapshot_sha256: Some("e".repeat(64)),
            expected_checkpoint_before: None,
            checkpoint_after: Some("full:immediate-loser".to_string()),
            freshness_target_seconds: 300,
            gap_codes: Vec::new(),
            warning_codes: Vec::new(),
        }),
        safe_mismatches: Vec::new(),
    };
    let result = engine
        .commit_decision(
            &plan,
            state,
            losing_decision,
            PendingDecisionKind::Reconciled,
            None,
        )
        .await
        .expect("an immediate CAS loss is replaced by a terminal local proof");

    assert_eq!(result.state.progress.phase, SnapshotPhase::Failed);
    assert!(result
        .proof
        .gaps
        .iter()
        .any(|gap| gap.safe_reason_code == "snapshot_checkpoint_changed"));
    assert_eq!(
        sqlx::query_scalar::<_, String>(
            "SELECT state FROM tally_observation_batches WHERE id = ?1",
        )
        .bind(&batch_id)
        .fetch_one(&pool)
        .await
        .unwrap(),
        "failed"
    );
    assert_eq!(
        sqlx::query_scalar::<_, String>(
            "SELECT state FROM tally_snapshot_window_attempts WHERE id = ?1",
        )
        .bind(&lost_ack_attempt.attempt_id)
        .fetch_one(&pool)
        .await
        .unwrap(),
        "abandoned"
    );
    assert_eq!(
        mirror
            .freshness(
                &plan.mirror_company_id,
                pack_code(plan.pack),
                plan.started_at_unix_ms + 300,
            )
            .await
            .unwrap()
            .checkpoint_token
            .as_deref(),
        Some("full:immediate-winner")
    );
}

async fn assert_nonadvancing_pending_outcome_survives_checkpoint_advance(
    source_error: TallyError,
    expected_phase: SnapshotPhase,
    expected_reason: &str,
) {
    let (pool, mirror, store, mut plan) = setup().await;
    plan.resume_key = format!("resume-preserve-{expected_reason}");
    plan.run_id = format!("run-preserve-{expected_reason}");
    let checkpoint_before = seed_verified_checkpoint(&mirror, &plan).await;
    let connector = FakeConnector {
        batch: Mutex::new(VecDeque::from([Err(source_error)])),
        company: plan.company.clone(),
        requests: Mutex::new(Vec::new()),
    };
    let crash_store = FailAfterFirstCommitPendingStore {
        inner: store.clone(),
        failed: AtomicBool::new(false),
    };
    FullSnapshotEngine::new(&mirror, &crash_store, &connector)
        .run(&plan, &AtomicCancellation::default())
        .await
        .expect_err("inject crash after the original terminal decision is durable");
    let pending = store.load(&plan.resume_key).await.unwrap().unwrap();
    assert_eq!(pending.progress.phase, SnapshotPhase::CommitPending);
    assert!(pending.pending_commit.as_ref().is_some_and(|commit| {
        commit.intended_checkpoint.is_none()
            && commit.safe_reason_code.as_deref() == Some(expected_reason)
    }));
    let original_pending_completed_at = pending
        .pending_commit
        .as_ref()
        .unwrap()
        .completed_at_unix_ms;
    let orphan_started_at = original_pending_completed_at.saturating_add(1);
    let orphan = mirror
        .begin_snapshot_window_attempt(BeginSnapshotWindowAttemptInput {
            batch_id: pending.batch_id.clone().unwrap(),
            window_id: plan.windows[0].id.clone(),
            started_at_unix_ms: orphan_started_at,
        })
        .await
        .expect("inject normal-clock orphan after pending decision")
        .attempt;
    while Utc::now().timestamp_millis() < orphan_started_at {
        tokio::task::yield_now().await;
    }

    let winning_batch = mirror
        .begin_batch(BeginBatchInput {
            run_id: format!("winner-after-{expected_reason}"),
            capability_snapshot_id: plan.capability_snapshot_id.clone(),
            company_id: plan.mirror_company_id.clone(),
            pack_id: pack_code(plan.pack).to_string(),
            pack_schema_major: plan.pack_schema_version.major,
            pack_schema_minor: plan.pack_schema_version.minor,
            source_transport: plan.source_transport.clone(),
            source_release: None,
            requested_from_yyyymmdd: None,
            requested_to_yyyymmdd: None,
            started_at_unix_ms: plan.started_at_unix_ms + 200,
        })
        .await
        .unwrap();
    let winning_checkpoint = format!("full:winner-after-{expected_reason}");
    mirror
        .commit_batch(CommitBatchInput::test_only(CommitBatchParts {
            batch_id: winning_batch,
            proof_contract_version: 1,
            outcome: RunOutcome::Completed,
            verification: VerificationState::Verified,
            completed_at_unix_ms: plan.started_at_unix_ms + 300,
            record_counts_sha256: None,
            snapshot_sha256: Some("c".repeat(64)),
            expected_checkpoint_before: Some(checkpoint_before),
            checkpoint_after: Some(winning_checkpoint.clone()),
            freshness_target_seconds: 300,
            gap_codes: Vec::new(),
            warning_codes: Vec::new(),
        }))
        .await
        .unwrap();

    let resumed = FullSnapshotEngine::new(&mirror, &store, &connector)
        .run(&plan, &AtomicCancellation::default())
        .await
        .expect("a non-advancing terminal decision is independent of the live checkpoint");
    assert_eq!(resumed.state.progress.phase, expected_phase);
    assert!(resumed
        .proof
        .gaps
        .iter()
        .any(|gap| gap.safe_reason_code == expected_reason));
    assert!(!resumed
        .proof
        .gaps
        .iter()
        .any(|gap| gap.safe_reason_code == "snapshot_checkpoint_changed"));
    assert!(!resumed
        .proof
        .gaps
        .iter()
        .any(|gap| gap.safe_reason_code == "local_clock_moved_backwards"));
    assert!(
        resumed.proof.completed_at_unix_ms.unwrap() > original_pending_completed_at,
        "normal-clock orphan completion floor must rebuild the pending decision"
    );
    assert_eq!(
        sqlx::query_scalar::<_, String>(
            "SELECT state FROM tally_snapshot_window_attempts WHERE id = ?1",
        )
        .bind(orphan.attempt_id)
        .fetch_one(&pool)
        .await
        .unwrap(),
        "abandoned"
    );
    assert_eq!(
        mirror
            .freshness(
                &plan.mirror_company_id,
                pack_code(plan.pack),
                plan.started_at_unix_ms + 400,
            )
            .await
            .unwrap()
            .checkpoint_token,
        Some(winning_checkpoint)
    );
}

#[tokio::test]
async fn failed_and_cancelled_pending_outcomes_survive_checkpoint_advance() {
    assert_nonadvancing_pending_outcome_survives_checkpoint_advance(
        TallyError::Unsupported {
            code: "endpoint_circuit_open".to_string(),
        },
        SnapshotPhase::Failed,
        "endpoint_circuit_open",
    )
    .await;
    assert_nonadvancing_pending_outcome_survives_checkpoint_advance(
        TallyError::Cancelled,
        SnapshotPhase::Cancelled,
        "run_cancelled",
    )
    .await;
}

#[tokio::test]
async fn closed_commit_pending_recovers_historical_receipt_after_checkpoint_advances() {
    let (_, mirror, store, plan) = setup().await;
    let first_batch = mirror
        .begin_batch(BeginBatchInput {
            run_id: plan.run_id.clone(),
            capability_snapshot_id: plan.capability_snapshot_id.clone(),
            company_id: plan.mirror_company_id.clone(),
            pack_id: pack_code(plan.pack).to_string(),
            pack_schema_major: plan.pack_schema_version.major,
            pack_schema_minor: plan.pack_schema_version.minor,
            source_transport: plan.source_transport.clone(),
            source_release: None,
            requested_from_yyyymmdd: Some("20260701".to_string()),
            requested_to_yyyymmdd: Some("20260731".to_string()),
            started_at_unix_ms: plan.started_at_unix_ms,
        })
        .await
        .unwrap();
    let record_counts = BTreeMap::new();
    let first_checkpoint = "full:first-historical".to_string();
    let first_receipt = mirror
        .commit_batch(CommitBatchInput::test_only(CommitBatchParts {
            batch_id: first_batch.clone(),
            proof_contract_version: 3,
            outcome: RunOutcome::Completed,
            verification: VerificationState::Verified,
            completed_at_unix_ms: 3_000,
            record_counts_sha256: Some(proof_record_counts_sha256(&record_counts)),
            snapshot_sha256: Some("a".repeat(64)),
            expected_checkpoint_before: None,
            checkpoint_after: Some(first_checkpoint.clone()),
            freshness_target_seconds: 300,
            gap_codes: Vec::new(),
            warning_codes: Vec::new(),
        }))
        .await
        .unwrap();
    let proof = ProofManifest {
        proof_contract_version: 3,
        run_id: plan.run_id.clone(),
        source_identity: plan.company.identity.clone(),
        pack: plan.pack,
        pack_schema_version: plan.pack_schema_version,
        outcome: bridge_tally_core::RunOutcome::Completed,
        verification: bridge_tally_core::VerificationState::Verified,
        freshness: Freshness::Fresh,
        started_at_unix_ms: plan.started_at_unix_ms,
        completed_at_unix_ms: Some(3_000),
        record_counts,
        snapshot_sha256: Some("a".repeat(64)),
        gaps: Vec::new(),
    };
    let mut pending_state = DurableSnapshotState::new(&plan, Freshness::NeverVerified).unwrap();
    pending_state.batch_id = Some(first_batch.clone());
    pending_state.set_phase(SnapshotPhase::CommitPending, None);
    pending_state.pending_commit = Some(PendingCommit {
        kind: PendingDecisionKind::Reconciled,
        completed_at_unix_ms: 3_000,
        safe_reason_code: None,
        intended_checkpoint: Some(first_checkpoint.clone()),
        expected_receipt_facts_sha256: Some(sha256_json(&first_receipt.facts).unwrap()),
        reconciled_proof: Some(Box::new(proof.clone())),
    });
    store.save(&mut pending_state).await.unwrap();

    let next_batch = mirror
        .begin_batch(BeginBatchInput {
            run_id: "later-run".to_string(),
            capability_snapshot_id: plan.capability_snapshot_id.clone(),
            company_id: plan.mirror_company_id.clone(),
            pack_id: pack_code(plan.pack).to_string(),
            pack_schema_major: plan.pack_schema_version.major,
            pack_schema_minor: plan.pack_schema_version.minor,
            source_transport: plan.source_transport.clone(),
            source_release: None,
            requested_from_yyyymmdd: None,
            requested_to_yyyymmdd: None,
            started_at_unix_ms: 3_100,
        })
        .await
        .unwrap();
    mirror
        .commit_batch(CommitBatchInput::test_only(CommitBatchParts {
            batch_id: next_batch,
            proof_contract_version: 1,
            outcome: RunOutcome::Completed,
            verification: VerificationState::Verified,
            completed_at_unix_ms: 3_500,
            record_counts_sha256: None,
            snapshot_sha256: Some("b".repeat(64)),
            expected_checkpoint_before: Some(first_checkpoint),
            checkpoint_after: Some("full:later".to_string()),
            freshness_target_seconds: 300,
            gap_codes: Vec::new(),
            warning_codes: Vec::new(),
        }))
        .await
        .unwrap();

    let connector = FakeConnector {
        batch: Mutex::new(VecDeque::new()),
        company: plan.company.clone(),
        requests: Mutex::new(Vec::new()),
    };
    let recovered = FullSnapshotEngine::new(&mirror, &store, &connector)
        .resolve_closed_batch(&plan, pending_state, proof)
        .await
        .expect("historical immutable receipt must recover independently of checkpoint head");
    assert_eq!(recovered.receipt.proof_id, Some(first_receipt.proof_id));
    assert_eq!(recovered.state.progress.phase, SnapshotPhase::Completed);
    assert!(connector.requests.lock().unwrap().is_empty());
}

#[tokio::test]
async fn completed_window_attempt_recovers_before_state_save_without_refetching_extraction() {
    let (_, mirror, store, mut plan) = setup().await;
    plan.resume_key = "resume-completed-window-attempt".to_string();
    plan.run_id = "run-completed-window-attempt".to_string();
    let source = core_groups(2);
    let connector = ReportConnector {
        inner: FakeConnector {
            batch: Mutex::new(VecDeque::from([Ok(source.clone()), Ok(source)])),
            company: plan.company.clone(),
            requests: Mutex::new(Vec::new()),
        },
    };
    let crash_store = FailBeforeFirstCompletedWindowSaveStore {
        inner: store.clone(),
        failed: AtomicBool::new(false),
    };

    let error = FullSnapshotEngine::new(&mirror, &crash_store, &connector)
        .run(&plan, &AtomicCancellation::default())
        .await
        .expect_err("crash after the normalized attempt commits but before state save");
    assert!(matches!(
        error,
        SnapshotError::StateInvariant("injected_crash_after_window_attempt_completion")
    ));
    assert_eq!(connector.inner.requests.lock().unwrap().len(), 1);

    let persisted = store.load(&plan.resume_key).await.unwrap().unwrap();
    let progress = persisted.windows.get(&plan.windows[0].id).unwrap();
    assert_eq!(progress.phase, WindowPhase::Staging);
    let attempt = progress
        .stage_attempt
        .as_ref()
        .expect("durable attempt ref");
    let receipt = mirror
        .load_latest_completed_window_receipt(&attempt.batch_id, &attempt.window_id)
        .await
        .unwrap()
        .expect("the normalized SQLite attempt completed before the crash");
    assert_eq!(receipt.attempt_id, attempt.attempt_id);

    let resumed = FullSnapshotEngine::new(&mirror, &store, &connector)
        .run(&plan, &AtomicCancellation::default())
        .await
        .expect("resume consumes the local receipt and only performs the stability reread");
    assert!(resumed.state.progress.phase.is_terminal());
    assert_eq!(connector.inner.requests.lock().unwrap().len(), 2);
    let completed = resumed.state.windows.get(&plan.windows[0].id).unwrap();
    assert_eq!(completed.phase, WindowPhase::Complete);
    assert!(completed.stage_attempt.is_none());
    assert_eq!(
        completed
            .stage_receipt
            .as_ref()
            .map(|stored| stored.attempt.attempt_id.as_str()),
        Some(receipt.attempt_id.as_str())
    );
}

#[tokio::test]
async fn cancellation_after_receipt_recovery_preserves_provenance_gap_and_count() {
    let (_, mirror, store, mut plan) = setup().await;
    plan.resume_key = "resume-recovered-unavailable-provenance".to_string();
    plan.run_id = "run-recovered-unavailable-provenance".to_string();
    let source = core_groups(2);
    let connector = ReportConnector {
        inner: FakeConnector {
            batch: Mutex::new(VecDeque::from([Ok(source)])),
            company: plan.company.clone(),
            requests: Mutex::new(Vec::new()),
        },
    };
    let crash_store = FailBeforeFirstCompletedWindowSaveStore {
        inner: store.clone(),
        failed: AtomicBool::new(false),
    };

    FullSnapshotEngine::new(&mirror, &crash_store, &connector)
        .run(&plan, &AtomicCancellation::default())
        .await
        .expect_err("crash after unavailable-provenance receipt completion");
    let cancellation = AtomicCancellation::default();
    cancellation.cancel();
    let result = FullSnapshotEngine::new(&mirror, &store, &connector)
        .run(&plan, &cancellation)
        .await
        .expect("recovered receipt is terminalized locally");

    assert_eq!(result.state.progress.phase, SnapshotPhase::Cancelled);
    assert!(result
        .state
        .gap_codes
        .contains("record_provenance_unavailable"));
    assert_eq!(
        result.proof.record_counts["locally_staged.provenance_unavailable"],
        2
    );
    let ledger_receipt = mirror
        .historical_commit_receipt_for_batch(
            result.state.batch_id.as_deref().expect("terminal batch id"),
            &plan.run_id,
        )
        .await
        .expect("bound terminal receipt");
    assert_eq!(ledger_receipt.facts.provenance_unavailable_records, 2);
    assert_eq!(connector.inner.requests.lock().unwrap().len(), 1);
}

#[tokio::test]
async fn cancelled_resume_replays_future_dated_abandonment_after_lost_ack() {
    let (pool, mirror, store, mut plan) = setup().await;
    plan.resume_key = "resume-future-dated-window-attempt".to_string();
    plan.run_id = "run-future-dated-window-attempt".to_string();
    let batch_id = mirror
        .begin_batch(BeginBatchInput {
            run_id: plan.run_id.clone(),
            capability_snapshot_id: plan.capability_snapshot_id.clone(),
            company_id: plan.mirror_company_id.clone(),
            pack_id: pack_code(plan.pack).to_string(),
            pack_schema_major: plan.pack_schema_version.major,
            pack_schema_minor: plan.pack_schema_version.minor,
            source_transport: plan.source_transport.clone(),
            source_release: plan.source_release.clone(),
            requested_from_yyyymmdd: Some(plan.windows[0].range.from_yyyymmdd.clone()),
            requested_to_yyyymmdd: Some(plan.windows[0].range.to_yyyymmdd.clone()),
            started_at_unix_ms: plan.started_at_unix_ms,
        })
        .await
        .expect("begin pre-crash batch");
    let future_attempt_start = Utc::now()
        .timestamp_millis()
        .saturating_add(24 * 60 * 60 * 1_000);
    let attempt = mirror
        .begin_snapshot_window_attempt(BeginSnapshotWindowAttemptInput {
            batch_id: batch_id.clone(),
            window_id: plan.windows[0].id.clone(),
            started_at_unix_ms: future_attempt_start,
        })
        .await
        .expect("persist attempt from a clock that was ahead")
        .attempt;
    let mut crashed = DurableSnapshotState::new(&plan, Freshness::NeverVerified).unwrap();
    crashed.batch_id = Some(batch_id);
    let window_id = plan.windows[0].id.clone();
    let progress = crashed.windows.get_mut(&window_id).unwrap();
    progress.phase = WindowPhase::Staging;
    progress.stage_attempt = Some(WindowStageAttempt::from(&attempt));
    crashed.set_phase(SnapshotPhase::Stage, Some(window_id));
    store
        .save(&mut crashed)
        .await
        .expect("persist synthetic crash state");

    let cancellation = AtomicCancellation::default();
    cancellation.cancel();
    let connector = FakeConnector {
        batch: Mutex::new(VecDeque::new()),
        company: plan.company.clone(),
        requests: Mutex::new(Vec::new()),
    };
    let crash_store = FailAfterClockRollbackAbandonmentStore {
        inner: store.clone(),
        failed: AtomicBool::new(false),
    };
    let error = FullSnapshotEngine::new(&mirror, &crash_store, &connector)
        .run(&plan, &cancellation)
        .await
        .expect_err("crash after abandonment commit loses the transient acknowledgement");
    assert!(matches!(
        error,
        SnapshotError::StateInvariant("injected_crash_after_window_abandonment")
    ));
    let pre_replay = store.load(&plan.resume_key).await.unwrap().unwrap();
    assert_eq!(
        pre_replay.windows[&plan.windows[0].id].phase,
        WindowPhase::Staging
    );
    assert!(!pre_replay.gap_codes.contains("local_clock_moved_backwards"));

    let result = FullSnapshotEngine::new(&mirror, &store, &connector)
        .run(&plan, &cancellation)
        .await
        .expect("idempotent replay restores durable abandonment evidence and terminalizes");

    assert_eq!(result.state.progress.phase, SnapshotPhase::Cancelled);
    assert!(result
        .state
        .gap_codes
        .contains("local_clock_moved_backwards"));
    assert!(result
        .proof
        .gaps
        .iter()
        .any(|gap| gap.safe_reason_code == "local_clock_moved_backwards"));
    assert!(
        result.proof.completed_at_unix_ms.unwrap_or_default() >= future_attempt_start,
        "proof completion cannot precede the orphan attempt completion floor"
    );
    let immutable_receipt = mirror
        .historical_commit_receipt_for_batch(
            result.state.batch_id.as_deref().unwrap(),
            &plan.run_id,
        )
        .await
        .unwrap();
    assert!(immutable_receipt.facts.completed_at_unix_ms >= future_attempt_start);
    assert!(connector.requests.lock().unwrap().is_empty());
    let stored_attempt = sqlx::query_as::<_, (String, i64)>(
        "SELECT state, completed_at_unix_ms FROM tally_snapshot_window_attempts WHERE id = ?1",
    )
    .bind(&attempt.attempt_id)
    .fetch_one(&pool)
    .await
    .expect("read abandoned future-dated attempt");
    assert_eq!(
        stored_attempt,
        ("abandoned".to_string(), future_attempt_start)
    );
}

#[tokio::test]
async fn begin_lost_ack_replays_implicit_abandonment_clock_evidence_into_proof() {
    let (_, mirror, store, mut plan) = setup().await;
    plan.resume_key = "resume-begin-lost-ack-clock".to_string();
    plan.run_id = "run-begin-lost-ack-clock".to_string();
    let batch_id = mirror
        .begin_batch(BeginBatchInput {
            run_id: plan.run_id.clone(),
            capability_snapshot_id: plan.capability_snapshot_id.clone(),
            company_id: plan.mirror_company_id.clone(),
            pack_id: pack_code(plan.pack).to_string(),
            pack_schema_major: plan.pack_schema_version.major,
            pack_schema_minor: plan.pack_schema_version.minor,
            source_transport: plan.source_transport.clone(),
            source_release: plan.source_release.clone(),
            requested_from_yyyymmdd: Some(plan.windows[0].range.from_yyyymmdd.clone()),
            requested_to_yyyymmdd: Some(plan.windows[0].range.to_yyyymmdd.clone()),
            started_at_unix_ms: plan.started_at_unix_ms,
        })
        .await
        .unwrap();
    let future_attempt_start = Utc::now()
        .timestamp_millis()
        .saturating_add(24 * 60 * 60 * 1_000);
    mirror
        .begin_snapshot_window_attempt(BeginSnapshotWindowAttemptInput {
            batch_id: batch_id.clone(),
            window_id: plan.windows[0].id.clone(),
            started_at_unix_ms: future_attempt_start,
        })
        .await
        .expect("persist an attempt whose begin acknowledgement was lost");
    let mut durable = DurableSnapshotState::new(&plan, Freshness::NeverVerified).unwrap();
    durable.batch_id = Some(batch_id);
    let window_id = plan.windows[0].id.clone();
    durable.windows.get_mut(&window_id).unwrap().phase = WindowPhase::Validating;
    durable.set_phase(SnapshotPhase::Validate, Some(window_id));
    store.save(&mut durable).await.unwrap();

    let source = core_groups(0);
    let connector = ReportConnector {
        inner: FakeConnector {
            batch: Mutex::new(VecDeque::from([
                Ok(source.clone()),
                Ok(source.clone()),
                Ok(source),
            ])),
            company: plan.company.clone(),
            requests: Mutex::new(Vec::new()),
        },
    };
    let crash_store = FailAfterClockRollbackBeginStore {
        inner: store.clone(),
        failed: AtomicBool::new(false),
    };
    let error = FullSnapshotEngine::new(&mirror, &crash_store, &connector)
        .run(&plan, &AtomicCancellation::default())
        .await
        .expect_err("crash after new attempt creation loses begin result and state save");
    assert!(matches!(
        error,
        SnapshotError::StateInvariant("injected_crash_after_window_attempt_begin")
    ));
    let before_replay = store.load(&plan.resume_key).await.unwrap().unwrap();
    assert!(!before_replay
        .gap_codes
        .contains("local_clock_moved_backwards"));

    let result = FullSnapshotEngine::new(&mirror, &store, &connector)
        .run(&plan, &AtomicCancellation::default())
        .await
        .expect("later begin recovers cumulative durable abandonment evidence");
    assert!(result
        .state
        .gap_codes
        .contains("local_clock_moved_backwards"));
    assert!(result
        .proof
        .gaps
        .iter()
        .any(|gap| gap.safe_reason_code == "local_clock_moved_backwards"));
}

#[tokio::test]
async fn early_cancellation_recovers_orphan_begin_clock_evidence_before_proof() {
    let (pool, mirror, store, mut plan) = setup().await;
    plan.resume_key = "resume-orphan-begin-early-cancel".to_string();
    plan.run_id = "run-orphan-begin-early-cancel".to_string();
    let batch_id = mirror
        .begin_batch(BeginBatchInput {
            run_id: plan.run_id.clone(),
            capability_snapshot_id: plan.capability_snapshot_id.clone(),
            company_id: plan.mirror_company_id.clone(),
            pack_id: pack_code(plan.pack).to_string(),
            pack_schema_major: plan.pack_schema_version.major,
            pack_schema_minor: plan.pack_schema_version.minor,
            source_transport: plan.source_transport.clone(),
            source_release: plan.source_release.clone(),
            requested_from_yyyymmdd: Some(plan.windows[0].range.from_yyyymmdd.clone()),
            requested_to_yyyymmdd: Some(plan.windows[0].range.to_yyyymmdd.clone()),
            started_at_unix_ms: plan.started_at_unix_ms,
        })
        .await
        .unwrap();
    let future_attempt_start = Utc::now()
        .timestamp_millis()
        .saturating_add(24 * 60 * 60 * 1_000);
    let orphan = mirror
        .begin_snapshot_window_attempt(BeginSnapshotWindowAttemptInput {
            batch_id: batch_id.clone(),
            window_id: plan.windows[0].id.clone(),
            started_at_unix_ms: future_attempt_start,
        })
        .await
        .expect("attempt commit succeeds before its state-ref save")
        .attempt;
    let mut durable = DurableSnapshotState::new(&plan, Freshness::NeverVerified).unwrap();
    durable.batch_id = Some(batch_id);
    let window_id = plan.windows[0].id.clone();
    durable.windows.get_mut(&window_id).unwrap().phase = WindowPhase::Validating;
    durable.set_phase(SnapshotPhase::Validate, Some(window_id));
    store.save(&mut durable).await.unwrap();

    let cancellation = AtomicCancellation::default();
    cancellation.cancel();
    let connector = FakeConnector {
        batch: Mutex::new(VecDeque::new()),
        company: plan.company.clone(),
        requests: Mutex::new(Vec::new()),
    };
    let result = FullSnapshotEngine::new(&mirror, &store, &connector)
        .run(&plan, &cancellation)
        .await
        .expect("orphan cleanup precedes the earliest terminal decision");

    assert_eq!(result.state.progress.phase, SnapshotPhase::Cancelled);
    assert!(result
        .proof
        .gaps
        .iter()
        .any(|gap| gap.safe_reason_code == "local_clock_moved_backwards"));
    assert!(result.proof.completed_at_unix_ms.unwrap() >= future_attempt_start);
    let immutable_receipt = mirror
        .historical_commit_receipt_for_batch(
            result.state.batch_id.as_deref().unwrap(),
            &plan.run_id,
        )
        .await
        .unwrap();
    assert!(immutable_receipt.facts.completed_at_unix_ms >= future_attempt_start);
    assert!(connector.requests.lock().unwrap().is_empty());
    let stored = sqlx::query_as::<_, (String, i64, Option<String>)>(
        "SELECT state, completed_at_unix_ms, terminal_safe_reason_code \
         FROM tally_snapshot_window_attempts WHERE id = ?1",
    )
    .bind(orphan.attempt_id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(
        stored,
        (
            "abandoned".to_string(),
            future_attempt_start,
            Some("local_clock_moved_backwards".to_string()),
        )
    );
}

#[tokio::test]
async fn partial_window_attempt_then_disappearance_fails_without_advancing_checkpoint() {
    let (_, mirror, store, mut plan) = setup().await;
    plan.resume_key = "resume-partial-window-disappearance".to_string();
    plan.run_id = "run-partial-window-disappearance".to_string();
    let previous_checkpoint = seed_verified_checkpoint(&mirror, &plan).await;
    let first = core_groups_with_provenance(MAX_WINDOW_STAGE_CHUNK + 1);
    let mut second = first.clone();
    let PackBatch::CoreAccounting(second_core) = &mut second.batch else {
        panic!("core fixture");
    };
    second_core.groups.remove(0);
    second
        .record_evidence
        .as_mut()
        .expect("synthetic provenance")
        .remove(0);
    let connector = ReportConnector {
        inner: FakeConnector {
            batch: Mutex::new(VecDeque::from([Ok(first), Ok(second)])),
            company: plan.company.clone(),
            requests: Mutex::new(Vec::new()),
        },
    };
    let crash_store = FailBeforeSecondStagingHeartbeatStore {
        inner: store.clone(),
        staging_heartbeats: Mutex::new(0),
    };

    let error = FullSnapshotEngine::new(&mirror, &crash_store, &connector)
        .run(&plan, &AtomicCancellation::default())
        .await
        .expect_err("crash after the first normalized membership chunk");
    assert!(matches!(
        error,
        SnapshotError::StateInvariant("injected_crash_after_partial_window_staging")
    ));
    assert_eq!(connector.inner.requests.lock().unwrap().len(), 1);
    let persisted = store.load(&plan.resume_key).await.unwrap().unwrap();
    assert_eq!(
        persisted.windows[&plan.windows[0].id].phase,
        WindowPhase::Staging
    );
    assert_eq!(
        mirror
            .freshness(
                &plan.mirror_company_id,
                pack_code(plan.pack),
                Utc::now().timestamp_millis(),
            )
            .await
            .unwrap()
            .checkpoint_token,
        Some(previous_checkpoint.clone())
    );

    let result = FullSnapshotEngine::new(&mirror, &store, &connector)
        .run(&plan, &AtomicCancellation::default())
        .await
        .expect("disappearance is committed as a truthful terminal result");
    assert_eq!(result.state.progress.phase, SnapshotPhase::Failed);
    assert!(!result.receipt.checkpoint_advanced);
    assert!(result
        .state
        .gap_codes
        .contains("source_changed_during_resume"));
    assert_eq!(
        result.proof.record_counts["locally_staged.accepted"],
        (MAX_WINDOW_STAGE_CHUNK + 1) as u64
    );
    assert_eq!(result.proof.record_counts["locally_staged.rejected"], 0);
    let batch_id = result.state.batch_id.as_deref().expect("terminal batch id");
    let ledger_receipt = mirror
        .historical_commit_receipt_for_batch(batch_id, &plan.run_id)
        .await
        .expect("terminal ledger receipt");
    assert_eq!(
        ledger_receipt.facts.accepted_records,
        (MAX_WINDOW_STAGE_CHUNK + 1) as i64
    );
    assert_eq!(ledger_receipt.facts.rejected_records, 0);
    assert_eq!(connector.inner.requests.lock().unwrap().len(), 2);
    assert_eq!(
        mirror
            .freshness(
                &plan.mirror_company_id,
                pack_code(plan.pack),
                Utc::now().timestamp_millis(),
            )
            .await
            .unwrap()
            .checkpoint_token,
        Some(previous_checkpoint)
    );
}

#[tokio::test]
async fn normalized_staging_keeps_large_window_state_and_save_generation_compact() {
    let (_, mirror, store, mut plan) = setup().await;
    plan.resume_key = "resume-large-normalized-window".to_string();
    plan.run_id = "run-large-normalized-window".to_string();
    let record_count = (MAX_WINDOW_STAGE_CHUNK * 4) + 1;
    let source = core_groups(record_count);
    let connector = ReportConnector {
        inner: FakeConnector {
            batch: Mutex::new(VecDeque::from([Ok(source.clone()), Ok(source)])),
            company: plan.company.clone(),
            requests: Mutex::new(Vec::new()),
        },
    };
    let metrics_store = SaveMetricsStore {
        inner: store,
        saves: Mutex::new(0),
        max_state_json_bytes: Mutex::new(0),
    };

    let result = FullSnapshotEngine::new(&mirror, &metrics_store, &connector)
        .run(&plan, &AtomicCancellation::default())
        .await
        .expect("large normalized window remains resumable and bounded");
    let saves = *metrics_store.saves.lock().unwrap();
    let max_state_json_bytes = *metrics_store.max_state_json_bytes.lock().unwrap();
    assert!(result.state.progress.phase.is_terminal());
    assert_eq!(result.state.generation as usize, saves);
    assert!(
        saves < 32,
        "durable generations must follow phases, not {record_count} records: {saves}"
    );
    assert!(
        max_state_json_bytes < 128 * 1024,
        "record identities leaked into compact state: {max_state_json_bytes} bytes"
    );
    assert_eq!(connector.inner.requests.lock().unwrap().len(), 2);
}

#[tokio::test]
async fn committed_window_with_foreign_master_text_diagnostic_persists_safe_warning_code() {
    let (_, mirror, store, plan) = setup().await;
    let source = core_groups_with_foreign_master_text_diagnostic();
    let connector = ReportConnector {
        inner: FakeConnector {
            batch: Mutex::new(VecDeque::from([Ok(source.clone()), Ok(source)])),
            company: plan.company.clone(),
            requests: Mutex::new(Vec::new()),
        },
    };

    let result = FullSnapshotEngine::new(&mirror, &store, &connector)
        .run(&plan, &AtomicCancellation::default())
        .await
        .expect("diagnosed window commits with a safe operator warning");

    assert!(result
        .state
        .warning_codes
        .contains(&WarningCode::ForeignMasterTextRenderingDegraded));
    let receipt = mirror
        .historical_commit_receipt_for_batch(
            result.state.batch_id.as_deref().expect("committed batch"),
            &plan.run_id,
        )
        .await
        .expect("committed receipt");
    assert_eq!(
        receipt.facts.warning_codes,
        vec![bridge_tally_core::FOREIGN_MASTER_TEXT_RENDERING_WARNING_CODE.to_string()]
    );
    let export = mirror
        .redacted_proof_export(
            &plan.mirror_company_id,
            result
                .receipt
                .proof_id
                .as_deref()
                .expect("committed proof id"),
            10_000,
        )
        .await
        .expect("diagnosed window remains exportable as a redacted proof");
    assert!(export
        .json
        .contains(bridge_tally_core::FOREIGN_MASTER_TEXT_RENDERING_WARNING_CODE));
}

#[tokio::test]
async fn abandoned_diagnosed_attempt_does_not_emit_warning_after_clean_reread() {
    let (_, mirror, store, mut plan) = setup().await;
    plan.resume_key = "resume-abandoned-diagnosed-attempt".to_string();
    plan.run_id = "run-abandoned-diagnosed-attempt".to_string();
    let diagnosed = core_groups_with_foreign_master_text_diagnostic();
    let clean = core_groups(1);
    let connector = ReportConnector {
        inner: FakeConnector {
            batch: Mutex::new(VecDeque::from([
                Ok(diagnosed),
                Ok(clean.clone()),
                Ok(clean),
            ])),
            company: plan.company.clone(),
            requests: Mutex::new(Vec::new()),
        },
    };
    let crash_store = FailAfterFirstStagingSaveStore {
        inner: store.clone(),
        failed: AtomicBool::new(false),
    };

    let error = FullSnapshotEngine::new(&mirror, &crash_store, &connector)
        .run(&plan, &AtomicCancellation::default())
        .await
        .expect_err("crash after the diagnosed attempt is durably staged");
    assert!(matches!(
        error,
        SnapshotError::StateInvariant("injected_crash_after_window_attempt_staging")
    ));
    let persisted = store.load(&plan.resume_key).await.unwrap().unwrap();
    let staged = persisted.windows[&plan.windows[0].id]
        .stage_attempt
        .as_ref()
        .expect("the diagnosed warning is bound to the open attempt");
    assert!(staged
        .warning_codes
        .contains(&WarningCode::ForeignMasterTextRenderingDegraded));
    assert!(!persisted
        .warning_codes
        .contains(&WarningCode::ForeignMasterTextRenderingDegraded));

    let result = FullSnapshotEngine::new(&mirror, &store, &connector)
        .run(&plan, &AtomicCancellation::default())
        .await
        .expect("abandon the diagnosed attempt and complete the clean reread");
    assert!(!result
        .state
        .warning_codes
        .contains(&WarningCode::ForeignMasterTextRenderingDegraded));
    let receipt = mirror
        .historical_commit_receipt_for_batch(
            result.state.batch_id.as_deref().expect("committed batch"),
            &plan.run_id,
        )
        .await
        .expect("committed receipt");
    assert!(!receipt
        .facts
        .warning_codes
        .contains(&bridge_tally_core::FOREIGN_MASTER_TEXT_RENDERING_WARNING_CODE.to_string()));
    assert_eq!(connector.inner.requests.lock().unwrap().len(), 3);
}

#[tokio::test]
async fn committed_clean_window_has_no_foreign_master_text_warning_code() {
    let (_, mirror, store, plan) = setup().await;
    let source = core_groups(1);
    let connector = ReportConnector {
        inner: FakeConnector {
            batch: Mutex::new(VecDeque::from([Ok(source.clone()), Ok(source)])),
            company: plan.company.clone(),
            requests: Mutex::new(Vec::new()),
        },
    };

    let result = FullSnapshotEngine::new(&mirror, &store, &connector)
        .run(&plan, &AtomicCancellation::default())
        .await
        .expect("clean window commits without the foreign-text warning");

    assert!(!result
        .state
        .warning_codes
        .contains(&WarningCode::ForeignMasterTextRenderingDegraded));
}

#[test]
fn aggregate_reconciliation_record_budget_accepts_the_exact_boundary() {
    assert!(!aggregate_record_budget_exceeded([Ok(50_000), Ok(50_000),])
        .expect("the exact aggregate boundary is valid"));
    assert!(aggregate_record_budget_exceeded([Ok(50_000), Ok(50_001),])
        .expect("the first record above the boundary is detected"));
}

#[tokio::test]
async fn committed_over_budget_pending_recovers_exact_proof_without_hydration() {
    let (_, mirror, store, plan) = setup().await;
    let batch_id = mirror
        .begin_batch(BeginBatchInput {
            run_id: plan.run_id.clone(),
            capability_snapshot_id: plan.capability_snapshot_id.clone(),
            company_id: plan.mirror_company_id.clone(),
            pack_id: pack_code(plan.pack).to_string(),
            pack_schema_major: plan.pack_schema_version.major,
            pack_schema_minor: plan.pack_schema_version.minor,
            source_transport: plan.source_transport.clone(),
            source_release: plan.source_release.clone(),
            requested_from_yyyymmdd: Some(plan.windows[0].range.from_yyyymmdd.clone()),
            requested_to_yyyymmdd: Some(plan.windows[0].range.to_yyyymmdd.clone()),
            started_at_unix_ms: plan.started_at_unix_ms,
        })
        .await
        .unwrap();
    let planned = &plan.windows[0];
    let mut evidence = canonicalize_window(
        &CanonicalWindowContext {
            requested_pack: plan.pack,
            schema_version: plan.pack_schema_version,
            source_identity: &plan.company.identity,
            query_profile: &planned.query_profile,
            filters_sha256: &planned.filters_sha256,
            external_references: &plan.external_references,
            window_id: &planned.id,
            requested_window: &planned.range,
        },
        &core_groups(0),
    )
    .unwrap()
    .evidence;
    let over_budget = u32::try_from(MAX_RECONCILIATION_RECORDS + 1).unwrap();
    evidence.deduped_count = u64::from(over_budget);
    evidence.record_set_sha256 = Some("a".repeat(64));
    let mut state = DurableSnapshotState::new(&plan, Freshness::NeverVerified).unwrap();
    state.batch_id = Some(batch_id.clone());
    state
        .gap_codes
        .insert("source_count_unavailable".to_string());
    let progress = state.windows.get_mut(&planned.id).unwrap();
    progress.phase = WindowPhase::Complete;
    progress.stage_receipt = Some(WindowStageReceipt {
        attempt: WindowStageAttempt {
            attempt_id: "committed-over-budget-attempt".to_string(),
            batch_id: batch_id.clone(),
            window_id: planned.id.clone(),
            attempt_ordinal: 1,
            warning_codes: BTreeSet::new(),
        },
        member_count: over_budget,
        membership_sha256: "a".repeat(64),
        receipt_sha256: "b".repeat(64),
    });
    progress.evidence = Some(evidence);
    state.set_phase(SnapshotPhase::Reconcile, None);

    let completed_at = plan.started_at_unix_ms + 100;
    let record_counts = BTreeMap::new();
    let snapshot_sha256 = "c".repeat(64);
    let proof = ProofManifest {
        proof_contract_version: 3,
        run_id: plan.run_id.clone(),
        source_identity: plan.company.identity.clone(),
        pack: plan.pack,
        pack_schema_version: plan.pack_schema_version,
        outcome: bridge_tally_core::RunOutcome::Completed,
        verification: bridge_tally_core::VerificationState::Partial,
        freshness: Freshness::NeverVerified,
        started_at_unix_ms: plan.started_at_unix_ms,
        completed_at_unix_ms: Some(completed_at),
        record_counts: record_counts.clone(),
        snapshot_sha256: Some(snapshot_sha256.clone()),
        gaps: vec![bridge_tally_core::Gap {
            pack: plan.pack,
            field_or_invariant: "source_count_unavailable".to_string(),
            state: CapabilityState::Unknown,
            safe_reason_code: "source_count_unavailable".to_string(),
        }],
    };
    let decision = ReconciliationDecision {
        proof: proof.clone(),
        mirror_commit: CommitBatchInput::test_only(CommitBatchParts {
            batch_id: batch_id.clone(),
            proof_contract_version: 3,
            outcome: RunOutcome::Completed,
            verification: VerificationState::Partial,
            completed_at_unix_ms: completed_at,
            record_counts_sha256: Some(proof_record_counts_sha256(&record_counts)),
            snapshot_sha256: Some(snapshot_sha256),
            expected_checkpoint_before: None,
            checkpoint_after: None,
            freshness_target_seconds: plan.freshness_target_seconds,
            gap_codes: vec!["source_count_unavailable".to_string()],
            warning_codes: Vec::new(),
        }),
        safe_mismatches: Vec::new(),
    };
    let connector = FakeConnector {
        batch: Mutex::new(VecDeque::new()),
        company: plan.company.clone(),
        requests: Mutex::new(Vec::new()),
    };
    let engine = FullSnapshotEngine::new(&mirror, &store, &connector);
    let (_pending_state, staged) = engine
        .stage_commit_decision(
            &plan,
            state,
            decision,
            PendingDecisionKind::Reconciled,
            None,
        )
        .await
        .unwrap();
    let immutable_receipt = mirror
        .commit_batch(staged.mirror_commit)
        .await
        .expect("commit before acknowledgement is lost");

    // The fake normalized attempt does not exist. Exact terminal recovery proves no canonical
    // map hydration was attempted after the lost acknowledgement.
    let recovered = engine
        .run(&plan, &AtomicCancellation::default())
        .await
        .expect("persisted compact proof recovers the immutable receipt");
    assert_eq!(recovered.state.progress.phase, SnapshotPhase::Partial);
    assert_eq!(recovered.proof, proof);
    assert_eq!(recovered.receipt.proof_id, Some(immutable_receipt.proof_id));
    assert!(connector.requests.lock().unwrap().is_empty());
}

#[tokio::test]
async fn over_budget_max_window_state_terminalizes_before_hydration_and_is_restart_stable() {
    let (_, mirror, store, mut plan) = setup().await;
    plan.resume_key = "resume-over-budget-many-window".to_string();
    plan.run_id = "run-over-budget-many-window".to_string();
    let first_day = NaiveDate::from_ymd_opt(2026, 7, 1).unwrap();
    plan.windows = (0..MAX_SNAPSHOT_WINDOWS)
        .map(|offset| {
            let day = first_day + ChronoDuration::days(offset as i64);
            let day = day.format("%Y%m%d").to_string();
            PlannedWindow::deterministic(
                plan.pack,
                ReadWindow {
                    from_yyyymmdd: day.clone(),
                    to_yyyymmdd: day,
                },
            )
        })
        .collect();
    plan.capability_canary_window = Some(plan.windows[0].clone());
    plan.validate()
        .expect("maximum-window plan remains bounded");

    let batch_id = mirror
        .begin_batch(BeginBatchInput {
            run_id: plan.run_id.clone(),
            capability_snapshot_id: plan.capability_snapshot_id.clone(),
            company_id: plan.mirror_company_id.clone(),
            pack_id: pack_code(plan.pack).to_string(),
            pack_schema_major: plan.pack_schema_version.major,
            pack_schema_minor: plan.pack_schema_version.minor,
            source_transport: plan.source_transport.clone(),
            source_release: plan.source_release.clone(),
            requested_from_yyyymmdd: Some(plan.windows[0].range.from_yyyymmdd.clone()),
            requested_to_yyyymmdd: Some(
                plan.windows[MAX_SNAPSHOT_WINDOWS - 1]
                    .range
                    .to_yyyymmdd
                    .clone(),
            ),
            started_at_unix_ms: plan.started_at_unix_ms,
        })
        .await
        .unwrap();
    let first = &plan.windows[0];
    let mut base_evidence = canonicalize_window(
        &CanonicalWindowContext {
            requested_pack: plan.pack,
            schema_version: plan.pack_schema_version,
            source_identity: &plan.company.identity,
            query_profile: &first.query_profile,
            filters_sha256: &first.filters_sha256,
            external_references: &plan.external_references,
            window_id: &first.id,
            requested_window: &first.range,
        },
        &core_groups(0),
    )
    .unwrap()
    .evidence;
    let membership_sha256 = "a".repeat(64);
    base_evidence.deduped_count = 98;
    base_evidence.record_set_sha256 = Some(membership_sha256.clone());

    let mut state = DurableSnapshotState::new(&plan, Freshness::NeverVerified).unwrap();
    state.batch_id = Some(batch_id.clone());
    for (index, planned) in plan.windows.iter().enumerate() {
        let mut evidence = base_evidence.clone();
        evidence.window_id = planned.id.clone();
        evidence.from_yyyymmdd = planned.range.from_yyyymmdd.clone();
        evidence.to_yyyymmdd = planned.range.to_yyyymmdd.clone();
        let progress = state.windows.get_mut(&planned.id).unwrap();
        progress.phase = WindowPhase::Complete;
        progress.stage_receipt = Some(WindowStageReceipt {
            attempt: WindowStageAttempt {
                attempt_id: format!("budget-attempt-{index}"),
                batch_id: batch_id.clone(),
                window_id: planned.id.clone(),
                attempt_ordinal: 1,
                warning_codes: BTreeSet::new(),
            },
            member_count: 98,
            membership_sha256: membership_sha256.clone(),
            receipt_sha256: "b".repeat(64),
        });
        progress.evidence = Some(evidence);
    }
    state.set_phase(SnapshotPhase::Reconcile, None);
    assert!(reconciliation_record_budget_exceeded(&state).unwrap());
    store.save(&mut state).await.unwrap();

    // None of the fake normalized receipts exist. Any hydration attempt would therefore fail
    // as corrupt; durable terminal success proves the count-only preflight ran first.
    let connector = FakeConnector {
        batch: Mutex::new(VecDeque::new()),
        company: plan.company.clone(),
        requests: Mutex::new(Vec::new()),
    };
    let first_result = FullSnapshotEngine::new(&mirror, &store, &connector)
        .run(&plan, &AtomicCancellation::default())
        .await
        .expect("over-budget durable membership terminalizes without hydration");
    assert_eq!(first_result.state.progress.phase, SnapshotPhase::Failed);
    assert!(!first_result.receipt.checkpoint_advanced);
    assert!(first_result
        .proof
        .gaps
        .iter()
        .any(|gap| gap.safe_reason_code == RECONCILIATION_RECORD_BUDGET_CODE));
    assert!(connector.requests.lock().unwrap().is_empty());

    let resumed = FullSnapshotEngine::new(&mirror, &store, &connector)
        .run(&plan, &AtomicCancellation::default())
        .await
        .expect("terminal restart reuses the same proof without hydration or retry");
    assert_eq!(resumed.proof, first_result.proof);
    assert_eq!(resumed.receipt, first_result.receipt);
    assert!(connector.requests.lock().unwrap().is_empty());
}

#[tokio::test]
async fn v4_nonterminal_state_remains_inspectable_but_cannot_resume() {
    let (_, mirror, store, plan) = setup().await;
    let mut legacy = DurableSnapshotState::new(&plan, Freshness::NeverVerified).unwrap();
    legacy.state_version = LEGACY_SNAPSHOT_STATE_VERSION_V4;
    store.save(&mut legacy).await.unwrap();

    let loaded = store.load(&plan.resume_key).await.unwrap().unwrap();
    assert_eq!(loaded.state_version, LEGACY_SNAPSHOT_STATE_VERSION_V4);
    loaded
        .validate_invariants()
        .expect("v4 remains readable for operator inspection");
    assert_eq!(store.load_recent(10).await.unwrap().len(), 1);
    assert!(matches!(
        loaded.recoverable_plan(),
        Err(SnapshotError::ResumePlanUnavailable)
    ));

    let connector = FakeConnector {
        batch: Mutex::new(VecDeque::new()),
        company: plan.company.clone(),
        requests: Mutex::new(Vec::new()),
    };
    assert!(matches!(
        FullSnapshotEngine::new(&mirror, &store, &connector)
            .run(&plan, &AtomicCancellation::default())
            .await,
        Err(SnapshotError::ResumePlanUnavailable)
    ));
    assert!(connector.requests.lock().unwrap().is_empty());
}

#[tokio::test]
async fn duplicate_live_company_identity_fails_before_any_snapshot_read() {
    let (_, mirror, store, plan) = setup().await;
    let connector = AmbiguousCompanyConnector {
        inner: FakeConnector {
            batch: Mutex::new(VecDeque::new()),
            company: plan.company.clone(),
            requests: Mutex::new(Vec::new()),
        },
    };

    let result = FullSnapshotEngine::new(&mirror, &store, &connector)
        .run(&plan, &AtomicCancellation::default())
        .await
        .expect("an ambiguous live identity becomes a durable terminal result");

    assert_eq!(result.state.progress.phase, SnapshotPhase::Failed);
    assert!(result
        .proof
        .gaps
        .iter()
        .any(|gap| gap.safe_reason_code == "company_identity_ambiguous"));
    assert!(connector.inner.requests.lock().unwrap().is_empty());
}

#[tokio::test]
async fn sealed_empty_canary_authorizes_truthful_partial_and_is_idempotent() {
    let (pool, mirror, store, plan) = setup().await;
    let connector = FakeConnector {
        batch: Mutex::new(VecDeque::from([Ok(
            CanonicalPackWindow::without_source_count_evidence(PackBatch::CoreAccounting(
                CoreAccountingBatch::default(),
            )),
        )])),
        company: plan.company.clone(),
        requests: Mutex::new(Vec::new()),
    };
    let cancellation = AtomicCancellation::default();
    let engine = FullSnapshotEngine::new(&mirror, &store, &connector);
    let first = engine.run(&plan, &cancellation).await.unwrap();
    assert_eq!(first.state.progress.phase, SnapshotPhase::Partial);
    assert!(!first.receipt.checkpoint_advanced);
    assert!(first
        .proof
        .gaps
        .iter()
        .any(|gap| gap.safe_reason_code == "source_count_unavailable"));
    assert!(first.state.gap_codes.contains("source_count_unavailable"));
    assert_eq!(
        first.state.gap_codes,
        first
            .proof
            .gaps
            .iter()
            .map(|gap| gap.safe_reason_code.clone())
            .collect()
    );
    let proof_id = first
        .receipt
        .proof_id
        .as_deref()
        .expect("partial snapshot stores a durable proof");
    let export = mirror
        .redacted_proof_export(&plan.mirror_company_id, proof_id, 10_000)
        .await
        .expect("hash-valid durable partial proof is exportable");
    assert!(export.json.contains("\"verification_state\": \"partial\""));
    assert!(export.json.contains("\"authenticity_claim\": \"none\""));
    assert!(!export.json.contains(&plan.company.display_name));
    assert!(!export.json.contains(&plan.company.identity.company_guid));
    assert!(!export.json.contains(&plan.run_id));
    assert!(!export.json.contains(proof_id));
    assert!(mirror
        .redacted_proof_export("wrong-company", proof_id, 10_000)
        .await
        .is_err());
    let second = engine.run(&plan, &cancellation).await.unwrap();
    assert_eq!(first.proof.snapshot_sha256, second.proof.snapshot_sha256);
    let freshness = mirror
        .freshness(
            &plan.mirror_company_id,
            pack_code(plan.pack),
            Utc::now().timestamp_millis(),
        )
        .await
        .unwrap();
    assert_eq!(freshness.checkpoint_token, None);
    let terminal_mutation = sqlx::query(
        "UPDATE tally_snapshot_run_states SET generation = generation + 1 \
         WHERE resume_key = ?1",
    )
    .bind(&plan.resume_key)
    .execute(&pool)
    .await;
    assert!(terminal_mutation.is_err());
    let terminal_delete =
        sqlx::query("DELETE FROM tally_snapshot_run_states WHERE resume_key = ?1")
            .bind(&plan.resume_key)
            .execute(&pool)
            .await;
    assert!(terminal_delete.is_err());
}

#[tokio::test]
async fn parse_failure_and_cancellation_leave_previous_verified_checkpoint_active() {
    let (_pool, mirror, store, mut plan) = setup().await;
    let previous = seed_verified_checkpoint(&mirror, &plan).await;
    let cancellation = AtomicCancellation::default();

    plan.resume_key = "resume-failed".to_string();
    plan.run_id = "run-failed".to_string();
    let failing = FakeConnector {
        batch: Mutex::new(VecDeque::from([Err(TallyError::InvalidData {
            code: "synthetic_parse_failure".to_string(),
        })])),
        company: plan.company.clone(),
        requests: Mutex::new(Vec::new()),
    };
    let failed = FullSnapshotEngine::new(&mirror, &store, &failing)
        .run(&plan, &cancellation)
        .await
        .unwrap();
    assert_eq!(failed.state.progress.phase, SnapshotPhase::Failed);
    assert!(!failed.receipt.checkpoint_advanced);
    assert_eq!(
        mirror
            .freshness(
                &plan.mirror_company_id,
                pack_code(plan.pack),
                Utc::now().timestamp_millis(),
            )
            .await
            .unwrap()
            .checkpoint_token,
        Some(previous.clone())
    );

    plan.resume_key = "resume-cancelled".to_string();
    plan.run_id = "run-cancelled".to_string();
    let cancelled = AtomicCancellation::default();
    cancelled.cancel();
    let connector = FakeConnector {
        batch: Mutex::new(VecDeque::new()),
        company: plan.company.clone(),
        requests: Mutex::new(Vec::new()),
    };
    let result = FullSnapshotEngine::new(&mirror, &store, &connector)
        .run(&plan, &cancelled)
        .await
        .unwrap();
    assert_eq!(result.state.progress.phase, SnapshotPhase::Cancelled);
    assert!(!result.receipt.checkpoint_advanced);
    assert_eq!(
        mirror
            .freshness(
                &plan.mirror_company_id,
                pack_code(plan.pack),
                Utc::now().timestamp_millis(),
            )
            .await
            .unwrap()
            .checkpoint_token,
        Some(previous)
    );
}

#[tokio::test]
async fn future_dated_run_terminalizes_with_clamped_proof_and_ledger_creation_time() {
    let (pool, mirror, store, mut plan) = setup().await;
    plan.resume_key = "resume-future-run-clock".to_string();
    plan.run_id = "run-future-run-clock".to_string();
    plan.started_at_unix_ms = Utc::now()
        .timestamp_millis()
        .saturating_add(24 * 60 * 60 * 1_000);
    let cancellation = AtomicCancellation::default();
    cancellation.cancel();
    let connector = FakeConnector {
        batch: Mutex::new(VecDeque::new()),
        company: plan.company.clone(),
        requests: Mutex::new(Vec::new()),
    };

    let result = FullSnapshotEngine::new(&mirror, &store, &connector)
        .run(&plan, &cancellation)
        .await
        .expect("future-dated batch reaches a truthful terminal proof without waiting");
    assert_eq!(result.state.progress.phase, SnapshotPhase::Cancelled);
    assert_eq!(
        result.proof.completed_at_unix_ms,
        Some(plan.started_at_unix_ms)
    );
    assert!(result
        .proof
        .gaps
        .iter()
        .any(|gap| gap.safe_reason_code == "local_clock_moved_backwards"));
    let ledger_times = sqlx::query_as::<_, (i64, i64)>(
        "SELECT completed_at_unix_ms, created_at_unix_ms FROM tally_proof_ledger \
         WHERE run_id = ?1",
    )
    .bind(&plan.run_id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(ledger_times.0, plan.started_at_unix_ms);
    assert!(ledger_times.1 >= ledger_times.0);
}

#[tokio::test]
async fn runtime_cancellation_during_source_stability_is_terminal() {
    let (_, mirror, store, plan) = setup().await;
    let cancellation = AtomicCancellation::default();
    let connector = RuntimeCancelledStabilityConnector {
        inner: FakeConnector {
            batch: Mutex::new(VecDeque::new()),
            company: plan.company.clone(),
            requests: Mutex::new(Vec::new()),
        },
        request_count: Mutex::new(0),
    };

    let result = FullSnapshotEngine::new(&mirror, &store, &connector)
        .run(&plan, &cancellation)
        .await
        .expect("cancellation is committed as a truthful terminal result");
    assert_eq!(result.state.progress.phase, SnapshotPhase::Cancelled);
    assert!(!cancellation.is_cancelled());
    assert_eq!(connector.inner.requests.lock().unwrap().len(), 2);
    assert!(!result.receipt.checkpoint_advanced);
    assert!(result
        .proof
        .gaps
        .iter()
        .any(|gap| gap.safe_reason_code == "run_cancelled"));
    assert_eq!(
        mirror
            .freshness(
                &plan.mirror_company_id,
                pack_code(plan.pack),
                Utc::now().timestamp_millis(),
            )
            .await
            .unwrap()
            .checkpoint_token,
        None
    );
}

#[tokio::test]
async fn runtime_cancellation_during_period_report_survives_commit_pending_resume() {
    let (_, mirror, store, plan) = setup().await;
    let cancellation = AtomicCancellation::default();
    let connector = RuntimeCancelledReportConnector {
        inner: FakeConnector {
            batch: Mutex::new(VecDeque::new()),
            company: plan.company.clone(),
            requests: Mutex::new(Vec::new()),
        },
    };
    let crash_store = FailAfterFirstCommitPendingStore {
        inner: store.clone(),
        failed: AtomicBool::new(false),
    };
    let error = FullSnapshotEngine::new(&mirror, &crash_store, &connector)
        .run(&plan, &cancellation)
        .await
        .expect_err("inject crash after the cancelled decision is durable");
    assert!(matches!(
        error,
        SnapshotError::StateInvariant("injected_crash_after_commit_pending")
    ));
    assert!(!cancellation.is_cancelled());
    let pending = store.load(&plan.resume_key).await.unwrap().unwrap();
    assert_eq!(pending.progress.phase, SnapshotPhase::CommitPending);
    assert_eq!(
        pending
            .pending_commit
            .as_ref()
            .and_then(|pending| pending.safe_reason_code.as_deref()),
        Some("run_cancelled")
    );

    let result = FullSnapshotEngine::new(&mirror, &store, &connector)
        .run(&plan, &cancellation)
        .await
        .expect("cancelled proof resumes without another source request");
    assert_eq!(result.state.progress.phase, SnapshotPhase::Cancelled);
    assert!(!result
        .state
        .gap_codes
        .contains("report_tie_out_unavailable"));
    assert!(result.state.gap_codes.contains("run_cancelled"));
    assert!(!result.receipt.checkpoint_advanced);
    assert_eq!(
        result.proof.outcome,
        bridge_tally_core::RunOutcome::Cancelled
    );
    let receipt = mirror
        .historical_commit_receipt_for_batch(
            result.state.batch_id.as_deref().unwrap(),
            &plan.run_id,
        )
        .await
        .unwrap();
    assert_eq!(receipt.facts.outcome, RunOutcome::Cancelled);
    assert_eq!(receipt.facts.gap_codes, vec!["run_cancelled"]);
}

#[tokio::test]
async fn runtime_cancellation_during_end_probe_is_terminal() {
    let (_, mirror, store, plan) = setup().await;
    let cancellation = AtomicCancellation::default();
    let connector = RuntimeCancelledEndProbeConnector {
        inner: FakeConnector {
            batch: Mutex::new(VecDeque::new()),
            company: plan.company.clone(),
            requests: Mutex::new(Vec::new()),
        },
    };

    let result = FullSnapshotEngine::new(&mirror, &store, &connector)
        .run(&plan, &cancellation)
        .await
        .expect("runtime cancellation is committed as Cancelled");
    assert!(!cancellation.is_cancelled());
    assert_eq!(result.state.progress.phase, SnapshotPhase::Cancelled);
    assert_eq!(
        result.proof.outcome,
        bridge_tally_core::RunOutcome::Cancelled
    );
    assert!(!result.receipt.checkpoint_advanced);
    assert!(result.state.gap_codes.contains("run_cancelled"));
    assert!(!result
        .state
        .gap_codes
        .contains("end_profile_check_unavailable"));
}

#[tokio::test]
async fn reviewed_tally_error_codes_are_precise_allowlisted_and_persisted() {
    let cases = [
        (
            TallyError::Protocol {
                code: "response_truncated".to_string(),
            },
            "response_truncated",
        ),
        (
            TallyError::InvalidData {
                code: "company_identity_mismatch".to_string(),
            },
            "company_identity_mismatch",
        ),
        (
            TallyError::Protocol {
                code: "company_identity_not_found".to_string(),
            },
            "company_identity_not_found",
        ),
        (
            TallyError::Protocol {
                code: "company_identity_ambiguous".to_string(),
            },
            "company_identity_ambiguous",
        ),
        (
            TallyError::Unsupported {
                code: "endpoint_queue_deadline_exceeded".to_string(),
            },
            "endpoint_queue_deadline_exceeded",
        ),
        (
            TallyError::Protocol {
                code: "source_supplied_sensitive_text".to_string(),
            },
            "tally_protocol_failed",
        ),
        (
            TallyError::InvalidData {
                code: "source_supplied_sensitive_text".to_string(),
            },
            "response_parse_failed",
        ),
        (
            TallyError::Unsupported {
                code: "source_supplied_sensitive_text".to_string(),
            },
            "capability_not_supported",
        ),
    ];

    for (index, (error, expected)) in cases.into_iter().enumerate() {
        let (_, mirror, store, mut plan) = setup().await;
        plan.resume_key = format!("resume-safe-code-{index}");
        plan.run_id = format!("run-safe-code-{index}");
        let connector = FakeConnector {
            batch: Mutex::new(VecDeque::from([Err(error)])),
            company: plan.company.clone(),
            requests: Mutex::new(Vec::new()),
        };

        let result = FullSnapshotEngine::new(&mirror, &store, &connector)
            .run(&plan, &AtomicCancellation::default())
            .await
            .expect("safe connector failure becomes a durable terminal result");
        assert_eq!(result.state.progress.phase, SnapshotPhase::Failed);
        assert_eq!(
            result.state.gap_codes,
            BTreeSet::from([expected.to_string()])
        );
        assert_eq!(result.proof.gaps.len(), 1);
        assert_eq!(result.proof.gaps[0].safe_reason_code, expected);
        let receipt = mirror
            .historical_commit_receipt_for_batch(
                result.state.batch_id.as_deref().unwrap(),
                &plan.run_id,
            )
            .await
            .unwrap();
        assert_eq!(receipt.facts.gap_codes, vec![expected]);
        assert!(!receipt
            .facts
            .gap_codes
            .iter()
            .any(|code| code.contains("sensitive")));
    }
}

#[tokio::test]
async fn multi_leaf_source_stability_heartbeats_the_owned_lease() {
    let (_, mirror, store, mut plan) = setup().await;
    plan.resume_key = "resume-multi-leaf-heartbeat".to_string();
    plan.run_id = "run-multi-leaf-heartbeat".to_string();
    plan.windows = ["20260701", "20260702"]
        .into_iter()
        .map(|date| {
            PlannedWindow::deterministic(
                CapabilityPackId::CoreAccounting,
                ReadWindow {
                    from_yyyymmdd: date.to_string(),
                    to_yyyymmdd: date.to_string(),
                },
            )
        })
        .collect();
    plan.capability_canary_window = Some(plan.windows[0].clone());
    let heartbeat_store = HeartbeatCountingStore {
        inner: store,
        heartbeats: Mutex::new(0),
    };
    let connector = FakeConnector {
        batch: Mutex::new(VecDeque::new()),
        company: plan.company.clone(),
        requests: Mutex::new(Vec::new()),
    };

    FullSnapshotEngine::new(&mirror, &heartbeat_store, &connector)
        .run(&plan, &AtomicCancellation::default())
        .await
        .expect("multi-leaf run retains its lease through stability and commit");
    assert_eq!(connector.requests.lock().unwrap().len(), 4);
    assert!(*heartbeat_store.heartbeats.lock().unwrap() >= 8);
}

#[tokio::test]
async fn connector_await_renews_the_lease_while_a_call_is_in_flight() {
    let (_, mirror, store, plan) = setup().await;
    let mut state = DurableSnapshotState::new(&plan, Freshness::NeverVerified).unwrap();
    store.save(&mut state).await.unwrap();
    let heartbeat_store = HeartbeatCountingStore {
        inner: store,
        heartbeats: Mutex::new(0),
    };
    let connector = FakeConnector {
        batch: Mutex::new(VecDeque::new()),
        company: plan.company.clone(),
        requests: Mutex::new(Vec::new()),
    };
    let engine = FullSnapshotEngine::new(&mirror, &heartbeat_store, &connector);

    let result = engine
        .await_connector(&state, &AtomicCancellation::default(), async {
            tokio::time::sleep(Duration::from_millis(90)).await;
            Ok::<_, TallyError>(())
        })
        .await
        .unwrap();
    assert!(matches!(result, ConnectorAwait::Completed(Ok(()))));
    assert!(*heartbeat_store.heartbeats.lock().unwrap() >= 3);
}

#[tokio::test]
async fn expired_lease_cannot_be_revived_by_heartbeat_or_state_save() {
    let (pool, _, store, plan) = setup().await;
    let mut state = DurableSnapshotState::new(&plan, Freshness::NeverVerified).unwrap();
    store.save(&mut state).await.unwrap();
    sqlx::query(
        "UPDATE tally_snapshot_run_states SET lease_expires_at_unix_ms = ?1 \
         WHERE resume_key = ?2",
    )
    .bind(Utc::now().timestamp_millis().saturating_sub(1))
    .bind(&plan.resume_key)
    .execute(&pool)
    .await
    .unwrap();

    assert!(matches!(
        store.heartbeat(&state).await,
        Err(SnapshotError::LeaseUnavailable)
    ));
    state.warning_codes.insert(WarningCode::AdaptiveWindowSplit);
    assert!(matches!(
        store.save(&mut state).await,
        Err(SnapshotError::StateConflict)
    ));
    let contender =
        SqliteSnapshotStateStore::for_worker(pool, "synthetic-contending-worker".to_string())
            .unwrap();
    assert!(contender.claim(&plan.resume_key).await.unwrap());
}

#[tokio::test]
async fn file_backed_restart_reclaims_future_utc_lease_after_clock_rollback() {
    let (_directory, pool, crashed_worker, plan) = setup_file_backed_lease_store().await;
    let mut state = DurableSnapshotState::new(&plan, Freshness::NeverVerified).unwrap();
    assert!(matches!(
        crashed_worker.save(&mut state).await,
        Err(SnapshotError::LeaseUnavailable)
    ));
    assert_eq!(state.generation, 0);
    assert!(!crashed_worker.claim(&plan.resume_key).await.unwrap());
    crashed_worker.save(&mut state).await.unwrap();
    sqlx::query(
        "UPDATE tally_snapshot_run_states SET lease_expires_at_unix_ms = ?1 \
         WHERE resume_key = ?2",
    )
    .bind(i64::MAX)
    .bind(&plan.resume_key)
    .execute(&pool)
    .await
    .unwrap();

    // Dropping the worker models a process crash: the kernel releases its advisory lock,
    // even though the durable UTC expiry is now arbitrarily far in the future.
    drop(crashed_worker);
    let restarted_worker =
        SqliteSnapshotStateStore::for_worker(pool, "synthetic-restarted-worker".to_string())
            .unwrap();
    assert!(restarted_worker.claim(&plan.resume_key).await.unwrap());
    let mut recovered = restarted_worker
        .load(&plan.resume_key)
        .await
        .unwrap()
        .unwrap();
    restarted_worker.heartbeat(&recovered).await.unwrap();
    recovered
        .warning_codes
        .insert(WarningCode::AdaptiveWindowSplit);
    restarted_worker.save(&mut recovered).await.unwrap();
}

#[tokio::test]
async fn file_backed_live_owner_cannot_be_stolen_when_utc_lease_is_expired() {
    let (_directory, pool, live_worker, plan) = setup_file_backed_lease_store().await;
    assert!(!live_worker.claim(&plan.resume_key).await.unwrap());
    let mut state = DurableSnapshotState::new(&plan, Freshness::NeverVerified).unwrap();
    live_worker.save(&mut state).await.unwrap();
    sqlx::query(
        "UPDATE tally_snapshot_run_states SET lease_expires_at_unix_ms = ?1 \
         WHERE resume_key = ?2",
    )
    .bind(i64::MIN)
    .bind(&plan.resume_key)
    .execute(&pool)
    .await
    .unwrap();

    let contender =
        SqliteSnapshotStateStore::for_worker(pool, "synthetic-live-contender".to_string()).unwrap();
    assert!(matches!(
        contender.claim(&plan.resume_key).await,
        Err(SnapshotError::LeaseUnavailable)
    ));
    // Wall-clock jumps also cannot make the actual owner lose save authority.
    live_worker.heartbeat(&state).await.unwrap();
    state.warning_codes.insert(WarningCode::AdaptiveWindowSplit);
    live_worker.save(&mut state).await.unwrap();
}

#[tokio::test]
async fn durable_store_rejects_corruption_and_plan_drift() {
    let (pool, _mirror, store, mut plan) = setup().await;
    let mut state = DurableSnapshotState::new(&plan, Freshness::NeverVerified).unwrap();
    store.save(&mut state).await.unwrap();
    let original = plan.clone();
    plan.source_transport = "json_ex".to_string();
    let loaded = store.load(&plan.resume_key).await.unwrap().unwrap();
    store
        .heartbeat(&loaded)
        .await
        .expect("the owning worker can renew the exact loaded generation");
    assert_eq!(loaded.recoverable_plan().unwrap(), original);
    assert_eq!(
        store
            .load_by_run_id(&loaded.run_id)
            .await
            .unwrap()
            .unwrap()
            .resume_key,
        loaded.resume_key
    );
    assert_eq!(store.load_recent(10).await.unwrap().len(), 1);
    let mut legacy = loaded.clone();
    legacy.plan = None;
    assert!(matches!(
        legacy.recoverable_plan(),
        Err(SnapshotError::ResumePlanUnavailable)
    ));
    let contender = SqliteSnapshotStateStore::for_worker(
        pool.clone(),
        "synthetic-contending-worker".to_string(),
    )
    .unwrap();
    assert!(matches!(
        contender.claim(&loaded.resume_key).await,
        Err(SnapshotError::LeaseUnavailable)
    ));
    let mut advanced = loaded.clone();
    let mut stale = loaded.clone();
    advanced
        .warning_codes
        .insert(WarningCode::AdaptiveWindowSplit);
    store.save(&mut advanced).await.unwrap();
    stale
        .warning_codes
        .insert(WarningCode::ForeignMasterTextRenderingDegraded);
    assert!(matches!(
        store.save(&mut stale).await,
        Err(SnapshotError::StateConflict)
    ));
    let loaded = store.load(&plan.resume_key).await.unwrap().unwrap();
    assert!(loaded
        .warning_codes
        .contains(&WarningCode::AdaptiveWindowSplit));
    assert!(!loaded
        .warning_codes
        .contains(&WarningCode::ForeignMasterTextRenderingDegraded));
    assert!(matches!(
        loaded.assert_resumable_with(&plan),
        Err(SnapshotError::ResumePlanMismatch)
    ));
    let mut changed_references = original.clone();
    changed_references.external_references = ExternalReferenceCatalog::Complete {
        company_ids: BTreeSet::new(),
        voucher_ids: BTreeSet::new(),
        ledger_ids: BTreeSet::new(),
    };
    assert!(matches!(
        loaded.assert_resumable_with(&changed_references),
        Err(SnapshotError::ResumePlanMismatch)
    ));
    let mut changed_start = original.clone();
    changed_start.started_at_unix_ms += 1;
    assert!(matches!(
        loaded.assert_resumable_with(&changed_start),
        Err(SnapshotError::ResumePlanMismatch)
    ));
    let mut changed_freshness = original.clone();
    changed_freshness.freshness_target_seconds += 1;
    assert!(matches!(
        loaded.assert_resumable_with(&changed_freshness),
        Err(SnapshotError::ResumePlanMismatch)
    ));
    let mut malformed_filter = original;
    malformed_filter.windows[0].filters_sha256 = CanonicalText::parse("not-a-digest").unwrap();
    assert!(matches!(
        malformed_filter.validate(),
        Err(SnapshotError::InvalidPlan("windows"))
    ));
    let mut duplicate_plan = loaded.recoverable_plan().unwrap();
    duplicate_plan.resume_key = "duplicate-resume-key".to_string();
    let mut duplicate_state =
        DurableSnapshotState::new(&duplicate_plan, Freshness::NeverVerified).unwrap();
    assert!(store.save(&mut duplicate_state).await.is_err());
    assert_eq!(
        store
            .load_by_run_id(&loaded.run_id)
            .await
            .unwrap()
            .unwrap()
            .resume_key,
        loaded.resume_key
    );
    let identity_mutation = sqlx::query(
        "UPDATE tally_snapshot_run_states SET run_id = 'different-run' \
         WHERE resume_key = ?1",
    )
    .bind(&plan.resume_key)
    .execute(&pool)
    .await;
    assert!(identity_mutation.is_err());
    sqlx::query("UPDATE tally_snapshot_run_states SET row_sha256 = ?1 WHERE resume_key = ?2")
        .bind("0".repeat(64))
        .bind(&plan.resume_key)
        .execute(&pool)
        .await
        .unwrap();
    assert!(matches!(
        store.load(&plan.resume_key).await,
        Err(SnapshotError::CorruptState)
    ));
    let state_sha256: String = sqlx::query_scalar(
        "SELECT state_sha256 FROM tally_snapshot_run_states WHERE resume_key = ?1",
    )
    .bind(&plan.resume_key)
    .fetch_one(&pool)
    .await
    .unwrap();
    sqlx::query("UPDATE tally_snapshot_run_states SET row_sha256 = ?1 WHERE resume_key = ?2")
        .bind(snapshot_state_row_sha256(
            &loaded.resume_key,
            &loaded.run_id,
            loaded.generation,
            &state_sha256,
        ))
        .bind(&plan.resume_key)
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query(
        "UPDATE tally_snapshot_run_states SET state_json = '{\"corrupt\":true}' \
         WHERE resume_key = ?1",
    )
    .bind(&plan.resume_key)
    .execute(&pool)
    .await
    .unwrap();
    assert!(matches!(
        store.load(&plan.resume_key).await,
        Err(SnapshotError::CorruptState)
    ));
}
