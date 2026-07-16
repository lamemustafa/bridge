use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use bridge_tally_core::{CapabilityPackId, VerificationState};
use serde::Serialize;

use crate::db::tally_mirror::TallyMirrorRepository;
use crate::sync::snapshot::{
    AtomicCancellation, DurableSnapshotState, FullSnapshotEngine, SnapshotError, SnapshotPhase,
    SnapshotPlan, SnapshotStateStore, SqliteSnapshotStateStore,
};
use crate::tally::RuntimeTallyConnector;

const MAX_TRACKED_RUNS: usize = 100;

#[derive(Debug, Clone, Serialize)]
pub struct SnapshotJobStatus {
    pub run_id: String,
    pub mirror_company_id: Option<String>,
    pub pack_id: Option<CapabilityPackId>,
    pub requested_from_yyyymmdd: Option<String>,
    pub requested_to_yyyymmdd: Option<String>,
    pub phase: SnapshotPhase,
    pub active_window_id: Option<String>,
    pub completed_windows: u32,
    pub total_windows: u32,
    pub verification: Option<VerificationState>,
    pub proof_id: Option<String>,
    pub proof_sha256: Option<String>,
    pub gap_codes: Vec<String>,
    pub warning_codes: Vec<String>,
    pub failure_code: Option<String>,
    /// True only when durable non-terminal state exists but no worker is attached in this process.
    pub requires_resume: bool,
    /// False for legacy/corrupt detached states that are inspectable but not safely resumable.
    pub resume_available: bool,
}

struct SnapshotJob {
    plan: SnapshotPlan,
    mirror: TallyMirrorRepository,
    connector: RuntimeTallyConnector,
    cancellation: Arc<AtomicCancellation>,
    terminal: Arc<Mutex<Option<SnapshotJobStatus>>>,
}

#[derive(Clone, Default)]
pub struct SnapshotCoordinator {
    jobs: Arc<Mutex<HashMap<String, SnapshotJob>>>,
}

impl SnapshotCoordinator {
    pub async fn start(
        &self,
        plan: SnapshotPlan,
        connector: RuntimeTallyConnector,
        mirror: TallyMirrorRepository,
    ) -> Result<SnapshotJobStatus, &'static str> {
        let cancellation = Arc::new(AtomicCancellation::default());
        let terminal = Arc::new(Mutex::new(None));
        let lease_owner = uuid::Uuid::new_v4().to_string();
        let store = SqliteSnapshotStateStore::for_worker(mirror.pool_clone(), lease_owner)
            .map_err(|_| "snapshot_lease_invalid")?;
        store
            .migrate()
            .await
            .map_err(|_| "snapshot_state_migration_missing")?;
        store
            .claim(&plan.resume_key)
            .await
            .map_err(|error| match error {
                SnapshotError::LeaseUnavailable => "snapshot_run_owned_elsewhere",
                _ => "snapshot_state_unavailable",
            })?;
        let registration = (|| {
            let mut jobs = self
                .jobs
                .lock()
                .map_err(|_| "snapshot_registry_unavailable")?;
            if jobs.contains_key(&plan.run_id) {
                let replaceable = jobs.get(&plan.run_id).is_some_and(|job| {
                    job.terminal.lock().is_ok_and(|status| {
                        status.as_ref().is_some_and(|status| status.requires_resume)
                    })
                });
                if replaceable {
                    jobs.remove(&plan.run_id);
                } else {
                    return Err("snapshot_run_already_exists");
                }
            }
            if jobs.len() >= MAX_TRACKED_RUNS {
                if let Some(completed) = jobs
                    .iter()
                    .find(|(_, job)| job.terminal.lock().is_ok_and(|state| state.is_some()))
                    .map(|(run_id, _)| run_id.clone())
                {
                    jobs.remove(&completed);
                } else {
                    return Err("snapshot_run_limit_reached");
                }
            }
            jobs.insert(
                plan.run_id.clone(),
                SnapshotJob {
                    plan: plan.clone(),
                    mirror: mirror.clone(),
                    connector: connector.clone(),
                    cancellation: Arc::clone(&cancellation),
                    terminal: Arc::clone(&terminal),
                },
            );
            Ok::<(), &'static str>(())
        })();
        if let Err(error) = registration {
            let _ = store.release(&plan.resume_key).await;
            return Err(error);
        }

