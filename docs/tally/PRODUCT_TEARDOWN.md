# Competitor teardown and the first-five-minutes problem

**Purpose.** What Tally companion products actually put on screen, what accountants respond
to, and what Bridge should build first. Written because Bridge currently has no demonstrable
value in a first demo — a diagnosis the founder made and the research confirms.

**Method.** Vendor material, review aggregators and practice-facing sources, July 2026. Where
a claim is a vendor's own marketing it is labelled as such. This is desk research, not
hands-on use of competitor products.

---

## 1. The diagnosis

Bridge's current first screen offers a capability passport, a gap map, truth states and a
mirror explorer. Every one of those describes **the sync**. None describes **the business**.

An accountant opening a Tally companion for the first time is asking *"what does this tell me
about my client's books that I didn't already know?"* Bridge currently answers *"here is how
confident we are about data you cannot see yet."* That is a second-order answer to a
first-order question, and it is why there is nothing to show in five minutes.

The plan's own §2.1 got this right — *"evidence must sit **under** workflows, not replace
them"* — and then the product was built the other way round.

---

## 2. What the market actually leads with

### 2.1 Consensus across every source

**Outstanding receivables with ageing analysis is the single most cited convincer.** It appears
in every practice-facing source reviewed, ahead of automation, ahead of reporting breadth,
ahead of AI. The reason is structural: it is the report that maps directly onto a business's
cash position, and it is the question a proprietor asks their accountant most often.

Standard shape: bill-by-bill outstandings, bucketed by age (0–30 / 31–60 / 60+, sometimes
custom slabs), sliceable by party, with drill-down from bucket → party → bill.

### 2.2 Biz Analyst — the scale leader's structure

1M+ installs, 4.2★. Vendor describes the first screen as *"all your business-related critical
figures in a single screen including Sales, Outstandings, Bank."*

| Section | Content |
| --- | --- |
| **Dashboard** | Sales, Outstandings, Bank — single screen, no navigation required |
| **Sales Analysis** | Top customers, best-selling items, growth trend, visual |
| **Outstanding & Receivables** | Aged outstandings — **plus send statement/ledger to the customer from the app** |
| **Reports** | Top customers, top suppliers, items sold, items purchased |
| **Inventory** | Item-wise stock, sales, purchase and price trends |
| **Daybook** | All vouchers in a period, filterable by type |

Two things worth noting. The dashboard is **one screen with three numbers** — not a
configurable tile grid. And Outstanding is the only section with an **outbound action**
attached; everything else is read-only viewing.

### 2.3 TallyPrime's own dashboard — the baseline to beat

Tally ships tiles for sales and purchase trends, trading details, assets & liabilities, cash
in/out flow, and order outstanding, arranged horizontally or vertically.

**Strategic consequence:** anything Bridge builds that merely *re-presents* these is
competing with a free feature already inside the product the customer owns. Bridge's screens
must answer questions Tally's dashboard cannot — which, given Tally's dashboard is per-company
and Bridge holds a mirror across many, is a real opening.

### 2.4 The AI/automation cohort

Vyapar TaxOne (ex-Suvit) and AI Accountant lead on **data-entry automation** — documents and
bank statements into Tally — not on reporting. Their demo value is "watch this invoice become
a voucher."

That is a different wedge from Bridge's, it is well-funded, and the plan already correctly
ruled against fighting it. Noted here only to be explicit that Bridge's first screen should
**not** try to be an ingestion demo.

---

## 3. What to steal, and what to deliberately not steal

### Steal

| Pattern | Why |
| --- | --- |
| **Outstandings, aged, as the landing screen** | Universally the most compelling report; directly answerable from data Bridge can already read |
| **One screen, few numbers, no configuration** | Biz Analyst's dashboard is three figures. Configurability is a power-user feature, not a first-demo feature |
| **Drill-down as the only navigation** | Bucket → party → bill. No menu tree to learn |
| **An action attached to the primary view** | Outstandings that can be *sent* beat outstandings you can only look at |
| **Party-centric framing** | Accountants think in parties, not in vouchers. Every competitor organises around party |

### Do not steal

