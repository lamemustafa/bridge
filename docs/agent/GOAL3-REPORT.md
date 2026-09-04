# Goal 3b report

Completed on `feat/agent-connector`.

## Delivered

- Moved the MCP implementation into `bridge_lib::agent`; the binary is now a
  thin stdio wrapper.
- Exposed only the safe `VerifiedCompanyIdentity::from_observed_companies`
  constructor. It still rejects missing tuples, duplicates, and
  presentation-equivalent display scopes.
- `outstandings` calls `TallyRuntime::fetch_outstandings`, retaining the
  paired native bills/group/ledger pipeline, INR admission, exact partial
  reason, exact totals, ageing basis/buckets, open bills, top parties, and
  unallocated exposure.
- `ledger_movement` consumes the runtime's literal-window voucher entries and
  exact decimal arithmetic; `ledger_masters` reuses the paired party-master
  source for compliance fields. `changed_since` now returns its actual egress
  row count, master changes, and observed high-water fields when exposed.
- `tools/list` now publishes concrete JSON schemas, enums, defaults, required
  fields, and evidence-oriented descriptions. README examples document the
  actual envelope and safety boundary.

## Sealed surface

Changed sealed source files: `src-tauri/src/lib.rs` (exports the library
module), `src-tauri/src/tally/mod.rs` (validated public constructor), and
`src-tauri/src/tally/runtime.rs` (paired party-master adapter). The Tauri
command file was not changed. `agent.rs` and `agent_import.rs` changed as the
MCP implementation, but are not listed by the authorization's sealed-file
set.

Before reseal: 163 pinned files. `rehash-surface` reported 5 changed manifest
entries. Three are the authorized source changes above; the remaining two are
the already-stale `src-tauri/Cargo.toml` and `src-tauri/Cargo.lock` pins (their
working-tree bytes were unchanged by this goal). After reseal: 163 pinned
files, with a newly sealed surface digest and matrix pointer. All matrix cells
remain `unknown`; resealing makes no public compatibility or live-Tally claim.

## Verification

```text
cargo check --manifest-path src-tauri/Cargo.toml --bin bridge_mcp
Finished dev profile ...

cargo test --manifest-path src-tauri/Cargo.toml --lib agent::tests
2 passed; 0 failed

cargo test --manifest-path src-tauri/Cargo.toml --lib commands::tests
28 passed; 0 failed

cargo clippy --manifest-path src-tauri/Cargo.toml --bin bridge_mcp -- -D warnings
Finished dev profile

corepack pnpm test
103 Node tests, 6 Vitest tests, and 2 Playwright tests passed

compatibility gate
compatibility_gate_passed:unknown_claims=11:evidenced_claims=0
```

Remaining evidence gap: no live Tally or Windows/macOS host validation was
performed. The simulator covers the MCP envelope, typed-down endpoint, egress,
and redaction; it cannot qualify a Tally release or a compatibility-matrix
cell.