        let initial = status_from_plan(&plan);
        tauri::async_runtime::spawn(async move {
            let engine = FullSnapshotEngine::new(&mirror, &store, &connector);
            let final_status = match engine.run(&plan, cancellation.as_ref()).await {
                Ok(result) => SnapshotJobStatus {
                    run_id: plan.run_id.clone(),
                    mirror_company_id: Some(plan.mirror_company_id.clone()),
                    pack_id: Some(plan.pack),
                    requested_from_yyyymmdd: requested_bounds(&plan).0,
                    requested_to_yyyymmdd: requested_bounds(&plan).1,
                    phase: result.state.progress.phase,
                    active_window_id: result.state.progress.active_window_id,
                    completed_windows: result.state.progress.completed_windows,
                    total_windows: result.state.progress.total_windows,
                    verification: Some(result.proof.verification),
                    proof_id: result.receipt.proof_id,
                    proof_sha256: result.receipt.proof_sha256,
                    gap_codes: result.state.gap_codes.into_iter().collect(),
                    warning_codes: result.state.warning_codes.into_iter().collect(),
                    failure_code: None,
                    requires_resume: false,
                    resume_available: false,
                },
                Err(error) => {
                    let _ = store.release(&plan.resume_key).await;
                    match store.load(&plan.resume_key).await {
                        Ok(Some(state)) => {
                            let requires_resume = !state.progress.phase.is_terminal();
                            let mut status = status_from_state(state, requires_resume);
                            status.failure_code = Some(snapshot_error_code(&error).to_string());
                            status
                        }
                        _ => SnapshotJobStatus {
                            phase: SnapshotPhase::Failed,
                            failure_code: Some(snapshot_error_code(&error).to_string()),
                            requires_resume: false,
                            resume_available: false,
                            ..status_from_plan(&plan)
                        },
                    }
                }
            };
            if let Ok(mut state) = terminal.lock() {
                *state = Some(final_status);
            }
        });
        Ok(initial)
    }

    pub async fn status(
        &self,
        run_id: &str,
        fallback_mirror: &TallyMirrorRepository,
    ) -> Result<SnapshotJobStatus, &'static str> {
        let tracked = {
            let jobs = self
                .jobs
                .lock()
                .map_err(|_| "snapshot_registry_unavailable")?;
            jobs.get(run_id).map(|job| {
                (
                    job.plan.clone(),
                    job.mirror.clone(),
                    Arc::clone(&job.terminal),
                )
            })
        };
        let Some((plan, mirror, terminal)) = tracked else {
            let store = SqliteSnapshotStateStore::new(fallback_mirror.pool_clone());
            let state = store
                .load_by_run_id(run_id)
                .await
                .map_err(|_| "snapshot_state_unavailable")?
                .ok_or("snapshot_run_not_found")?;
            let requires_resume = !state.progress.phase.is_terminal();
            return Ok(status_from_state(state, requires_resume));
        };
        if let Some(status) = terminal
            .lock()
            .map_err(|_| "snapshot_status_unavailable")?
            .clone()
        {
            return Ok(status);
        }
        let store = SqliteSnapshotStateStore::new(mirror.pool_clone());
        let Some(state) = store
            .load(&plan.resume_key)
            .await
            .map_err(|_| "snapshot_state_unavailable")?
        else {
            return Ok(status_from_plan(&plan));
        };
        Ok(status_from_state(state, false))
    }

    pub async fn recent(
        &self,
        mirror: &TallyMirrorRepository,
        limit: u32,
    ) -> Result<Vec<SnapshotJobStatus>, &'static str> {
        let tracked = self
            .jobs
            .lock()
            .map_err(|_| "snapshot_registry_unavailable")?
            .iter()
            .map(|(run_id, job)| {
                let terminal = job.terminal.lock().ok().and_then(|status| status.clone());
                (run_id.clone(), terminal)
            })
            .collect::<HashMap<_, _>>();
        let store = SqliteSnapshotStateStore::new(mirror.pool_clone());
        let states = store
            .load_recent(limit)
            .await
            .map_err(|_| "snapshot_state_unavailable")?;
        Ok(states
            .into_iter()
            .map(|state| {
                if let Some(Some(status)) = tracked.get(&state.run_id) {
                    return status.clone();
                }
                let requires_resume =
                    !state.progress.phase.is_terminal() && !tracked.contains_key(&state.run_id);
                status_from_state(state, requires_resume)
            })
            .collect())
    }

    pub fn cancel(&self, run_id: &str) -> Result<bool, &'static str> {
        let jobs = self
            .jobs
            .lock()
            .map_err(|_| "snapshot_registry_unavailable")?;
        let job = jobs.get(run_id).ok_or("snapshot_run_not_found")?;
        if job
            .terminal
            .lock()
            .map_err(|_| "snapshot_status_unavailable")?
            .is_some()
        {
            return Ok(false);
        }
        job.cancellation.cancel();
        job.connector.cancel();
        Ok(true)
    }
}