| Pattern | Why not |
| --- | --- |
| Configurable tile dashboards | Tally already has one, free, inside the product |
| Inventory depth | The plan killed it; it is a different buyer |
| Mobile-first | Biz Analyst owns it; Bridge has no mobile asset |
| AI ingestion demos | Funded competitors own this; deterministic import is the plan's chosen ground |
| Feature breadth as a selling point | Every incumbent's worst reviews are about trust, not missing features |

---

## 4. What Bridge can uniquely show — and can build today

Everything below is buildable from **capabilities already verified against a live Tally**. No
writes, no licensed instance, no Phase 4.

| View | Data needed | Verified available? |
| --- | --- | --- |
| Outstandings aged by party | Bill allocations, party, amounts, dates | **Yes** — 464 bill-allocation occurrences in one capture |
| Sales / purchase trend by month | Voucher type, date, amount | **Yes** |
| Cash and bank position | Ledger balances by group | **Yes** (compute from vouchers, not `ClosingBalance` — see guide §6.4) |
| Top parties by exposure | Party ledger name, amounts | **Yes** |
| **"What changed since you last looked"** | `AlterID` filtering | **Yes — and the market believes this is impossible** |

That last row is the differentiator. Published community guidance holds that Tally cannot
filter server-side on anything but dates, so competitors re-download whole books and diff
locally. Bridge can ask for *only what changed* and get a handful of rows. Nobody is doing
this.

**The combination is the product:** a receivables view that is *demonstrably current*, with a
one-line "3 vouchers changed since your last check" that no competitor can offer, and the
evidence layer sitting underneath as the reason to believe it.

---

## 5. Recommended first screen

> **One screen. Outstandings aged by party, with a change indicator.**

```
Aarav Trading Company Demo          synced 4 min ago · 3 changes since yesterday
─────────────────────────────────────────────────────────────────────────────
Receivable      ₹ 48,21,660        Payable        ₹ 12,04,900
─────────────────────────────────────────────────────────────────────────────
                 0–30      31–60      61–90       90+
Receivable    ₹ 18.2L    ₹ 14.6L     ₹ 9.1L     ₹ 6.3L
─────────────────────────────────────────────────────────────────────────────
Top exposure                          outstanding      oldest bill
  Bright Retail Pvt Ltd                  ₹ 6,84,200        112 days
  Nova Components LLP                    ₹ 5,12,900         64 days
  …
```

Everything on it is derivable from verified reads. It answers a first-order question in the
first five seconds. And the two pieces of chrome — *"synced 4 min ago"* and *"3 changes since
yesterday"* — are exactly where the evidence layer earns its place: not as a screen of its own,
but as the line that makes the numbers trustworthy.

**Deliberately absent from the first screen:** capability passport, gap map, truth-state
matrix, mirror explorer. All retained, all one click away under an Evidence section, none of
them competing for the first impression.

---

## 6. What this implies for the roadmap

The plan sequences **Drift Sentinel** as the acquisition wedge. Drift needs sign-off
checkpoints, before/after history, tombstone logic and complete-scan guarantees — and the
guide's §2.8 shows tombstones are the most dangerous thing in the system, with four separate
routes to a false-empty read.

**Outstandings-aged needs none of that.** It is a pure read, computable today, and it is the
report the market says actually convinces.

**Suggested reordering:** ship the outstandings view first as the thing that earns attention;
add "what changed since you last looked" as the differentiator on top of it, which is a
smaller, safer subset of Drift; and let full Drift Sentinel — checkpoints, diffs, tombstones —
follow once the read path has proven itself in daily use.

This does not abandon the wedge. It sequences the wedge behind something demonstrable, and it
builds the same substrate either way.

---

## 7. Open questions this research did not settle

- Actual screen-by-screen UX of Vyapar TaxOne and AI Accountant — vendor sites describe
  features, not flows. Would need trial accounts.
- Whether CAs will pay for an outstandings view given Tally shows outstandings natively. The
  differentiator has to be *cross-company* and *provably current*, not the report itself.
- Pricing sensitivity for a read-only product. The GTM doc assumes reads are the free tier;
  if outstandings-aged is the hook, that assumption needs revisiting.

## 8. Changelog

| Date | Change |
| --- | --- |
| 2026-07-30 | Created. Desk research, July 2026. |
