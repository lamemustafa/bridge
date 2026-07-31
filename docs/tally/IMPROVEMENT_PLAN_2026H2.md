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
| **Biz Analyst** | 10 entry types incl. sales/purchase | Desktop sync agent | Two-way | 1M+ installs, 4.2★ (Play Store listing); complaints: "unending" sync issues, missing fields. **Data handling — claim withdrawn 2026-07-31, see Sources:** the vendor's privacy policy discloses sharing with group/affiliate entities and third-party service providers including marketing partners. An earlier revision of this row asserted "unencrypted data shared with third parties"; that was **unsupported** and is withdrawn. Whether such transfers are encrypted is **undetermined** — the policy's encryption language is scoped to stored data, and the Play Store data-safety page was not retrieved. No security conclusion is drawn in either direction. |
| **AI Accountant** | Vouchers, mappings, sales invoices | Local agent; XML for R/W, ODBC for analytics; **AlterID-tracked incremental sync** | Two-way, scheduled | Maker-checker approval, review-before-post with rationale, duplicate/voucher-lock handling; lists custom-TDL/UDF fields as a known break risk |
| **ClearTax connector** | e-invoice/e-way-bill fields | **TDL plugin inside Tally + connector app (ODBC)**; per-machine installs | Two-way (compliance fields) | Owns e-invoicing; in-Tally UI |
| **Tally native** (the platform threat) | — | — | — | Built-in GSTR-2B download + recon with granular status buckets (resolution still manual, per-company); TallyPrime 6.0 connected banking; 7.x AI features |
| **DIY long tail** | File-based XML import | Gateway of Tally → Import | One-way | NIKASH converters, TaxGuru VBA recon (6–10 hrs/GSTIN/month VLOOKUP baseline) — the actual majority workflow |

#### Sources for §2.1–2.4

Sources behind the claims in §2.1–2.4, listed so any contributor can re-check
or refresh them.

