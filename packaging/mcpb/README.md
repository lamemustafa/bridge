# MCPB assembly

Build the host `bridge_mcp` binary before assembling an MCPB archive:

```sh
node scripts/package-mcpb.mjs
```

The committed `manifest.json` is a template listing every supported platform; it is not an archive manifest. The command builds the host `bridge_mcp` binary, then creates `packaging/mcpb/stage/` with a generated manifest that advertises only the current host's operating system and architecture. Its binary is staged at `bin/<target-triple>/bridge_mcp` (or `.exe` on Windows), alongside `LICENSE`, `NOTICE`, `THIRD_PARTY_LICENSES.txt`, and `THIRD_PARTY_LICENSES_RUST.txt`.

The verifier rejects any staged manifest entry that has no matching binary, so each archive must be assembled from that host-specific `stage/` directory. The binary and staged resources are ignored local build outputs and must not be committed. Run the script independently on each supported host before creating its archive.
