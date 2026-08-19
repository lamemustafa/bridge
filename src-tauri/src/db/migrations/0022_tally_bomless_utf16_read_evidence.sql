DROP TRIGGER IF EXISTS tally_selected_read_observations_no_update;
DROP TRIGGER IF EXISTS tally_selected_read_observation_authority_required;
DROP TRIGGER IF EXISTS tally_selected_read_observations_no_delete;

PRAGMA legacy_alter_table = ON;

ALTER TABLE tally_selected_read_observations
  RENAME TO tally_selected_read_observations_v21;

CREATE TABLE tally_selected_read_observations (
  scope_id TEXT NOT NULL,
  capability_snapshot_id TEXT NOT NULL,
  capability_kind TEXT NOT NULL CHECK (capability_kind = 'feature'),
  capability_key TEXT NOT NULL CHECK (
    capability_key IN ('selected_ledger_read', 'selected_voucher_window_read')
  ),
  capability_state TEXT NOT NULL CHECK (capability_state IN ('supported', 'unknown')),
  confidence TEXT NOT NULL CHECK (confidence IN ('observed', 'unknown')),
  safe_reason_code TEXT NOT NULL,
  result_bucket TEXT NOT NULL CHECK (
    result_bucket IN ('empty_observed', 'non_empty_observed', 'rejected', 'skipped')
  ),
  request_sha256 TEXT CHECK (
    request_sha256 IS NULL OR (
      length(request_sha256) = 64 AND request_sha256 NOT GLOB '*[^0-9a-f]*'
    )
  ),
  decoded_response_sha256 TEXT CHECK (
    decoded_response_sha256 IS NULL OR (
      length(decoded_response_sha256) = 64 AND
      decoded_response_sha256 NOT GLOB '*[^0-9a-f]*'
    )
  ),
  response_encoding TEXT CHECK (
    response_encoding IS NULL OR response_encoding IN (
      'utf8', 'utf8_bom', 'utf16le', 'utf16le_bom', 'utf16be_bom'
    )
  ),
  company_context_verified INTEGER NOT NULL CHECK (company_context_verified IN (0, 1)),
  schema_verified INTEGER NOT NULL CHECK (schema_verified IN (0, 1)),
  record_count_verified INTEGER NOT NULL CHECK (record_count_verified IN (0, 1)),
  identity_evidence_state TEXT NOT NULL CHECK (
    identity_evidence_state IN ('verified', 'not_applicable_empty', 'unverified')
  ),
  date_window_verified INTEGER NOT NULL CHECK (date_window_verified IN (0, 1)),
  PRIMARY KEY (scope_id, capability_key),
  UNIQUE (capability_snapshot_id, capability_key),
  CHECK (
    (capability_state = 'supported' AND confidence = 'observed' AND
      result_bucket IN ('empty_observed', 'non_empty_observed') AND
      request_sha256 IS NOT NULL AND decoded_response_sha256 IS NOT NULL AND
      response_encoding IS NOT NULL AND
      company_context_verified = 1 AND schema_verified = 1 AND
      record_count_verified = 1 AND
      ((result_bucket = 'empty_observed' AND identity_evidence_state = 'not_applicable_empty') OR
       (result_bucket = 'non_empty_observed' AND identity_evidence_state = 'verified')) AND
      ((capability_key = 'selected_ledger_read' AND date_window_verified = 0) OR
       (capability_key = 'selected_voucher_window_read' AND date_window_verified = 1)))
    OR
    (capability_state = 'unknown' AND
      ((result_bucket = 'rejected' AND confidence = 'observed') OR
       (result_bucket = 'skipped' AND confidence = 'unknown')) AND
      request_sha256 IS NULL AND decoded_response_sha256 IS NULL AND
      response_encoding IS NULL AND company_context_verified = 0 AND
      schema_verified = 0 AND record_count_verified = 0 AND
      identity_evidence_state = 'unverified' AND date_window_verified = 0)
  ),
  FOREIGN KEY (scope_id, capability_snapshot_id)
    REFERENCES tally_selected_read_scopes(id, capability_snapshot_id)
    ON DELETE RESTRICT,
  FOREIGN KEY (capability_snapshot_id, capability_kind, capability_key)
    REFERENCES tally_capability_items(snapshot_id, capability_kind, capability_key)
    ON DELETE RESTRICT
);

INSERT INTO tally_selected_read_observations(
  scope_id, capability_snapshot_id, capability_kind, capability_key,
  capability_state, confidence, safe_reason_code, result_bucket,
  request_sha256, decoded_response_sha256, response_encoding,
  company_context_verified, schema_verified, record_count_verified,
  identity_evidence_state, date_window_verified
)
SELECT
  scope_id, capability_snapshot_id, capability_kind, capability_key,
  capability_state, confidence, safe_reason_code, result_bucket,
  request_sha256, decoded_response_sha256, response_encoding,
  company_context_verified, schema_verified, record_count_verified,
  identity_evidence_state, date_window_verified
FROM tally_selected_read_observations_v21;

DROP TABLE tally_selected_read_observations_v21;

CREATE TRIGGER tally_selected_read_observations_no_update
BEFORE UPDATE ON tally_selected_read_observations
BEGIN
  SELECT RAISE(ABORT, 'selected read observations are immutable');
END;

CREATE TRIGGER tally_selected_read_observation_authority_required
BEFORE INSERT ON tally_selected_read_observations
WHEN NOT EXISTS (
  SELECT 1 FROM tally_capability_items AS item
  WHERE item.snapshot_id = NEW.capability_snapshot_id
    AND item.capability_kind = 'feature'
    AND item.capability_key = NEW.capability_key
    AND item.capability_state = NEW.capability_state
    AND item.confidence = NEW.confidence
    AND item.safe_reason_code = NEW.safe_reason_code
)
BEGIN
  SELECT RAISE(ABORT, 'selected read observation authority is incomplete');
END;

CREATE TRIGGER tally_selected_read_observations_no_delete
BEFORE DELETE ON tally_selected_read_observations
BEGIN
  SELECT RAISE(ABORT, 'selected read observations are immutable');
END;

PRAGMA legacy_alter_table = OFF;

INSERT OR IGNORE INTO tally_schema_migrations(version, description, applied_at_unix_ms)
VALUES (22, 'explicit BOM-less UTF-16LE selected-read evidence', 0);
