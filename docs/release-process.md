# Release process

Bridge supports development and unsigned bundle smoke validation on Windows
and macOS. A smoke bundle is not a production release.

## Supported build baseline

- Source release line: `0.2.x` under Apache-2.0
- Node.js: supported 22 or 24 releases; CI uses `.node-version`
- pnpm: the exact `packageManager` version in `package.json`
- Rust: the exact channel and components in `rust-toolchain.toml`
- Hosts: current GitHub-hosted Windows and macOS runners plus native maintainer
  validation for vendor integrations

## Candidate gates

### Compatibility-surface reseal

Any dependency update that changes a pinned file (including `package.json`,
`src-tauri/Cargo.toml`, `src-tauri/Cargo.lock`, or either workflow) requires a
deliberate compatibility-surface reseal before the claim gate can pass. From
`tools`, run these three commands in order. `--output` asks the compatibility
tool to stage and replace the destination itself:

```bash
cargo run --locked -p bridge-tally-compatibility -- rehash-surface \
  ../docs/tally/compatibility/compatibility-surface.json .. \
  --output ../docs/tally/compatibility/compatibility-surface.json

cargo run --locked -p bridge-tally-compatibility -- seal-surface \
  ../docs/tally/compatibility/compatibility-surface.json \
  --output ../docs/tally/compatibility/compatibility-surface.json

cargo run --locked -p bridge-tally-compatibility -- repoint-matrix \
  ../docs/tally/compatibility/compatibility-matrix.json \
  ../docs/tally/compatibility/compatibility-surface.json \
  --output ../docs/tally/compatibility/compatibility-matrix.json
```

```powershell
cargo run --locked -p bridge-tally-compatibility -- rehash-surface `
  ../docs/tally/compatibility/compatibility-surface.json .. `
  --output ../docs/tally/compatibility/compatibility-surface.json

cargo run --locked -p bridge-tally-compatibility -- seal-surface `
  ../docs/tally/compatibility/compatibility-surface.json `
  --output ../docs/tally/compatibility/compatibility-surface.json

cargo run --locked -p bridge-tally-compatibility -- repoint-matrix `
  ../docs/tally/compatibility/compatibility-matrix.json `
  ../docs/tally/compatibility/compatibility-surface.json `
  --output ../docs/tally/compatibility/compatibility-matrix.json
