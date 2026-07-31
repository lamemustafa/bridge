# Bridge × Tally: Market Research & Improvement Plan

**Date:** 2026-07-24 · **Repo:** `lamemustafa/bridge` (audited at PR #78) · **Method:** codebase audit + 24-source verified web research + 4-persona ideation, 2 adversarial critiques, arbiter synthesis

> Execution companions: [PROMPT_PLAYBOOK.md](./PROMPT_PLAYBOOK.md) (per-phase implementation/review/rectification/preservation prompts + orchestrator), [EXECUTION_LOG.md](./EXECUTION_LOG.md) (per-PR invariant log), [BACKLOG.md](./BACKLOG.md) (parked scope), [LICENSED_LAB_QUALIFICATION_CHECKLIST.md](./LICENSED_LAB_QUALIFICATION_CHECKLIST.md). Where this plan conflicts with `TALLY_INTEGRATION_RESEARCH_AND_CODEX_PLAN.md`, **this plan wins** (see the supersession note at the top of that file).

---

## 0. Executive summary

**Where Bridge is:** a superbly engineered, read-only Tally evidence console. 13 layered Rust crates, loopback-only transport, strict protocol parsing, atomic checkpointed snapshots into an encrypted SQLCipher mirror, Proof-of-Sync, Gap Map. But: **zero writes possible in any shipped build**, voucher reads deliberately stripped of narration/GSTIN/bill data, every compatibility claim `unknown` with `missing` evidence, and ~30 recent PRs spent on "sealed canary" ceremony for a synthetic write that has never touched a live Tally. The founder's diagnosis is correct: no CA will use it today.

**Where the market is:** every serious competitor (Vyapar TaxOne née Suvit, Finsights, CredFlow, Biz Analyst, AI Accountant) ships the same architecture Bridge already has — a local desktop connector speaking to Tally's XML gateway — and every one of them is drowning in the same complaint: **sync you can't trust** (entries vanishing, duplicates, stale ledgers, 24-hour deletion lag, silent failures). Nobody proves what synced. That is Bridge's thesis, validated — but evidence must sit *under* workflows, not replace them.

**The plan in one paragraph:** Unseal the write machinery and delete the ceremony (keep the evidence). Restore full-fidelity reads. Ship **Drift Sentinel** — "know every voucher your client changed after you signed off, with before/after" — as the read-only acquisition wedge no competitor has. Rent a licensed TallyPrime in month 2. Then build the write substrate (outbox, batch-of-1, readback-verified posting) and the expansion product: **Excel/CSV → review grid → maker-checker → post → Proof-of-Post**. Realistic solo-dev horizon: wedge in ~3 months, daily-use write product by ~8–10 months.

**The north-star sentence** (what a partner must be able to say): *"Every entry my juniors post is approved, verified against Tally, and evidenced; and I know within a day if a client edits a voucher I've already signed off."*

---

## 1. Current state of Bridge's Tally integration

### What works today (all read-only)
- Probe/company discovery (PR #78 adds the explicit "N companies discovered → choose/verify" prompt — good, keep).
- Reads: companies, groups, ledgers, voucher types, vouchers, ledger period balances — via reviewed XML/TDL profiles, loopback-only, size/time-bounded, STATUS=1-enforced.
- Full CoreAccounting snapshot pipeline → canonical model (exact decimals, fail-closed) → reconciliation → Proof-of-Sync → SQLCipher mirror, with atomic checkpoints and resumability.
- Evidence UI: capability passport, gap map, truth states, mirror explorer.

### The blockers
| Blocker | Detail |
|---|---|
| **No writes** | Even the single synthetic canary ledger is behind two disabled compile-time flags + attestations + sealed one-shot dispatch; no Tauri command exists. UI hard-codes `write capability: Unknown`. |
| **Minimized reads** | Vouchers lack narration, party GSTIN/address, bill allocations, inventory/GST lines — useless for recon, scrutiny, or any review UI. |
| **Zero live evidence** | Compatibility matrix: every cell `unknown`, evidence `missing`. No `Unsupported` signing key even exists. The "evidence product" has no evidence. |
| **Only CoreAccounting wired** | IndiaTax / Bills-Outstandings / Inventory packs are feature-gated parsers with no runtime. |
| **No cloud path for Tally data** | AXAL sync exists only for DSC/documents; Tally data needs a versioned destination contract (fine for now — local-first is the positioning). |
| **Velocity sink** | ~30 PRs of pre-dispatch safety ritual produced zero rows of evidence. Safety engineering has been optimizing ceremony before dispatch instead of verifiability after dispatch. |

---

## 2. Market research (July 2026, verified claims)

### 2.1 Landscape

| Product | Write into Tally | Mechanism | Sync | Notes |
|---|---|---|---|---|
| **Vyapar TaxOne** (ex-Suvit, absorbed by Vyapar) | Ledgers + vouchers from bank/sales/purchase docs | Desktop connector → XML gateway (manual host/port) | One-way push + reads for GST | Scale leader: claims 10k+ CA firms, 30k+ accountants; AI/OCR ingestion, ledger auto-suggest from history, review-before-post, "zero duplicate entries" marketing |
| **Finsights** | Vouchers, invoices, stock entries | Desktop connector beside Tally (both must stay open) | Two-way, ~10-min cycles; **Tally deletions propagate only every 24h** | CA-focused; client-invitation model for client-maintained books; unlimited companies |
| **CredFlow** | Receipts, invoices, quotations, sales orders | Desktop connector; company must be open; **refuses Education-mode Tally** | Two-way | Receivables/dunning company (SMS/WhatsApp/call reminders); sync-reliability complaints |
| **Biz Analyst** | 10 entry types incl. sales/purchase | Desktop sync agent | Two-way | 1M+ installs, 4.2★; complaints: "unending" sync issues, missing fields, Play-Store data-safety page admits unencrypted data shared with third parties |
| **AI Accountant** | Vouchers, mappings, sales invoices | Local agent; XML for R/W, ODBC for analytics; **AlterID-tracked incremental sync** | Two-way, scheduled | Maker-checker approval, review-before-post with rationale, duplicate/voucher-lock handling; lists custom-TDL/UDF fields as a known break risk |
| **ClearTax connector** | e-invoice/e-way-bill fields | **TDL plugin inside Tally + connector app (ODBC)**; per-machine installs | Two-way (compliance fields) | Owns e-invoicing; in-Tally UI |
| **Tally native** (the platform threat) | — | — | — | Built-in GSTR-2B download + recon with granular status buckets (resolution still manual, per-company); TallyPrime 6.0 connected banking; 7.x AI features |
| **DIY long tail** | File-based XML import | Gateway of Tally → Import | One-way | NIKASH converters, TaxGuru VBA recon (6–10 hrs/GSTIN/month VLOOKUP baseline) — the actual majority workflow |

> Deeper landscape (Zoho/Munim/Open/EnKash/GST connectors/Tally-native remote),
> cited competitor pricing, and a public-record UX teardown of the four leading
> flows are in [MARKET_RESEARCH_ADDENDUM.md](./MARKET_RESEARCH_ADDENDUM.md). Its
> findings confirm every ruling below and sharpen the UX bets (§ Now/Next).

### 2.2 Structural takeaways
1. **The on-prem connector is unavoidable and Bridge already is one** — with a stronger engineering base than the connectors CAs complain about.
2. **Sync trust is the universal open wound.** Every incumbent's worst reviews are trust failures. None can prove completeness, attribute failures, or detect Tally-side edits/deletions promptly.
3. **Tally native is absorbing adjacent value** (2B recon, banking, AI): pure-reporting and portal-integration plays erode. Data-entry automation, multi-client practice ops, and *evidence about the books* remain defensible.
4. **Regulatory tailwind with a date:** since Jan 2026, excess ITC vs GSTR-2B auto-flags on the portal; MCA Edit-Log rules make "what changed in the books" a partner-level anxiety.
5. **Education mode:** competitors refuse it (CredFlow). It permits voucher entry only on the 1st/2nd/31st. It is a fine regression rig and an honest Passport state — but nothing can be marked `Verified` from it, and a licensed instance is a hard prerequisite for a credible write story.

### 2.3 CA workflows that consume the hours
- **Bank statement → vouchers** (the biggest hour pool; ledger suggestions learned per-client from narration patterns).
- **Excel/CSV registers → vouchers** (pure transcription; saved per-client column mappings make month 2 near-zero-touch).
- **GSTR-2B ↔ purchase register recon** (fuzzy multi-field matching, exception queues; Tally native buckets well but resolves one voucher at a time, one company at a time).
- **Receivables follow-up** (CredFlow's turf; skip).
- **Multi-client management** (50–200 companies per firm, staff roles, per-client sync health, deadline rhythm: 7th/11th/20th).
- **Audit/verification** ("what changed since I signed off" — served by *nobody*).

### 2.4 UX patterns to steal / fix
**Steal:** review-before-post as the *only* path to Tally (Suvit); saved mapping templates; maker-checker (AI Accountant); Tally's own recon-bucket vocabulary; duplicate detection made visible.
**Fix (the industry's sins):** silent sync failure and single green dots (show *last-verified* vs *latest-attempt* as two timestamps, always); stale data without self-degrading freshness; "posted" claims from HTTP counters (post ≠ verified until re-read); errors in XML language instead of accountant language; black-box AI suggestions (show the rationale).

### 2.5 Technical ground truth for deep two-way sync
- Gateway: Import/Export/Execute; broad read surface (24 voucher types, 13 master types proven publicly); writes for masters and vouchers with `ACTION=Create/Alter/Cancel/Delete`.
- Import response = STATUS + CREATED/ALTERED/…/ERRORS counters + coarse LINEERROR, **no per-record IDs** (only LASTVCHID/LASTMID) → idempotency, duplicate prevention, and readback verification are the integrator's job.
- **No server-side AlterID filtering** (date-range only) → incremental sync = periodic GUID+AlterID index scan diffed locally; same GUID + higher AlterID = edited; absent from a *complete verified* scan = deleted; lower AlterID = backup restored → re-baseline. Back-dated vouchers get fresh AlterIDs, so date-unbounded scans catch them.
- ODBC strictly read-only. Inline per-request TDL shapes exports without installing anything. No concurrent writes — single-writer serialization mandatory. Omitting SVCURRENTCOMPANY writes to whatever company is open (the ecosystem's worst failure mode; Bridge already pins).
- Custom TDL/UDF fields in client Tallys break naive schemas — quarantine unknowns on read; per-installation write qualification before certifying writes there.

---

## 3. The debate: what survived, what was ruled, what died

Four persona proposals (CA operator, product strategist, protocol engineer, UX designer) were attacked by two adversarial critics (engineering-reality, CA-adoption) and reconciled by an arbiter. Full transcripts are preserved in the session scratchpad.

### 3.1 Consensus (adopt)
1. **Full-fidelity reads first** — narration, party GSTIN/address, bill allocations, GST/inventory lines; quarantine-on-unknown for custom TDL/UDF; encoding/name-normalization hardening (non-English fixtures). Everything else depends on this.
2. **The write substrate** — outbox state machine (WAL-durable before dispatch), **batch-size-1** (counters are unattributable at N>1; the current `MAX_LEDGER_WRITE_BATCH=10` is wrong), UDF-embedded BridgeTxnID + **date/amount/ledger-set fingerprint** as mandatory secondary dedupe, readback-confirmed-only ("posted" = re-read from Tally, never counters), LASTVCHID cross-checked against the idempotency key (foreign-writer race), OutcomeUnknown recovery with pre-image AlterID checks, single-writer actor, fail-closed company pinning, **Cancel (not Delete) as the compensation primitive**, no fictional rollback.
3. **Maker-checker + Proof-of-Post** — review-before-post is the only path from file to Tally; approval identity recorded; exportable per-batch evidence pack. Marketed as *supplementary* workpaper evidence, never MCA-Edit-Log equivalence (gateway writes appear in Tally's log as the logged-in Tally user).
4. **Excel/CSV → review grid → post pipeline** with saved per-client column mappings — the expansion product.
5. **Drift Sentinel** — checkpoint → "changed/new/deleted/back-dated since sign-off" with before/after diffs. Firm-maintained books only in v1; calm "backup restored, re-baselining" state distinct from tamper alarm.
6. **Honest freshness UX** — Sync Beacon with dual timestamps; Gap Map reborn as a fix-it list; Truth States compressed to three visual tiers (Verified+time / Attention+reason+fix / Broken+remediation).
7. **Incremental sync v2** — ALTMSTID/ALTVCHID cheap probe, segmented per-FY/month GUID+AlterID scans (a full-books unbounded export can hang a 500k-voucher Tally at 11am — segment + off-hours + visible progress/cancel), verified-scan-only tombstones, wired to the existing `bridge-tally-incremental` crate (well-shaped, just unwired).
8. **Kill the ceremony, keep the evidence** — rule adopted verbatim: *no safety mechanism without a demonstrated failure mode it prevents; no capability claim without a receipt.*
9. **Declared topology honesty** — v1 supports: local single-machine, loaded-company, licensed Tally, no TallyVault, no gateway auth. Tally-on-cloud/RDP (a large and growing install base!), multi-user LAN, gateway-security setups = explicit `Unsupported` Passport states, not silent failures.

### 3.2 Contested → rulings
| Item | Ruling |
|---|---|
| GSTR-2B recon | **Defer to Later (month 9+ gate)**, scoped to the *bulk-resolution* layer across many GSTINs (consume 2B JSON uploads; no portal OTP). Don't fight TallyPrime's flagship solo now; don't cede the only deadline-driven workflow forever. |
| Licensed-Tally timing | **Rent TallyPrime Silver in month 2** — before the first real write ships. Edu stays the daily regression rig; **nothing is ever marked `Verified` from Edu or simulator.** Cheapest de-risk in the plan. |
| Bank statements vs Excel first | **Excel/CSV first.** Same review-grid pipeline; bank statements arriving as CSV/Excel flow through unchanged. The bank-format zoo + PDF/OCR is a permanent maintenance tail — fast-follow, not v1. |
| Lead marketing claim | **Drift Sentinel + Proof-of-Post lead** (fear with a face; the answer to why firms churned). Proof-of-Sync/Passport are substance behind the demo, never the headline. Kill "data minimization" claim; rewrite to "full-fidelity, local, encrypted" in the same commit that un-minimizes reads. |
| Education-mode UX | Passport-detected restriction only. The "reschedule for the 31st" scheduling feature is **deleted** — a test constraint leaking into product design. |
| Multi-company control tower | Descoped to Later; redesigned around *expected staleness* ("open these 6 companies today" worklist) — a green wall over unloaded companies is the exact silent failure the Truth Layer exists to prevent. |
| Capability Passport | **Build it, don't sell it.** It's the internal gate, the 10-second "Run connection check" support self-test, and the topology-honesty vehicle. Never leads a pitch. |
| Remote agent on client machines | **Killed for this horizon** (solo dev cannot operate a fleet product; reputational risk lands on the firm). Drift v1 = firm-maintained books (typically ~half a firm's clients) — enough for the wedge. |

### 3.3 Killed (don't build)
Canary/attestation/dual-flag machinery and 6 of 8 digest newtypes · e-invoice/e-way bill (ClearTax's turf, needs GSP + TDL installs) · connected banking/payments (Tally native) · receivables dunning (CredFlow's company) · mobile dashboards (Biz Analyst's turf; no mobile asset) · AI OCR at scale (arms race vs funded teams; deterministic import covers ~70% provably) · TDL plugin with in-Tally UI · inventory depth/store-keeper flows · Education-mode posting scheduler · Period Freeze as a headline product (stays as plumbing) · bank-statement PDF/OCR parsing (v1) · GSTR-1 prep engine and TDS engine (rules-maintenance tails; revisit after month 12) · "80% time saved"-style unprovable claims and cryptographic-signature marketing language.

---

## 4. Strategy

### 4.1 Positioning
> For CA/CS firms burned by "sync issues" in every Tally companion app, Bridge is the two-way Tally integration that **proves** every read and write — posted means read-back-verified, and you know when anyone changes the books after you've signed off.

Marketable one-liners: *"Every competitor says 'synced.' Bridge proves it."* · *"Audit-grade sync for the audit profession."*

### 4.2 The wedge and the expansion
- **Acquisition wedge — Drift Sentinel** (read-only, ships first): "Know, firm-wide, every voucher your client changed after you signed off — with before/after." No incumbent equivalent (Tally's Edit Log can't be queried across companies; Finsights takes 24h to notice deletions). Sells a *liability fear* (closes faster than a time saving), lands inside firms **without asking them to abandon Suvit**, prices per audit client in audit season, and requires none of the unproven write path.
- **Expansion product — the verified write pipeline**: Excel/CSV import → saved mappings → review grid → maker-checker → serialized post → readback-verified Proof-of-Post. Spends the trust Drift earned.
- **Cold-start weapon:** Bridge reads 12 months of posted vouchers before ever writing — reverse-engineer narration→ledger mappings from history so suggestions are good on day one (the incumbents' mapping-history moat, neutralized structurally).

### 4.3 Live-demo proof points (on a real licensed TallyPrime, on messy books, never samples)
1. **The tamper catch:** checkpoint; someone edits one voucher, back-dates one, deletes one directly in Tally; Bridge lists exactly those three with diffs within one sync cycle — *including the back-dated one*.
2. **Completeness under fire:** kill Tally mid-sync; restart; Bridge reports exactly what is Verified vs Stale vs unread — no silent green, dual timestamps intact.
3. **Full fidelity on hostile data:** custom-TDL company, 100k vouchers — narration/GSTIN/bill refs matching to the paisa and character, "verified N minutes ago" live. (Post-writes, a fourth beat: watch a row flip "Posted — verifying…" → "Verified in Tally, 14:32".)

---

## 5. Roadmap (one developer + AI codegen; honest calendar)

### NOW — months 0–3: read-side truth becomes a sellable product
| # | Work | Exit criterion |
|---|---|---|
| 1 | **Unseal & simplify** (wks 1–3): delete canary/attestation/dual-flag machinery; writes compile in, gated by **one runtime per-company write allowlist (default off)** — the sole surviving gate (it prevents a demonstrated failure mode: dev build pointed at real books); generalize import-evidence parsing beyond ledgers | Canary code gone; write path compiles behind runtime consent |
| 2 | **Full-fidelity reads** (wks 3–8): narration, GSTIN/address, bill allocations, GST fields; quarantine lane for unknown TDL/UDF; encoding/normalization hardening; rewrite privacy docs + claims to "full-fidelity, local, encrypted" | Clean round-trip diff (export → canonical → re-export) on Edu across all wired voucher types |
| 3 | **Drift Sentinel v1 + Sync Beacon** (wks 8–12): checkpoint → changed/new/deleted/back-dated list with before/after diffs; segmented GUID+AlterID scans on `bridge-tally-incremental`; verified-scan-only tombstones; backup-restore re-baseline state; dual-timestamp Beacon | Demo proof points 1 & 2 pass on the licensed box |
| 4 | **Rent licensed TallyPrime (month 2)** — dedicated qualification VM; Edu demoted to regression rig | First real (signed) compatibility-matrix rows |

### NEXT — months 3–8: the write substrate, then the thin product
| # | Work | Exit criterion |
|---|---|---|
| 5 | **Write core**: outbox + batch-1 + readback verification + LASTVCHID cross-check + crash-mid-dispatch recovery; ledger create/alter | `Verified` on the licensed box, kill-test passes |
| 6 | **Voucher Create** (payment/receipt/journal/contra): UDF+fingerprint idempotency qualified per version; **Cancel** qualified as compensation; Alter-by-GUID qualified per version with Cancel+Create fallback saga | Voucher CRUD `Verified` (licensed); Edu restriction honestly surfaced |
| 7 | **The thin product loop**: Excel/CSV import → saved per-client mappings → Review grid (confidence *words* + inspectable rationale + per-row errors in accountant language) → Post Queue stepper (Draft→Validated→Previewed→Approved→Posting→Posted→**Verified**) → Proof-of-Post PDF. Single company. History-seeded ledger suggestions | — |
| 8 | **One design-partner firm**: scratch company on their licensed Tally first, then one real client | **Definition of done:** one article posts one client's weekly register for four consecutive weeks with zero unexplained, duplicated, or missing vouchers, and the partner files one Proof-of-Post pack |

### LATER — months 8–12: expand only what the wedge earned
Alter drafts + "Changed in Tally" chips in the Daybook · sales/purchase vouchers with GST ledger splits + party auto-create behind separate approval (GSTIN checksum, dedupe vs existing masters) · bank-statement CSV/Excel variants + visible/editable rule promotion · multi-client worklist designed around expected staleness and filing deadlines (7th/11th/20th) · master-hygiene reports (duplicate candidates, GSTIN checksum, propose-only) · **GSTR-2B bulk-resolution layer** (gated: substrate ran one clean quarter + partners asking) · concurrency hardening + 500-voucher soak + failure-mode playbook → GA.

### Explicitly deferred hooks (design-compatible, no code now)
- **AXAL/ComplyEaze:** relay the *evidence layer first* (proofs, receipts, drift alerts — small, non-sensitive payloads) via a versioned destination contract before ever moving raw books; preserves the privacy positioning while enabling richer cloud/AI features.
- **Pulse/WhatsApp:** drift alarms and posting-approval requests as messages (approval flows, not dunning).
- **Tally-on-cloud topology (addendum 2026-07-24):** hosted-RDP Tally (TallyOnCloud-style providers) is a large and growing install base that v1 declares `Unsupported` in the Passport. The eventual story is a headless Bridge agent running *inside* the hosted VM with the desktop UI attaching to its mirror — architecturally compatible with the loopback-only rule (the agent is loopback-local to Tally). Parked in BACKLOG.md; revisit when a design partner runs hosted Tally, not before GA of the local topology.
- **Client-maintained books (addendum 2026-07-24):** Drift Sentinel v1 covers firm-maintained books only (~half a typical firm's clients). The remote client-machine agent stays killed for this horizon, but two lighter paths can extend Drift coverage later and are parked in BACKLOG.md: (a) periodic client backup/TCP-file ingestion — diff a restored backup against the checkpoint mirror offline, no software on client machines; (b) the Finsights-style client-invitation model once a cloud relay exists. Neither blocks the wedge.

---

## 6. Engineering appendix (what to keep/simplify/delete)

**Keep (load-bearing):** company pinning fail-closed · single-writer serialization + circuit breaker (`bridge-tally-runtime`) · ExactDecimal · STATUS=1 enforcement · SQLCipher mirror + atomic checkpoints · `bridge-tally-incremental` tombstone/checkpoint model · import-evidence + readback parsers in `bridge-tally-protocol` (generalize beyond ledgers) · compatibility-matrix schema + Ed25519 receipt signing (worthless empty, differentiating populated) · fail-closed canonicalization for known fields (quarantine for unknown).

**Simplify:** two compile-time flags + attestations + sealed one-shot dispatch → one runtime per-company allowlist + per-batch approval · eight digest newtypes → two (payload, response) on the outbox row · qualification harness keeps receipt emission, loses synthetic-only orientation (simulator stays the regression suite; it can never mint `Verified`).

**Delete:** all `FIXTURE_CANARY_*` machinery, attestation apparatus, sealed dispatch envelope, "write capability: Unknown" dead-ends (replaced by Passport states fed from real receipts).

**Write-path invariants (non-negotiable):** row fsynced before dispatch · one object per import · readback + field diff before `CONFIRMED` (mismatch → `CONFIRMED_WITH_DIVERGENCE`, surfaced) · alters carry pre-image AlterID; concurrent foreign edit → `MANUAL`, never blind retry · deletion pre-checks references from the mirror · absence tombstones only from complete verified scans · a truncated scan never mass-tombstones · backup-restore (AlterID regression) → calm re-baseline, not tamper alarm.

---

## 7. Immediate next actions

1. **Merge PR #78** (it's a good, small company-discovery UX fix consistent with this plan).
2. Open the **M0 "unseal & simplify"** PR series: delete canary machinery, add the per-company write allowlist, generalize import-evidence parsing.
3. Start the **full-fidelity read** profile work (voucher FETCH extension + quarantine lane) — it gates everything.
4. **Budget the TallyPrime Silver rental** and stand up the qualification VM (month 2).
5. Rewrite `docs/tally/privacy-model.md` + README claims ("full-fidelity, local, encrypted") alongside the un-minimization commit.
6. Line up **one design-partner CA firm** with a licensed TallyPrime for the scratch-company qualification protocol.

---

## 8. Deviations from live evidence — 2026-07-29

First contact with a real Tally. Instance: **TallyPrime Edit Log (EL), Release 7.0, Educational Mode**, Windows 10, ODBC enabled, port 9000, **no TDLs configured**. Reached from the dev machine over an SSH loopback forward. Raw captures in `.bridge-live/captures/` (gitignored).

Where this section conflicts with §§0–7 above, **this section wins**. Nothing here is `Verified` — Education and Edit Log EL are both outside the matrix's promotion rules.

### 8.1 Writes work on Education mode

Three imports succeeded: two `LEDGER ACTION="Create"` and one `VOUCHER ACTION="Create"` (Journal, dated `20260401`), each returning `CREATED=1, ERRORS=0`, the voucher carrying `LASTVCHID=295`.

**Supersedes:** §2.5 ("a licensed instance is a hard prerequisite for a credible write story"), §3.2 Licensed-Tally-timing ("Rent TallyPrime Silver in month 2 — before the first real write ships"), §5 NOW item 4, and §7 item 4. The Education restriction is on the **voucher date** (1st, 2nd, 31st of a month), not on the calendar day of entry — so the write *mechanism* is testable continuously, for free, starting now. A licensed instance remains required to *certify* (`Verified`) but is no longer required to *build or de-risk*. Phase 4 moves earlier; the Silver rental moves later and loses its urgency.

### 8.2 `ERRORS=0` on a failed write — counter-based success detection is unsafe

A voucher dated `20260415` (illegal under Education) was rejected, and the response was:

```
<LINEERROR>Voucher date is missing for: 'Journal' voucher BRIDGE-PROBE-VCH-002...</LINEERROR>
<CREATED>0</CREATED>  <ERRORS>0</ERRORS>  <EXCEPTIONS>1</EXCEPTIONS>
```

**`ERRORS` stayed 0 on a failure.** The signal is `EXCEPTIONS=1` plus `LINEERROR`. Any success test of the form `ERRORS == 0` reports a rejected voucher as posted. Note also that the error text is *wrong*: the date was present, not missing — Tally's message describes neither the real cause nor a usable remediation.

**Adds to §6 write-path invariants:** a dispatch is successful only when the intended counter incremented **and** `ERRORS == 0` **and** `EXCEPTIONS == 0` **and** no `LINEERROR` is present. `LINEERROR` text is untrusted for cause attribution and must never be surfaced as a diagnosis.

### 8.3 Import responses carry no `STATUS` field

Import responses have a bare `<RESPONSE>` root — no `ENVELOPE`, no `HEADER`, no `STATUS`. `bridge-tally-protocol`'s `parse_import_outcome` already accepts this shape correctly (`lib.rs:1991`, extras at `lib.rs:2097`).

**Amends the PROMPT_PLAYBOOK GLOBAL RULES** clause "HTTP 200 is never Tally success; require application STATUS=1 parsing": that rule applies to **exports only**. Imports have no STATUS and are judged by the §8.2 rule. Left unsplit, an implementer will "correct" the parser into rejecting every successful write.

### 8.4 Duplicate voucher creates succeed — there is no natural idempotency

Re-sending the identical voucher payload, same `VOUCHERNUMBER`, produced `CREATED=1, LASTVCHID=296` — **a second voucher**. Tally does not dedupe on voucher number.

**Confirms §3.1.2 as load-bearing rather than defensive:** the UDF `BridgeTxnID` + `(date, amount, ledger-set, voucher-type)` fingerprint is the *only* thing standing between a crash-retry and a duplicated client voucher.

### 8.5 Re-creating an existing master silently becomes an Alter

Re-sending the identical ledger `ACTION="Create"` returned `CREATED=0, ALTERED=1` — no error. A retry silently **overwrites** the existing master with the retry payload, including any defaulted fields.

**Adds to §6:** master creates require a pre-existence read before dispatch, and `CREATED` vs `ALTERED` must persist as distinct outbox outcomes. "No duplicate was made" is not the same as "my create succeeded."

### 8.6 `LASTMID` is 0 on successful master creates; `LASTVCHID` works

Both ledger creates returned `LASTMID=0` despite `CREATED=1`. **Confirms §5.1.4's choice**: masters must be read back by normalized name. `LASTVCHID` is populated for vouchers and usable, still subject to the foreign-writer cross-check.

### 8.7 AlterID high-water marks move — Drift Sentinel's mechanism is sound

Company-level `ALTVCHID 440 → 441` (one voucher created) and `ALTMSTID 253 → 255` (two masters created), correlating exactly with the writes performed. §3.1.7's cheap-probe design is viable on this release.

### 8.8 CONFIRMED DEFECT — the export is bounded by the company period, not by the requested window

Two direct collection exports, one requesting `20260401`–`20260401` and one requesting `20260403`–`20260403`, returned **byte-identical result sets of 75 vouchers across 24 distinct dates spanning `20250401`–`20260302`** — i.e. FY 2025-26, the company's current period. `SVFROMDATE`/`SVTODATE` had no effect at all.

The decisive observation: the Journal voucher created in §8.1, dated `20260401` (FY 2026-27), returned `CREATED=1` with `LASTVCHID=295` and moved `ALTVCHID` — **and does not appear in the export at all.** The period is a hard visibility boundary, not a filter.

Three consequences, in ascending severity:

1. **The selected-read window is fiction.** A 31-day request returns the entire current period. `bridge.tally.vouchers/3` *echoes* the requested window (`$$String:##SVFromDate`) rather than enforcing it, so the response asserts a bound it never applied. This contradicts **ADR 0015**, which asserts V3 is exact-scope evidence bound to "an echoed exact `FROMDATE`/`TODATE` window". That bound has never held.

2. **Back-dated vouchers outside the current period are invisible.** §2.5 claims "date-unbounded scans catch them". False on this release — no scan sees outside the loaded period. Drift Sentinel must drive the period explicitly per FY segment. This makes §3.1.7's segmented-scan design mandatory for *correctness*, not merely for performance as stated.

3. **A successful write can be invisible to its own readback.** This is the dangerous one. Under §5.1.4, `CONFIRMED` requires readback; a write landing outside the current period readbacks as absent → `OUTCOME_UNKNOWN` → the §5.1.5 recovery path probes, finds nothing, and may re-dispatch. §8.4 proves Tally accepts the duplicate. **That is a demonstrated path to duplicating a client's voucher**, found before Phase 4 wrote a line of code.

**Mandatory additions to §6 write-path invariants:** every read profile must set the company period explicitly and assert the returned data's date span against it; a readback that lands outside the asserted period is `MANUAL`, never `OUTCOME_UNKNOWN`, and never re-dispatched. No read-window claim in the matrix, UI, or ADR 0015 is supportable until the profiles carry explicit period control.

### 8.9 Confirmed compatibility defect — V2 cannot produce boundary evidence

D2 sent the exact rendered `bridge.tally.vouchers/2` request (`BRIDGE Voucher Export V2`, company pinned, window `20260403`–`20260403`) to the lab instance. Result: **HTTP 000, 0 bytes, client timeout at 600 seconds.**

A second, earlier dispatch of the same request ran for **3,340 seconds (56 minutes)** before its client gave up, also returning zero bytes. Two independent attempts, 56 minutes and 10 minutes, produced no response at all.

The durable public evidence is limited to the exact request size, HTTP `000`,
zero response bytes, and the fact that the profile did not complete. The
operator-side diagnostic record, event signature, and isolation sequence stay
in gitignored `.bridge-live/`.

The public engineering rule is independent of those private diagnostics:

> No `$$` function may reference a TDL identifier containing spaces.

Four shipped profiles violate that rule. The V2 period-boundary question
therefore remains unmeasurable with the shipped report profile, while the
profile defect itself is confirmed. Phase 2 must replace the affected profile
family rather than extend it.

### 8.17 P0 SEVERITY — some `SVTODATE` values silently return zero rows

Qualifies §8.13. The `<FILTERS>` mechanism bounds correctly *when it returns anything*, but certain end dates make the entire query return zero rows for a range that provably contains vouchers. Deterministic: five identical requests, five zero results.

Measured, `SVFROMDATE=20250401` throughout, `FILTERS` on `$Date >= ##SVFromDate AND $Date <= ##SVToDate`:

| `SVTODATE` | Rows | Dates returned |
| --- | --- | --- |
| `20250401` | 4 | 20250401 |
| `20250402` | 7 | 20250401, 20250402 |
| **`20250430`** | **0** | — |
| `20250501` | 10 | …+ 20250501 |
| `20250601` | 17 | …+ 20250601 |
| **`20250629`** | **0** | — |
| **`20250630`** | **0** | — |
| `20250701` | 22 | …+ 20250701 |
| `20250731` | 26 | …+ 20250702 |

The `FETCH` list is irrelevant — `DATE` alone, `DATE, VOUCHERNUMBER`, and `DATE, VOUCHERNUMBER, ALLLEDGERENTRIES.*` all return zero at `20250630`. Only `SVTODATE` matters.

**ROOT CAUSE FOUND (same day, after further probing): `SVTODATE` must be a date Education mode considers valid — day 1, 2 or 31.** Any other day-of-month is silently ignored and the period widens to the entire book.

Twenty-three data points, no exceptions, with `SVFROMDATE=20250401` held constant:

| Day of month in `SVTODATE` | Behaviour |
| --- | --- |
| 1, 2, 31 (`20250401`, `20250402`, `20250501`, `20250531`, `20250601`, `20250701`, `20250702`, `20250731`, `20251231`, `20260102`, `20260131`) | **honoured** — data bounded to the request |
| 15, 28, 29, 30 (`20250415`, `20250428`, `20250429`, `20250430`, `20250530`, `20250628`, `20250629`, `20250630`, `20250730`, `20251215`, `20251230`, `20260115`) | **rejected** — period silently widens to the whole book |

This is the same 1st/2nd/31st restriction that governs voucher entry in Education mode, applied to period boundaries. `20250430` fails and `20250531` succeeds because April has no 31st while May does — which is why month-end appeared to explain some cases and not others.

**Severity is therefore Education-specific, not P0 for production** — with two caveats. First, this is **unverified on a licensed instance**; the assumption that licensed Tally accepts arbitrary dates is untested and must be confirmed before any claim rests on it. Second, one earlier observation does not fit the model: `w6-readback` used `SVFROMDATE=SVTODATE=20260401` (a valid day-1 date) yet returned 75 vouchers spanning FY 2025-26 rather than that single day. The `SVTODATE` rule is well-supported for the tested family; it is not yet a complete model of period resolution.

**Practical rule for the lab:** window boundaries must use day 1, 2 or 31. Any other boundary silently returns the whole book.

**The failure-mode lesson survives regardless of trigger**, and is the part worth engineering around: when Tally does not honour a requested period it does not error — it **silently substitutes a wider one**. Without a filter you get far too much data; with a filter you get zero. Both return `STATUS=1` and both look successful.

**Why this is P0.** A segmented scan choosing natural month boundaries — `1-Apr..30-Apr` — returns zero rows and looks like a *successful, complete, empty* scan. Not truncated, not an error, `STATUS=1`. Under §3.1.7's rules, absence from a complete verified scan produces deletion tombstones, so this would **tombstone an entire month of a client's vouchers as deleted**. §6's invariant "a truncated scan never mass-tombstones" does not help: this scan is not truncated.

**Mandatory invariant, independent of cause:** an empty segment is never trusted on its own evidence. Any window returning zero rows must be corroborated by a strictly wider window before the emptiness is treated as real; if the wider window returns rows, the segment result is `Partial` with a reason, never `complete`. A zero-row scan may not produce a tombstone under any circumstances without that corroboration.

**Consequence for §8.13:** its claim that Phase 2's date gate is "solved" is too strong. `<FILTERS>` is necessary and it bounds correctly, but it is not sufficient for safe segmentation. Deriving the failure rule — or proving corroboration is cheap enough to always apply — is now Phase 2 Unit A's first task.

### 8.18 Named `FETCH` sub-paths resolve one level deep only

Confirmed by the Unit A implementer and reproduced here. `ALLLEDGERENTRIES.LEDGERNAME` and the bill-allocation sub-paths resolve (206 allocations, 52 `BILLTYPE` values). `ALLLEDGERENTRIES.RATEDETAILS.GSTRATE` resolves to **zero** elements, as do `ALLLEDGERENTRIES.RATEDETAILS.*` and `ALLLEDGERENTRIES.RATEDETAILS`. The data exists — the same window under `ALLLEDGERENTRIES.*` yields 56 `GSTRATE` elements, and the full book yields 1,510.

`GSTRATE` sits at `VOUCHER > ALLLEDGERENTRIES.LIST > RATEDETAILS.LIST > GSTRATE` — two list levels deep. **Named curation appears to work one level down and not two.**

This forces a choice §8.15's rule did not anticipate: GST rate data is reachable only via `ALLLEDGERENTRIES.*`, at 19,658 B/voucher versus 3,142 B curated — the 6.3× penalty, for one field group. Options, none yet tested: a second request fetching only ledger entries wildcard for vouchers needing GST detail; a different named syntax; or accepting the wildcard cost for GST-bearing voucher types only. **Unit A must not silently adopt the wildcard**; the trade-off is an owner decision.

### 8.16 §8.8 RESOLVED — the period is a default, not a boundary. `<FILTERS>` reaches the whole book.

Four probes settle what §8.8 left open, and they overturn its causal claim.

**T1 — the company's own period fields** (`TYPE=Company`, native methods):

```
STARTINGFROM 20240401   BOOKSFROM 20240401
ENDINGAT     20260401   LASTVOUCHERDATE 20260401   ALTVCHID 442
```

The books run from 1-Apr-2024. The About screen's "CURRENT PERIOD 1-Apr-26 to 31-Mar-27" is a **display** period, not the data extent.

**T2 — collection with no date variables at all: 2 vouchers**, both dated `20260401` — the two probe journals. An unfiltered collection returns the *current display period*, which happens to contain only those two.

**T3 — collection with a `<FILTERS>` predicate spanning `20200101`–`20301231`: 150 vouchers**, earliest `20240401` (a Sales voucher never seen in any prior read).

**T4 — `<FILTERS>` on `20260401`–`20260402`: exactly the 2 probe vouchers.**

**Conclusions, superseding §8.8:**

1. **The period is a default, not a visibility boundary.** `<FILTERS>` crosses it freely. The whole book — 150 vouchers across two financial years — is reachable in one request.
2. **The earlier "75 vouchers, `20250401`–`20260302`" observation was an artifact of setting `SVFROMDATE`/`SVTODATE`.** Those variables shift which period is loaded without filtering precisely to the requested range. They are neither a filter nor inert, which is the worst of both and why they must never be relied on for scoping.
3. **§8.8 consequence 2 is withdrawn.** Back-dated vouchers outside the display period are *not* invisible. Drift Sentinel does not need to drive the period; a wide `<FILTERS>` predicate sees everything, and §3.1.7's segmentation is required only for payload size (§8.15), not for correctness.
4. **§8.8 consequence 3 is substantially withdrawn.** I described "a demonstrated path to duplicating a client's voucher" — a write landing outside the readback window, resolving to `OUTCOME_UNKNOWN`, then being re-dispatched into a duplicate. T4 shows a `<FILTERS>`-scoped readback finds the written voucher. The hazard was real for the *current* implementation, which uses none of this, but it is a consequence of the defective read path rather than an inherent Tally property, and it disappears once Phase 2 lands. **The §5.1.5 rule still stands** — a readback that cannot locate its object must escalate to `MANUAL`, never re-dispatch — because that rule costs nothing and covers the cases we have not seen.

**What survives from §8.8 unchanged:** `SVFROMDATE`/`SVTODATE` do not filter collection membership, `bridge.tally.vouchers/3` echoes a window it never enforces, and ADR 0015's "exact-scope evidence" claim is therefore unsupported. Those remain true and remain defects.

**Net effect:** the read model has no structural blind spot. Every object type is reachable, every date range is reachable, and the mechanism is one `<FILTERS>` clause. Phase 2's remaining hard problems are payload size and local balance computation — both bounded and measured.

### 8.14 `ClosingBalance` is not window-scoped — balances must be computed, not read

Same Ledger collection, four windows, reading `BRIDGE-PROBE-LEDGER-A` (whose only transactions are two journals dated `20260401`):

| `SVTODATE` | `ClosingBalance` |
| --- | --- |
| `20250630` (mid-period) | `-200.00` |
| `20260331` | **empty** |
| `20260501` | `-200.00` |
| *no window variables at all* | `-200.00` |

Three of four windows — including the baseline with **no date variables whatsoever** — return the identical figure. The window has no effect. And `-200.00` derives entirely from vouchers dated `20260401`, which lie outside three of the four windows tested.

**Ruling for Phase 2: a ledger's `ClosingBalance` is a lifetime/as-of-now figure and must never be used as a period balance.** Reading it per period yields a number that silently includes out-of-period transactions — a wrong balance presented as a correct one, which is the single most damaging defect class available to a reconciliation product.

Period balances must be **computed locally from window-filtered vouchers**, which §8.13's `<FILTERS>` mechanism now makes reliable. This moves work into the canonical layer and is more effort than reading a field, but it is the only version that produces a correct number.

The empty value at `SVTODATE=20260331` remains unexplained. Whatever its cause, an empty `TYPE="Amount"` must fail closed or quarantine — never coerce to `0.00`.

**Contradiction to resolve — §8.8's stated cause is doubtful.** §8.8 attributes the unfiltered export's `20250401`–`20260302` span to "the company's current period". The instance's About screen reports **CURRENT PERIOD 1-Apr-26 to 31-Mar-27** and **DATE OF LAST ENTRY 1-Apr-26**. If the current period were governing, the export would have *included* the `20260401` vouchers and *excluded* the FY 2025-26 range; it did the opposite. So the observed span is real and reproducible, but the period explanation is not established. §8.8's *observations* stand; its *cause* is unproven and should not be built on until someone probes it deliberately.

### 8.15 Curated `FETCH` cuts payload 6.3× — segmentation ceiling is ~10,000 vouchers

Same filtered two-day window, 6 vouchers, identical apart from the ledger-entry fetch:

| Fetch | Bytes | Per voucher |
| --- | --- | --- |
| Curated (`ALLLEDGERENTRIES.LEDGERNAME`, `.AMOUNT`, `.ISDEEMEDPOSITIVE`) | 18,852 | **3,142 B** |
| Wildcard (`ALLLEDGERENTRIES.*`) | 117,948 | 19,658 B |

**Dotted sub-paths in `FETCH` work** — the curated response retained 22 `LEDGERNAME` elements and all 6 narrations. Fine-grained curation is available, so §8.12's "never use `.*`" rule is implementable rather than aspirational.

**Concrete segmentation ceiling:** at 3,142 B per voucher, the 32 MiB response cap is reached at roughly **10,600 vouchers per request**. Monthly segments suffice for typical companies; high-volume clients need weekly or finer. This replaces the plan's qualitative "segment per FY/month" guidance with a measured number, and the segment size must be derived from observed per-voucher cost rather than assumed — a company carrying inventory lines or long narrations will run heavier than 3.1 KB.

### 8.15a Owner deviation (2026-07-30) — outstandings requires the wildcard

§8.15 remains valid for profiles whose named fields preserve their values. It is superseded
for bill-level outstandings. On an identical `20250401` window (1,632 vouchers, 4,894
allocations), curated `BILLALLOCATIONS.NAME/.BILLTYPE/.AMOUNT` returned zero named bills and
reported every typed allocation as `On Account`; `ALLLEDGERENTRIES.*` preserved one `New Ref`
and one `Agst Ref`. Because those two types open and settle a bill, the curated answer is
silently wrong for the Unit A computation.

Unit A therefore has one narrow wildcard exception. Its calibration read must use that same
wildcard request shape, and its segment plan must derive row capacity from observed bytes and
elapsed time. The older ~5,000-row default and ~10,600-row curated ceiling do not apply. A
two-pass candidate/detail design remains measurement work, not an implementation assumption.

### 8.13 Collection-based reads verified across every object type — Phase 2's date gate is solved

Probes G1–G4 on the live instance, all `TYPE=COLLECTION`, all returning `HEADER/STATUS=1`:

| Probe | Object type | Result |
| --- | --- | --- |
| G1 | `Group` | 28 rows, 18,651 B, 0.03 s |
| G2 | `VoucherType` | 24 rows, 17,594 B, 0.04 s |
| G3 | `Ledger` + `OpeningBalance`/`ClosingBalance` | 12,181 B |
| G4 | `Voucher` with a `<FILTERS>` date predicate | **6 rows, only the two requested dates** |

**G4 settles §8.12's one unproven assumption.** A collection carrying

```
<SYSTEM TYPE="Formulae" NAME="BridgeDateWindow">$Date &gt;= ##SVFromDate AND $Date &lt;= ##SVToDate</SYSTEM>
<FILTERS>BridgeDateWindow</FILTERS>
```

returned **6 vouchers dated only `20260301`–`20260302`**, against **75 vouchers across 24 dates** for the same collection unfiltered. Collection-level `<FILTERS>` genuinely bounds the window even though `SVFROMDATE`/`SVTODATE` alone do not (§8.8). The revised Phase 2 date-scoping design is proven, and it uses no `$$` function, so it stays clear of the §8.9 crash rule.

Combined with §8.10a, this means the collection shape is verified for **Company, Group, Ledger, VoucherType and Voucher** — every object type the CoreAccounting snapshot needs — while the report-based builders that the application actually calls are all defective. The replacement path is proven before the thing it replaces has ever worked.

**New Phase 2 hazard — empty computed values.** G3 was run over two windows. The narrower window returned `<CLOSINGBALANCE TYPE="Amount">-200.00</CLOSINGBALANCE>` and `200.00` for two ledgers; the wider window returned `<CLOSINGBALANCE TYPE="Amount"></CLOSINGBALANCE>` — **empty, not zero, not absent** — for the same ledgers. Two consequences:

1. Date variables *do* affect computed values even where they do not affect collection membership. Balance reads and object reads have different scoping semantics and must not share an assumption.
2. A parser that coerces an empty `Amount` to `0.00` would silently report a wrong balance. This is the NULL-vs-empty conflation the Phase 2 review hunt list already calls out, now with live evidence: **empty must fail closed or quarantine, never default.** Why the wider window empties the field is not yet understood and must be resolved before any balance is mirrored.

### 8.10a Blast radius correction (2026-07-29, second pass) — the app read path is a different, larger surface

The table below covers `bridge-tally-protocol/src/xml_read_profiles.rs`, which holds **2** violating instances. A verification sweep found a second TDL surface that the table missed: **`src-tauri/src/tally/tdl_engine.rs` holds 9**, and it — not `xml_read_profiles.rs` — is what the application actually calls.

`tdl_engine.rs` defines ten request builders. Five embed a `$$` function referencing a spaced identifier:

| Builder | Collection referenced | Called from |
| --- | --- | --- |
| `ledgers_request` | `BRIDGE Ledger Collection V1` | `connector.rs:122` |
| `groups_request` | `BRIDGE Group Collection V1` | `connector.rs:109` |
| `voucher_types_request` | `BRIDGE Voucher Type Collection V1` | `connector.rs:136` |
| `vouchers_request` (and `selected_vouchers_request`) | `BRIDGE Voucher Collection V1` | `connector.rs:154` |
| `ledger_period_balances_request` | `BRIDGE Ledger Period Collection V1` | `connector.rs:286` |

**Consequence:** the CoreAccounting snapshot reads groups, ledgers, voucher types, vouchers and ledger period balances. Every one of those five builders carries the defect. The snapshot pipeline — and therefore the canonical model, reconciliation, Proof-of-Sync and the mirror that depend on it — cannot ever have completed against a real Tally. Three of the five object types (Groups, Voucher Types, Ledger Period Balances) were never probed on 2026-07-29 and remain entirely unverified.

Only `company_list_request`, `standard_ledger_identity_request` and `standard_ledger_catalog_request` are clean — and `tdl_engine.rs` obtains all three by delegating to `xml_read_profiles::compatibility`, which is why they were the three that worked live.

**Phase 2 scope grows accordingly:** the collection rebuild in §8.12 must cover `tdl_engine.rs`'s builders, not only the seven profiles in `xml_read_profiles.rs`. Fixing one file would leave the live application path untouched.

### 8.10 Blast radius — 4 of 7 read profiles violate the identifier rule

| Profile | Spaced identifier referenced by a `$$` function |
| --- | --- |
| `company_list_v1` | no |
| `standard_ledger_identity_v1` | no |
| `standard_ledger_catalog_v1` | no |
| `ledgers_v1` | **yes** |
| `ledger_canary_readback_v1` | **yes** |
| `vouchers_v2` | **yes** |
| `vouchers_v3` | **yes** |

The three conforming profiles are discovery and name-listing surfaces. Every
profile intended to read accounting rows violates the retained safety rule and
must remain non-promotable until Phase 2 replaces it.

### 8.11 Why a naming correction alone does not save the report family

Private lab evidence also established two public-safe contract findings:

1. Custom report output depends on rendering geometry that the current profile
   family does not model explicitly.
2. The working direct-row response shape omits the `HEADER/STATUS=1` envelope
   required by Bridge's strict export parser.

The replacement direction is therefore collection-based, not a narrow rename:

| | Custom report (`TYPE=DATA`) | Direct collection (`TYPE=COLLECTION`) |
| --- | --- | --- |
| Identifier safety rule | violated by four shipped profiles | no violation observed |
| Returns `HEADER/STATUS=1` | no | yes |
| Response shape vs parser | mismatched | matches collection parsers |
| Voucher fidelity | fields declared individually | curated `FETCH` supports required nested data |
| Request size | substantially larger | substantially smaller |
| Existing Bridge precedent | discovery report only | `standard_ledger_identity_v1` |

Collections preserve the export-status invariant and align with existing
parsers. Replacing `render_vouchers`, `render_ledgers`, and their derivatives
is the opening move of Phase 2.

### 8.12 Phase 2, revised — collection-based full-fidelity reads

Supersedes §5 NOW item 2 and `PROMPT_PLAYBOOK.md` §3.1. The original phase
extended a custom-report design that violates the identifier rule, depends on
unmodelled rendering geometry, and returns a shape Bridge's strict parser
rejects. Extending it would amplify all three defects.

**Revised mission:** replace the custom-report profile family with `TYPE=COLLECTION` exports carrying full-fidelity `FETCH` lists, and delete the report renderers.

**Phase 2 is delivered as four units. This mission and the exit criterion below are
PHASE-level, not unit-level.** Deletion of the report renderers belongs to Unit C, not to
Unit A — the old path must keep compiling until the replacement is proven against a live
Tally, or a failed rewrite leaves the read path with neither implementation working.

| Unit | Scope | Deletes anything? |
| --- | --- | --- |
| **A** | One collection read profile + parser + tolerant entity handling + one outstandings screen, computed in memory | **No** — additive only |
| **B** | Canonical model, mirror schema, migrations | No |
| **C** | **Delete `render_vouchers` / `render_ledgers` / `render_ledger_canary_readback`; rewire `tdl_engine.rs` and `connector.rs`** | **Yes — this is where deletion happens** |
| **D** | Period balance computation from filtered vouchers | No |

A unit satisfies only its own scope. The Phase 2 exit criterion is met when A–D are all
complete.

**Design inputs established live on 2026-07-29:**

1. **Response shape.** A collection export returns `ENVELOPE > HEADER(VERSION, STATUS) > BODY > DESC(CMPINFO) + DATA > COLLECTION > VOUCHER|LEDGER`. It carries `STATUS=1`, so the export-side invariant is preserved rather than relaxed. The existing report-shape parsers (`ENVELOPE > BODY > LEDGER`) do not match and must be replaced, not adapted.
2. **Payload weight is the binding constraint.** A `FETCH` of `ALLLEDGERENTRIES.*` returned **1.5 MB for 75 vouchers — roughly 20 KB per voucher**, because `.*` also pulls `OLDAUDITENTRYIDS`, `AUDITENTRIES`, `INTERESTCOLLECTION` and every other sub-list. Extrapolated to a 100k-voucher company that is ~2 GB against a 32 MiB response cap. **Curated `FETCH` lists, not `.*`, and mandatory segmentation.** Field selection is now a size decision, not only a fidelity decision.
3. **Date scoping must be solved inside the collection.** `SVFROMDATE`/`SVTODATE` do not filter a collection (§8.8) — the company period governs, and objects outside it are invisible. The TDL mechanism for this is a collection `<FILTERS>` clause with a `<SYSTEM TYPE="Formulae">` predicate; Bridge already uses that pattern in `render_ledger_canary_readback`. A date-bounded read must be proven to actually bound, with a negative test showing an out-of-window voucher is excluded.
4. **Everything Phase 2 needs is available.** Confirmed present in a single collection response: `NARRATION`, `PARTYLEDGERNAME`, `PARTYGSTIN`, `ALTERID`, `MASTERID`, `GUID`, `REMOTEID`, `VCHKEY`, `ISCANCELLED`, `ISDELETED`, `ALLLEDGERENTRIES.LIST`, `BILLALLOCATIONS.LIST` (464 occurrences), `GSTRATE` (766 occurrences), `ALLINVENTORYENTRIES.LIST`.
5. **Quarantine matters more, not less.** A collection returns many fields Bridge does not model. The quarantine lane is now the primary mechanism for tolerating that, and the boundary between "unknown field, quarantine" and "unfetched field, ignore" must be explicit.

**Exit criterion (revised):** a date-bounded collection profile returns full-fidelity vouchers from the live Education instance with `STATUS=1`; an out-of-window voucher is provably excluded; a curated `FETCH` keeps a full-company read inside the response cap or segments honestly; the report renderers and their parsers are deleted; and no `$$` function references a spaced name anywhere in the tree.

The operator-side diagnostic record and any vendor disclosure material remain
only under gitignored `.bridge-live/`. The public plan intentionally records
no dialog text, failure signature, diagnostic filename, or isolation ladder.

### 8.8-orig (superseded framing, retained for audit)

Two direct collection exports, one requesting `20260401`–`20260401` and one requesting `20260403`–`20260403`, returned **byte-identical result sets of 75 vouchers** spanning 2025-04 to 2026-03. `SVFROMDATE`/`SVTODATE` were ignored entirely by a bare `<COLLECTION><TYPE>Voucher</TYPE>`.

`BRIDGE Voucher Collection V1` (`xml_read_profiles.rs:668`) has exactly that shape — no `<FILTER>`, no `<BELONGSTO>`, no date scoping — and relies solely on the same static variables. `bridge.tally.vouchers/3` **echoes** the requested window (`$$String:##SVFromDate`) rather than enforcing it, so a request would report the window it was asked for while returning the whole book.

If confirmed, this contradicts **ADR 0015**, which asserts `bridge.tally.vouchers/3` is exact-scope evidence bound to "an echoed exact `FROMDATE`/`TODATE` window", and it means the 31-day selected-read bound has never held.

**Not yet confirmed:** the probes used `TYPE=Collection` (direct collection export); Bridge uses `TYPE=Data` with a `REPORT`, whose period context may behave differently. One decisive test — send Bridge's exact V2 request and count distinct dates — settles it. **Until settled, no read-window claim in the matrix, UI, or ADR 0015 is supportable.**
