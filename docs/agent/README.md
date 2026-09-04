# Bridge MCP

`bridge_mcp` is Bridge's newline-delimited JSON-RPC 2.0 MCP server. It uses
Bridge's loopback-only Tally XML transport for reads. It can render a local
voucher import file, but it never sends that file—or any import request—to
Tally.

Build and run it with Rust 1.96:

```sh
rustup run 1.96.0-aarch64-apple-darwin cargo run --manifest-path src-tauri/Cargo.toml --bin bridge_mcp
```

Configure it with `BRIDGE_TALLY_HOST` (default `localhost`),
`BRIDGE_TALLY_PORT` (default `9000`), `BRIDGE_AGENT_DATA_DIR` (Bridge's
platform application-data directory by default), `BRIDGE_AGENT_MAX_ROWS`
(default `500`), `BRIDGE_AGENT_MAX_BYTES` (default `200000`), and
`BRIDGE_AGENT_REDACTION` (`none`, `mask_parties`, or `drop_narration`). The
host is validated by `bridge-tally-transport`; non-loopback hosts are refused.

Claude Desktop example:

```json
{
  "mcpServers": {
    "bridge-tally": {
      "command": "/absolute/path/to/bridge_mcp",
      "env": {"BRIDGE_TALLY_HOST": "localhost", "BRIDGE_TALLY_PORT": "9000"}
    }
  }
}
```

Cursor uses the same server object in `.cursor/mcp.json`:

```json
{"mcpServers":{"bridge-tally":{"command":"/absolute/path/to/bridge_mcp"}}}
```

The read tools are `tally_status`, `list_companies`, `outstandings`,
`ledger_masters`, `ledger_movement`, `vouchers`, `changed_since`,
`read_evidence`, and `egress_log`. Each call returns compact JSON with the
company identity where scoped, a read timestamp, request/response commitments,
byte count, completeness reason, and truncation state. Every call appends a
metadata-only receipt to `agent-egress.jsonl`; receipt lines never contain
voucher bodies. Redaction happens before a result reaches the client.

## Voucher-file loop (manual Tally import only)

1. Call `voucher_schema` and produce a payload matching its schema. Transaction
   IDs are client-supplied, unique, and retained in the local import ledger.
2. Call `validate_masters` with every ledger name. Correct every `near_miss`
   with the exact live spelling; Bridge never creates masters.
3. Call `build_import_xml` with the payload. It checks exact decimal balance,
   company date extent, live masters, and previously built transaction IDs,
   then writes `<data_dir>/imports/<batch_id>.xml` and records an append-only
   `agent-import-ledger.jsonl` line.
4. In Tally, with the intended company open, use **Gateway of Tally → Import →
   Vouchers** to import the file. Bridge does not dispatch this step.
5. Call `verify_import` with the company GUID and batch ID. It reads the date
   window back, compares the exact signed ledger entries, reports missing or
   divergent rows and duplicates, writes `.proof.json` and `.proof.md`, and
   appends the verification status to the local import ledger.

The file path is deliberately not a direct-posting path. Masters must already
exist and match exactly. On the observed Education-mode Tally profile, voucher
dates were only accepted for day 1, 2, or 31; Bridge does not infer a connected
installation's licence mode, so an accountant must account for that restriction
before manual import.

Safety boundary: local loopback only, bounded responses, verified company tuple
selection, append-only receipts, and no agent import dispatch. Unsupported:
Tally Cloud Access and every non-loopback Tally host. `changed_since` also
explicitly does not claim deletion detection from AlterID alone. A
`posted_verified` result is a readback comparison of the selected date window,
not a claim that every Tally configuration or licence mode has been qualified.
