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

## Voucher-file loop (manual Tally import only; disabled by default)

`build_import_xml` and `verify_import` are hidden unless the operator sets
`BRIDGE_AGENT_ENABLE_IMPORT=1`. No live-Tally import/read-back evidence is
recorded yet, so enabling this local planning path returns
`live_evidence: "none_recorded"` and links to `docs/agent/GOAL2-REPORT.md`.
It must not be described as live-verified.

1. Call `voucher_schema` and produce a payload matching its schema. Transaction
   IDs are client-supplied, unique, and retained in the local import ledger.
2. Call `validate_masters` with every ledger name. Correct every `near_miss`
   with the exact live spelling; Bridge never creates masters.
3. Call `build_import_xml` with the payload. It checks exact decimal balance,
   company date extent, live masters, and previously built transaction IDs,
   then writes `<data_dir>/imports/<batch_id>.xml` and records an append-only
   `agent-import-ledger.jsonl` line. `voucher_number` is optional: when absent,
   Tally applies the voucher type's own numbering configuration; when supplied,
   it is validated and sent so a Manual-type duplicate policy can reject it.
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
not live-Tally qualification or a claim that every Tally configuration or
licence mode has been qualified.

## Evidence-shaped outputs

Every tool response carries the same outer evidence envelope. This is an
illustrative response shape from the synthetic simulator test; it is not a
live-Tally compatibility claim:

```json
{
  "company": {"name":"BRIDGE SYNTHETIC BOOK","guid":"00000000-0000-4000-8000-000000000001","identity_state":"verified_tuple"},
  "read_at":"2026-09-04T00:00:00.000Z",
  "evidence":{"request_sha256":"…","response_sha256":"…","bytes":123,"state":"complete"},
  "truncated":false,
  "result":{"companies":[{"name":"BRIDGE SYNTHETIC BOOK","guid":"00000000-0000-4000-8000-000000000001"}]}
}
```

`outstandings` returns the runtime's paired native result. A complete read has
exact totals, four ageing buckets, top parties, open bills, and unallocated
amount/count; a refused runtime read instead has `state: "partial"` and its
exact `partial_reason`. `ledger_movement` returns literal-window voucher
movement with exact decimal `opening`, `debit`, `credit`, `closing`, parent,
and `vouchers_touching`. `ledger_masters` accepts `fields: "compliance"` to
return the paired party-master GSTIN/PAN/MSME/bank/IFSC/email/phone/state and
address observations; `mask_parties` redacts the ledger name before it leaves
the server. `changed_since` returns changed voucher/master records, the
observed company `ALTVCHID`/`ALTMSTID` when exposed, and always states that
deletion detection is unsupported.
