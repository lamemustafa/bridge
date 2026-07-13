# Bridge bootstrap analysis summary

Bridge was extracted from a pre-existing desktop sync implementation into an
independent Tauri repository. This record avoids developer-machine paths and
private workspace details.

Public code, asset, and licensing boundaries are recorded in
[provenance.md](./provenance.md).

## Reused architecture

- React and TypeScript frontend workflow wiring
- Rust command handlers for Tally session and data operations
- DSC token probing and certificate-summary extraction
- Document scanning and upload orchestration
- Local schema, sync, and Tauri capability structure

## Identity decisions

- Runtime package, crate, binary, bundle, and visible product names use Bridge.
- Protocol terms such as `axal` remain where they are part of an integration
  contract and changing them would alter behavior.
- Checkout, asset, and tool paths must resolve from the repository or a standard
  application-data location, never from a contributor's home directory.

## Readiness requirements

- Frontend and Rust compile checks pass.
- Desktop development and package builds run from an arbitrary clone path.
- Native Windows and macOS builds are both validated.
- Vendor PKCS#11 dependencies are documented and selected per host.
- Governance templates, review controls, and private vulnerability reporting
  are configured in the managed repository.
- Tracked content and publication history contain no secrets or personal or
  customer data.

Readiness is established by current evidence in pull requests and CI, not by
this historical summary alone.
