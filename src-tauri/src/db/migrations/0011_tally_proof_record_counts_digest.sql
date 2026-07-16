ALTER TABLE tally_proof_ledger
  ADD COLUMN record_counts_sha256 TEXT
  CHECK (
    record_counts_sha256 IS NULL OR
    (LENGTH(record_counts_sha256) = 64 AND
     record_counts_sha256 NOT GLOB '*[^0-9a-f]*')
  );

INSERT OR IGNORE INTO tally_schema_migrations(version, description, applied_at_unix_ms)
VALUES (11, 'proof-bound canonical record count digest', 0);
