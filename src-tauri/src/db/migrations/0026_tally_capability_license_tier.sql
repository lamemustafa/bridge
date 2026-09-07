-- Historical snapshots retain an unknown tier; no observation is inferred.
ALTER TABLE tally_capability_snapshots ADD COLUMN license_tier TEXT
  CHECK (license_tier IS NULL OR license_tier IN ('silver', 'gold'));

INSERT INTO tally_schema_migrations(version, description, applied_at_unix_ms)
VALUES (26, 'Retain observed capability license tier', 0);
