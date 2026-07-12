pub const INITIAL_SCHEMA: &str = r#"
CREATE TABLE IF NOT EXISTS outbox (
  id TEXT PRIMARY KEY,
  created INTEGER NOT NULL,
  operation TEXT NOT NULL,
  target TEXT NOT NULL,
  payload TEXT NOT NULL,
  status TEXT NOT NULL,
  attempts INTEGER DEFAULT 0,
  last_err TEXT,
  next_try INTEGER
);

CREATE TABLE IF NOT EXISTS sync_log (
  id TEXT PRIMARY KEY,
  ts INTEGER NOT NULL,
  company TEXT NOT NULL,
  record_type TEXT NOT NULL,
  record_id TEXT NOT NULL,
  action TEXT NOT NULL,
  old_val TEXT,
  new_val TEXT,
  actor TEXT
);

CREATE TABLE IF NOT EXISTS altmastid_cache (
  company TEXT NOT NULL,
  obj_type TEXT NOT NULL,
  last_id INTEGER NOT NULL,
  synced_at INTEGER NOT NULL,
  PRIMARY KEY (company, obj_type)
);

CREATE TABLE IF NOT EXISTS conflict_queue (
  id TEXT PRIMARY KEY,
  company TEXT NOT NULL,
  record_type TEXT NOT NULL,
  record_id TEXT NOT NULL,
  tally_val TEXT NOT NULL,
  axal_val TEXT NOT NULL,
  tally_ts INTEGER NOT NULL,
  axal_ts INTEGER NOT NULL,
  status TEXT NOT NULL,
  resolution TEXT,
  resolved_at INTEGER
);

CREATE TABLE IF NOT EXISTS companies (
  name TEXT PRIMARY KEY,
  address TEXT,
  state TEXT,
  phone TEXT,
  email TEXT,
  gst_number TEXT,
  last_synced INTEGER,
  is_active INTEGER DEFAULT 1
);

CREATE TABLE IF NOT EXISTS ledgers (
  id TEXT PRIMARY KEY,
  company TEXT NOT NULL,
  name TEXT NOT NULL,
  parent TEXT,
  address TEXT,
  email TEXT,
  phone TEXT,
  mobile TEXT,
  pincode TEXT,
  state TEXT,
  gst_reg_type TEXT,
  party_gstin TEXT,
  income_tax_no TEXT,
  opening_balance TEXT,
  tally_altmastid INTEGER,
  last_modified INTEGER,
  FOREIGN KEY (company) REFERENCES companies(name)
);
"#;