fn snapshot_error_code(error: &SnapshotError) -> &'static str {
    match error {
        SnapshotError::InvalidPlan(_) => "snapshot_plan_invalid",
        SnapshotError::ResumePlanMismatch => "snapshot_resume_plan_mismatch",
        SnapshotError::ResumePlanUnavailable => "snapshot_resume_plan_unavailable",
        SnapshotError::CorruptState => "snapshot_state_corrupt",
        SnapshotError::StateMigrationMissing => "snapshot_state_migration_missing",
        SnapshotError::LeaseUnavailable => "snapshot_run_owned_elsewhere",
        SnapshotError::LeaseIo(_) => "snapshot_process_lock_failed",
        SnapshotError::StateConflict => "snapshot_state_generation_changed",
        SnapshotError::StateStore(_) => "snapshot_state_store_failed",
        SnapshotError::Mirror(_) => "snapshot_mirror_failed",
        SnapshotError::Reconciliation(_) => "snapshot_reconciliation_failed",
        SnapshotError::ConcurrentCheckpoint => "snapshot_checkpoint_changed",
        SnapshotError::StateInvariant(_) => "snapshot_state_invariant_failed",
        SnapshotError::Serialization => "snapshot_serialization_failed",
    }
}

fn status_from_plan(plan: &SnapshotPlan) -> SnapshotJobStatus {
    let (requested_from_yyyymmdd, requested_to_yyyymmdd) = requested_bounds(plan);
    SnapshotJobStatus {
        run_id: plan.run_id.clone(),
        mirror_company_id: Some(plan.mirror_company_id.clone()),
        pack_id: Some(plan.pack),
        requested_from_yyyymmdd,
        requested_to_yyyymmdd,
        phase: SnapshotPhase::Prepare,
        active_window_id: None,
        completed_windows: 0,
        total_windows: plan.windows.len() as u32,
        verification: None,
        proof_id: None,
        proof_sha256: None,
        gap_codes: Vec::new(),
        warning_codes: Vec::new(),
        failure_code: None,
        requires_resume: false,
        resume_available: false,
    }
}

fn status_from_state(state: DurableSnapshotState, requires_resume: bool) -> SnapshotJobStatus {
    let resume_available = requires_resume && state.recoverable_plan().is_ok();
    let mirror_company_id = state
        .plan
        .as_ref()
        .map(|plan| plan.mirror_company_id.clone());
    let pack_id = state.plan.as_ref().map(|plan| plan.pack);
    let (requested_from_yyyymmdd, requested_to_yyyymmdd) =
        state.plan.as_ref().map_or((None, None), requested_bounds);
    SnapshotJobStatus {
        run_id: state.run_id,
        mirror_company_id,
        pack_id,
        requested_from_yyyymmdd,
        requested_to_yyyymmdd,
        phase: state.progress.phase,
        active_window_id: state.progress.active_window_id,
        completed_windows: state.progress.completed_windows,
        total_windows: state.progress.total_windows,
        verification: state.proof.as_ref().map(|proof| proof.verification),
        proof_id: state
            .commit_receipt
            .as_ref()
            .and_then(|receipt| receipt.proof_id.clone()),
        proof_sha256: state
            .commit_receipt
            .as_ref()
            .and_then(|receipt| receipt.proof_sha256.clone()),
        gap_codes: state.gap_codes.into_iter().collect(),
        warning_codes: state.warning_codes.into_iter().collect(),
        failure_code: None,
        requires_resume,
        resume_available,
    }
}

fn requested_bounds(plan: &SnapshotPlan) -> (Option<String>, Option<String>) {
    (
        plan.windows
            .iter()
            .map(|window| window.range.from_yyyymmdd.as_str())
            .min()
            .map(str::to_string),
        plan.windows
            .iter()
            .map(|window| window.range.to_yyyymmdd.as_str())
            .max()
            .map(str::to_string),
    )
}

#[cfg(test)]
pub(crate) fn status_from_state_for_test(
    state: DurableSnapshotState,
    requires_resume: bool,
) -> SnapshotJobStatus {
    status_from_state(state, requires_resume)
}
