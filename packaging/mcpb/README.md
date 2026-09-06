# MCPB assembly

Build the host `bridge_mcp` binary before assembling an MCPB archive:

```sh
node scripts/package-mcpb.mjs
npx --yes @anthropic-ai/mcpb@2.1.2 validate packaging/mcpb/stage/manifest.json
npx --yes @anthropic-ai/mcpb@2.1.2 pack packaging/mcpb/stage src-tauri/target/release/bridge-tally.mcpb
npx --yes @anthropic-ai/mcpb@2.1.2 info src-tauri/target/release/bridge-tally.mcpb
```

The committed `manifest.json` is a schema-valid template; it is not an archive manifest. The command builds the host `bridge_mcp` release binary with the locked dependencies, then replaces `packaging/mcpb/stage/` with a clean host-specific stage. Its binary is staged at `bin/<target-triple>/bridge_mcp` (or `.exe` on Windows), alongside `LICENSE`, `NOTICE`, `THIRD_PARTY_LICENSES.txt`, and `THIRD_PARTY_LICENSES_RUST.txt`.

The generated manifest uses the [official MCPB schema](https://github.com/anthropics/mcpb/blob/main/MANIFEST.md): a string entry point, an explicit launch command, environment substitutions for user settings, and an operating-system compatibility declaration. MCPB 0.1 has no architecture compatibility field. Build and distribute separate archives for macOS arm64, macOS x64, and Windows x64; identify the architecture in each distributed filename and select the archive matching the client host.

The verifier requires a matching binary, launch command, operating-system declaration, and legal resources. The official CLI validates the complete manifest and creates the archive from the host-specific `stage/` directory. The binary, archive, and staged resources are ignored local build outputs and must not be committed. Run the commands independently on each supported host. CI packages Windows and the hosted macOS runner architecture; schema validation and packaging do not establish installation or live Tally interoperability in an MCP client.
