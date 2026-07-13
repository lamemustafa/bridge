## Functional summary

Brief summary of what this PR changes.

## Source issue/rectify reference

- Link to related issue(s) and area/severity labels (if applicable).

## Why

Why is this needed for Bridge now?

## Testing

- [ ] Manual run notes
- [ ] Command checks (`corepack pnpm run ...`)
- [ ] Screenshots/logs attached where UI/behavior changed
- [ ] One completed [`review-checklist.md`](../review-checklist.md) line is linked here: <!-- paste permalink -->
- [ ] `docs/rectify-guidelines.md` updated if this is a regression fix
- [ ] Native Windows validation completed or explained as not applicable
- [ ] Native macOS validation completed or explained as not applicable

## Compatibility and rollback

- Migration/sync compatibility impact: <!-- None, or describe old/new compatibility -->
- Rollback procedure: <!-- Required when an existing workflow changes -->
- Destructive database migration: <!-- No, or explain safeguards and rollback -->

## Security impact

- DSC/Tally/credential impact: <!-- None, or describe the change -->
- Security-focused reviewer comment: <!-- Required for DSC or credential-path changes -->

## Checklist

- [ ] Security implications reviewed (especially DSC, Tally, and credential flows)
- [ ] Migration compatibility and rollback impact documented
- [ ] Error handling paths still return actionable errors
- [ ] No leftover debug logs with sensitive values
- [ ] No personal/customer data, certificate output, local usernames, or
  developer-specific absolute paths added
- [ ] Tests, screenshots, and fixtures use synthetic or redacted data
- [ ] I have the right to submit these changes under Apache-2.0, and any
  third-party license or NOTICE obligations are documented and preserved
- [ ] Branching and labels align with managed-git policy
