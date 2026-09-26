-- A CA's Schedule III grouping decisions (#737, ADR 0020): an append-only
-- event log. Nothing here is updated or deleted; the current decision for a
-- ledger is a fold of its events.
--
-- Keyed by the observed company tuple and the financial year, not by an
-- endpoint-scoped tally_companies row: a decision belongs to the book, not to
-- the port it was reached on (ADR 0020 item 3). The display name is recorded
-- per event for the register and is not part of the key, so a company rename
-- keeps its decisions.
--
-- Head and outcome codes are parsed in Rust when a row is read, so a later
-- catalogue does not need a table rebuild; SQL holds only the structure.
CREATE TABLE IF NOT EXISTS schedule_iii_grouping_events (
  event_sequence INTEGER NOT NULL UNIQUE CHECK (event_sequence > 0),
  id TEXT NOT NULL PRIMARY KEY,
  company_guid TEXT NOT NULL CHECK (
    length(company_guid) BETWEEN 1 AND 128 AND company_guid = lower(company_guid)
  ),
  company_number TEXT NOT NULL CHECK (length(company_number) BETWEEN 1 AND 64),
  books_from_yyyymmdd TEXT NOT NULL CHECK (
    length(books_from_yyyymmdd) = 8 AND books_from_yyyymmdd NOT GLOB '*[^0-9]*'
  ),
  company_display_name TEXT NOT NULL CHECK (length(trim(company_display_name)) > 0),
  financial_year INTEGER NOT NULL CHECK (financial_year BETWEEN 2000 AND 2999),
  kind TEXT NOT NULL CHECK (kind IN ('set', 'carry_forward', 'withdraw', 'review')),
  ledger_guid TEXT NOT NULL CHECK (
    length(ledger_guid) BETWEEN 1 AND 128 AND ledger_guid = lower(ledger_guid)
  ),
  ledger_name TEXT NOT NULL CHECK (length(trim(ledger_name)) > 0),
  made_against TEXT CHECK (made_against IS NULL OR json_valid(made_against)),
  head TEXT,
  reason TEXT,
  reference TEXT CHECK (reference IS NULL OR length(trim(reference)) > 0),
  target_event_id TEXT REFERENCES schedule_iii_grouping_events(id) ON DELETE RESTRICT,
  declared_by TEXT NOT NULL CHECK (length(trim(declared_by)) BETWEEN 1 AND 256),
  recorded_at_unix_ms INTEGER NOT NULL CHECK (recorded_at_unix_ms > 0),
  -- Every term is written so that NULL cannot satisfy it: SQLite passes a
  -- CHECK whose expression is NULL.
  CHECK (
    (kind = 'set'
      AND made_against IS NOT NULL AND head IS NOT NULL
      AND reason IS NOT NULL AND length(trim(reason)) > 0 AND target_event_id IS NULL)
    OR (kind = 'carry_forward'
      AND made_against IS NOT NULL AND head IS NOT NULL
      AND reason IS NOT NULL AND length(trim(reason)) > 0 AND target_event_id IS NOT NULL)
    OR (kind = 'withdraw'
      AND made_against IS NULL AND head IS NULL AND reference IS NULL
      AND reason IS NOT NULL AND length(trim(reason)) > 0 AND target_event_id IS NOT NULL)
    OR (kind = 'review'
      AND made_against IS NULL AND head IS NULL AND reason IS NULL AND reference IS NULL
      AND target_event_id IS NOT NULL)
  )
);

CREATE INDEX IF NOT EXISTS idx_schedule_iii_grouping_events_book_year
ON schedule_iii_grouping_events(company_guid, financial_year, event_sequence);

-- A decision is withdrawn at most once.
CREATE UNIQUE INDEX IF NOT EXISTS uq_schedule_iii_grouping_events_withdrawal
ON schedule_iii_grouping_events(target_event_id) WHERE kind = 'withdraw';

-- A withdrawal or a review names a decision of the same book, year and ledger.
CREATE TRIGGER IF NOT EXISTS schedule_iii_grouping_events_target_same_decision
BEFORE INSERT ON schedule_iii_grouping_events
WHEN NEW.kind IN ('withdraw', 'review') AND NOT EXISTS (
  SELECT 1 FROM schedule_iii_grouping_events AS target
  WHERE target.id = NEW.target_event_id
    AND target.kind IN ('set', 'carry_forward')
    AND target.company_guid = NEW.company_guid
    AND target.company_number = NEW.company_number
    AND target.books_from_yyyymmdd = NEW.books_from_yyyymmdd
    AND target.financial_year = NEW.financial_year
    AND target.ledger_guid = NEW.ledger_guid
)
BEGIN
  SELECT RAISE(ABORT, 'a withdrawal or review must name a decision of the same book, year and ledger');
END;

-- A carry-forward names a decision on the same ledger in the previous year.
-- Only the company GUID must match: a year-end split keeps the GUID but can
-- change the company number and books-from (TALLY_PROTOCOL_REFERENCE.md
-- §9.11b), and a person confirms each carry-forward.
CREATE TRIGGER IF NOT EXISTS schedule_iii_grouping_events_carry_forward_previous_year
BEFORE INSERT ON schedule_iii_grouping_events
WHEN NEW.kind = 'carry_forward' AND NOT EXISTS (
  SELECT 1 FROM schedule_iii_grouping_events AS target
  WHERE target.id = NEW.target_event_id
    AND target.kind IN ('set', 'carry_forward')
    AND target.company_guid = NEW.company_guid
    AND target.financial_year = NEW.financial_year - 1
    AND target.ledger_guid = NEW.ledger_guid
)
BEGIN
  SELECT RAISE(ABORT, 'a carry-forward must name a decision on the same ledger in the previous year');
END;

CREATE TRIGGER IF NOT EXISTS schedule_iii_grouping_events_no_update
BEFORE UPDATE ON schedule_iii_grouping_events
BEGIN
  SELECT RAISE(ABORT, 'grouping decision events are immutable');
END;

CREATE TRIGGER IF NOT EXISTS schedule_iii_grouping_events_no_delete
BEFORE DELETE ON schedule_iii_grouping_events
BEGIN
  SELECT RAISE(ABORT, 'grouping decision events cannot be deleted');
END;

INSERT INTO tally_schema_migrations(version, description, applied_at_unix_ms)
VALUES (28, 'schedule iii grouping decision events', 0);
