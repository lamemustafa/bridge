# Tally Truth Layer

Bridge's Tally integration is designed to be inspectable and fail closed. It
must distinguish network reachability from Tally application success, observed
capability from assumption, and a completed request from a verified snapshot.

## Public contracts

- [Support matrix](./support-matrix.md) records what is implemented, what has
  been observed on a real Tally host, and what remains planned.
- [Executable compatibility matrix](./compatibility/compatibility-matrix.json)
  enumerates exact claim cells and fails closed when positive claims lack
  current reviewed evidence.
- [Privacy model](./privacy-model.md) defines what may be retained or included
  in diagnostics.
- [2026H2 improvement plan](./IMPROVEMENT_PLAN_2026H2.md) is the current
  execution authority: market research, phase roadmap (unseal → full-fidelity
  reads → Drift Sentinel → write substrate → product loop), and rulings.
- [Prompt playbook](./PROMPT_PLAYBOOK.md) holds the per-phase implementation,
  review/rectification, and change-preservation prompts plus the orchestrator
  loop; [Execution log](./EXECUTION_LOG.md) and [Backlog](./BACKLOG.md) are its
  working files.
- [Licensed-lab qualification checklist](./LICENSED_LAB_QUALIFICATION_CHECKLIST.md)
  enumerates the per-version probes that must produce compatibility receipts.
- [Research and execution plan](./TALLY_INTEGRATION_RESEARCH_AND_CODEX_PLAN.md)
  contains the original source research, product model, threat analysis, and
  staged implementation plan (superseded in part — see its header note).

## Operator tools

- [`scripts/bank_statement_import.py`](../../scripts/bank_statement_import.py) turns a
  password-protected bank-statement PDF into TallyPrime import XML (Payment / Receipt /
  Contra), for hand-import through Gateway of Tally > Import > Vouchers. Offline; it never
  contacts Tally. SBI and HDFC statement layouts are supported.

  What it proves before it writes anything: every row reproduces its printed running
  balance; the chain lands on the closing balance the statement prints **and** reproduces
  its printed debit and credit totals (the closing balance alone is the net, so a dropped
  tail whose two sides cancel would still pass); and the account's own digits appear in the
  statement header, so the wrong statement cannot be posted to the ledger you named. It then
  re-parses its own output.

  What it *cannot* prove is which company Tally has open — see §9.11d — so
  `--confirm-open-company` makes that an explicit operator step rather than a silent one.
  Contract tests run in CI: `python3 scripts/bank_statement_import.test.py`.

  It was written because Bridge's own writer could not express a bank statement at all: it
  qualified **Journal only** (`LIVE_QUALIFIED_VOUCHER_TYPES` in `src-tauri/src/agent_import.rs`),
  while money out is a Payment, money in a Receipt, and an own-account or ATM movement a
  Contra. **That is still the case on master**; qualifying the three types is in flight and
  not landed, so nothing here should be read as a plan of record.

  Even once it lands, this tool covers a part Bridge does not: it reads the **statement
  PDF**, whereas `build_import_xml` takes an already-structured payload. It also covers books
  the writer refuses, such as one whose bank ledger sits under a money group Bridge has not
  yet observed a captured ledger beneath.

  No statement, password, ledger name or account number lives in this repository — all are
  supplied at run time.

### Statement-layout findings

Behaviour of the **bank's PDF and of `pdftotext`**, not of Tally — so it is recorded here
rather than in [`TALLY_PROTOCOL_REFERENCE.md`](./TALLY_PROTOCOL_REFERENCE.md), whose stated
purpose is how the XML gateway behaves and whose confidence markers mean a captured request
and response. There is no gateway exchange to capture for any of this. Same discipline:
state the evidence, and never record a claim without saying what it rests on.

Code that encodes one of these cites the finding by number instead of restating it.

#### S1. A narration cell wraps at the column edge, and the wrap can fall inside a reference number

**Evidence: measured through `parse_pages` on constructed pages at the real HDFC column
geometry** (`test_a_wrapped_ach_reference_survives_the_parser`). The underlying wrap is
visible in the committed HDFC capture, where a UPI reference is split across two lines of
the narration cell.

