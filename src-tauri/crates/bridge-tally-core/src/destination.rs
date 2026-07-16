use crate::{
    CapabilityPackId, DeliveryReceipt, DeliverySession, DestinationAdapter, PackBatch,
    PackSchemaVersion, ProofManifest, RunOutcome, TallyError, VerificationState,
};
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::sync::Arc;

const AXAL_TALLY_CONTRACT_VERSION: u16 = 1;
const MAX_IDEMPOTENCY_KEY_BYTES: usize = 200;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AxalTallyCapabilities {
    pub contract_version: u16,
    pub accepted_pack_versions: BTreeMap<CapabilityPackId, PackSchemaVersion>,
    pub max_batch_bytes: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BeginDeliveryRequest {
    pub contract_version: u16,
    pub proof: ProofManifest,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeliverBatchRequest {
    pub contract_version: u16,
    pub delivery_id: String,
    pub pack: CapabilityPackId,
    pub batch: PackBatch,
    pub content_sha256: String,
    pub idempotency_key: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FinalizeDeliveryRequest {
    pub contract_version: u16,
    pub delivery_id: String,
    pub proof: ProofManifest,
}

#[async_trait]
pub trait AxalTallyGateway: Send + Sync {
    async fn capabilities(&self) -> Result<AxalTallyCapabilities, TallyError>;
    async fn begin_delivery(
        &self,
        request: BeginDeliveryRequest,
    ) -> Result<DeliverySession, TallyError>;
    async fn deliver_batch(
        &self,
        request: DeliverBatchRequest,
    ) -> Result<DeliveryReceipt, TallyError>;
    async fn finalize_delivery(
        &self,
        request: FinalizeDeliveryRequest,
    ) -> Result<DeliveryReceipt, TallyError>;
}

#[derive(Debug, Default)]
pub struct UnconfiguredAxalTallyGateway;

#[async_trait]
impl AxalTallyGateway for UnconfiguredAxalTallyGateway {
    async fn capabilities(&self) -> Result<AxalTallyCapabilities, TallyError> {
        Err(TallyError::Unsupported {
            code: "axal_tally_contract_not_configured".to_string(),
        })
    }

    async fn begin_delivery(
        &self,
        _request: BeginDeliveryRequest,
    ) -> Result<DeliverySession, TallyError> {
        Err(TallyError::Unsupported {
            code: "axal_tally_contract_not_configured".to_string(),
        })
    }

    async fn deliver_batch(
        &self,
        _request: DeliverBatchRequest,
    ) -> Result<DeliveryReceipt, TallyError> {
        Err(TallyError::Unsupported {
            code: "axal_tally_contract_not_configured".to_string(),
        })
    }

    async fn finalize_delivery(
        &self,
        _request: FinalizeDeliveryRequest,
    ) -> Result<DeliveryReceipt, TallyError> {
        Err(TallyError::Unsupported {
            code: "axal_tally_contract_not_configured".to_string(),
        })
    }
}

#[derive(Clone)]
pub struct AxalDestinationAdapter<G> {
    gateway: Arc<G>,
}

impl<G> AxalDestinationAdapter<G>
where
    G: AxalTallyGateway,
{
    pub fn new(gateway: Arc<G>) -> Self {
        Self { gateway }
    }
}

#[async_trait]
impl<G> DestinationAdapter for AxalDestinationAdapter<G>
where
    G: AxalTallyGateway + 'static,
{
    async fn supported_packs(
        &self,
    ) -> Result<BTreeMap<CapabilityPackId, PackSchemaVersion>, TallyError> {
        let capabilities = self.gateway.capabilities().await?;
        validate_capabilities(&capabilities)?;
        Ok(capabilities.accepted_pack_versions)
    }

    async fn begin_delivery(&self, proof: &ProofManifest) -> Result<DeliverySession, TallyError> {
        validate_deliverable_proof(proof)?;
        let capabilities = self.gateway.capabilities().await?;
        validate_capabilities(&capabilities)?;
        let accepted_version = capabilities
            .accepted_pack_versions
            .get(&proof.pack)
            .ok_or_else(|| TallyError::Unsupported {
                code: "axal_pack_not_accepted".to_string(),
            })?;
        if accepted_version != &proof.pack_schema_version {
            return Err(TallyError::Unsupported {
                code: "axal_pack_schema_not_accepted".to_string(),
            });
        }

        let session = self
            .gateway
            .begin_delivery(BeginDeliveryRequest {
                contract_version: AXAL_TALLY_CONTRACT_VERSION,
                proof: proof.clone(),
            })
            .await?;
        validate_session(&session, &capabilities, proof)?;
        Ok(session)
    }

    async fn deliver_batch(
        &self,
        session: &DeliverySession,
        batch: &PackBatch,
        content_sha256: &str,
        idempotency_key: &str,
    ) -> Result<DeliveryReceipt, TallyError> {
        validate_session_shape(session)?;
        validate_idempotency_key(idempotency_key)?;
        validate_sha256(content_sha256, "invalid_content_sha256")?;
        let pack = pack_id(batch);
        if !session.accepted_pack_versions.contains_key(&pack) {
            return Err(TallyError::Unsupported {
                code: "delivery_session_does_not_accept_pack".to_string(),
            });
        }
        let canonical = serde_json::to_vec(batch).map_err(|_| TallyError::InvalidData {
            code: "batch_serialization_failed".to_string(),
        })?;
        if canonical.len() as u64 > session.max_batch_bytes {
            return Err(TallyError::InvalidData {
                code: "batch_exceeds_negotiated_limit".to_string(),
            });
        }
        let calculated_hash = sha256_hex(&canonical);
        if calculated_hash != content_sha256 {
            return Err(TallyError::InvalidData {
                code: "batch_content_hash_mismatch".to_string(),
            });
        }

        let receipt = self
            .gateway
            .deliver_batch(DeliverBatchRequest {
                contract_version: AXAL_TALLY_CONTRACT_VERSION,
                delivery_id: session.delivery_id.clone(),
                pack,
                batch: batch.clone(),
                content_sha256: content_sha256.to_string(),
                idempotency_key: idempotency_key.to_string(),
            })
            .await?;
        validate_receipt(&receipt, session, content_sha256, false)?;
        Ok(receipt)
    }

    async fn finalize_delivery(
        &self,
        session: &DeliverySession,
        proof: &ProofManifest,
    ) -> Result<DeliveryReceipt, TallyError> {
        validate_session_shape(session)?;
        validate_deliverable_proof(proof)?;
        let proof_hash =
            sha256_hex(
                &serde_json::to_vec(proof).map_err(|_| TallyError::InvalidData {
                    code: "proof_serialization_failed".to_string(),
                })?,
            );
        let receipt = self
            .gateway
            .finalize_delivery(FinalizeDeliveryRequest {
                contract_version: AXAL_TALLY_CONTRACT_VERSION,
                delivery_id: session.delivery_id.clone(),
                proof: proof.clone(),
            })
            .await?;
        validate_receipt(&receipt, session, &proof_hash, true)?;
        Ok(receipt)
    }
}

fn validate_capabilities(capabilities: &AxalTallyCapabilities) -> Result<(), TallyError> {
    if capabilities.contract_version != AXAL_TALLY_CONTRACT_VERSION {
        return Err(TallyError::Unsupported {
            code: "axal_tally_contract_version_unsupported".to_string(),
        });
    }
    if capabilities.max_batch_bytes == 0 || capabilities.accepted_pack_versions.is_empty() {
        return Err(TallyError::Protocol {
            code: "axal_tally_capabilities_invalid".to_string(),
        });
    }
    if capabilities
        .accepted_pack_versions
        .values()
        .any(|version| version.major == 0)
    {
        return Err(TallyError::Protocol {
            code: "axal_pack_schema_invalid".to_string(),
        });
    }
    Ok(())
}

fn validate_session(
    session: &DeliverySession,
    capabilities: &AxalTallyCapabilities,
    proof: &ProofManifest,
) -> Result<(), TallyError> {
    validate_session_shape(session)?;
    if session.max_batch_bytes > capabilities.max_batch_bytes
        || session.accepted_pack_versions != capabilities.accepted_pack_versions
        || session.accepted_pack_versions.get(&proof.pack) != Some(&proof.pack_schema_version)
    {
        return Err(TallyError::Protocol {
            code: "axal_delivery_session_changed_negotiated_scope".to_string(),
        });
    }
    Ok(())
}

fn validate_session_shape(session: &DeliverySession) -> Result<(), TallyError> {
    if session.delivery_id.is_empty()
        || session.delivery_id.len() > 200
        || session.delivery_id.chars().any(char::is_control)
        || session.max_batch_bytes == 0
        || session.accepted_pack_versions.is_empty()
    {
        return Err(TallyError::Protocol {
            code: "axal_delivery_session_invalid".to_string(),
        });
    }
    Ok(())
}

fn validate_deliverable_proof(proof: &ProofManifest) -> Result<(), TallyError> {
    if proof.proof_contract_version == 0
        || proof.run_id.is_empty()
        || proof.outcome != RunOutcome::Completed
        || proof.verification != VerificationState::Verified
        || proof.snapshot_sha256.is_none()
        || !proof.gaps.is_empty()
        || proof.completed_at_unix_ms.is_none()
    {
        return Err(TallyError::InvalidData {
            code: "proof_not_verified_for_delivery".to_string(),
        });
    }
    validate_sha256(
        proof.snapshot_sha256.as_deref().unwrap_or_default(),
        "proof_snapshot_hash_invalid",
    )
}

fn validate_idempotency_key(value: &str) -> Result<(), TallyError> {
    if value.is_empty()
        || value.len() > MAX_IDEMPOTENCY_KEY_BYTES
        || value.chars().any(|character| {
            character.is_control()
                || !(character.is_ascii_alphanumeric()
                    || matches!(character, '-' | '_' | ':' | '.'))
        })
    {
        return Err(TallyError::InvalidData {
            code: "idempotency_key_invalid".to_string(),
        });
    }
    Ok(())
}

fn validate_sha256(value: &str, code: &'static str) -> Result<(), TallyError> {
    if value.len() != 64
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return Err(TallyError::InvalidData {
            code: code.to_string(),
        });
    }
    Ok(())
}

fn validate_receipt(
    receipt: &DeliveryReceipt,
    session: &DeliverySession,
    expected_hash: &str,
    must_be_committed: bool,
) -> Result<(), TallyError> {
    if receipt.delivery_id != session.delivery_id
        || receipt.receipt_id.is_empty()
        || receipt.receipt_id.len() > 200
        || receipt.receipt_id.chars().any(char::is_control)
        || receipt.content_sha256 != expected_hash
        || (must_be_committed && !receipt.committed)
        || (!must_be_committed && receipt.committed)
    {
        return Err(TallyError::Protocol {
            code: "axal_delivery_receipt_invalid".to_string(),
        });
    }
    Ok(())
}

fn pack_id(batch: &PackBatch) -> CapabilityPackId {
    match batch {
        PackBatch::CoreAccounting(_) => CapabilityPackId::CoreAccounting,
        PackBatch::IndiaTax(_) => CapabilityPackId::IndiaTax,
        PackBatch::BillsAndPayments(_) => CapabilityPackId::BillsAndPayments,
        PackBatch::Inventory(_) => CapabilityPackId::Inventory,
    }
}

fn sha256_hex(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let digest = Sha256::digest(bytes);
    let mut encoded = String::with_capacity(digest.len() * 2);
    for byte in digest {
        encoded.push(HEX[(byte >> 4) as usize] as char);
        encoded.push(HEX[(byte & 0x0f) as usize] as char);
    }
    encoded
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{CoreAccountingBatch, Freshness, SourceIdentity, PROOF_CONTRACT_VERSION};
    use std::sync::Mutex;

    struct FakeGateway {
        capabilities: AxalTallyCapabilities,
        corrupt_receipt_hash: bool,
        delivered_keys: Mutex<Vec<String>>,
    }

    #[async_trait]
    impl AxalTallyGateway for FakeGateway {
        async fn capabilities(&self) -> Result<AxalTallyCapabilities, TallyError> {
            Ok(self.capabilities.clone())
        }

        async fn begin_delivery(
            &self,
            _request: BeginDeliveryRequest,
        ) -> Result<DeliverySession, TallyError> {
            Ok(DeliverySession {
                delivery_id: "delivery-1".to_string(),
                accepted_pack_versions: self.capabilities.accepted_pack_versions.clone(),
                max_batch_bytes: self.capabilities.max_batch_bytes,
            })
        }

        async fn deliver_batch(
            &self,
            request: DeliverBatchRequest,
        ) -> Result<DeliveryReceipt, TallyError> {
            self.delivered_keys
                .lock()
                .expect("delivery keys lock")
                .push(request.idempotency_key);
            Ok(DeliveryReceipt {
                delivery_id: request.delivery_id,
                receipt_id: "receipt-1".to_string(),
                content_sha256: if self.corrupt_receipt_hash {
                    "0".repeat(64)
                } else {
                    request.content_sha256
                },
                committed: false,
            })
        }

        async fn finalize_delivery(
            &self,
            request: FinalizeDeliveryRequest,
        ) -> Result<DeliveryReceipt, TallyError> {
            Ok(DeliveryReceipt {
                delivery_id: request.delivery_id,
                receipt_id: "receipt-final".to_string(),
                content_sha256: sha256_hex(
                    &serde_json::to_vec(&request.proof).expect("serialize proof"),
                ),
                committed: true,
            })
        }
    }

    fn capabilities() -> AxalTallyCapabilities {
        AxalTallyCapabilities {
            contract_version: AXAL_TALLY_CONTRACT_VERSION,
            accepted_pack_versions: BTreeMap::from([(
                CapabilityPackId::CoreAccounting,
                PackSchemaVersion { major: 1, minor: 0 },
            )]),
            max_batch_bytes: 1024 * 1024,
        }
    }

    fn proof() -> ProofManifest {
        ProofManifest {
            proof_contract_version: PROOF_CONTRACT_VERSION,
            run_id: "run-1".to_string(),
            source_identity: SourceIdentity {
                bridge_source_lineage: "lineage-1".to_string(),
                company_guid: "company-1".to_string(),
                observed_fingerprint: "fingerprint-1".to_string(),
            },
            pack: CapabilityPackId::CoreAccounting,
            pack_schema_version: PackSchemaVersion { major: 1, minor: 0 },
            outcome: RunOutcome::Completed,
            verification: VerificationState::Verified,
            freshness: Freshness::Fresh,
            started_at_unix_ms: 1,
            completed_at_unix_ms: Some(2),
            record_counts: BTreeMap::new(),
            snapshot_sha256: Some("a".repeat(64)),
            gaps: Vec::new(),
        }
    }

    #[tokio::test]
    async fn delivers_only_verified_hash_matching_batches_and_receipts() {
        let gateway = Arc::new(FakeGateway {
            capabilities: capabilities(),
            corrupt_receipt_hash: false,
            delivered_keys: Mutex::new(Vec::new()),
        });
        let adapter = AxalDestinationAdapter::new(gateway.clone());
        let session = adapter
            .begin_delivery(&proof())
            .await
            .expect("begin delivery");
        let batch = PackBatch::CoreAccounting(CoreAccountingBatch::default());
        let content_hash = sha256_hex(&serde_json::to_vec(&batch).expect("serialize batch"));
        adapter
            .deliver_batch(&session, &batch, &content_hash, "run-1:core:0")
            .await
            .expect("deliver batch");
        adapter
            .finalize_delivery(&session, &proof())
            .await
            .expect("finalize delivery");
        assert_eq!(
            gateway
                .delivered_keys
                .lock()
                .expect("delivery keys lock")
                .as_slice(),
            ["run-1:core:0"]
        );
    }

    #[tokio::test]
    async fn rejects_partial_proofs_changed_hashes_and_corrupt_receipts() {
        let gateway = Arc::new(FakeGateway {
            capabilities: capabilities(),
            corrupt_receipt_hash: true,
            delivered_keys: Mutex::new(Vec::new()),
        });
        let adapter = AxalDestinationAdapter::new(gateway);
        let mut partial = proof();
        partial.verification = VerificationState::Partial;
        assert!(adapter.begin_delivery(&partial).await.is_err());

        let session = adapter
            .begin_delivery(&proof())
            .await
            .expect("begin delivery");
        let batch = PackBatch::CoreAccounting(CoreAccountingBatch::default());
        assert!(adapter
            .deliver_batch(&session, &batch, &"b".repeat(64), "run-1:core:0")
            .await
            .is_err());
        let content_hash = sha256_hex(&serde_json::to_vec(&batch).expect("serialize batch"));
        assert!(adapter
            .deliver_batch(&session, &batch, &content_hash, "run-1:core:0")
            .await
            .is_err());
    }

    #[tokio::test]
    async fn unconfigured_gateway_fails_explicitly() {
        let adapter = AxalDestinationAdapter::new(Arc::new(UnconfiguredAxalTallyGateway));
        let error = adapter
            .supported_packs()
            .await
            .expect_err("unconfigured gateway must not imply support");
        assert!(matches!(error, TallyError::Unsupported { .. }));
    }
}
