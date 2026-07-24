# Bridge × Tally: commercial model, support & go-to-market

**Date:** 2026-07-24 · Companion to [IMPROVEMENT_PLAN_2026H2.md](./IMPROVEMENT_PLAN_2026H2.md).
Resolves the open commercial questions the plan deferred and the adoption
critic raised: if the code is open source, what is sold? who does a partner
call at 9 PM on the 10th? per-entity or per-firm pricing? how does a solo-dev
open-source tool overcome the incumbent's retraining moat?

> Pricing figures below are **decision hypotheses** to validate against the
> live competitor-pricing research and the first design-partner conversations.
> Every number tagged _(validate)_ is a starting anchor, not a committed price.

---

## 1. The three questions, answered decisively

1. **What is sold, given the code is open?** → **Open-core.** The engine is
   open and checkable (that *is* the trust brand); the commercial product is
   the signed desktop build + the paid capability tiers + support + updates +
   the evidence/compliance surfaces. You pay for assurance and outcomes, not
   for source you could read.
2. **Who does the firm call?** → A **legal operating entity** with a brand, a
   GST number, a DPA, and a **deadline-aware support SLA** (faster guaranteed
   response around the 7th/11th/20th). "One developer + AI codegen" is an
   internal fact, never a customer-facing disclosure; the customer buys a
   company with a support commitment.
3. **Per-entity or per-firm?** → **Per firm, banded by client count** — this
   is the *actual* CA-practice norm (Vyapar TaxOne: ₹10,000/yr flat, unlimited
   clients ≈ ₹833/mo; AI Accountant: ₹3k/₹15k/₹20k tiers for ≤1/≤10/≤25
   clients). Per-entity is the SME-owner tools (CredFlow, Biz Analyst) and
   would make Bridge look absurd at 80 clients against TaxOne's flat rate. The
   **Drift Sentinel wedge is the exception — sold seasonally per audit client**,
   because that maps to how audit engagements are already scoped and billed.

---

## 2. Open-core boundary (what's free vs paid)

| Layer | License | Rationale |
| --- | --- | --- |
| Rust engine crates (transport, protocol, canonical, incremental, evidence signing) | **Open source** | The verifiability claim must be checkable; open source *is* the moat, not a giveaway of the moat. |
| Deterministic simulator + fixtures | **Open source** | Contributor reproducibility; no book data. |
| Read/snapshot/mirror + Sync Beacon | **Free tier** (signed build) | Land motion — costs nothing to run, seeds the habit, demonstrates honesty. |
| **Drift Sentinel** | **Paid** (wedge SKU) | The uncontested capability; the acquisition wedge. |
| **Write pipeline** (Excel→review→post→Proof-of-Post, maker-checker, mapping rules) | **Paid** | The expansion product; the daily-use value. |
| Multi-client console, evidence/compliance exports, priority support | **Paid (firm tier)** | Renewal drivers. |
| Signed/notarized installers, auto-update, per-version qualification receipts | **Paid** | Assurance the free/self-built path doesn't carry. |

Rule: anything that is a *checkable trust claim* stays open (so a skeptic can
verify it); anything that is an *outcome or assurance* is paid. This keeps the
"honesty brand" and the revenue model from contradicting each other.

## 3. Pricing model

**Unit:** **per firm, banded by client-count** — the CA-practice convention
(TaxOne flat-unlimited; AI Accountant client-count tiers). Not per user (firms
add articles seasonally and would game it). Not pure per-entity (that's the
SME-owner model and prices Bridge out of a 50–80-client firm against TaxOne's
₹833/mo). Drift is the one seasonal per-audit-client exception.

**Competitive price anchors (cited, July 2026):**
- Vyapar TaxOne (leader): **₹10,000/yr flat, unlimited clients** (CA SKU),
  ₹12,000/yr Advocate/Accountant; ICAI-CMP discount channel. ≈ **₹833/mo**.
- AI Accountant: **₹3,000 (≤1 client) / ₹15,000 (≤10) / ₹20,000 (≤25) /
  Platinum custom** per year.
- CredFlow / Biz Analyst: per-company / per-device (SME model) — expensive at CA
  scale, not the comparison set.
- TallyPrime base (context): Silver ₹22,500 one-time or ~₹750/mo rental + TSS
  ₹4,500/yr.

**The truth this forces:** Bridge **cannot win the CA segment on price** —
TaxOne's ₹833/mo unlimited is a floor no solo product should undercut. Bridge
must price *at a premium* justified by a capability TaxOne lacks (Drift,
Proof-of-Post verification), not compete on the wrong axis. The brand is
assurance, and assurance carries a premium.

**Tiers (hypotheses to validate with design partners):**

| Tier | Who | What | Anchor _(validate)_ |
| --- | --- | --- | --- |
| **Free — Verify** | Any firm | Reads, snapshot mirror, Sync Beacon, connection self-test | ₹0 |
| **Drift** (seasonal) | Audit-season buyers | Drift Sentinel per audit client, checkpoints, drift packs | per audit client per month, billed for the audit window _(validate vs ~₹500–1,000/entity TaxOne band)_ |
| **Post** | Firms doing daily entry | Everything in Drift + write pipeline + Proof-of-Post + mapping rules, single-company workspaces | per active entity per month |
| **Firm** | Multi-client firms | Post + multi-client console + priority deadline-aware support + evidence exports | per-entity with a firm-level floor + volume bands (50/80/150 clients) |

**Packaging notes:**
- Drift is deliberately a **seasonal, low-commitment** entry — it lands inside
  the firm during audit season without displacing Suvit, then Post/Firm expand
  once trust is earned.
