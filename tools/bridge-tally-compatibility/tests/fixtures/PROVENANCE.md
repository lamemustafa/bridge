# `compatibility-*-schema1.json` — provenance

The schema-1 compatibility surface and support matrix, byte for byte as master
held them at `3d2a4b05`, before bridge#760 moved both to schema 2. They are the
fixtures for two properties of that change:

- **Continuity:** the surface digest the gate now computes equals the
  `manifest_sha256` schema 1 stored for the same pins. The schema change alone
  does not move the digest; any change to a pinned file still does.
- **Refusal:** the tool refuses a schema-1 surface or matrix, as written and
  with the stored digest removed.

Made with `git show 3d2a4b05:docs/tally/compatibility/compatibility-surface.json`
and the same for `compatibility-matrix.json`; nothing is edited. They name only
repository paths, file hashes and synthetic claim scopes: no personal or client
data.

| file | bytes | sha256 |
|---|---|---|
| `compatibility-surface-schema1.json` | 45061 | `848bb5186861a5e9063f3aa3b963df32601f3f98f48075caa7d7a4e536002b06` |
| `compatibility-matrix-schema1.json` | 7276 | `8ffd0d07d12918dcc8ecb8fa4162bd901868a9f270c7886d1a7a84a9a95e9829` |
