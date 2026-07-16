CREATE TABLE IF NOT EXISTS tally_incremental_capability_observations (
  id TEXT PRIMARY KEY,
  scope_sha256 TEXT NOT NULL CHECK (
    length(scope_sha256) = 64 AND scope_sha256 NOT GLOB '*[^0-9a-f]*'
  ),
  scope_json TEXT NOT NULL CHECK (json_valid(scope_json)),
  capability_snapshot_id TEXT NOT NULL,
  company_id TEXT NOT NULL,
  verifier_contract_version INTEGER NOT NULL CHECK (verifier_contract_version > 0),
  response_sha256 TEXT NOT NULL CHECK (
    length(response_sha256) = 64 AND response_sha256 NOT GLOB '*[^0-9a-f]*'
  ),
  capability_state TEXT NOT NULL CHECK (
    capability_state IN ('supported', 'unsupported', 'unknown', 'not_configured')
  ),
  confidence TEXT NOT NULL CHECK (
    confidence IN ('documented', 'observed', 'inferred', 'unknown')
  ),
  identifier_semantics TEXT NOT NULL CHECK (
    identifier_semantics IN ('monotonic_per_object', 'unknown')
  ),
  inclusive_lower_bound_observed INTEGER NOT NULL CHECK (
    inclusive_lower_bound_observed IN (0, 1)
  ),
  explicit_source_high_watermark_observed INTEGER NOT NULL CHECK (
    explicit_source_high_watermark_observed IN (0, 1)
  ),
  observed_at_unix_ms INTEGER NOT NULL CHECK (observed_at_unix_ms > 0),
  UNIQUE(scope_sha256, capability_snapshot_id, observed_at_unix_ms),
  FOREIGN KEY (capability_snapshot_id)
    REFERENCES tally_capability_snapshots(id) ON DELETE RESTRICT,
  FOREIGN KEY (company_id) REFERENCES tally_companies(id) ON DELETE RESTRICT
);

CREATE INDEX IF NOT EXISTS idx_tally_incremental_capability_scope_observed
  ON tally_incremental_capability_observations(scope_sha256, observed_at_unix_ms DESC);