`parse_pages` keeps two readings of every wrapped text cell, and **neither is correct for
every row**:

| Where the wrap falls | `narr` (de-wrapped) | `narr_spaced` (space-joined) |
| --- | --- | --- |
| inside the reference | reference intact ✅ | `...-12345 67890` |
| between two words of the name | `NORTHWINDTRADERS` — welded ❌ | name intact ✅ |
| inside one word of the name | word intact ✅ | `TRAD ERS` — split ❌ |

Name extraction therefore reads `narr_spaced`, and any pattern that treats a trailing
reference as a delimiter must tolerate internal spaces in it — `\d[\d\s]*`, not `\d+$`.
Anchoring on `\d+$` makes every wrapped reference `UNRESOLVED`.

**Residual:** row 3. A wrap inside a single word leaves a space `narr_spaced` cannot tell
from a real one, and the reading that would get it right is the one that fails row 2. The
information needed to separate them is not carried past `_dewrap`. Asserted in the contract
test so it is a recorded limitation rather than a surprise.

#### S2. HDFC prints an ACH counterparty between `TP ACH` and the final bank reference

**Evidence: observed in real HDFC narrations; the parsing rule is asserted end-to-end** in
`test_ach_party_ends_at_the_final_bank_reference`. Shape: `ACH D- TP ACH <name>-<reference>`.

The delimiter is the **final** hyphen-plus-digits, not the first: a name may legitimately
contain a hyphenated number (`STUDIO-54`, `UNIT-7`). A non-greedy boundary resolved
`ACH D- TP ACH STUDIO-54 INDUSTRIES-1234567890` to `STUDIO`, and a mapping row for `STUDIO`
then silently posts an unrelated counterparty's transaction to that ledger. Combined with
S1, the reference is `(\d[\d\s]*)$` and the match is greedy.

The architectural decisions are recorded in:

- [Transport negotiation](../adr/0001-tally-transport-negotiation.md)
- [Company identity](../adr/0002-tally-company-identity.md)
- [Sync truth states](../adr/0003-tally-sync-truth-states.md)
- [Write safety](../adr/0004-tally-write-safety.md)
- [Synthetic qualification authority](../adr/0010-tally-synthetic-qualification-authority.md)
- [Live compatibility evidence authority](../adr/0012-tally-live-compatibility-evidence.md)
- [Party outstanding confidence authority](../adr/0013-tally-party-outstanding-confidence-authority.md)

## Non-negotiable invariants

1. Production Tally traffic is loopback-only and redirects are rejected.
2. Company-scoped operations name and verify the intended company.
3. HTTP success is never treated as Tally application success.
4. Unknown, unsupported, and not-configured are distinct states.
5. A checkpoint advances only after durable staging and reconciliation.
6. Absence in an incremental response is not deletion.
7. Writes are disabled unless their exact runtime capability was observed, and
   ambiguous post-send outcomes are never retried automatically.
8. Logs, fixtures, screenshots, and support bundles use synthetic data and safe
   reason codes, not book data.

## Contributor verification

The deterministic simulator and canonical core are the default development
surface. Live tests are supplemental and must record the exact product,
release, operating mode, host platform, and company fixture used. Education
mode is tested as Education mode; Bridge does not bypass its restrictions.

The simulator and fixture rules live in
[`src-tauri/crates/tally-protocol-simulator`](../../src-tauri/crates/tally-protocol-simulator/README.md).
The parser-only evidence contract lives in
[`tools/bridge-tally-qualification`](../../tools/bridge-tally-qualification/README.md).
The separate live-observation DTO and release gate live in
[`tools/bridge-tally-compatibility`](../../tools/bridge-tally-compatibility/README.md).
That crate performs no network requests. The separate
[`bridge-tally-live-read`](../../tools/bridge-tally-live-read/README.md)
controller requires a reviewed synthetic fixture and two interactive
confirmations, and exposes only byte-identical sealed production read profiles.
See the [Education runbook](./compatibility/live-education-runbook.md). No
receipt should be manufactured by hand.

Do not advertise a capability from a descriptor or roadmap item. A capability
becomes supported only when the exact release, mode, transport, query profile,
required fields, and invariants have current evidence.
