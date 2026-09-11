//! Wire identities are separate from caller-selected transaction labels.
use super::{ImportLedgerLine, ImportVoucher};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use uuid::Uuid;

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub(super) enum ImportIdentityScheme {
    BatchV1,
}

/// The random, persisted batch UUID separates independent file generations.
/// Domain separation and tuple encoding keep identities deterministic without
/// treating a caller's commonly reused transaction label as a Tally upsert key.
pub(in crate::agent) fn import_identity(batch_id: &str, txn_id: &str) -> Uuid {
    let input = serde_json::to_vec(&("bridge.mcp.import.v1", batch_id, txn_id))
        .expect("string tuple serializes");
    let digest = Sha256::digest(input);
    let mut bytes = [0; 16];
    bytes.copy_from_slice(&digest[..16]);
    uuid::Builder::from_custom_bytes(bytes).into_uuid()
}

impl ImportLedgerLine {
    pub(super) fn attribution_tag(&self, voucher: &ImportVoucher) -> String {
        match self.identity_scheme {
            Some(ImportIdentityScheme::BatchV1) => {
                import_identity(&self.batch_id, &voucher.bridge_txn_id).to_string()
            }
            // Retain old file and journal interpretation; never rewrite a saved file.
            None => voucher.bridge_txn_id.clone(),
        }
    }
}
