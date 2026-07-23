# Market research addendum (2026-07-24)

Extends the landscape in [IMPROVEMENT_PLAN_2026H2.md](./IMPROVEMENT_PLAN_2026H2.md) §2
with three gap-coverage passes: missing products, competitor pricing, and a
public-record UX teardown. Every claim is cited; items that could not be
confirmed in a primary source are marked **UNVERIFIED**.

---

## A. Expanded landscape (products not in the original table)

| Vendor | Read/Write into Tally | Mechanism | Direction | Notable |
| --- | --- | --- | --- | --- |
| **Zoho Books** | Read (import) | Manual file export/import | One-way, one-time **migration** | A *replacement* for Tally, not a co-existing sync partner — removes it as a live competitor. ([Zoho migration help](https://www.zoho.com/in/books/help/migration/tally-to-zoho-books.html)) |
| **Munim** | Read | "Tally Connector" (transport UNVERIFIED) | One-way → Munim (GST prep) | Low-cost Tally alternative; ₹3,299+/yr. ([Munim helpdesk](https://themunim.com/helpdesk/how-to-import-your-data-to-munim-gst/)) |
| **Open (open.money)** | **Write (claimed)** | **UNVERIFIED** (method undisclosed) | Two-way; auto-JV per transaction | Only banking/AP player claiming to auto-create vouchers *inside* Tally — but hides how. ([Open blog](https://open.money/blog/tally-synchronisation-integration-with-open/)) |
| **EnKash** | Sync/reconcile (write UNVERIFIED) | UNVERIFIED | Reconciliation-oriented | "Vouchers" in its marketing = its rewards product, not Tally. ([EnKash](https://www.enkash.com/auto-reconciliation)) |
| **PayMate** | None confirmed | Generic ERP API | — | No confirmed Tally voucher write. ([PayMate](https://paymate.in/enterprise.html)) |
| **GSTZen** | **Read + write-back (IRN/QR into voucher)** | TDL add-on + port-9000 XML + **Chrome extension** bridge | Two-way (e-invoicing) | Chrome-only; multi-step setup. Same class as ClearTax. ([GSTZen](https://gstzen.in/einvoicing/methods-of-e-invoice/e-invoice-integration-for-tally-prime.html)) |
| **MasterGST (Masters India)** | Read (write-back UNVERIFIED) | DLL connector plugin | One-way export | Invoices managed in MasterGST. ([MasterGST](https://mastergst.com/tally-integration-connector-for-einvoice.html)) |
| **Cygnet** | Read + return JSON/PDF | Pre-built connectors/APIs (enterprise GSP) | Round-trip (in-voucher write UNVERIFIED) | Enterprise-skewed. "Cygnature" is a separate e-sign product. ([Cygnet](https://www.cygnet.one/products/e-invoicing/india)) |
| **Hisabkitab / Giddh / Refrens / Hisab** | Read/sync (varies; some UNVERIFIED) | Tally Connector / migration plugin | Mostly one-way | Newer AI/cloud entrants; Giddh is migration-flavored. |

### Tally's own remote/mobile/audit capabilities (competitive baseline)

- **Remote Access (Tally.NET):** full read+write remotely, but needs a **local
  TallyPrime client install + active TSS**; encrypted XML/HTTP; concurrency by
  license (Silver 1 / Gold 10). Not a mobile story. ([TallyHelp](https://help.tallysolutions.com/tally-prime/connected-services/remote-access-faq-tally/))
- **Reports in Browser (TRiB):** **read-only** — "you can only view and
  download vouchers in a browser," no create/edit; report generated in the
  local client and streamed; **no native mobile app**. ([TallyHelp](https://help.tallysolutions.com/tally-prime/connected-services/browser-reports-faq-tally/))
  → **Strategic point:** Tally's *own* mobile/browser offering is read-only with
  no native app. A write-capable, evidence-backed local tool has real whitespace.
- **Edit Log (MCA audit trail, mandatory since 1 Apr 2023):** records
  create/alter/delete for transactions **and** masters — who, when, action,
  before/after values; tamper-proof. **Two caveats material to Bridge:**
  1. Programmatic export of the Edit Log change-history via XML/ODBC is
     **UNVERIFIED / probably unsupported** — it's an in-product report
     (PDF/Excel), not a queryable API stream. (Do not build Drift on the
     assumption you can read Tally's Edit Log over the gateway.)
  2. A third-party gateway/TDL write is attributed in the Edit Log to the
     **logged-in Tally user of that instance** (typically admin), not a distinct
     connector identity (mechanistic inference; a licensed-lab test item — see
     [LICENSED_LAB_QUALIFICATION_CHECKLIST.md](./LICENSED_LAB_QUALIFICATION_CHECKLIST.md)
     D1). → **This confirms the plan's rule: Proof-of-Post is *supplementary*
     workpaper evidence, never MCA-Edit-Log equivalence.** ([Tally: Audit Trail](https://tallysolutions.com/tally/audit-trail-in-tallyprime/))

**Cross-cutting:** GST/e-invoicing connectors (GSTZen, MasterGST, ClearTax)
converge on the **TDL-plugin + port-9000** pattern with per-machine installs —
the opposite of Bridge's zero-install-in-Tally posture. Confirms the plan's
"don't build e-invoicing" ruling and the clean-architecture differentiation.

## B. Pricing (cited, July 2026)

**The CA-practice pricing unit is per-firm (often flat/unlimited or client-count
banded), NOT per-entity.** Per-entity/per-device is the SME-owner segment.

| Product | Price (INR) | Unit | Source |
| --- | --- | --- | --- |
| **Vyapar TaxOne** (leader) | ₹10,000/yr (CA, **unlimited clients**); ₹12,000/yr Advocate/Accountant | Per firm, flat | [taxone.vyapar.com/pricing](https://taxone.vyapar.com/pricing) |
| **AI Accountant** | ₹3,000 (≤1) / ₹15,000 (≤10) / ₹20,000 (≤25) / Platinum custom | Per firm, banded by client count | [softwaresuggest](https://www.softwaresuggest.com/ai-accountant) (Sep 2025) |
| **CredFlow** | ₹3,499–14,999/yr tiers (or ₹999–2,499/mo) | Per company/entity | [techjockey](https://www.techjockey.com/detail/credflow) |
| **Biz Analyst** | from ₹250/mo | Per device per Tally license | [help.bizanalyst.in](https://help.bizanalyst.in/biz-analyst-manual/faqs/pricing) |
| **Finsights** | Tally On The Go ₹999; WhatsApp Alerts ₹2,999 | Module/subscription | [technologycounter](https://technologycounter.com/products/finsights) |
| **ClearTax connector** | from ₹499/mo + volume tiers | Per module + volume | [aidukan](https://aidukan.in/cleartax-price-india/) |
| **Vouchrit / TallyGraphs** | NOT FOUND (custom / dormant) | — | — |
| **TallyPrime base** (context) | Silver ₹22,500 one-time or ~₹750/mo rental; TSS ₹4,500/yr | Per license | [markitsolutions](https://www.markitsolutions.in/pricing/) |

**Entry price for a 50–80-client CA firm:** ~**₹800–1,700/month** (TaxOne flat
₹10k/yr ≈ ₹833/mo is the anchor). → Bridge cannot win on price; it must charge
a premium on the capability gap (Drift, Proof-of-Post). See
[GO_TO_MARKET_AND_PRICING.md](./GO_TO_MARKET_AND_PRICING.md).

## C. UX teardown (public record) — patterns to steal and fix

Detailed, cited per-flow findings; the three cross-cutting conclusions matter most.

### Per-product highlights
- **Vyapar TaxOne bank→Tally:** Bulk-Upload → Banking → company + bank-ledger
  select → upload (dupe-name warning) → **async processing (Excel ~30min, PDF
  ~1hr, scanned PDF up to 12hrs)** → auto-map date/amount/type/narration → grid
  with **search-and-bulk-assign by party** + inline ledger create (GSTIN fetch)
  + rules → "Send to Tally" → **verify in Tally's Day Book (not in-product)**.
  No per-row confidence shown; mapping is exact-match, brittle. ([taxone import help](https://taxone.vyapar.com/help/articles/import-the-bank-statement), [skillcourse walkthrough](https://skillcourse.in/import-pdf-transactions-into-tally-suvit/))
- **Biz Analyst mobile entry:** FAB → Create Sales Invoice (permission-gated) →
  left-drawer company switch → line items with closing-balance/godown → save →
  auto-sync. **New invoices default to "optional vouchers" in Tally** (accounting
  gotcha). Sync status = "Last Sync Time" stamp; **failure surfaces as figures
  showing 0**, not an error. ([create sales invoice](https://help.bizanalyst.in/features/data-entry/how-to-create-sales-invoice), [figures showing 0](https://help.bizanalyst.in/biz-analyst-manual/support/sync-issues/all-figures-showing-0-in-mobile-app))
- **AI Accountant maker-checker:** proposal queue → predict ledger + GST codes →
  **Approve / Adjust / Lock-rule** (three-way) → exceptions lane for CA review →
  optional maker-checker gate for "sensitive writebacks" → duplicate/naming/sync
  validation. Confidence-score *display* not public (INFERRED grid). Vocabulary
  ("maker-checker", "ledger scrutiny", "exceptions queue") is the differentiator.
  ([Tally integration](https://www.aiaccountant.com/blog/tally-integration-with-ai-accountant), [CoA AI mapping](https://www.aiaccountant.com/blog/chart-of-accounts-ai-mapping))
- **CredFlow connector setup:** download desktop app → keep company open →
  **configure ODBC + port (manual, mismatch = #1 failure)** → Add Company → sync.
  Errors (not-connected, educational-mode-unsupported, **company-name mismatch on
  rename**) are deferred to Freshdesk KB, not surfaced in-app. ([add company](https://credflow.freshdesk.com/support/solutions/articles/82000909704-how-to-add-company-for-tally-software-), [syncing-issues folder](https://credflow.freshdesk.com/support/solutions/folders/82000694831))

### Three cross-cutting whitespace conclusions (all confirm plan bets)
1. **Confidence transparency is an open gap across all four** — everyone claims
   AI mapping; none *shows* per-row confidence with a bulk-accept-above-threshold
   control. → Validates plan S3 (confidence *words* + inspectable rationale +
   Accept-all-Matched; Suggested behind an explicit toggle).
2. **Sync failure is universally under-communicated** — TaxOne bare "Failed",
   Biz Analyst "figures → 0" (reads as *real data* — dangerous in accounting),
   CredFlow KB-deferred "not connected". → Validates the Sync Beacon
   (dual-timestamp, never green-from-cache) and accountant-language error catalog.
3. **Verification happens in Tally, not in-product** (TaxOne, Biz Analyst) — the
   loop is left open. → Validates Proof-of-Post readback ("Verified in Tally
   14:32", echo the posted voucher number in-app).

### New concrete refinements folded into the plan
- **Auto-detect the gateway port and running companies** (CredFlow's manual
  port field is its #1 support ticket). PR #78 already added company discovery;
  extend the onboarding self-test to probe/suggest the port.
- **Make optional-vs-regular voucher explicit at post time** (Biz Analyst's
  silent optional-voucher default is a trust trap) — surface it in the S4
  preview with a one-line consequence.
- **Bind sync to company GUID, never name** (CredFlow breaks on rename) —
  already a plan invariant; now with a named competitor failure to cite.
- **Local, synchronous Excel/CSV import is a marketable speed edge** vs TaxOne's
  30min–12hr async queue — reinforces the "Excel/CSV first, skip OCR" ruling.
- **One-click "Lock rule" from the corrected row** (AI Accountant pattern) —
  tighten the plan's "rule promotion after 2 corrections" to also offer inline
  rule creation from the row the user just fixed.

## D. Research-confidence note

The primary verification pass (deep-research workflow) confirmed 20 core
landscape claims 3-0 before hitting a model usage limit; the remainder rest on
single primary sources (vendor docs/help centers), corroborated where possible
by the repo's own prior research. Treat UNVERIFIED-tagged items above as
directional. Nothing here changes the plan's rulings; it deepens the landscape
and confirms the UX and positioning bets.
