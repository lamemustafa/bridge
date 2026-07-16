ALTER TABLE tally_observation_batches
  ADD COLUMN provenance_unavailable_records INTEGER NOT NULL DEFAULT 0
  CHECK (provenance_unavailable_records >= 0);

ALTER TABLE tally_proof_ledger
  ADD COLUMN provenance_unavailable_records INTEGER NOT NULL DEFAULT 0
  CHECK (provenance_unavailable_records >= 0);

INSERT OR IGNORE INTO tally_schema_migrations(version, description, applied_at_unix_ms)
VALUES (10, 'receipt-bound provenance-unavailable record counts', 0);
