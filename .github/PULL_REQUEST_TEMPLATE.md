## Outcome and reason

What concrete user or maintainer workflow changes, and why now?

## Scope, reuse, and impact

- Scope and explicit exclusions:
- Existing component reused:
- What is deleted (or why no deletion is justified):
- What breaks if this is not built:
- Net LOC change (production, tests/fixtures, generated files):
- Source issue or rectify reference (with area/severity labels when applicable):
- Migration/sync compatibility and rollback procedure (required when an
  existing workflow changes):
- Destructive database migration: <!-- No, or explain safeguards and rollback -->
- Security impact for DSC, Tally, credentials, endpoints, or customer data:
  <!-- None, or describe the change -->
- Security-focused reviewer comment: <!-- Required for DSC or credential-path changes -->

## Validation and evidence

- Exact candidate SHA:
- Commands and results (`corepack pnpm ...`, `cargo ...`, or reproduction):
- Captured/fixture/live scope and known limitations:
- Manual/UI evidence (screenshots or logs) when behavior changes:
- [ ] One completed [`review-checklist.md`](../review-checklist.md) line is
      linked here: <!-- paste permalink -->
- [ ] Native Windows validation completed or explained as not applicable
- [ ] Native macOS validation completed or explained as not applicable
- [ ] Rectify issue linked and [`docs/rectify-guidelines.md`](../docs/rectify-guidelines.md)
      followed if this is a regression fix

## Checklist

- [ ] Security implications reviewed (especially DSC, Tally, and credential flows)
- [ ] Migration compatibility and rollback impact documented
- [ ] Error handling paths still return actionable errors
- [ ] No leftover debug logs with sensitive values
- [ ] No personal/customer data, certificate output, local usernames, or
  developer-specific absolute paths added
- [ ] Potential vulnerabilities use the private `SECURITY.md` flow until
  coordinated disclosure is safe
- [ ] Tests, screenshots, and fixtures use synthetic or redacted data
- [ ] I have the right to submit these changes under Apache-2.0, and any
  third-party license or NOTICE obligations are documented and preserved
- [ ] Branching and labels align with managed-git policy
