CREATE TABLE IF NOT EXISTS tally_snapshot_run_states (
  resume_key TEXT PRIMARY KEY,
  run_id TEXT NOT NULL,
  generation INTEGER NOT NULL CHECK (generation > 0),
  state_sha256 TEXT NOT NULL CHECK (
    length(state_sha256) = 64 AND state_sha256 NOT GLOB '*[^0-9a-f]*'
  ),
  state_json TEXT NOT NULL CHECK (json_valid(state_json)),
  updated_at_unix_ms INTEGER NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_tally_snapshot_run_states_run
  ON tally_snapshot_run_states(run_id);

INSERT OR IGNORE INTO tally_schema_migrations(version, description, applied_at_unix_ms)
VALUES (4, 'durable resumable Tally snapshot run state', 0);

