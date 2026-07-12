# Rectify guidelines

Every production-affecting bug or behavior regression should enter a tracked rectify loop.

## Trigger conditions

- A merged change causes an unintended functional regression in:
  - Tally sync reliability
  - DSC token/certificate extraction
  - Document scan/upload behavior
  - Local persistence or migration integrity
- A user reports that violates an existing acceptance criterion from the PR checklist.

## Flow

1. Open issue with:
   - `type:rectify`
   - severity label (`severity:p1` / `severity:p2` / `severity:p3`)
   - short summary and reproduction steps
   - pointer to commit/PR that introduced the regression
   - synthetic reproduction data with personal/customer data and machine paths removed.
2. Tag the owning area:
   - `area:tally`, `area:dsc`, `area:documents`, or `area:infra`
3. Open a dedicated fix branch:
   - format: `rectify/<area>/<short-slug>`
4. Use `review-checklist.md` plus the original checklist item from the source PR.
5. Add explicit verification notes before merge:
   - command run outputs
   - manual check evidence
   - expected vs observed outputs for the failure path
   - native Windows and macOS results when platform behavior may be affected
   - rollback and migration compatibility notes.

## Exit criteria

- Regression is reproducible/validated as fixed.
- All related checklist items are re-checked.
- Parent issue is closed with a link to the fix commit/PR.
- Shared evidence is redacted and contains no credentials, certificate data,
  personal/customer data, usernames, or absolute local paths.