-- This receipt is the immutable generation-1 establishment event. It is intentionally
-- separate from the generic proof ledger: a pack proof alone cannot attest exact-query
-- AlterID coverage or an explicit source high watermark.
CREATE TABLE IF NOT EXISTS tally_incremental_establishment_receipts (
  id TEXT PRIMARY KEY,
  scope_sha256 TEXT NOT NULL CHECK (
    length(scope_sha256) = 64 AND scope_sha256 NOT GLOB '*[^0-9a-f]*'
  ),
  scope_json TEXT NOT NULL CHECK (json_valid(scope_json)),
  capability_observation_id TEXT NOT NULL,
  proof_id TEXT NOT NULL,
  proof_sha256 TEXT NOT NULL CHECK (
    length(proof_sha256) = 64 AND proof_sha256 NOT GLOB '*[^0-9a-f]*'
  ),
  batch_id TEXT NOT NULL,
  snapshot_plan_sha256 TEXT NOT NULL CHECK (
    length(snapshot_plan_sha256) = 64 AND snapshot_plan_sha256 NOT GLOB '*[^0-9a-f]*'
  ),
  source_response_sha256 TEXT NOT NULL CHECK (
    length(source_response_sha256) = 64 AND source_response_sha256 NOT GLOB '*[^0-9a-f]*'
  ),
  coverage_manifest_sha256 TEXT NOT NULL CHECK (
    length(coverage_manifest_sha256) = 64 AND coverage_manifest_sha256 NOT GLOB '*[^0-9a-f]*'
  ),
  source_high_watermark_decimal TEXT NOT NULL CHECK (
    source_high_watermark_decimal = '0' OR (
      source_high_watermark_decimal NOT GLOB '*[^0-9]*' AND
      substr(source_high_watermark_decimal, 1, 1) BETWEEN '1' AND '9' AND
      (length(source_high_watermark_decimal) < 20 OR (
        length(source_high_watermark_decimal) = 20 AND
        source_high_watermark_decimal <= '18446744073709551615'
      ))
    )
  ),
  max_observed_alter_id_decimal TEXT CHECK (
    max_observed_alter_id_decimal IS NULL OR
    max_observed_alter_id_decimal = '0' OR (
      max_observed_alter_id_decimal NOT GLOB '*[^0-9]*' AND
      substr(max_observed_alter_id_decimal, 1, 1) BETWEEN '1' AND '9' AND
      (length(max_observed_alter_id_decimal) < 20 OR (
        length(max_observed_alter_id_decimal) = 20 AND
        max_observed_alter_id_decimal <= '18446744073709551615'
      ))
    )
  ),
  source_record_count INTEGER NOT NULL CHECK (source_record_count >= 0),
  accepted_record_count INTEGER NOT NULL CHECK (accepted_record_count >= 0),
  deduplicated_record_count INTEGER NOT NULL CHECK (deduplicated_record_count >= 0),
  numeric_alter_id_count INTEGER NOT NULL CHECK (numeric_alter_id_count >= 0),
  rejected_record_count INTEGER NOT NULL CHECK (rejected_record_count = 0),
  duplicate_identity_count INTEGER NOT NULL CHECK (duplicate_identity_count = 0),
  missing_identity_count INTEGER NOT NULL CHECK (missing_identity_count = 0),
  out_of_scope_record_count INTEGER NOT NULL CHECK (out_of_scope_record_count = 0),
  verifier_contract_version INTEGER NOT NULL CHECK (verifier_contract_version > 0),
  receipt_sha256 TEXT NOT NULL UNIQUE CHECK (
    length(receipt_sha256) = 64 AND receipt_sha256 NOT GLOB '*[^0-9a-f]*'
  ),
  created_at_unix_ms INTEGER NOT NULL CHECK (created_at_unix_ms > 0),
  CHECK (
    source_record_count = accepted_record_count AND
    source_record_count = deduplicated_record_count AND
    source_record_count = numeric_alter_id_count AND
    ((source_record_count = 0 AND max_observed_alter_id_decimal IS NULL) OR
     (source_record_count > 0 AND max_observed_alter_id_decimal IS NOT NULL)) AND
    (max_observed_alter_id_decimal IS NULL OR
     length(max_observed_alter_id_decimal) < length(source_high_watermark_decimal) OR
     (length(max_observed_alter_id_decimal) = length(source_high_watermark_decimal) AND
      max_observed_alter_id_decimal <= source_high_watermark_decimal))
  ),
  UNIQUE(scope_sha256, proof_id, source_high_watermark_decimal),
  FOREIGN KEY (capability_observation_id)
    REFERENCES tally_incremental_capability_observations(id) ON DELETE RESTRICT,
  FOREIGN KEY (proof_id) REFERENCES tally_proof_ledger(id) ON DELETE RESTRICT,
  FOREIGN KEY (batch_id) REFERENCES tally_observation_batches(id) ON DELETE RESTRICT
);

CREATE TABLE IF NOT EXISTS tally_incremental_checkpoint_heads (
  scope_sha256 TEXT PRIMARY KEY CHECK (
    length(scope_sha256) = 64 AND scope_sha256 NOT GLOB '*[^0-9a-f]*'
  ),
  scope_json TEXT NOT NULL CHECK (json_valid(scope_json)),
  establishment_receipt_id TEXT NOT NULL UNIQUE,
  high_watermark_decimal TEXT NOT NULL CHECK (
    high_watermark_decimal = '0' OR (
      high_watermark_decimal NOT GLOB '*[^0-9]*' AND
      substr(high_watermark_decimal, 1, 1) BETWEEN '1' AND '9' AND
      (length(high_watermark_decimal) < 20 OR (
        length(high_watermark_decimal) = 20 AND
        high_watermark_decimal <= '18446744073709551615'
      ))
    )
  ),
  generation INTEGER NOT NULL CHECK (generation = 1),
  state TEXT NOT NULL CHECK (state = 'active'),
  established_at_unix_ms INTEGER NOT NULL CHECK (established_at_unix_ms > 0),
  FOREIGN KEY (establishment_receipt_id)
    REFERENCES tally_incremental_establishment_receipts(id) ON DELETE RESTRICT
);

