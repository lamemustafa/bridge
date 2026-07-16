# ADR 0007: Tally accounting evidence and redacted proof authority

## Status

Accepted for the first PR 08 implementation slice.

## Decision

- Core Accounting pack schema v3 models Tally's documented signed-amount and
  `ISDEEMEDPOSITIVE` polarity semantics as a typed debit/credit field. Polarity
  is part of canonical serialization and therefore changes the record and
  snapshot hash. Deterministic fixtures cover this contract; live Education-
  profile validation remains pending.
- Schema v3 also requires Tally's `ISOPTIONAL` state. Cancelled and optional
  voucher entries remain preserved evidence but are excluded from ordinary-book
  movement and polarity claims. Scenario-inclusive reporting is not inferred.
- Portable reconciliation uses exact decimal strings. It checks reference
  integrity, signed voucher balance, and agreement between the signed amount
  and Tally polarity without floating-point arithmetic.
- Cancelled or optional vouchers with no entries do not produce a false missing-entry
  mismatch. A non-cancelled zero-entry voucher remains an explicit
  applicability gap until its voucher-class semantics are modeled.
- An experimental Bridge-defined ledger-balance report uses documented
  `TBalOpening` and `TBalClosing` fetch names as a separate cross-view
  corroboration path. Bridge validates the returned company GUID, requested
  date echo, internal row-count consistency, and equality of returned candidate
  ledger identities with the Core mirror before exact-decimal comparison of
  `closing - opening` movement. The response is locally associated with the
  plan's schema, query profile, filter hash, and report hash; those local values
  are not Tally attestations. Signed semantics, ordinary-books applicability,
  identity stability, completeness, and Education-profile behavior remain
  live-unverified, so production evidence is `period_report_profile_unobserved`
  and cannot establish `Verified`.
- After report reads, the engine performs a full semantic reread and then a
  cache-bypassing capability probe. Reread equality is recorded as
  `bracketed_full_reread_v1`-style stability evidence, not as atomic isolation:
  Tally documents no cross-request snapshot transaction, so
  `source_cut_atomicity_unavailable` still prevents `Verified`.
- Voucher header totals and tax totals remain unavailable. Bridge does not infer
  them from an internally balanced entry set.
- Identical complete-scope masters repeated by date-window exports deduplicate.
  A changed hash for the same source identity is source drift and prevents a
  checkpoint.
- Production proof commits accept a sealed input obtainable only from the
  reconciliation builders. Direct caller-authored `Verified` commit inputs are
  not part of the production API.
- Proof summaries and redacted exports revalidate stored proof hashes and chain
  linkage. Export additionally requires a hash-valid terminal durable snapshot
  receipt. The public-support export is an allow-list DTO and excludes names,
  source identities, endpoints, internal run/batch/proof/capability IDs,
  checkpoint tokens, source rows, amounts, payloads, and drill-down hashes.
- The `public_support_v1` document also omits stable snapshot and proof-ledger
  commitments so independently shared exports cannot be correlated through a
  source-derived hash. Those commitments remain visible only in the local
  operator console.
- The export says `checksum_only` and `authenticity_claim: none`. A local hash
  chain is consistency evidence, not a signature and not proof that the
  loopback responder was genuine Tally.
- Encrypted durable window evidence retains bounded internal mismatch tokens.
  The local console remaps them to ordinal `local-record-*` aliases only after
  proof and durable-state validation. Neither the internal tokens nor the
  aliases enter the public support export.

## Migration and rollback

Core schema v1/v2 interrupted snapshots are intentionally not resumed by a build
that emits schema v3 canonical entries. They remain historical evidence; the
operator must start a new full snapshot. Rollback to a v1 build cannot consume
v3 plans and should likewise require a new full snapshot rather than silently
dropping polarity or optional-voucher state.

The redacted export is read-only and introduces no Tally write path. Removing
the UI preview does not mutate the proof ledger or mirror. Existing v1 proof
rows remain readable, but an export requires the newer durable receipt checks
and is refused when those checks cannot be established.

Proof contract v3 binds a domain-separated digest of the complete canonical
record-count map into both receipt facts and the proof-ledger hash. Additive
migration 0011 leaves v1/v2 rows nullable and preserves their original hash
serialization; only v3 rows require the digest.

## Remaining evidence gates

No supported environment may call Core Accounting `Verified` until the exact
profile provides complete source counts and capability-validated candidate
identities, voucher header/applicability coverage, validated cross-view report
semantics, and a product-supported atomic source-cut or an equally strong
documented isolation mechanism. The implemented experimental cross-view,
full reread, and fresh end-profile checks are useful evidence, but report-profile
and cross-request atomicity gates remain deliberately open.
