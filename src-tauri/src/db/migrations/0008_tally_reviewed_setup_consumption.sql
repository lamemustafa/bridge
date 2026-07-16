CREATE TABLE IF NOT EXISTS tally_reviewed_setup_consumptions (
  review_commitment_sha256 TEXT PRIMARY KEY CHECK (
    length(review_commitment_sha256) = 64 AND
    review_commitment_sha256 NOT GLOB '*[^0-9a-f]*'
  ),
  setup_payload_sha256 TEXT NOT NULL CHECK (
    length(setup_payload_sha256) = 64 AND
    setup_payload_sha256 NOT GLOB '*[^0-9a-f]*'
  ),
  capability_snapshot_id TEXT NOT NULL UNIQUE,
  company_id TEXT NOT NULL,
  consumed_at_unix_ms INTEGER NOT NULL CHECK (consumed_at_unix_ms > 0),
  FOREIGN KEY (capability_snapshot_id) REFERENCES tally_capability_snapshots(id) ON DELETE RESTRICT,
  FOREIGN KEY (company_id) REFERENCES tally_companies(id) ON DELETE RESTRICT
);

CREATE TRIGGER IF NOT EXISTS tally_reviewed_setup_consumptions_no_update
BEFORE UPDATE ON tally_reviewed_setup_consumptions
BEGIN
  SELECT RAISE(ABORT, 'reviewed setup consumptions are immutable');
END;

CREATE TRIGGER IF NOT EXISTS tally_reviewed_setup_consumptions_no_delete
BEFORE DELETE ON tally_reviewed_setup_consumptions
BEGIN
  SELECT RAISE(ABORT, 'reviewed setup consumptions are immutable');
END;

CREATE TRIGGER IF NOT EXISTS tally_reviewed_setup_consumption_authority_required
BEFORE INSERT ON tally_reviewed_setup_consumptions
WHEN NOT EXISTS (
  SELECT 1 FROM tally_capability_snapshots AS snapshot
  JOIN tally_companies AS company ON company.id = NEW.company_id
  WHERE snapshot.id = NEW.capability_snapshot_id
    AND snapshot.endpoint_id = company.endpoint_id
)
BEGIN
  SELECT RAISE(ABORT, 'reviewed setup consumption authority is incomplete');
END;

INSERT OR IGNORE INTO tally_schema_migrations(version, description, applied_at_unix_ms)
VALUES (8, 'durable idempotent reviewed-setup consumption authority', 0);