CREATE TRIGGER IF NOT EXISTS tally_incremental_capabilities_no_update
BEFORE UPDATE ON tally_incremental_capability_observations
BEGIN
  SELECT RAISE(ABORT, 'incremental capability observations are immutable');
END;

CREATE TRIGGER IF NOT EXISTS tally_incremental_capabilities_no_delete
BEFORE DELETE ON tally_incremental_capability_observations
BEGIN
  SELECT RAISE(ABORT, 'incremental capability observations are immutable');
END;

CREATE TRIGGER IF NOT EXISTS tally_incremental_establishment_authority_required
BEFORE INSERT ON tally_incremental_establishment_receipts
WHEN NOT EXISTS (
  SELECT 1
  FROM tally_incremental_capability_observations AS capability
  JOIN tally_proof_ledger AS proof ON proof.id = NEW.proof_id
  JOIN tally_observation_batches AS batch ON batch.id = NEW.batch_id
  JOIN tally_capability_snapshots AS snapshot
    ON snapshot.id = proof.capability_snapshot_id
  JOIN tally_companies AS company ON company.id = proof.company_id
  JOIN tally_snapshot_run_states AS durable ON durable.run_id = proof.run_id
  WHERE capability.id = NEW.capability_observation_id
    AND capability.scope_sha256 = NEW.scope_sha256
    AND capability.scope_json = NEW.scope_json
    AND capability.company_id = proof.company_id
    AND capability.capability_snapshot_id = proof.capability_snapshot_id
    AND capability.capability_state = 'supported'
    AND capability.confidence = 'observed'
    AND capability.identifier_semantics = 'monotonic_per_object'
    AND capability.inclusive_lower_bound_observed = 1
    AND capability.explicit_source_high_watermark_observed = 1
    AND proof.entry_sha256 = NEW.proof_sha256
    AND proof.batch_id = NEW.batch_id
    AND proof.outcome = 'completed'
    AND proof.verification_state = 'verified'
    AND proof.completed_at_unix_ms IS NOT NULL
    AND proof.snapshot_sha256 IS NOT NULL
    AND proof.gap_codes_json = '[]'
    AND proof.warning_codes_json = '[]'
    AND proof.rejected_records = 0
    AND proof.accepted_records = NEW.accepted_record_count
    AND batch.run_id = proof.run_id
    AND batch.capability_snapshot_id = proof.capability_snapshot_id
    AND batch.company_id = proof.company_id
    AND batch.pack_id = proof.pack_id
    AND batch.pack_id = json_extract(NEW.scope_json, '$.pack')
    AND batch.pack_schema_major = json_extract(NEW.scope_json, '$.pack_schema_version.major')
    AND batch.pack_schema_minor = json_extract(NEW.scope_json, '$.pack_schema_version.minor')
    AND batch.source_transport = json_extract(NEW.scope_json, '$.transport')
    AND batch.source_release = json_extract(NEW.scope_json, '$.release')
    AND batch.state = 'verified'
    AND batch.snapshot_sha256 = proof.snapshot_sha256
    AND batch.accepted_records = NEW.accepted_record_count
    AND batch.rejected_records = 0
    AND snapshot.profile_version = json_extract(NEW.scope_json, '$.capability_profile_version')
    AND snapshot.product = json_extract(NEW.scope_json, '$.product')
    AND snapshot.release = json_extract(NEW.scope_json, '$.release')
    AND snapshot.mode = json_extract(NEW.scope_json, '$.mode')
    AND snapshot.mode_confidence = 'observed'
    AND company.endpoint_id = snapshot.endpoint_id
    AND company.company_guid = json_extract(NEW.scope_json, '$.company_guid') COLLATE NOCASE
    AND company.identity_confidence = 'observed'
    AND durable.row_sha256 IS NOT NULL
    AND durable.plan_sha256 = NEW.snapshot_plan_sha256
    AND json_extract(durable.state_json, '$.plan_sha256') = NEW.snapshot_plan_sha256
    AND json_extract(durable.state_json, '$.batch_id') = NEW.batch_id
    AND json_extract(durable.state_json, '$.progress.phase') = 'completed'
    AND json_extract(durable.state_json, '$.commit_receipt.proof_id') = NEW.proof_id
    AND json_extract(durable.state_json, '$.commit_receipt.proof_sha256') = NEW.proof_sha256
    AND EXISTS (
      SELECT 1 FROM json_each(durable.state_json, '$.plan.windows') AS window
      WHERE json_extract(window.value, '$.query_profile') =
              json_extract(NEW.scope_json, '$.query_profile')
        AND json_extract(window.value, '$.filters_sha256') =
              json_extract(NEW.scope_json, '$.filters_sha256')
    )
)
BEGIN
  SELECT RAISE(ABORT, 'incremental establishment authority is incomplete');
