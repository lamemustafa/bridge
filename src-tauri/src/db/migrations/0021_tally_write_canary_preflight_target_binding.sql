ALTER TABLE tally_write_canary_preflight_evidence
  ADD COLUMN canonical_endpoint_sha256 TEXT;

ALTER TABLE tally_write_canary_preflight_evidence
  ADD COLUMN company_identity_sha256 TEXT;

CREATE TRIGGER IF NOT EXISTS tally_write_canary_preflight_evidence_requires_target_binding
BEFORE INSERT ON tally_write_canary_preflight_evidence
WHEN NEW.canonical_endpoint_sha256 IS NULL
  OR length(NEW.canonical_endpoint_sha256) != 64
  OR NEW.canonical_endpoint_sha256 GLOB '*[^0-9a-f]*'
  OR NEW.company_identity_sha256 IS NULL
  OR length(NEW.company_identity_sha256) != 64
  OR NEW.company_identity_sha256 GLOB '*[^0-9a-f]*'
BEGIN
  SELECT RAISE(ABORT, 'canary preflight evidence requires an exact target binding');
END;

INSERT OR IGNORE INTO tally_schema_migrations(version, description, applied_at_unix_ms)
VALUES (21, 'immutable endpoint and company target binding for local Tally synthetic write preflight evidence', 0);
