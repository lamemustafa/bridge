This is an unsigned Bridge MCPB preview for evaluation only.

It is not a production release, is not code-signed or notarized, and must not
be presented as one. It includes an unsigned third-party PDFium shared library
(bblanchon/pdfium-binaries, pinned by SHA-256 in
packaging/pdfium/pdfium.lock.json) beside the server binary. The asset checksums and payload-free smoke evidence
identify the exact archive and source commit.

Validation scope: the preview workflow builds and smoke-tests the packaged
stdio server on hosted Windows x64 and Apple Silicon Mac runners. That proves
the archive launches, exposes its local tool catalog, and parses a synthetic
encrypted bank statement with the bundled PDFium; it does not establish
live Tally behaviour or Claude Desktop conversational tool calls on either
host. Native Windows Tally/Claude Desktop validation remains outstanding, and
Intel Mac is not qualified.
