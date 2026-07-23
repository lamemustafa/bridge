# Licensed-lab qualification checklist

What to verify, per Tally version, on the rented licensed TallyPrime lab VM
(plan §NOW item 4; playbook Phases 3–5). Every row that completes produces a
signed compatibility-matrix receipt for the exact
(product, release, mode, platform, transport, operation) tuple — see
[compatibility/README.md](./compatibility/README.md). Nothing here may be
answered from folklore, blogs, or the Education instance; Edu results are
recorded as education-mode cells only.

**Version matrix to run:** TallyPrime 7.1 (primary), 7.0, 6.x (one build),
each licensed; Education 7.1 for the education-mode cells. ERP 9 6.6.3 only
for read-profile compatibility cells.

**Fixture companies (synthetic only):**
- `QF-EN` — English, small, clean chart of accounts.
- `QF-IN` — Devanagari/Gujarati/Tamil ledger + party names, non-ASCII narrations.
- `QF-UDF` — custom TDL loaded defining mandatory UDF fields on vouchers.
- `QF-BIG` — generated 100k+ vouchers across 3 FYs (scan-cost measurements).
- `QF-LOCK` — closed/locked prior period, books-from mid-FY.
- `QF-EDITLOG` — Edit Log enabled (MCA audit-trail configuration).

Each row: **ID · question · how · consumed by**.

## A. Write grammar (Phase 4 blockers)

| ID | Question to answer with evidence | How | Consumed by |
| --- | --- | --- | --- |
| A1 | Ledger Create/Alter/Delete accepted shapes; Delete failure shape when ledger is referenced by a voucher | Import each; capture counters + LINEERROR | Outbox, master CRUD |
| A2 | Voucher Create accepted for payment/receipt/journal/contra incl. narration, bill allocations, cost centres | Import per type on QF-EN and QF-IN | Voucher writes |
| A3 | Voucher **Alter by REMOTEID/GUID**: required identity fields (DATE? VOUCHERTYPENAME? VOUCHERNUMBER? VCHKEY?); full-replacement vs partial semantics | Alter with progressively minimal envelopes | Alter path vs Cancel+Create fallback decision |
| A4 | Voucher **Cancel** via ACTION=Cancel: identity requirements, CANCELLED counter, voucher number stays reserved | Cancel + readback + daybook check | Compensation primitive |
| A5 | Voucher **Delete**: identity strictness, DELETED counter, absence on readback and on next index scan | Delete + verify absence | Delete path |
| A6 | LASTVCHID / LASTMID: returned when? Clobbered by a foreign write between import and readback? | Scripted foreign write race (second session typing in Tally) | Readback binding + cross-check rule |
| A7 | Counter semantics: IGNORED vs ERRORS on duplicate name create; ALTERED on no-op alter; multiple TALLYMESSAGE behavior (informational only — batch stays 1) | Matrix of malformed/duplicate imports | Evidence parser golden tests |

## B. Idempotency & identity (Phase 4)

| ID | Question | How | Consumed by |
| --- | --- | --- | --- |
| B1 | Inline-TDL **UDF definition on import**: accepted? persisted? exported back on read? survives foreign Alter of the voucher? | Create with BridgeTxnID UDF; read back; alter in UI; read again | Idempotency key authority decision (UDF vs narration) per version |
| B2 | Narration-suffix key: survives UI edits? truncation limits (observed narration max length)? | Long-narration create + UI edit | Fallback key + fingerprint mandate |
| B3 | Master name uniqueness: case sensitivity, leading/trailing space handling, Unicode normalization (QF-IN names differing only by case/NFC form) | Create near-collision pairs | Name-keyed idempotency, dedupe |
| B4 | Voucher **auto-numbering**: methods (Automatic, Manual, Auto-manual, Multi-user auto) vs imported VOUCHERNUMBER — is a supplied number honored, ignored, or collided? Number behavior on Cancel (reserved?) and on Alter | Import into voucher types configured per method | Duplicate prevention; number display in review grid |
| B5 | Multi-currency voucher create/read round-trip (rate, forex gain/loss ledger) | QF-EN with USD party | Scope decision: in/out of Phase 4 |

## C. Change detection (Phase 3 blockers)