END;

-- This unconditional gate remains effective regardless of same-timing trigger order: an
-- earlier relational rejection is already fail-closed, while a relationally valid insert
-- reaches this rejection. Remove it only in a reviewed migration that atomically constructs
-- the receipt from verifier-owned evidence and bracketing source-watermark observations.
CREATE TRIGGER IF NOT EXISTS tally_incremental_establishment_disabled
BEFORE INSERT ON tally_incremental_establishment_receipts
BEGIN
  SELECT RAISE(ABORT, 'incremental establishment verifier is not enabled');
END;

CREATE TRIGGER IF NOT EXISTS tally_incremental_establishment_no_update
BEFORE UPDATE ON tally_incremental_establishment_receipts
BEGIN
  SELECT RAISE(ABORT, 'incremental establishment receipts are immutable');
END;

CREATE TRIGGER IF NOT EXISTS tally_incremental_establishment_no_delete
BEFORE DELETE ON tally_incremental_establishment_receipts
BEGIN
  SELECT RAISE(ABORT, 'incremental establishment receipts are immutable');
END;

CREATE TRIGGER IF NOT EXISTS tally_incremental_checkpoint_head_authority_required
BEFORE INSERT ON tally_incremental_checkpoint_heads
WHEN NOT EXISTS (
  SELECT 1 FROM tally_incremental_establishment_receipts AS receipt
  WHERE receipt.id = NEW.establishment_receipt_id
    AND receipt.scope_sha256 = NEW.scope_sha256
    AND receipt.scope_json = NEW.scope_json
    AND receipt.source_high_watermark_decimal = NEW.high_watermark_decimal
    AND receipt.created_at_unix_ms = NEW.established_at_unix_ms
)
BEGIN
  SELECT RAISE(ABORT, 'incremental checkpoint head lacks exact establishment receipt');
END;

CREATE TRIGGER IF NOT EXISTS tally_incremental_checkpoint_heads_no_update
BEFORE UPDATE ON tally_incremental_checkpoint_heads
BEGIN
  SELECT RAISE(ABORT, 'incremental checkpoint advancement is not enabled');
END;

CREATE TRIGGER IF NOT EXISTS tally_incremental_checkpoint_heads_no_delete
BEFORE DELETE ON tally_incremental_checkpoint_heads
BEGIN
  SELECT RAISE(ABORT, 'incremental checkpoint heads are immutable evidence');
END;

INSERT OR IGNORE INTO tally_schema_migrations(version, description, applied_at_unix_ms)
VALUES (6, 'scope-bound incremental establishment foundation', 0);