> **This block is NOT a complete provenance audit, and an earlier revision
> wrongly claimed it was.** This document has never been citation-complete;
> promising exhaustiveness simply moved the defect rather than fixing it. What
> follows is what has actually been sourced or explicitly marked. The remaining
> claims — and the rule that any statement about a named competitor's security
> or data handling must be sourced or removed rather than graded — are tracked
> in **[issue #103](https://github.com/lamemustafa/bridge/issues/103)**.
> Treat anything in §§0–4 not covered below as **unsourced until checked.**

Covered here:

- **§2.1** — every row of the landscape table (8 rows: 6 cited, 2 recorded
  gaps).
- **§2.2** — the regulatory claims in takeaway 4 (cited *and corrected*; see
  below).
- **§2.3** — the workflow figures ("50–200 companies per firm", hour pools)
  are **internal estimates from practitioner conversation, not measured or
  sourced.** The 7th/11th/20th filing rhythm is **planning shorthand, not a
  cited due-date table** — it does not state which returns or taxpayer
  categories it covers, so do not treat it as universal (#103).
- **§2.4** — prioritisation only, marked inline as an internal hypothesis.

- **Tally native** — [Import data](https://help.tallysolutions.com/import-data-in-tally/),
  [Understanding Excel import & mapping templates](https://help.tallysolutions.com/getting-started-with-importing-data-into-tallyprime/),
  [Import FAQ](https://help.tallysolutions.com/import-data-faq/),
  [Data-exception resolution](https://help.tallysolutions.com/tally-prime/import-data/data-exceptions-resolutions-in-tallyprime/),
  [Auto-create vouchers from bank statements](https://help.tallysolutions.com/auto-create-vouchers/).
  **Gap:** the row's **GSTR-2B reconciliation** and **7.x AI** claims are
  uncited, and §3.3 uses the platform-threat assessment to defer GSTR work.
  Unsourced until checked (#103).
- **Vyapar TaxOne** — [bank-statement import help](https://taxone.vyapar.com/help/articles/import-the-bank-statement).
  **Gap:** the scale figures ("10k+ CA firms, 30k+ accountants") and the
  "scale leader" label have no retrieved vendor page. Unsourced until
  checked (#103).
- **Biz Analyst** — [data entry](https://help.bizanalyst.in/features/data-entry/how-to-create-sales-invoice),
  [sync-failure behaviour](https://help.bizanalyst.in/biz-analyst-manual/support/sync-issues/all-figures-showing-0-in-mobile-app),
  [Play Store listing](https://play.google.com/store/apps/details?id=in.bizanalyst) and
  [Play Store data-safety page](https://play.google.com/store/apps/datasafety?id=in.bizanalyst)
  for install/rating figures, [privacy policy](https://bizanalyst.in/privacy.html)
  for data handling.
  **Correction — a claim about this vendor was wrong and has been struck.**
  The row previously asserted that the Play Store data-safety page "admits
  unencrypted data shared with third parties". The vendor's privacy policy
  does not support that, and **neither confirms nor refutes it** — the two
  disclosures are differently scoped, so state each with its scope and leave
  the question open:
  - **Sharing (supported):** *"We may transfer/share information (personal and
    non-personal) … with another SiliconVeins group / corporate entities or
    affiliates"* and *"to any third-party service providers including
    marketing partners"*.
  - **Encryption (scoped to storage, not to transfers):** *"including
    appropriate encryption and physical security measures to guard against
    unauthorized access to systems **where we store personal data**"*. This
    says nothing about whether data shared with third parties is encrypted.
  - **Play Store data-safety page:** **not retrieved.** No conclusion drawn.

  So the original allegation is withdrawn as **unsupported**, and the
  encryption question is left **undetermined** rather than answered in the
  other direction. A first attempt at this correction claimed the policy
  "states the opposite" on encryption — that replaced one unsupported
  vendor-security conclusion with another, and was itself withdrawn.
  **Publishing an unsourced security allegation about a named competitor in a
  public repository is the most serious defect this review surfaced**, and
  over-correcting into the opposite claim is the same defect wearing a
  friendlier face; it is handled rather
  than merely cited, and the "evidence grades" note below does not license
  claims of this kind.
- **CredFlow** — [company setup / connector requirements](https://credflow.freshdesk.com/support/solutions/articles/82000909704-how-to-add-company-for-tally-software-),
  [syncing-issues support folder](https://credflow.freshdesk.com/support/solutions/folders/82000694831)
  for the sync-reliability complaints that §2.2.2 uses to argue sync trust is
  the universal open wound.
- **AI Accountant** — [Tally integration](https://www.aiaccountant.com/blog/tally-integration-with-ai-accountant),
  [chart-of-accounts mapping](https://www.aiaccountant.com/blog/chart-of-accounts-ai-mapping).
- **ClearTax connector** — [Tally connector installation guide](https://docs.cleartax.in/e-invoicing-v2/userguide/guides-tally-connector/installation-guide-for-tally-connector),
  [Tally connector FAQ](https://docs.cleartax.in/e-invoicing-v2/userguide/guides-tally-connector/frequently-asked-questions-for-tally-connector),
  [Tally add-on overview](https://docs.cleartax.in/cleartax-docs/add-ons-and-extensions/tally-add-on).
  **Terminology correction:** the guide confirms the **ODBC port** dependency
  and **per-machine installation** ("install the application and activate the
  company in both the server and client systems"), and describes pasting a
  **TCP path** into Tally — the compiled add-on artifact — rather than the
  phrase "TDL plugin" used in §2.1 and §3.3. The architecture claim stands;
  the wording in the table is looser than the source.
- **Finsights** — **gap 1: no verifiable citation. See below.**
- **Regulatory claims (§2.2.4)** — **GST:** GSTN
  [interest-calculation advisory](https://tutorial.gst.gov.in/downloads/news/advisory_on_interest_calculator.pdf)
  (the January-2026 item, which concerns interest, *not* ITC);
  [hard-locking explainer](https://cleartax.in/s/hard-locking-in-gstr-3b) and
  [Table 3 rollout summary](https://www.indiafilings.com/learn/hard-locking-in-gstr-3b)
  for the July-2025 outward-liability locking;
  [planned Table 4 ITC locking](https://www.taxscan.in/top-stories/big-gst-change-from-july-2026-gstr-3b-itc-locking-explained-1448389)
  for the ~July-2026 target.
  **MCA:** Companies (Accounts) Amendment Rules, 2021 — see
  [the rule text and applicability discussion](https://taxguru.in/company-law/companies-mandates-use-software-audit-trail-each-transaction.html)
  and [the notification summary](https://lexplosion.in/mca-issues-companies-accounts-amendment-rules-2021-companies-using-accounting-software-for-maintaining-books-of-account-mandated-to-use-software-that-allows-recording-of-audit-trail-of-every-trans/).
  > **Honest limitation on these two.** Apart from the GSTN interest advisory
  > PDF, every link above is a **secondary summary** — a tax-practice or
  > news write-up of a GSTN advisory or an MCA notification. **The primary
  > GSTN hard-locking advisories and the MCA gazette notification have not
  > been retrieved for this document.** They are good enough to establish that
  > the *previous* §2.2.4 claim was wrong, which is why it was corrected, but
  > **not** good enough to quote commercially. Anyone relying on the dates
  > must go to the GSTN advisory and the gazette text directly.
- **DIY long tail** — **gap 2: uncited.** The "NIKASH converters, TaxGuru VBA
  recon" examples and the **"6–10 hrs/GSTIN/month VLOOKUP baseline"** have no
  source in this repo. That baseline is the implicit denominator for any
  time-saving argument, so treat it as an **internal estimate, not a measured
  figure**, and do not quote it externally until someone sources or measures
  it.

**Open gap — the Finsights 24-hour deletion latency is uncited, and a wedge
rests on it.** §2.1 states that Finsights propagates Tally deletions only every
24 hours; §0 repeats it as market validation and **§4.2 leans on it directly**
to argue Drift Sentinel is the acquisition wedge ("Finsights takes 24h to
notice deletions"). That figure originates from vendor-published help material
that could not be re-fetched at the time of writing (`finsights.biz` returned
HTTP 403), so it is recorded here as **vendor-stated, not independently
measured, and currently unverifiable from this repo.** It is a competitor
behaviour we have never observed ourselves. Treat the wedge argument as
provisional until someone re-fetches the vendor page or measures the latency
directly; do not cite the number onward as established fact.

**Evidence grades within the table.** Mechanism, sync model and documented
behaviour come from the vendor documentation above. Scale figures ("10k+ CA
firms", "1M+ installs, 4.2★") are **vendor or app-store marketing claims,
reproduced as claims, not independently audited.** Complaint patterns are
**review-signal, not measurement.** Treat the first as citable, the second two
as directional.

> **What is held privately, and what does not depend on it.** Competitor
> *pricing*, a synthesised UX teardown, and a deeper landscape sweep
> (Zoho/Munim/Open/EnKash/GST connectors/Tally-native remote) live in the
> private lamemustafa knowledge hub rather than in this public repo, because
> they are commercially sensitive. **No ruling in this document rests on that
> private material** — the rulings below follow from the public vendor sources
> above plus code analysis of this repository, and a contributor without hub
> access can verify and challenge all of them. Where private research informed
> a *prioritisation* rather than a finding, it is marked inline as an internal
> hypothesis — see §2.4.
>
> **No live-Tally probe record is checked in on this branch.** Do not read the
> rulings as backed by recorded measurements against a running Tally: the
> formal evidence system is **empty** — every compatibility-matrix cell is
> `unknown` with `evidence_id: null`, which §1 states outright ("the evidence
> product has no evidence") — and `docs/tally/compatibility/evidence/` contains
> no evidence files. Live Education-mode probe findings exist only on the
> unmerged `feat/tally-outstandings-slice` branch and are **not** part of this
> document's public support until that work lands. Nothing here is
> compatibility-qualified, and no claim is backed by a licensed instance,
> because Bridge has never run against one.

### 2.2 Structural takeaways
1. **The on-prem connector is unavoidable and Bridge already is one** — with a stronger engineering base than the connectors CAs complain about.
2. **Sync trust is the universal open wound.** Every incumbent's worst reviews are trust failures. None can prove completeness, attribute failures, or detect Tally-side edits/deletions promptly.
3. **Tally native is absorbing adjacent value** (2B recon, banking, AI): pure-reporting and portal-integration plays erode. Data-entry automation, multi-client practice ops, and *evidence about the books* remain defensible.
4. **Regulatory tailwind — CORRECTED 2026-07-31, the original claim was wrong.** This previously read *"since Jan 2026, excess ITC vs GSTR-2B auto-flags on the portal."* Checking it against public GSTN advisories shows that is wrong on both the date and the mechanism: GSTR-3B hard-locking is phased, **Table 3 (outward liability) has been locked since the July 2025 period**, and **ITC (Table 4) locking is targeted for around July 2026 — announced, not yet in force** — and it is *hard-locking* to GSTR-2B, not an "auto-flag". The January 2026 advisory concerned **interest calculation**, not ITC. The MCA half stands: the **Companies (Accounts) Amendment Rules, 2021** (notified 24-03-2021) require accounting software to record an audit trail and edit log that cannot be disabled, applicable for financial years beginning on or after **1 April 2023** — which does make "what changed in the books" a partner-level anxiety. **Net effect on the argument: the MCA tailwind is real and already in force; the ITC tailwind is a near-term expectation, not a present fact, and must not be sold as one.**
5. **Education mode:** competitors refuse it (CredFlow). It permits voucher entry only on the 1st/2nd/31st. It is a fine regression rig and an honest Passport state — but nothing can be marked `Verified` from it, and a licensed instance is a hard prerequisite for a credible write story.

### 2.3 CA workflows that consume the hours
- **Bank statement → vouchers** (the biggest hour pool; ledger suggestions learned per-client from narration patterns).
- **Excel/CSV registers → vouchers** (pure transcription; saved per-client column mappings make month 2 near-zero-touch).
- **GSTR-2B ↔ purchase register recon** (fuzzy multi-field matching, exception queues; Tally native buckets well but resolves one voucher at a time, one company at a time).
- **Receivables follow-up** (CredFlow's turf; skip).
- **Multi-client management** (50–200 companies per firm, staff roles, per-client sync health, deadline rhythm: 7th/11th/20th).
- **Audit/verification** ("what changed since I signed off" — served by *nobody*).

### 2.4 UX patterns to steal / fix

> **Internal hypothesis — prioritisation only.** *Which* of these patterns to build first
> was ranked with the help of private comparative research (see §2.1 Sources). The
> observations themselves are all traceable to the public vendor documentation cited there;
> the ordering is an internal bet and should be challenged on its merits, not accepted on
> authority.

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