| ID | Question | How | Consumed by |
| --- | --- | --- | --- |
| C1 | ALTMSTID / ALTVCHID company-level high-water marks: exported? bumped by every master/voucher change incl. back-dated inserts? | Export company object before/after edits | Cheap-probe availability claim |
| C2 | Back-dated voucher insert: fresh AlterID assigned? caught by date-unbounded index scan? | Insert into prior month; scan | Drift Sentinel false-negative proof point |
| C3 | AlterID regression on **backup restore**: observed values, company GUID stability across restore | Backup, edit, restore, scan | Re-baseline (calm) state |
| C4 | Server-side filter behavior: date-range honored exactly (FY boundaries, from==to); AlterID filter attempt ignored (confirm per version) | Filtered exports vs known fixture | Scan segmentation design |
| C5 | Deletion visibility: deleted voucher absent from index scan; no other observable tombstone signal | Delete in UI; scan | Verified-scan-only tombstone rule |
| C6 | Scan cost on QF-BIG: collection generation time per FY segment; Tally UI responsiveness while scanning; safe request spacing | Timed segmented scans while a user types | Off-hours scheduling defaults, spacing |

## D. Edit Log / MCA audit trail (Phases 4–5 marketing accuracy)

| ID | Question | How | Consumed by |
| --- | --- | --- | --- |
| D1 | Does an XML-gateway write appear in the Edit Log on QF-EDITLOG? Attributed to which user (logged-in Tally user? "admin"?) | Gateway create + inspect Edit Log | Proof-of-Post wording: "supplementary evidence", exact attribution sentence |
| D2 | Is the Edit Log itself readable/exportable programmatically (XML collection? ODBC? report export only)? | Attempt export | Possible future drift corroboration source |
| D3 | Does Cancel/Delete via gateway log distinctly from UI cancel/delete? | Compare log entries | Compensation-audit story |
| D4 | Edit Log on vs off: any write-grammar behavior differences? | Re-run A2 with log on | Matrix dimension decision |

## E. Gateway security & topology (Passport probes)

| ID | Question | How | Consumed by |
| --- | --- | --- | --- |
| E1 | Tally user security enabled: does the gateway require/apply auth? Which operations fail and with what shape? | Enable security controls; probe | Passport `Unsupported` states |
| E2 | TallyVault-encrypted company: visible in company list? readable? writable? | Vault a fixture company | Declared-unsupported topology honesty |
| E3 | Company not loaded / multiple companies loaded: SVCURRENTCOMPANY targeting proof; write attempt against unloaded company fails how? | Load permutations | Company-pinning failure modes, error catalog |
| E4 | Two Tally instances on one machine (different ports): endpoint identity behavior | Run both; probe | Endpoint canonicalization |
| E5 | Education mode negative tests: voucher dated 15th rejected with what shape; master writes unrestricted; 31st literal on 30-day months | Edu instance | Passport education cells; error catalog |

## F. Encoding & robustness (Phase 2 confirmations on live builds)

| ID | Question | How | Consumed by |
| --- | --- | --- | --- |
| F1 | Response encoding per version (UTF-8/UTF-16LE/BOM) for QF-IN exports; any ill-formed XML (unescaped `&` in names)? | Byte-capture exports | Decoder qualification |
| F2 | Round-trip fidelity: QF-IN narration/GSTIN/bill refs byte-exact through export→canonical→re-export | Diff harness | Full-fidelity exit criterion on licensed builds |
| F3 | QF-UDF: unknown-UDF quarantine on read; write REJECTED shape when mandatory custom field missing; write accepted-but-blank risk | Import minimal voucher into QF-UDF | Per-company write qualification rule ("review-in-Tally-recommended" degradation) |
| F4 | Period lock (QF-LOCK): write into locked period — LINEERROR shape; alter of pre-lock voucher | Import attempts | Error catalog ("period locked") |
| F5 | Response-cap behavior: export exceeding 32 MiB window — truncation vs error; Partial labeling honest | QF-BIG unsegmented export | Bounded-read invariant |

## Operating rules

1. One checklist row = one scripted, rerunnable probe in the qualification
   harness; manual one-off observations don't count as evidence.
2. Every result — positive, negative, or weird — becomes a matrix receipt;
   `Unsupported` requires the profile-specific unsupported signature
   (playbook Phase 1 item 6), never a bare STATUS=0.
3. Rows A1–A7, B1–B4, C1–C5 are **blocking** for their consuming phase;
   the rest may trail but must complete before GA claims.
4. Re-run the full checklist on every new TallyPrime release before
   updating the supported-versions claim.
