# Bridge contributor guide

## Code of conduct

Follow [CODE_OF_CONDUCT.md](./CODE_OF_CONDUCT.md). Keep discussions
constructive, technical, and free of private customer or contributor data.

## Contribution license

Unless you explicitly state otherwise, any contribution intentionally
submitted for inclusion in Bridge is provided under the Apache License,
Version 2.0, without additional terms or conditions. Mark material that is
not a contribution conspicuously as `Not a Contribution`.

Only submit work that you have the right to license. Identify third-party
material in the pull request and preserve all applicable copyright,
attribution, and license notices. Update [NOTICE](./NOTICE) when a required
attribution is introduced.

## Development setup

Use the [supported development hosts](./README.md#supported-development-hosts) and
[contributor quick start](./README.md#contributor-quick-start) in the README.
That is the canonical prerequisite and command list; it covers the native
Tauri, Perl/SQLCipher, and LLVM/libclang requirements without duplicating them
here. After setup, run the smallest relevant check for the files you changed,
then the required checks on the final candidate:

```text
corepack pnpm test
corepack pnpm run cargo:test
corepack pnpm run build
corepack pnpm run cargo:check
```

Run `corepack pnpm run tauri:build` on each native target host when validating
packaging. Do not treat a Windows build as macOS evidence or the reverse.

## Branching

- Use short-lived branches from `master`.
- Use `feat/` for features, `fix/` for defects, `refactor/` for refactors,
  `chore/` for infrastructure or docs, and `rectify/` for regressions.
- Use an imperative commit subject no longer than 72 characters.

All changes go through a pull request. Follow
[docs/rectify-guidelines.md](./docs/rectify-guidelines.md) for regressions.
Potential vulnerabilities and credential/data leaks must use the private flow
in [SECURITY.md](./SECURITY.md); do not open a public issue or PR until
coordinated disclosure is safe.

## Pull-request requirements

Use the [pull-request template](./.github/PULL_REQUEST_TEMPLATE.md). It asks
for one concrete outcome, scope and reuse/deletion decisions, exact candidate
evidence, compatibility and rollback notes, and the required security and
platform details. Every pull request must link at least one completed item in
the [review checklist](./review-checklist.md).

Never paste raw production logs. Redact or replace names, email addresses,
company and tax identifiers, financial data, document contents, certificate
details, PINs, tokens, usernames, and absolute local paths.

## Issue and triage requirements

- Use the bug or feature template in `.github/ISSUE_TEMPLATE`.
- Assign exactly one area label: `area:tally`, `area:dsc`, `area:documents`,
  `area:infra`, or `area:security`.
- Assign a severity label for bugs: `severity:p1` through `severity:p4`.
- Use `type:rectify` for a regression introduced by a merged change and link
  the introducing pull request.
- Include the operating system and Bridge version for workflow bugs. Add Tally
  and DSC vendor versions only when relevant and safe to disclose.

Use synthetic reproduction data. Do not attach customer files, certificate
output, secrets, or machine-specific paths.
