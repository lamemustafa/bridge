use super::*;
use crate::agent::acquire_endpoint_dispatch_lease;
use crate::tally::{TallyConfig, TallyRuntime};
use bridge_tally_core::RequestContext;
use std::time::Duration;
use tokio::net::TcpListener;
use tokio::time::timeout;

fn connector(plan: &SnapshotPlan, config: TallyConfig) -> RuntimeTallyConnector {
    let window = plan.capability_canary_window.as_ref().unwrap();
    RuntimeTallyConnector::new(
        TallyRuntime::default(),
        config,
        plan.company.clone(),
        RequestContext {
            run_id: plan.run_id.clone(),
            company: plan.company.clone(),
            pack: plan.pack,
            schema_version: plan.pack_schema_version,
            window: window.range.clone(),
            query_profile: window.query_profile.clone(),
            filters_sha256: window.filters_sha256.clone(),
        },
    )
    .unwrap()
}

#[tokio::test]
async fn snapshot_worker_holds_posting_lease_until_cancellation_settles() {
    let (_, mirror, store, mut plan) = crate::sync::snapshot::tests::setup().await;
    plan.pack_schema_version = bridge_tally_core::CORE_ACCOUNTING_SCHEMA_VERSION;
    // A reserved local listener observes worker activity without serving any
    // fabricated Tally response or contacting a live instance.
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let config = TallyConfig {
        host: "127.0.0.1".into(),
        port: listener.local_addr().unwrap().port(),
    };
    let coordinator = SnapshotCoordinator::default();
    let posting_lease = acquire_endpoint_dispatch_lease(&config).unwrap();
    assert_eq!(
        coordinator
            .start(plan.clone(), connector(&plan, config.clone()), mirror.clone())
            .await
            .unwrap_err(),
        "Another Bridge snapshot or Journal posting is using this Tally endpoint. Wait for it to finish."
    );
    assert!(coordinator.jobs.lock().unwrap().is_empty());
    assert!(store.load(&plan.resume_key).await.unwrap().is_none());
    assert!(timeout(Duration::from_millis(50), listener.accept())
        .await
        .is_err());
    drop(posting_lease);

    coordinator
        .start(
            plan.clone(),
            connector(&plan, config.clone()),
            mirror.clone(),
        )
        .await
        .unwrap();
    let (_connection, _) = timeout(Duration::from_secs(10), listener.accept())
        .await
        .expect("worker did not begin its probe")
        .unwrap();
    assert_eq!(
        acquire_endpoint_dispatch_lease(&config).err().as_deref(),
        Some("import_admission_busy")
    );
    assert!(coordinator.cancel(&plan.run_id).unwrap());
    timeout(Duration::from_secs(10), async {
        loop {
            let status = coordinator.status(&plan.run_id, &mirror).await.unwrap();
            if status.phase.is_terminal() {
                assert_eq!(status.phase, SnapshotPhase::Cancelled);
                if let Ok(lease) = acquire_endpoint_dispatch_lease(&config) {
                    drop(lease);
                    break;
                }
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("cancelled worker did not settle and release posting lease");
}

#[tokio::test]
async fn snapshot_admission_failure_releases_posting_lease() {
    let (pool, mirror, _, mut plan) = crate::sync::snapshot::tests::setup().await;
    plan.pack_schema_version = bridge_tally_core::CORE_ACCOUNTING_SCHEMA_VERSION;
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let config = TallyConfig {
        host: "127.0.0.1".into(),
        port: listener.local_addr().unwrap().port(),
    };
    pool.close().await;
    let coordinator = SnapshotCoordinator::default();
    assert_eq!(
        coordinator
            .start(plan.clone(), connector(&plan, config.clone()), mirror)
            .await
            .unwrap_err(),
        "snapshot_state_migration_missing"
    );
    assert!(coordinator.jobs.lock().unwrap().is_empty());
    let _lease = acquire_endpoint_dispatch_lease(&config).unwrap();
}
