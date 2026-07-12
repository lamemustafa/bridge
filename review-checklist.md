# Review checklist

Link at least one completed checklist item in every pull request. Check every
item relevant to the change and mark non-applicable sections explicitly.

## Functional checks

- [ ] Repository identity and command entry points remain `bridge`-scoped.
- [ ] New or changed Tally calls use the serial queue path.
- [ ] Errors are actionable without exposing sensitive values.
- [ ] Database changes include migration compatibility and rollback notes.
- [ ] DSC operations validate token and certificate data without disclosing it.
- [ ] New or changed commands validate inputs and surface user-safe errors.
- [ ] AXAL protocol changes include a contract-level regression command or test.

## Privacy and security checks

- [ ] No secrets, credentials, certificate data, personal data, customer data,
  local usernames, or absolute machine paths appear in source, fixtures, logs,
  screenshots, documentation, or Git metadata intended for publication.
- [ ] Endpoints and command handlers enforce explicit input validation.
- [ ] File and library paths are repository-relative, app-data-relative, or
  user-selected; no path assumes a developer machine or operating system.
- [ ] Security-sensitive changes include a security-focused reviewer sign-off.
- [ ] DSC, Tally, credential, endpoint, and document changes include explicit
  security impact notes.

## Cross-platform checks

- [ ] Shared build and check commands pass without shell-specific syntax.
- [ ] Platform-specific behavior uses supported OS APIs and portable path joins.
- [ ] Native Windows validation evidence is attached when affected.
- [ ] Native macOS validation evidence is attached when affected.
- [ ] Missing platform evidence is documented as a release blocker or known gap.

## Packaging and release checks

- [ ] Changed entry points are covered by build and packaging commands.
- [ ] Frontend, backend, binary, bundle, and product names are consistent.
- [ ] CI is updated when commands, supported hosts, or publish logic change.
- [ ] Existing workflow changes include rollback and migration compatibility.
- [ ] Repository setup changes update
  [docs/bootstrap/managed-git.md](./docs/bootstrap/managed-git.md).
