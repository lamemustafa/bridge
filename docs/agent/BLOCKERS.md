# Goal 3 blocker — sealed runtime boundary

Date: 2026-09-04

Goal 3 requires the MCP `outstandings` tool to call the same native
outstandings pipeline used by the Tauri command. That pipeline is
`TallyRuntime::fetch_outstandings` in `src-tauri/src/tally/runtime.rs`; it
requires a `VerifiedCompanyIdentity`, whose only production constructor is
crate-private in `src-tauri/src/tally/mod.rs`.

The MCP binary currently compiles `src-tauri/src/agent.rs` as a module of the
separate `bridge_mcp` binary. It cannot construct that identity or use the
crate-private runtime boundary. Moving the agent into `bridge_lib`, exposing a
safe verified-identity constructor, or extracting the requested shared async
function would each modify one or more sealed files:

- `src-tauri/src/lib.rs`
- `src-tauri/src/tally/mod.rs`
- `src-tauri/src/tally/runtime.rs`
- `src-tauri/src/commands.rs` (if the command is changed to call an extracted
  helper)

All four appear in `docs/tally/compatibility/compatibility-surface.json` at
the current HEAD. Goal 1 explicitly says not to modify a listed file and to
stop and write this blocker when a sealed change is unavoidable.

The remaining Goal 3 requests depend on the same inaccessible runtime and
identity boundary for their verified source reads. Implementing them with the
agent's direct HTTP helper would bypass paired reads, response-bound identity
checks, and the established `Complete`/`Partial` contract, so it would not be
an acceptable substitute.

Required authorization to proceed: permit a compatibility-surface change and
the mandatory rehash/reseal workflow, or provide an existing unsealed
library-facing agent-read adapter that accepts an observed company tuple.
