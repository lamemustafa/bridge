# Bridge

Bridge is an open-source Tauri desktop application for AXAL local-edge sync
workflows. It combines a React/TypeScript interface with a Rust backend for
Tally, digital-signature certificate (DSC), document, sync, and local database
operations.

The repository is self-contained: build and development commands resolve files
relative to the clone, not to a developer-specific directory.

## Supported development hosts

Bridge is intended to build and run on Windows and macOS. Run platform checks
on a native host for each operating system; a successful build on one operating
system does not verify the other.

Shared prerequisites:

- Node.js 22 or 24 and Corepack (`.node-version` pins the CI baseline)
- the Rust toolchain pinned by `rust-toolchain.toml`
- the operating-system dependencies listed in the
  [Tauri prerequisites](https://v2.tauri.app/start/prerequisites/)

On Windows, install the Microsoft C++ build tools and WebView2 components
described by Tauri. On macOS, install Xcode Command Line Tools. DSC workflows
also require a vendor PKCS#11 library compatible with the host operating
system; never commit a private key, PIN, certificate dump, or locally installed
vendor library.

## Quick start

Run these commands from the repository root in PowerShell, Command Prompt, or a
POSIX-compatible shell:

```text
corepack pnpm install --frozen-lockfile
corepack pnpm run build
corepack pnpm run cargo:check
corepack pnpm run tauri:dev
```

`tauri:dev` starts the Vite development server and desktop application. It does
not require a fixed checkout location. The first Rust build can take several
minutes.

For a release build, run `corepack pnpm run tauri:build` on each target host.
CI-produced bundles are unsigned smoke artifacts only. Do not redistribute a
desktop installer until the signing, notarization, provenance, and rollback
gates in [the release runbook](./docs/release-process.md) are complete.

## Platform verification

Before claiming support for a platform, run the following on that platform:

```text
corepack pnpm install --frozen-lockfile
corepack pnpm run build
corepack pnpm run cargo:check
corepack pnpm run tauri:build
```

Also manually exercise the affected Tally, DSC, document, and sync workflows.
Vendor integrations may require host-specific software even though repository
paths and project commands are portable.

## Privacy and safe diagnostics

Do not commit or attach real customer, company, tax, certificate, credential,
financial, or document data. Before sharing logs, screenshots, fixtures, or
reproduction steps, replace personal and customer data with synthetic values
and remove local usernames and absolute paths. See [SECURITY.md](./SECURITY.md)
for private reporting and handling requirements.

## Repository map

- `src/` - React UI and API bindings
- `src-tauri/` - Rust core and Tauri configuration
- `docs/` - architecture, roadmap, and operational guidance
- `.github/` - issue and pull-request templates plus CI configuration

## Governance

- [Agent responsibilities](./AGENTS.md)
- [Contributor guide](./CONTRIBUTING.md)
- [Review checklist](./review-checklist.md)
- [Security policy](./SECURITY.md)
- [Rectification guidelines](./docs/rectify-guidelines.md)
- [Roadmap](./docs/step-by-step-roadmap.md)
- [Managed Git guidance](./docs/bootstrap/managed-git.md)
- [Source and asset provenance](./docs/provenance.md)
- [Release process](./docs/release-process.md)

## License

Bridge is licensed under the [Apache License, Version 2.0](./LICENSE).
Attribution notices are provided in [NOTICE](./NOTICE).
The historical `v0.1.0` release remains under the MIT license shipped with
that tag; current development source is version `0.2.0` under Apache-2.0.