```

`rehash-surface` reads the raw bytes of every existing pin and reports its
changed-entry count; it neither adds nor removes pins. `seal-surface` then
attests to the newly hashed manifest, and `repoint-matrix` updates the matrix
to that sealed digest. Do not run step 2 without step 1: sealing a manifest
whose file hashes are stale produces a valid-looking digest over stale source
content. CI intentionally checks the resulting surface but never reseals it.
Without `--output`, each command retains its stdout contract. With `--output`,
the tool writes raw UTF-8 without a BOM to a temporary file in the destination
directory and replaces the destination only after successful serialization.
This keeps failure fail-closed without relying on shell redirection or move
semantics.
On Unix, replacement preserves an existing destination's mode; a new
destination uses the normal `0666` mode subject to the process umask. Windows
uses its normal ACL semantics rather than POSIX mode bits.

#### Adding or removing a pin

The three commands above assume the surface is already valid and only the
*contents* of pinned files changed. Adding or removing an entry is different:
editing the file list invalidates `manifest_sha256` immediately, and
`rehash-surface` validates that checksum before it does anything. Run in the
documented order it fails with `surface_checksum_mismatch` and changes nothing.

So when the pin set itself changes, run `seal-surface` first to attest the new
file list, then run the ordinary three-command sequence in full.

This is the one case that inverts the standing rule against sealing before
rehashing. That rule exists because `seal-surface` never reads the repository,
so sealing stale hashes hides stale source under a fresh digest. Here the
concern does not apply: the first seal only re-attests a file list whose one
new digest was computed from disk, and the `rehash-surface` that follows
re-reads every pin, including the new one, before the second seal. Never stop
after that first seal.

Two further constraints apply:

- `MAX_SURFACE_FILES` caps the pin count, and `RESERVED_SURFACE_FILES` bounds
  how far the cap may exceed it. When the surface is at its cap, adding a pin
  requires raising the constant, which the constant's own comment calls an
  explicit compatibility-surface decision -- record the reason in the commit.
- The tool pins its own source, so editing `tools/bridge-tally-compatibility`
  to raise that cap stales its digest and needs another reseal after the edit.
  Expect two passes, and run the tool's tests between them: a cap change can
  invalidate a test that hard-codes the old bound.

The PowerShell commands are intended for Windows PowerShell 5.1 and PowerShell
7+. They deliberately do not use `>`: Windows PowerShell 5.1 redirection was
measured to produce UTF-16LE. The output-path procedure is reasoned from the
tool's byte writer, not host-verified; before relying on it on a Windows host,
confirm the result with `Format-Hex` and require no UTF-8 BOM (`EF BB BF`).

Before cutting a candidate, regenerate and verify the Rust third-party notice
with the pinned generator:

```sh
cargo install --locked cargo-about --version 0.9.1 --features cli
corepack pnpm run license:generate:rust
corepack pnpm run license:all
```

1. Update `package.json`, `src-tauri/Cargo.toml`, and
   `src-tauri/tauri.conf.json` to the same version.
2. Move completed changelog entries into that version and retain the MIT notice
   for historical `v0.1.0`.
3. Run `corepack pnpm install --frozen-lockfile` and
   `corepack pnpm run license:all`.
4. Run frontend build, Rust format/check/test/Clippy, and native Tauri bundle
   builds on Windows and macOS.
5. Confirm the bundle-smoke jobs pass their automated MSI, NSIS, staged macOS
   app, and mounted DMG inspections for `LICENSE`, `NOTICE`,
   `THIRD_PARTY_LICENSES.txt`, and `THIRD_PARTY_LICENSES_RUST.txt`; manually
   inspect signed candidates again before publication.
6. Exercise Tally, DSC, documents, sync, and persistence using synthetic data;
   attach redacted evidence to the release PR.
   Keep repository-synthetic parser qualification receipts separate from the
   live Tally compatibility matrix: they cannot establish a product release,
   Education/licensed mode, HTTP/runtime behavior, or a performance budget.
   Do not broaden a Tally support claim without current exact-profile live
   evidence; state missing matrix evidence in the release notes.
   Run the executable Tally claim gate:

   ```sh
   cd tools
   cargo run --locked -p bridge-tally-compatibility -- gate \
     ../docs/tally/compatibility/compatibility-matrix.json \
     ../docs/tally/compatibility/compatibility-surface.json \
     ../docs/tally/compatibility/trusted-evidence-keys.json \
     ../docs/tally/compatibility/evidence ..
   ```

   Any positive cell without an exact fresh signed receipt is a release
   blocker. Unknown cells remain visible limitations; they are not failures
   unless release notes or product copy claim that scope.
   Live Education collection, when legitimately available, must follow the
   [read-only runbook](./tally/compatibility/live-education-runbook.md). Never
   upload `.bridge-live/` automatically or substitute the parser-only CI
   receipt for reviewed live evidence.
7. Confirm the release commit and tag contain no PII, machine paths, secrets,
   or unsigned third-party assets.
8. Confirm the `Dependency security` workflow passes and GitHub reports no open
   Dependabot or secret-scanning alerts.

## Signing and publication

- Windows production installers require an organization-controlled code-signing
  certificate and timestamp service.
- macOS production bundles require an organization-controlled Developer ID,
  hardened runtime, notarization, and stapling.
- Signing credentials belong in protected release environments with required
  reviewers; never place them in repository variables, logs, or artifacts.
- Publish SHA-256 checksums and provenance/attestation evidence with every
  downloadable artifact.
- Do not create or move a `v*` tag until signed artifacts from both supported
  platforms pass the candidate gates. Release tags must be immutable.

The repository intentionally does not auto-publish unsigned `v*` tag
artifacts. CI bundle jobs produce short-lived smoke evidence only until signing
and notarization ownership is configured. The macOS smoke build sets
`APPLE_SIGNING_IDENTITY=-` for that job so Tauri ad-hoc signs the assembled app
before creating the DMG. The staged app and the app inside the mounted DMG must
both pass `codesign --verify --deep --strict`; a mutation check confirms that
changing a copied app's legal resource fails verification. Ad-hoc signing seals
bundle integrity but supplies no verified publisher identity, Developer ID,
notarization or Gatekeeper approval. These artifacts remain previews without
publisher signing. Production signing defaults and the MCPB publication lane
are unchanged.

## MCPB previews and the install page

`.github/workflows/release-mcpb-preview.yml` is a manually dispatched,
two-platform preview lane. It produces the actual Windows x64 and macOS arm64
MCPB archives, validates and launches each archive without contacting Tally,
then publishes a durable **GitHub prerelease** only when every archive, checksum,
payload-free smoke result, and source-provenance record is present. Preview tags
must start with `mcp-preview-`; they cannot reuse a production `v*` tag. A
preview is unsigned and must never be described as signed, notarized, or ready
for production use.

This workflow intentionally has no production-signed channel. A raw MCPB
archive is not a notarization-and-stapling carrier for the enclosed macOS
binary. Before adding one, maintainers need a separately reviewed signed
distribution design, an organization-controlled Developer ID, notarization
credentials, a timestamped Windows signing certificate, protected release
environments, and host validation of the complete shipped carriers. Self-signed
certificates and OS-warning bypass instructions are not acceptable substitutes.

`site/` is a small static installer page. Its workflow is manual so publishing
it remains an explicit maintainer action. Once GitHub Pages is configured for
this repository, it resolves GitHub Release assets by exact release tag and
labels prerelease downloads as unsigned previews. It does not proxy Tally,
create an account, or run a cloud relay.

## Rollback

1. Mark the affected GitHub release as withdrawn and remove unsafe downloadable
   artifacts without moving or reusing its tag.
2. Publish a security advisory when coordinated disclosure is required.
3. Revert or rectify the source change through a pull request with migration
   compatibility notes.
4. Cut a new patch version; never replace an already published artifact under
   the same version or checksum.
5. Preserve release notes explaining impact, upgrade/rollback steps, and the
   last known-good version without including customer data.
