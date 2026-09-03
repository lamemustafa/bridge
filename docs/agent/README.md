# Bridge MCP (read-only)

`bridge_mcp` is Bridge's newline-delimited JSON-RPC 2.0 MCP server. It only
uses Bridge's loopback-only Tally XML transport; it never constructs an import
or write request.

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

The tools are `tally_status`, `list_companies`, `outstandings`,
`ledger_masters`, `ledger_movement`, `vouchers`, `changed_since`,
`read_evidence`, and `egress_log`. Each call returns compact JSON with the
company identity where scoped, a read timestamp, request/response commitments,
byte count, completeness reason, and truncation state. Every call appends a
metadata-only receipt to `agent-egress.jsonl`; receipt lines never contain
voucher bodies. Redaction happens before a result reaches the client.

Safety boundary: read-only, local loopback only, bounded responses, verified
company tuple selection, append-only egress receipts. Unsupported: Tally Cloud
Access and every non-loopback Tally host. `changed_since` also explicitly does
not claim deletion detection from AlterID alone.