- **Free trial:** 30 days of Post on up to 3 real client entities, with the
  history-seeded mapping suggestions active (so the trial is *not* the
  cold-start-worst-case the incumbent's month-36 engine beats — see §5).
- Anchor to **at or below Suvit/TaxOne per-entity parity** for Post; charge the
  premium only where Bridge has a capability the incumbent lacks (Drift,
  Proof-of-Post), never for parity features.

## 4. Support & SLA (the churn-objection answer)

The profession is deadline-driven; support latency is the #1 post-sale
complaint across CredFlow/Biz Analyst. Turn it into a differentiator.

- **Deadline-aware SLA:** guaranteed response windows tighten around statutory
  deadlines (TDS 7th, GSTR-1 11th, GSTR-3B 20th) — publish the calendar.
- **In-app diagnostics first:** the "Run connection check" self-test and the
  accountant-language error catalog deflect the most common tickets (gateway
  off, wrong company open, Tally closed) before they become calls.
- **Named-contact for Firm tier;** business-hours email for Post; community +
  docs for Free.
- **Status transparency:** a public status/known-issues page — the same
  honesty brand applied to operations.
- Solo-dev reality is managed by **deflection (self-test + catalog) + scope
  discipline (one supported topology) + async SLA**, not by pretending a
  24×7 desk exists.

## 5. Overcoming the incumbent retraining moat (cold-start)

The real switching cost is ledger-mapping history the incumbent has and Bridge
doesn't. Bridge's structural counter (turn the liability into an advantage):

- **Read-before-write history seeding:** on connect, mine 12 months of posted
  vouchers from the mirror into narration→ledger candidate rules, so Post's
  suggestions are useful on **day one**, not month 36.
- **Mapping import:** ingest a firm's existing mappings (from a TaxOne/Excel
  export where available) as a starting rule set.
- **Drift-first land:** Drift needs *no* mapping at all — it's read-only — so
  the firm adopts Bridge before ever confronting the mapping-retraining cost;
  by the time they try Post, Bridge has already read their history.

## 6. Data-handling posture (turn "local" into a sellable answer)

Local-first beats cloud for the paranoid partner, but "local SQLCipher on an
article's laptop" is *new* client-data sprawl the firm must answer for.

- **Data-map screen:** which machines hold which clients' encrypted mirrors,
  key custody, and a one-click wipe — so a partner can answer "who has copies
  of Sharma Exports' books?" (implement alongside multi-client; parked in
  [BACKLOG.md](./BACKLOG.md) until then).
- **DPA + sub-processor list:** a signable data-processing addendum so the firm
  can extend its own client data-handling representations to Bridge.
- **Deletion/erasure workflow:** finish the currently-unimplemented mirror
  erasure path before claiming "fully removable" (privacy-model.md gap).
- Marketing claim stays exactly: **"full-fidelity, local, SQLCipher-encrypted;
  nothing leaves the machine"** — checkable because the engine is open.

## 7. Go-to-market motion

1. **Design partners (months 0–6):** 3–5 CA firms with licensed TallyPrime.
   Recruit via direct CA network + the ComplyEaze/AXAL relationship. They get
   Drift + early Post free in exchange for the scratch-company qualification
   protocol and a reference. Definition-of-done (plan §NEXT) is the reference.
2. **Wedge land (months 3–9):** Drift Sentinel as an **audit-season product** —
   "know every voucher your client changed after you signed off." Fear-led,
   uncontested, no incumbent displacement required.
3. **Content + community:** CA-community channels (CAclubindia, ICAI study
   circles, LinkedIn CA cohorts), publishing the *evidence* angle (Proof-of-
   Post packs, drift catches) as concrete demos, not adjectives.
4. **Expansion (months 8–12):** Post/Firm upsell into landed Drift accounts;
   the multi-client console sells the renewal.
5. **Later:** ComplyEaze/AXAL cloud relay (evidence layer first) unlocks
   mobile/WhatsApp (Pulse) approval flows — a second wedge, not a pivot.

**Anti-goals:** no paid ads arms race vs funded incumbents; no conference-badge
"verifiability" pitch that gets nods and zero trials (lead with the fear/job,
substantiate with evidence); no distribution through Tally partners (channel
conflict with the platform whose gaps Bridge exploits).

## 8. Live-demo proof points (from the plan — the sales moment)

On a real licensed TallyPrime, messy books, never samples:
1. **Tamper catch** — edit/back-date/delete three vouchers in Tally; Bridge
   lists exactly those three with before/after diffs, including the back-dated
   one.
2. **Completeness under fire** — kill Tally mid-sync; Bridge reports exactly
   what's Verified vs Stale vs unread; no silent green.
3. **Full fidelity on hostile data** — custom-TDL company, 100k vouchers;
   narration/GSTIN/bill refs match to the paisa and character, live.

The sentence that closes: _"Every entry my juniors post is approved, verified
against Tally, and evidenced; and I know within a day if a client edits a
voucher I've already signed off."_

---

## 9. Open decisions for the founder

| Decision | Options | Recommendation |
| --- | --- | --- |
| Legal entity / brand for the commercial build | New entity vs under ComplyEaze | Under an existing entity if one carries the GST/DPA; a customer-facing brand is mandatory before first paid deal. |
| Drift seasonal vs annual pricing | Per-audit-client seasonal vs flat annual | Seasonal to lower the entry barrier; convert to annual on renewal. |
| Free-tier generosity | Reads free forever vs time-limited | Reads + Beacon free forever (land motion); Drift/Post paid. |
| License for the open crates | Permissive (MIT/Apache) vs copyleft (AGPL) | AGPL for the engine to deter a funded incumbent from absorbing it cloud-side while keeping it checkable; commercial license for the paid build. _(validate with counsel.)_ |
