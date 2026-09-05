# Goal 1 report — Bridge MCP

## Delivered

- `src-tauri/src/agent.rs`: newline-delimited JSON-RPC MCP handler, the nine
  read-only tool names, loopback transport use, bounded responses, identity
  lookup, evidence memory, redaction, and append-only egress receipts.
- `src-tauri/src/bin/bridge_mcp.rs`: the standalone executable entry point.
- `docs/agent/README.md` and `packaging/mcpb/manifest.json`: local-run,
  Claude Desktop/Cursor, configuration, and package metadata.

`bridge_mcp` accepts `initialize` (including both requested protocol versions),
`notifications/initialized`, `ping`, `tools/list`, and `tools/call`.

## Verification

```text
cargo test --bin bridge_mcp
running 2 tests
... literal FILTERS + redaction ... ok
... simulator company read + down endpoint receipt ... ok
test result: ok. 2 passed; 0 failed

cargo clippy --bin bridge_mcp -- -D warnings
Finished ...

cargo check --locked --workspace
Finished ...
```

All Rust commands above used `rustup run 1.96.0-aarch64-apple-darwin`; the
plain shell `cargo` is Rust 1.95 and cannot satisfy this workspace's declared
Rust 1.96 minimum. `corepack pnpm run cargo:check` therefore remains an
environmental failure under that shell, while its equivalent Rust-1.96 cargo
check passes.

## Honest gaps

No live Tally or Windows build was run. The simulator test validates the
company/read receipt path and a typed endpoint-down result; it does not yet
exercise every full Tally profile. `outstandings` returns the verified native
bill rows but deliberately marks totals/ageing/unallocated as partial until
the complete native ledger-residual pairing is wired. `ledger_masters` exposes
the ordinary profile; the compliance-only source remains explicitly
unsupported. `changed_since` uses the documented AlterID server filter but
does not claim deletion detection or company high-water values until their
dedicated profile is qualified.

Run locally:

```sh
rustup run 1.96.0-aarch64-apple-darwin cargo test --manifest-path src-tauri/Cargo.toml --bin bridge_mcp
rustup run 1.96.0-aarch64-apple-darwin cargo clippy --manifest-path src-tauri/Cargo.toml --bin bridge_mcp -- -D warnings
rustup run 1.96.0-aarch64-apple-darwin cargo run --manifest-path src-tauri/Cargo.toml --bin bridge_mcp
```
