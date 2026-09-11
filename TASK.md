Task ID: T4-2 — fix two confirmed review findings on PR #279
Outcome: an unnamed `On Account` allocation no longer aborts a voucher read, and an empty
`<GSTDUTYHEAD/>` on a non-tax ledger is classified from `TAXTYPE` instead of as unrecognised.

Repository: bridge. Worktree `/Users/tapishkhandelwal/Desktop/dev/worktrees/bridge-pr228-takeover`,
already on branch `tapish-codex/ship-billalloc-dutyhead-20260910` at `d63d3b8`.
Do NOT rebase, merge, pull or push. Model: gpt-5.6-terra, effort high.

Both findings were raised by review and **independently confirmed by the lead**. They are not
speculative; do not re-litigate whether they are real. Fix them.

## Finding A — unnamed `On Account` allocations abort the whole read

`src-tauri/src/agent_voucher_parse.rs` (around line 205) requires a non-empty `NAME` on every
bill allocation and otherwise returns `bill_allocation_field_missing`, which aborts the entire
voucher collection — not just the allocation.

An `On Account` allocation legitimately has **no bill identity**, so `NAME` is empty by design.
The repository already models this: `BillReferenceKind::OnAccount` in
`src-tauri/crates/bridge-tally-protocol/src/outstandings/model.rs` does not require a name, and
`docs/tally/IMPLEMENTATION_GUIDE.md` (~lines 640-642) records that it carries none.

Required behaviour:
- An allocation whose `BILLTYPE` is `On Account` is valid **without** a name. Represent that
  state explicitly — an absent name must stay distinguishable from an empty-string name and from
  a name that failed to parse. Do not substitute `""`, `null`-as-empty, or a placeholder.
- `New Ref`, `Agst Ref` and any other reference-bearing type still REQUIRE a name; keep failing
  closed for those.
- A missing or empty `BILLTYPE` is not silently treated as `On Account`. If the type itself is
  absent, that is still a malformed allocation.

Scale note: this is not hypothetical. The `Aarav Trading Company Demo` corpus contains ~1,628
`On Account` allocations in a single window, so today's code makes that book unreadable through
this path.

## Finding B — empty duty-head element is not classified from TAXTYPE

`src-tauri/crates/bridge-tally-protocol/src/lib.rs` (around line 198) reaches `NotTaxLedger` only
when the `GSTDUTYHEAD` element is **omitted**. When Tally returns the ordinary
`<TAXTYPE>Others</TAXTYPE><GSTDUTYHEAD/>` shape, the empty element is retained as
`Returned("")` and classified `Unrecognized`, so ordinary non-tax ledgers are mislabelled.

Verified by the lead against this repository's own capture,
`src-tauri/crates/bridge-tally-protocol/tests/fixtures/native/master_fields_lab_partial_alter_after.response.xml`:
it contains **3 self-closing `<GSTDUTYHEAD/>` elements alongside 3 `<TAXTYPE>Others</TAXTYPE>`**,
and **0** duty-head elements carrying a value.

Required behaviour: treat an empty returned duty head as **head-absent** for classification, then
apply the existing `TAXTYPE` logic — `Others` gives `NotTaxLedger`, a GST ledger with no head
gives `Absent`. A non-empty but unrecognised value must still be `Unrecognized` with its raw
string preserved verbatim; the irregular vocabulary (`CGST`, `IGST`, `State Tax`, `UT Tax`,
`Cess`, and **not** `SGST`) is unchanged.

## Tests — this is where both findings came from

The existing tests passed for the wrong reason: finding B's test omits the element rather than
emptying it, and nothing covers an unnamed allocation at all.

- Add a parse test with an `On Account` allocation carrying an empty `NAME`, asserting the
  voucher still reads and the state is represented explicitly. Add one asserting `New Ref`
  with an empty name still fails closed.
- Change finding B's test to use the **empty-element** shape, and add a case reading the real
  capture named above so the fixture on disk is what the test exercises.

Run only the tests covering these changes:
`rustup run 1.96.0 cargo test --manifest-path src-tauri/Cargo.toml --lib <filter>`
(the host default toolchain is 1.95 and will refuse the workspace). Do NOT run the full suite
or a workspace build.

**Do not reseal the compatibility surface** — the lead reseals once, after review.

Do not contact Tally. Offline only.

## Handback

`runs/T4-2-report.md`: files changed, commands with exit codes, test results, and how you
represented the unnamed `On Account` state. Commit to the branch. No push, no PR, no
self-acceptance.
