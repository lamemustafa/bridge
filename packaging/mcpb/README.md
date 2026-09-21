# MCPB assembly

Fetch the pinned PDFium build, then build the host `bridge_mcp` binary and assemble an MCPB archive:

```sh
python3 scripts/fetch-pdfium.py --platform macos-arm64 --dest /tmp/mcpb-pdfium   # windows-x64 on Windows
node scripts/package-mcpb.mjs --pdfium /tmp/mcpb-pdfium
npx --yes @anthropic-ai/mcpb@2.1.2 validate packaging/mcpb/stage/manifest.json
npx --yes @anthropic-ai/mcpb@2.1.2 pack packaging/mcpb/stage src-tauri/target/release/bridge-tally.mcpb
npx --yes @anthropic-ai/mcpb@2.1.2 info src-tauri/target/release/bridge-tally.mcpb
python3 scripts/check-mcpb-bundle.py src-tauri/target/release/bridge-tally.mcpb
```

When a reviewed release binary is already available, stage that binary without
building Rust again. This is useful for archive-only verification on the same
host architecture:

```sh
node scripts/package-mcpb.mjs --binary /path/to/bridge_mcp --pdfium /tmp/mcpb-pdfium
```

Staging refuses to run without `--pdfium`: the manifest always enables imports, so every archive lists `parse_bank_statement`, and without the library every call would fail. `fetch-pdfium.py` checks the archive, the extracted library and the licence notice it writes against `packaging/pdfium/pdfium.lock.json` before anything is used.

The committed `manifest.json` is a schema-valid template; it is not an archive manifest. The command builds the host `bridge_mcp` release binary with the locked dependencies, then replaces `packaging/mcpb/stage/` with a clean host-specific stage. Its binary is staged at `bin/<target-triple>/bridge_mcp` (or `.exe` on Windows), alongside `LICENSE`, `NOTICE`, `THIRD_PARTY_LICENSES.txt`, and `THIRD_PARTY_LICENSES_RUST.txt`. The PDFium library is staged beside the binary (`bin/<target-triple>/libpdfium.dylib` or `pdfium.dll`), where `parse_bank_statement` loads it, with `THIRD_PARTY_LICENSES_PDFIUM.txt` at the root: every licence file the pinned PDFium archive ships, byte for byte.

The generated manifest uses the [official MCPB schema](https://github.com/anthropics/mcpb/blob/main/MANIFEST.md): a string entry point, an explicit launch command, environment substitutions for user settings, and an operating-system compatibility declaration. MCPB 0.1 has no architecture compatibility field. The configuration UI keeps the local Tally host, numeric HTTP port (default `9000`), response redaction, and **Allow Journal posting**. Posting is **off by default** until bridge#574 and bridge#575 are fixed; turning it on adds `post_import`, and every new posting still requires native approval. The manifest maps `BRIDGE_AGENT_ENABLE_IMPORT` to a constant `true`, so file preparation and bank-statement parsing are available whatever the setting; `scripts/check-mcpb-bundle.py` refuses a manifest that defaults posting on or maps imports to anything else.

Build and distribute separate archives for Windows x64 and macOS arm64. Intel macOS is not currently qualified, so do not label an arm64 archive as universal macOS support. Identify the architecture in each distributed filename and select the archive matching the client host.

The verifier requires a matching binary, launch command, operating-system declaration, legal resources, and the PDFium library and notice; the smoke check additionally requires both to match the SHA-256 pinned for the host platform. The official CLI validates the complete manifest and creates the archive from the host-specific `stage/` directory. The binary, archive, and staged resources are ignored local build outputs and must not be committed. Run the commands independently on each supported host. CI packages Windows and the hosted macOS runner architecture, then extracts and launches each actual archive with a temporary data directory and loopback port 9. The bounded smoke checks initialization, the default tool catalog, the local voucher schema, and its egress receipt, and parses the repository's synthetic encrypted statement through `parse_bank_statement`, which proves the bundled PDFium loads from beside the binary; it never requests Tally data. Archive and binary hashes plus payload-free verification counts are retained in `mcpb-smoke.json` alongside the archive. This establishes packaged stdio execution on the runner; desktop-client installation and live Tally interoperability remain separate checks. Use `python` instead of `python3` for the local command on Windows.

CI smoke artifacts expire after seven days. The manual preview-release workflow instead publishes immutable, explicitly unsigned GitHub prereleases with the actual archive, checksum, source-provenance record, and payload-free smoke result. It has no production-signed channel: raw MCPB archives are not a notarization-and-stapling carrier for the macOS binary, so a production channel needs a separately reviewed signed distribution design and protected credentials.
