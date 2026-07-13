# Bridge implementation roadmap

## Completed repository baseline

- Independent React, Rust, and Tauri project structure
- Bridge package, crate, binary, and UI identity
- Contributor, security, review, issue, and pull-request guidance
- Managed Git and rectification playbooks

## Current hardening

1. Keep frontend and Rust checks green.
2. Remove developer-machine paths and personal/customer data from tracked
   content and publication history.
3. Make build assets and runtime discovery repository-relative or
   application-data-relative.
4. Validate development and packaging on native Windows and macOS hosts.
5. Add smoke and regression coverage for Tally, DSC, documents, sync, and local
   persistence.
6. Record platform-specific vendor dependencies without committing proprietary
   libraries, private keys, PINs, or certificate dumps.

## Managed repository controls

1. Require pull requests and the required checks on `master`.
2. Configure the area, severity, and type labels defined in
   [managed Git guidance](./bootstrap/managed-git.md).
3. Enable private vulnerability reporting.
4. Require [review-checklist.md](../review-checklist.md) links in pull requests.
5. Use [rectification guidelines](./rectify-guidelines.md) for regressions.

## Open-source operating model

1. Publish versioned release notes and a support matrix.
2. Define the Node, pnpm, Rust toolchain, and Rust minimum-supported-version
   policy.
3. Run formatting, lint, test, build, and platform packaging checks in CI.
4. Document release signing and rollback procedures for Windows and macOS.
5. Review external governance conventions before adopting them and record
   exact mappings, exceptions, and ownership in public project documentation.

## Completion criteria

- A fresh clone builds without paths outside the repository except standard
  toolchain and application-data locations.
- Native Windows and macOS development and package builds have current evidence.
- Tally, DSC, document, sync, and persistence workflows have regression checks.
- No public artifact contains secrets, personal/customer data, or contributor
  machine paths.
- Governance and rectification controls are enforced in the managed repository.
