-- Older Bridge binaries reran bootstrap migration 0002 after composite identity
-- retired this index. The current bootstrap is marker-gated before this repair,
-- so an upgraded mirror can safely retire the resurrected GUID-only constraint.
DROP INDEX IF EXISTS uq_tally_companies_guid;

INSERT INTO tally_schema_migrations(version, description, applied_at_unix_ms)
VALUES (27, 'retire GUID-only company index resurrected by legacy bootstrap', 0);
