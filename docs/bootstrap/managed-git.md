# Bridge managed Git guidance

Run commands from the repository root in any local clone. Repository setup is
already complete for an existing clone; do not reinitialize it.

## Verify repository identity

- `package.json` name is `bridge`.
- `src-tauri/Cargo.toml` package and default binary are `bridge`.
- `src-tauri/tauri.conf.json` product name is `Bridge`.
- No configuration refers to a developer-specific absolute path.

## Connect a remote for a new clone or migration

Use the repository URL supplied by its owner instead of embedding a personal
account in project documentation:

```text
git remote add origin <repository-url>
git push -u origin main
```

Do not publish until tracked files and Git author metadata have been checked for
personal information.

## Managed repository settings

- Require pull requests for `main`.
- Require one or more approvals and dismiss stale approvals after new commits.
- Require linear history and the `checks` status check.
- Enable private vulnerability reporting.
- Restrict merge permissions to maintainers.
- Require issue and pull-request templates.

## Governance labels

- Area: `area:tally`, `area:dsc`, `area:documents`, `area:infra`,
  `area:security`
- Severity: `severity:p1`, `severity:p2`, `severity:p3`, `severity:p4`
- Type: `type:bug`, `type:feature`, `type:chore`, `type:rectify`

Each actionable bug receives one area, one severity, and one type label.

## Ongoing checks

- Keep [the roadmap](../step-by-step-roadmap.md) current.
- Use [the review checklist](../../review-checklist.md) in every pull request.
- Follow [rectification guidelines](../rectify-guidelines.md) for regressions.
- Validate platform-sensitive changes natively on Windows and macOS.
- Scan public artifacts for secrets, personal/customer data, usernames, home
  directories, and absolute local paths.
