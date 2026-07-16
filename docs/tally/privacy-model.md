# Tally privacy model

Tally book data is sensitive by default. Bridge minimizes collection and keeps
the data plane local unless the operator explicitly configures a reviewed
destination contract.

## Data classes

| Class | Examples | Public logs/support bundles | Encrypted local mirror |
| --- | --- | --- | --- |
| Secrets | credentials, tokens, PINs, private keys | Never | Never as ordinary mirror records |
| Book data | company names, GSTINs, ledgers, vouchers, amounts, inventory | Never | Only when required for an enabled pack |
| Sensitive metadata | certificate metadata, local paths, usernames, raw Tally errors | Never | Only if an explicit feature requires it; otherwise discard |
| Safe operational evidence | generated IDs, counts, timings, hashes, allow-listed reason codes | Allowed after review | Allowed |
| Synthetic fixtures | obviously fictional companies and records | Allowed | Allowed |

## Storage boundaries

- The Tally mirror is an application-data-relative SQLCipher database.
- Its key is generated independently and stored through the operating-system
  credential facility; it is not derived from a username or checkout path.
- Initialization is locked, the key is applied before schema access, and an
  integrity check is required before use.
- Raw response bodies are not normal diagnostic output. Canonical records are
  retained only for enabled packs and are associated with source identity,
  content hashes, and proof state.
- A failed, partial, cancelled, or ambiguous run does not replace the last
  verified checkpoint.

## Network boundaries

- Tally HTTP is restricted to canonical loopback addresses. Redirects are
  disabled so a local response cannot redirect Bridge to another host.
- The local endpoint is capability-verified, not cryptographically
  authenticated. Another local process may impersonate the configured port.
- Delivery to AXAL or another destination requires an explicit, versioned
  adapter contract. The repository does not invent or guess remote endpoints.

## Diagnostics and deletion

Diagnostics should contain safe reason codes, phase, elapsed time, counts,
generated request/run IDs, and hashes. They must exclude raw XML/JSON, company
or record names, tax identifiers, amounts, local paths, and credentials.

Operators must be able to delete the mirror and associated OS credential as a
single documented reset operation. Until that workflow is implemented and
tested, no UI should claim that local Tally data has been fully erased.
