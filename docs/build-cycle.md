# Build and review cycle

Use a stable, isolated checkout for verification. Keep the normal development
checkout and its persistent Cargo target separate from release and evidence
builds. Record a commit or tree identity with each result; a passing check on a
moving checkout is not a reproducible result.

## Local iteration

Cargo already enables incremental compilation for development and test profiles.
Do not carry a historical `CARGO_INCREMENTAL=0` setting into ordinary local
iteration. Keep the repository's line-table debug information locally. CI and
compiler-cache experiments have different cache requirements.

On an 8-core Apple M3 with 16 GiB RAM and Rust 1.96.0, compile-only workspace
measurements at `bd8d594` (the same tree as `8f1aaba`) gave:

| Profile | Incremental | One-file rebuild median | Range | Samples |
| --- | --- | ---: | ---: | ---: |
| Line tables | Off | 26.345 s | 25.732–26.496 s | 3 |
| No debug information | Off | 20.409 s | 20.138–20.429 s | 3 |
| Line tables | On | 7.001 s | 6.811–7.531 s | 3 |

The command was `cargo test --locked --manifest-path src-tauri/Cargo.toml
--workspace --no-fail-fast --no-run --timings`. Each variant used its own target
and explicit `CARGO_PROFILE_DEV_DEBUG`, `CARGO_PROFILE_TEST_DEBUG`, and
`CARGO_INCREMENTAL` values. The one-file change appended whitespace to the crate
root; original bytes were restored afterward. These are sequential observations
on a shared Mac, not a prediction for another machine or a runtime speed claim.
Preparation states differed and are not a valid cold-build comparison. The first
repeated build sometimes recompiled Bridge; subsequent unchanged builds were
sub-second once settled. Do not omit that first-repeat cost when measuring a real workflow.

A separate fresh-target pair (one sample per profile) took 119.925 s with line
tables and 111.733 s without debug information. Both started with empty targets
and a populated registry. Load varied; this small sample does not establish a
portable cold-build percentage improvement.

## CI compilation and packaging

Native CI omits debug information only in its development/test profiles. Local
profiles, debug assertions, overflow checks, release optimization, Windows/macOS
test selection, and warning-denying Clippy remain unchanged. The diagnostic
tradeoff is less source detail in CI stack traces. Cargo timing HTML is retained
as `native-compiler-timings-*` artifacts, including when a later step fails.

The pinned Tauri CLI builds all binaries with `--bins` and enables
`tauri/custom-protocol`. CI stages the resulting `bridge_mcp` / `bridge_mcp.exe`
through the existing `package-mcpb.mjs --binary` path immediately after the Tauri
build and installer checks. A failed Tauri build stops staging. The standalone
packaging command still builds its own release binary when `--binary` is absent.

The prior [successful source run](https://github.com/lamemustafa/bridge/actions/runs/34199125671)
spent 6m37s in macOS MCP staging after the Tauri build, recompiling native
libraries and Tauri in a different feature/environment configuration. Reusing
the same job's output removes that second build invocation. Preserve the
installer legal-resource checks, official MCPB schema validation, archive
extraction, offline stdio execution, both posting settings, and copied legal
bytes when assessing the change. A ZIP listing alone is insufficient evidence.

Windows native target caching remains disabled because its previous multi-GB
save could exceed the job timeout after checks passed. Registry caching stays
enabled. Do not reverse that policy without a measured save/restore experiment.
The pinned rust-cache action hashes `CARGO*` environment values, so changing CI
profiles invalidates its compiler cache as expected.

## Compare complete measurements

Record runner/toolchain, exact source, selected features, cold/warm state,
compilation and test-execution time, cache hit/bytes/restore/save cost, and total
job time. Sum job durations only to estimate consumed runner time; parallel jobs
must not be summed as user wait time. Retain raw timings, report sample counts,
and avoid a percentile claim from a handful of runs.

The retained baseline had a 39m13s Windows native job and 75 summed job minutes.
The [first profile experiment](https://github.com/lamemustafa/bridge/actions/runs/34207728015)
passed the same 778 Windows / 786 macOS library tests and all remaining checks.
Windows took 29m00s, including a 21m23s main test step and 2m03s main Clippy step
(the baseline Clippy step took 10m33s). Total summed job time was 72m46s. This is
one hosted sample, not a stable percentage forecast; cache states and packaging
costs differed across the runs. The macOS profile cache was cold and saved
693,838,529 compressed bytes in a 37-second post-cache step. Windows retained
only registry/tool caching (167,987,643 compressed bytes; 44-second post-cache
step), not its target tree.
The [current dependency candidate](https://github.com/lamemustafa/bridge/actions/runs/34208820274),
with the same application sources and lockfile as the profile experiment, took
10m49s in Windows main Clippy with line tables. This second baseline supports the
observed compilation saving; it does not make unrelated cache states comparable.

The old Windows Clippy log rebuilt `openssl-sys`, `aws-lc-sys` and many build
dependencies. The profile experiment's Clippy log did not repeat those builds;
its timing report is dominated by Rust checking. This is evidence of avoided
compilation in that run, rather than removed tests or lint targets.

Its macOS native cache was warm. A cold run of another profile cannot establish
a speedup or regression against that warm result. Native C build scripts and
linking also limit what a Rust compiler cache can save.

Run licence and compatibility checks before an expensive validation cycle.
When pinned bytes legitimately change, use the existing deliberate
[compatibility reseal procedure](release-process.md#compatibility-surface-reseal);
never remove pins or promote unknown claims to repair a dependency PR. Review
related findings together, fix supported defects, and verify one final candidate.
Do not restart unchanged native checks merely to chase an empty review queue.

## Primary references

- [Cargo profiles](https://doc.rust-lang.org/cargo/reference/profiles.html)
- [Cargo timings](https://doc.rust-lang.org/cargo/reference/timings.html)
- [Tauri 2.11.4 binary/feature selection](https://github.com/tauri-apps/tauri/blob/tauri-cli-v2.11.4/crates/tauri-cli/src/interface/rust.rs)
- [Tauri upstream CI profile](https://github.com/tauri-apps/tauri/blob/e19121427332c8a14165999251cce415707a62fc/.github/workflows/lint-rust.yml)
- [Pinned rust-cache configuration](https://github.com/Swatinem/rust-cache/blob/6323deb102c322ba6fcbdcafc7e3dddab59af2b6/src/config.ts)
- [sccache Rust limits](https://github.com/mozilla/sccache/blob/05aafc82b9311edba4747a4656c106512ff181e8/docs/Rust.md)

Upstream workflows are design comparisons, not benchmark evidence for Bridge.
No new cache service, runner purchase, test-runner migration or reduction in
platform coverage is required by this change.
