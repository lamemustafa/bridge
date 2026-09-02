-- The composite identity migration introduced the complete observed-company
-- tuple, but its partial unique index did not reject incomplete observed rows.
-- Keep legacy rows honest: missing tuple fields are not inferred from a later
-- listing because that could bind a pin to a different post-split book.
UPDATE tally_companies
SET identity_confidence = 'unknown'
WHERE identity_confidence = 'observed'
  AND (
    company_guid IS NULL
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
    NEW.company_guid IS NULL
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
BEFORE UPDATE OF identity_confidence, company_guid, company_number, books_from_yyyymmdd
ON tally_companies
WHEN NEW.identity_confidence = 'observed'
  AND (
    NEW.company_guid IS NULL
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
