-- The composite identity migration introduced the complete observed-company
-- tuple, but its partial unique index did not reject incomplete observed rows.
-- Keep legacy rows honest: missing tuple fields are not inferred from a later
-- listing because that could bind a pin to a different post-split book.
UPDATE tally_companies
SET identity_confidence = 'unknown'
WHERE identity_confidence = 'observed'
  AND (
    display_name IS NULL
    OR length(trim(display_name, char(9) || char(10) || char(13) || ' ')) = 0
    OR company_guid IS NULL
    OR length(trim(company_guid, char(9) || char(10) || char(13) || ' ')) = 0
    OR company_number IS NULL
    OR length(trim(company_number, char(9) || char(10) || char(13) || ' ')) = 0
    OR books_from_yyyymmdd IS NULL
    OR length(trim(books_from_yyyymmdd, char(9) || char(10) || char(13) || ' ')) = 0
  );

CREATE TRIGGER tally_companies_observed_identity_required_on_insert
BEFORE INSERT ON tally_companies
WHEN NEW.identity_confidence = 'observed'
  AND (
    NEW.display_name IS NULL
    OR length(trim(NEW.display_name, char(9) || char(10) || char(13) || ' ')) = 0
    OR NEW.company_guid IS NULL
    OR length(trim(NEW.company_guid, char(9) || char(10) || char(13) || ' ')) = 0
    OR NEW.company_number IS NULL
    OR length(trim(NEW.company_number, char(9) || char(10) || char(13) || ' ')) = 0
    OR NEW.books_from_yyyymmdd IS NULL
    OR length(trim(NEW.books_from_yyyymmdd, char(9) || char(10) || char(13) || ' ')) = 0
  )
BEGIN
  SELECT RAISE(ABORT, 'observed company identity requires complete tuple');
END;

CREATE TRIGGER tally_companies_observed_identity_required_on_update
BEFORE UPDATE OF identity_confidence, display_name, company_guid, company_number, books_from_yyyymmdd
ON tally_companies
WHEN NEW.identity_confidence = 'observed'
  AND (
    NEW.display_name IS NULL
    OR length(trim(NEW.display_name, char(9) || char(10) || char(13) || ' ')) = 0
    OR NEW.company_guid IS NULL
    OR length(trim(NEW.company_guid, char(9) || char(10) || char(13) || ' ')) = 0
    OR NEW.company_number IS NULL
    OR length(trim(NEW.company_number, char(9) || char(10) || char(13) || ' ')) = 0
    OR NEW.books_from_yyyymmdd IS NULL
    OR length(trim(NEW.books_from_yyyymmdd, char(9) || char(10) || char(13) || ' ')) = 0
  )
BEGIN
  SELECT RAISE(ABORT, 'observed company identity requires complete tuple');
END;

INSERT OR IGNORE INTO tally_schema_migrations(version, description, applied_at_unix_ms)
VALUES (25, 'observed company identity requires a complete nonblank tuple', 0);

-- A v24 binary skips the retired v7 upgrade because v23 is installed. Retire
-- its v24 marker instead, so its normal upgrade path fails explicitly before
-- a GUID-only generic write can hit the tuple trigger without context.
DELETE FROM tally_schema_migrations WHERE version = 24;

CREATE TRIGGER tally_schema_migrations_reject_legacy_v24_after_observed_identity
BEFORE INSERT ON tally_schema_migrations
WHEN NEW.version = 24
  AND EXISTS (
    SELECT 1 FROM tally_schema_migrations WHERE version = 25
  )
BEGIN
  SELECT RAISE(ABORT, 'observed company identity requires a compatible Bridge binary');
END;
