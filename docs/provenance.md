# Source and asset provenance

This record covers material distributed from the public Bridge repository. It
avoids private repository URLs, developer-machine paths, and contributor
personal data.

## Code lineage and licensing boundary

- Bridge was extracted from a pre-existing AXAL desktop sync implementation
  maintained by the same project owner.
- Public commit `41485853a589c282187d3010b425e01ffe177e92`
  established the self-contained `v0.1.0` source release under MIT.
- Public commit `794260e83681c67be613fc76bbe0f35ad52d2454`
  established the Apache-2.0 source boundary for the `0.2.x` line. The
  repository owner authorized the relicensing through the managed repository
  change history.
- Protocol names such as AXAL remain identifiers for integration contracts;
  they do not imply a separate license for the public Bridge source.

The maintainer must retain any non-public source-revision mapping and rights
records needed to substantiate this public summary. Those records must not
contain customer data in release artifacts.

## Project-authored assets

- `src-tauri/app-icon.svg` is the project-authored source icon contributed by
  the repository owner under Apache-2.0.
- Raster, ICO, and ICNS files under `src-tauri/icons/` are generated derivatives
  of that vector source and carry the same project license.

Do not replace or add an icon, font, screenshot, fixture, or other asset unless
its source, contributor authority, license, and required attribution are added
to this record or `NOTICE`.

## Third-party material

- JavaScript production dependencies are locked by `pnpm-lock.yaml` and listed
  in `THIRD_PARTY_LICENSES.txt`.
- Rust dependencies are locked by `src-tauri/Cargo.lock`, governed by
  `src-tauri/about.toml`, and listed in `THIRD_PARTY_LICENSES_RUST.txt`.
- CI compares both locked release graphs to the checked-in reports and fails on
  missing components. The frontend report must also contain no stale entries.
  The Rust report may conservatively over-include a small number of crates
  because cargo-about evaluates configured target conditions as a union; the
  check names those entries in CI output.
- Required PKCS#11 attribution is preserved in `NOTICE` with an upstream source
  reference.

The Rust report is generated from the locked dependency graph for Windows x64
and macOS x64/Apple Silicon. Install the pinned generator and regenerate it
with:

```sh
cargo install --locked cargo-about --version 0.9.1 --features cli
corepack pnpm run license:generate:rust
corepack pnpm run license:all
```

`src-tauri/about.toml` is the reviewed license allowlist and target
configuration. Changes to that file or to either generated report require
review.

Proprietary vendor PKCS#11 libraries, private keys, PINs, certificate dumps,
customer files, and machine-specific configuration are not part of the public
repository or distributable bundle.
