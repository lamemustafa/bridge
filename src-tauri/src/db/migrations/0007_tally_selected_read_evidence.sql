DROP INDEX IF EXISTS uq_tally_companies_guid;

CREATE UNIQUE INDEX uq_tally_companies_guid
  ON tally_companies(endpoint_id, company_guid COLLATE NOCASE)
  WHERE company_guid IS NOT NULL;

CREATE TABLE IF NOT EXISTS tally_selected_read_scopes (
  id TEXT PRIMARY KEY,
  capability_snapshot_id TEXT NOT NULL UNIQUE,
  company_id TEXT NOT NULL,
  scope_contract_version INTEGER NOT NULL CHECK (scope_contract_version = 1),
  scope_commitment_sha256 TEXT NOT NULL UNIQUE CHECK (
    length(scope_commitment_sha256) = 64 AND
    scope_commitment_sha256 NOT GLOB '*[^0-9a-f]*'
  ),
  parent_review_sha256 TEXT NOT NULL CHECK (
    length(parent_review_sha256) = 64 AND
    parent_review_sha256 NOT GLOB '*[^0-9a-f]*'
  ),
  ledger_profile_id TEXT NOT NULL CHECK (ledger_profile_id = 'bridge.tally.ledgers/1'),
  voucher_profile_id TEXT NOT NULL CHECK (voucher_profile_id = 'bridge.tally.vouchers/3'),
  voucher_from_yyyymmdd TEXT NOT NULL CHECK (
    length(voucher_from_yyyymmdd) = 8 AND
    voucher_from_yyyymmdd NOT GLOB '*[^0-9]*'
  ),
  voucher_to_yyyymmdd TEXT NOT NULL CHECK (
    length(voucher_to_yyyymmdd) = 8 AND
    voucher_to_yyyymmdd NOT GLOB '*[^0-9]*' AND
    voucher_from_yyyymmdd <= voucher_to_yyyymmdd
  ),
  observed_at_unix_ms INTEGER NOT NULL CHECK (observed_at_unix_ms > 0),
  completeness_state TEXT NOT NULL CHECK (completeness_state = 'not_claimed'),
  no_writes_attempted INTEGER NOT NULL CHECK (no_writes_attempted = 1),
  raw_records_retained INTEGER NOT NULL CHECK (raw_records_retained = 0),
  UNIQUE (id, capability_snapshot_id),
  FOREIGN KEY (capability_snapshot_id) REFERENCES tally_capability_snapshots(id) ON DELETE RESTRICT,
  FOREIGN KEY (company_id) REFERENCES tally_companies(id) ON DELETE RESTRICT
);

CREATE TABLE IF NOT EXISTS tally_selected_read_observations (
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
      length(decoded_response_sha256) = 64 AND decoded_response_sha256 NOT GLOB '*[^0-9a-f]*'
    )
  ),
  response_encoding TEXT CHECK (
    response_encoding IS NULL OR response_encoding IN (
      'utf8', 'utf8_bom', 'utf16le_bom', 'utf16be_bom'
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

CREATE TRIGGER IF NOT EXISTS tally_selected_read_scopes_no_update
BEFORE UPDATE ON tally_selected_read_scopes
BEGIN
  SELECT RAISE(ABORT, 'selected read scopes are immutable');
END;

CREATE TRIGGER IF NOT EXISTS tally_selected_read_scope_authority_required
BEFORE INSERT ON tally_selected_read_scopes
WHEN NOT EXISTS (
  SELECT 1 FROM tally_capability_snapshots AS snapshot
  JOIN tally_companies AS company ON company.id = NEW.company_id
  WHERE snapshot.id = NEW.capability_snapshot_id
    AND company.endpoint_id = snapshot.endpoint_id
    AND NEW.observed_at_unix_ms >= snapshot.observed_at_unix_ms
)
BEGIN
  SELECT RAISE(ABORT, 'selected read scope authority is incomplete');
END;

CREATE TRIGGER IF NOT EXISTS tally_selected_read_scopes_no_delete
BEFORE DELETE ON tally_selected_read_scopes
BEGIN
  SELECT RAISE(ABORT, 'selected read scopes are immutable');
END;

CREATE TRIGGER IF NOT EXISTS tally_selected_read_observations_no_update
BEFORE UPDATE ON tally_selected_read_observations
BEGIN
  SELECT RAISE(ABORT, 'selected read observations are immutable');
END;

CREATE TRIGGER IF NOT EXISTS tally_selected_read_observation_authority_required
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

CREATE TRIGGER IF NOT EXISTS tally_selected_read_observations_no_delete
BEFORE DELETE ON tally_selected_read_observations
BEGIN
  SELECT RAISE(ABORT, 'selected read observations are immutable');
END;

CREATE TRIGGER IF NOT EXISTS tally_capability_items_no_update
BEFORE UPDATE ON tally_capability_items
BEGIN
  SELECT RAISE(ABORT, 'capability items are immutable');
END;

CREATE TRIGGER IF NOT EXISTS tally_capability_items_no_delete
BEFORE DELETE ON tally_capability_items
BEGIN
  SELECT RAISE(ABORT, 'capability items are immutable');
END;

INSERT OR IGNORE INTO tally_schema_migrations(version, description, applied_at_unix_ms)
VALUES (7, 'scoped selected-read evidence and case-insensitive company GUID authority', 0);
