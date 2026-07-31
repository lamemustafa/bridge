# The compliance segment — deep dive

**Written:** 2026-07-31, by the strategy thread.
**Why:** the founder corrected the obligations story. It was not a year of usage — it was
3–4 months, and it started with a specific, expensive event. That event is the sharpest
signal in this entire project and deserved its own research pass.

**Labels:** EVIDENCE · CLAIM (vendor marketing) · OPINION.

---

## 1. The origin story, stated precisely

FOUNDER'S ACCOUNT, 2026-07-31:

Yogesh and Vineet (Viniyug) were asked how they track their filings. They described the flow,
then volunteered a personal experience: **they missed a filing required after a GST
cancellation. The registration therefore never actually cancelled. They discovered this 6–7
months later and paid roughly ₹35,000 out of their own pockets.**

When event-based filings under obligations were proposed on the back of that story, **they
asked to be onboarded immediately.** Three junior staff have used it actively for 3–4 months.
The partners do not use it themselves.

### Why this is the strongest signal in the project

1. **It came from them unprompted**, in answer to an open question — not from a demo.
2. **It has a rupee number**, and the number came out of the *firm's* pocket, not the client's.
3. **The buyer felt the pain personally.** Every other idea in this project asks a CA to pay to
   save a ₹2,000/month junior some typing.
4. **They pulled.** Second time in this project's history — the other being the staff who
   stopped work waiting for the bank statement experiment.
5. **It converted to sustained usage.** Nothing else has.

### What the miss actually was — EVIDENCE

Almost certainly **GSTR-10, the final return**. Due within **3 months** of the cancellation
order or the cancellation date, whichever is later. Late fee ₹200/day (₹100 CGST + ₹100 SGST),
capped at ₹10,000. Non-filing triggers a show-cause notice; ignoring that produces a final
order with tax, interest and penalty. **The GSTIN status only becomes permanently Cancelled
after GSTR-10 is accepted** — which is exactly the failure mode described: the registration
"never got cancelled."

**OPINION.** The generalisable pain is not "we need a compliance calendar." It is:

> **An obligation appeared because something happened, nobody was watching, and by the time we
> found out it cost us money we couldn't bill.**

---

## 2. What is already solved — and it is a lot

I went looking for reasons this idea is dead. Here is what exists.

### 2a. Recurring compliance calendars — crowded, cheap, actively being won

| Product | Price | Scale | Evidence quality |
| --- | --- | --- | --- |
| **Finexo PMS** | **₹5,999/yr** premium (250 GST clients, 1,000 total, 10 users) up to **₹24,999/yr** enterprise | **1,500+ CA firms, 4 lakh+ clients, 1,00,000+ tasks/month**, 80 Google reviews at 5.0 | EVIDENCE, own site |
| **QwikCA** | **₹7,500 / ₹12,500 / ₹17,000 / ₹27,000 per year** (10/20/40/60 users, unlimited clients) | "4.8/5 by 5,000+ Indian CA firms" | EVIDENCE price, CLAIM scale |
| **Vider ATOM** | **₹1,788/user** | Forbes India recognised | EVIDENCE |
| **TaxAdda** | free tier + paid | — | EVIDENCE |
| **Turia, Lander Books, Fyond, CA Office Automation** | various | — | EVIDENCE they exist |

**What they do:** auto-create recurring GST, TDS and ITR tasks per client; assign to staff;
WhatsApp/SMS/email reminders before due dates; document vaults; billing.

**Finexo is the closest to us and the most instructive.** It auto-creates tasks from **client
registration data**, factoring in return type, QRMP selection, regular vs composition, and
**registration/cancellation dates**. That is derivation from data the CA entered — not from
watching anything.

**OPINION.** This category is not a gap. It is a live, competitive, cheap market with thousands
of paying firms, and Finexo's press coverage dates from **February–March 2026** — meaning new
entrants are winning it *right now*. Building a general compliance calendar in 2026 is arriving
five years late to a ₹500/month product.

**This means our own obligations feature, as a calendar, is a commodity.** Its value was never
the calendar.

### 2a-bis. Optotax — the one that hurts most. EVIDENCE.

Found on the second research pass and it is the most important competitor in this segment.

- **85,000+ CAs and 5,00,000+ businesses.**
- **Zero platform cost.** GSTR-1 and GSTR-3B filing free.
- Auto-fetches **GST notices directly from the portal**, tracks replies, keeps documentation
  trails — including **registration-related notices**.
- **Smart calendar reminders** to avoid penalties and missed deadlines.
- **Client-wise view**, switch between clients and GSTINs.
- Has shipped dedicated **GST Notice Manager** apps on both the App Store and Play Store.

**OPINION.** This is a free product, with 85,000 CAs, already doing portal fetch plus
multi-client view plus deadline reminders. Any paid GST-monitoring product must be
*obviously* better than free, to a profession that already has it installed.

The gap I identified in §3 may still exist — Optotax's notice fetch is **reactive** (a notice
means you are already late) and its calendar is **recurring** — but the distribution problem
just got much worse. We would be selling against free, with zero brand, into a base of 85,000
CAs who already chose someone else.

**Also EVIDENCE, and it raises the stakes on the underlying pain:** from July 2025 the portal
**blocks any GST return filed more than three years after its due date.** A missed GSTR-10
eventually becomes permanently unfileable — the registration can then never be cleanly closed.

### 2b. Notice monitoring — solved

EVIDENCE: **OptoTax** auto-fetches GST notices directly from the portal, tracks replies and
maintains documentation trails. **My Notice Track** monitors GST *and* Income Tax portals for
notices against a PAN or GSTIN and alerts. Multi-client dashboards, OCR extraction of notice
type, period, section, due date and officer, alerts by email/SMS/WhatsApp.

**OPINION.** This confirms what the market told the founder in 2025 — notices are covered, and
the frequency objection was correct. The notices work was not wasted, but it is not a wedge.

### 2c. Point-in-time GSTIN health — solved

EVIDENCE: **ClearTax GST Health Check** returns a GSTIN's filing history and a compliance
report recommending actions to fill gaps. **IRIS Peridot** checks return filing status and
supplier compliance health. **GSTVerify** scores any GSTIN 0–100 on registration status, filing
recency, entity age and taxpayer type.

**OPINION.** "Is this GSTIN compliant right now" is a free commodity lookup. Note carefully:
these are **one GSTIN, on demand, by a human who already suspects a problem.** None of them
watches a hundred GSTINs continuously and tells you when something *changed*.

---

## 3. The gap — stated as narrowly as I can defend it

Everything above is either **recurring** (derived from a calendar and data the CA typed in) or
**on-demand** (a human looks up one client because they already suspect something).

**Nothing found watches a firm's whole client base continuously and raises a new obligation
when a state changes on a government portal.**

That is precisely the shape of the ₹35,000 miss. The cancellation *happened*. It was visible on
the portal. The three-month clock started. No calendar contained it, because it was not
recurring, and no human looked it up, because nobody knew to.

### Honesty about the evidence

- Absence from search results is **weak evidence of absence.** QwikCA advertises "GST Portal
  Auto-Fetch" and its marketing page does not say how far that goes.
- Finexo already factors **cancellation dates** into task creation. If it also *detects* the
  cancellation, the gap narrows a lot.
- **Both are answerable by signing up for a free trial of each and looking.** That is a
  two-hour job and it should be done before writing any code.

---

## 4. The catalogue — what an event-triggered obligation engine would watch

### GST — EVIDENCE

| Trigger | Obligation | Window | Cost of missing |
| --- | --- | --- | --- |
| Cancellation order / cancellation | **GSTR-10** final return | 3 months | ₹200/day to ₹10,000; **GSTIN never actually cancels**; show-cause → order with tax + interest + penalty |
| New financial year / export intent | **LUT renewal** | before first zero-rated supply; FY27 LUT by 31 Mar 2026 | exports become taxable supplies |
| Turnover crosses limit, or condition breached | **CMP-04** | **7 days from the event** | retrospective loss of composition |
| Opting out of composition | **ITC-01** stock details | 30 days | ITC lost |
| Job work | **ITC-04** | half-yearly or annual by turnover | ITC exposure |

### MCA / ROC — EVIDENCE, and this is where the money is

**Most ROC forms attract ₹100 per day, per form, with NO upper cap.**

| Trigger | Form | Window |
| --- | --- | --- |
| Director appointed or resigned | DIR-12 | 30 days |
| Shares allotted | PAS-3 | 30 days |
| Authorised capital increased | SH-7 | 30 days |
| Charge created (any loan) | CHG-1 | 30 days |
| Registered office changed | INC-22 | 30 days |
| Shares transferred | SH-4 | 60 days |
| Incorporation | INC-20A | 180 days |
| Auditor appointed | ADT-1 | — |
| Annual | AOC-4, MGT-7/7A, DPT-3, MSME-1 | — |
| Every DIN holder | **DIR-3 KYC** | by 30 September | **DIN deactivated, ₹5,000 penalty, and every future MCA filing by that director is blocked** |

**OPINION.** The uncapped ₹100/day is the real prize. A GST miss caps at ₹10,000. An MCA miss
compounds forever — and CCFS-2026 exists precisely because so many companies are sitting on
years of it (see §6).

---

## 5. Can we actually see these events? — TESTED 2026-07-31, and the earlier claim was WRONG

An earlier version of this document said MCA master data is "public, free, no login" and
treated it as a usable data source. **That was based on blog posts. It was tested and it does
not hold.** Recording the tests so nobody repeats the mistake.

### Test 1 — MCA portal, direct. FAILED.

```
GET https://www.mca.gov.in/                                  → HTTP 403
GET https://www.mca.gov.in/.../master-data/MDS.html          → HTTP 403
GET https://www.mca.gov.in/mcafoportal/viewCompanyMasterData.do → HTTP 403
control: https://example.com                                  → HTTP 200
```
The 403 is served by **Akamai edge** (`errors.edgesuite.net` reference), not by our network.
A real browser navigation was also refused. MCA's own CSP header loads
`cdn.jsdelivr.net/npm/disable-devtool` — **they actively defend against automated access.**
Add the **MCA data-centre fire of 5 June 2026** and portal reliability is a live question.

**Conclusion:** MCA master data is free to *a human, in a browser, probably from an Indian IP,
one company at a time.* It is not an accessible data source for a product.

### Test 2 — data.gov.in bulk mirror. REACHED, AND IT IS THE WRONG DATA.

Resource `ec58dab7-d891-4abb-936e-d5d274a6ce9b`, queried live:

```
total records : 4,065,191
updated       : 2024-12-13     ← nineteen months stale
fields        : 13
```

**All 13 fields:** CIN · date of registration · company name · company status · company class ·
company category · authorised capital · paid-up capital · registered state · registrar of
companies · principal business activity · registered office address · sub category

**What is absent:** last AOC-4 filed · last MGT-7 filed · last AGM date · any filing history ·
directors · DINs · charges · **anything at all about compliance status.**

Quality within the fields it does have is poor. The first record returned:
`authorized_capital: "NA"`, `principal_business_activity: "NA"`, `sub_category: "NA"`.

**Conclusion:** this is a stale company *registry*, not a compliance feed. It cannot tell you
who has overdue filings, which was the entire premise.

### What this kills

**The CCFS-2026 free tool, as described in §6, cannot be built from open data.** There is no
public source of "which companies have overdue AOC-4 or MGT-7." Building it would require a
commercial provider — Probe42 (claims 1.7M companies, 2.2M directors), Surepass, Karza, Signzy
— **none of whom publish pricing; all are "contact sales".** With four weeks to the deadline
and unknown per-call costs, this is not a plan.

### What survives

**The GST half.** We hold credentials for roughly 60 Viniyug clients, and `pack` already
downloads portal documents with a live Chrome extension. Registration status, cancellation
dates, LUT status, composition status and return filing history **are** observable per client
with the credentials we have.

That is narrower than the MCA version and the penalties cap lower — GSTR-10 caps at ₹10,000
against MCA's uncapped ₹100/day — **but it is exactly the ₹35,000 story, and it is the half we
can actually reach.**

**Revised rule:** the MCA opportunity is real for the market and **not reachable by us without
paying a data provider.** Price it before planning around it.

---

## 5b. Original access table — kept for reference, now qualified by §5

| Source | Access | What's visible |
| --- | --- | --- |
| **MCA master data** | **PUBLIC. Free. No login required.** | incorporation date, company status, CIN, ROC code, registered office, **register of charges**, index of charges, **signatory details**, **director master data**, companies/directors under prosecution |
| **GST portal** | client credentials required (we hold ~60 sets for Viniyug) | registration status, cancellation, return filing history, notices, IMS |
| **Third-party MCA APIs** (Surepass and others) | commercial | CIN and DIN APIs on V3; **pricing not published, contact sales** |

**OPINION — this is the single most under-exploited fact found in the entire research.**
**MCA master data is public and requires no credentials at all.** A tool that takes a list of
CINs and returns each company's compliance exposure needs *no login, no client consent, no
scraping of a protected portal, and no legal grey area.* Compare that to everything else in
this project, which has been gated behind credentials.

---

## 6. A dated, live opportunity — CCFS-2026 — EVIDENCE

- **Companies Compliance Facilitation Scheme, 2026**, introduced by MCA on **24 February 2026**.
- **90% waiver on additional fees** for pending ROC filings — pay 10% of the late fee.
- Covers pending **AOC-4, MGT-7 and other ROC forms**; **MSC-1** dormant status at 50%;
  **STK-2** voluntary strike-off at 25%.
- **Companies only. Not LLPs.**
- Original deadline 15 July 2026. **Extended to 31 August 2026** by General Circular No.
  03/2026 dated 8 July 2026 — after a **fire at the MCA data centre on 5 June 2026** disrupted
  portal capacity.

**That is four weeks from today.**

**OPINION — the GTM idea this creates.** A free public tool: paste in a list of CINs, and it
returns, per company, the overdue ROC filings visible in public MCA master data and what the
90% waiver would save before 31 August. No login. No credentials. No client consent.

- The data is **public and free**
- The deadline is **real, dated, and imminent**
- The audience is **exactly CAs and CS professionals**
- The output is **a number in rupees they can forward to their client**
- It is **inherently shareable** in the WhatsApp groups where this profession actually lives

This is what `tools.complyeaze.com` was always supposed to be, pointed at something with a
deadline instead of at generic utilities.

**Risks, stated plainly:** four weeks is tight; it is a one-off event, so it is a lead magnet
and not a business; and it only works if MCA's public data can be read reliably at volume,
which is unproven and must be tested on day one, not week three. **The deliverable is a list of
CA firms who used it and gave an email — not revenue.**

---

## 7. Where this leaves the strategy

**The thesis, in one line:**

> Recurring deadlines are a solved, ₹500/month commodity. **The obligations that cost real
> money are the ones nobody scheduled** — they appeared because something changed on a
> government portal, and no one was watching.

This is consistent with everything else established:
- It is **cross-client**, which is the only ground Tally structurally cannot take
  (MARKET-LANDSCAPE.md §0)
- The buyer is **the CA**, not a ₹2,000/month junior, so it escapes the data-entry price ceiling
- It is the **only thing anyone has ever pulled us toward twice**
- MCA's half needs **no credentials at all**

**What must be checked before any code is written:**

1. Free trial of **Finexo** and **QwikCA**. Do they detect events, or only schedule them?
   Two hours.
2. Ask Yogesh: how many one-time or event-triggered filings has the firm missed or nearly
   missed in two years? What did each cost?
3. Ask Yogesh: **how many of your clients are companies with a CIN, not proprietorships?**
   The MCA half only applies to companies, and Viniyug is described as mostly proprietorships.
   **If the answer is "very few," the MCA opportunity is real for the market but not testable
   at this firm** — and that changes who the first customer is.
4. Read MCA public master data for three real CINs by hand. Confirm what is actually visible.

---

## 8. Sources

GSTR-10 and GST events: indiafilings.com/gstr-10 · bajajfinserv.in/gstr-10 ·
support.taxaj.com/portal/en/kb/articles/final-return-after-gst-cancellation-gstr-10 ·
taxbuddy.com/blog/gstr-10 · indiafilings.com/learn/how-to-file-and-renew-your-lut ·
busy.in/gst/how-to-opt-out-of-the-composition-scheme/ ·
taxguru.in/goods-and-service-tax/key-gst-compliances-year-deadlines-requirements.html

MCA / ROC penalties and event forms: commenda.io/india/penalties-for-non-compliance ·
registerkaro.in/post/penalty-for-not-filing-roc-annual-return ·
vakilsearch.com/article/roc-compliance-for-private-limited-company/ ·
ushmaassociates.com/penalties-for-non-compliance-with-roc-filings-in-2025/ ·
kanakkupillai.com/learn/penalty-for-late-roc-filing/

CCFS-2026: taxguru.in/company-law/companies-compliance-facilitation-scheme-2026-complete-guide.html ·
companyji.com/ccfs-scheme-2026-extended/ · juststart.co.in/blog/ccfs-mca-companies-compliance-facilitation-scheme/ ·
mondaq.com/india/directors-and-officers/1815536/ · ofinlegal.com/resources/ccfs-2026-mca-penalty-waiver-scheme/

MCA data access: vakilsearch.com/article/mca-master-data/ · mnscredit.com/blog/mca-master-data ·
surepass.io/mca-data-apis-cin-din-v3-portal/ · github.com/araystech/mca-data-api ·
opencorporates.com/registers/111

Practice management: finexo.in/practice-management-software-for-ca-tax-practitioners ·
qwikca.in/ca-practice-management-software/ · techjockey.com/detail/vider-atom · vider.in ·
techjockey.com/detail/taxadda-ca-office-practice-management-software · turia.in · fyond.com

Notices and health checks: open.money/blog/automating-gst-notice-tracking-and-response-management/ ·
mynoticetrack.com · optotax.com · cleartax.in/s/gst-health-check · smartgst.in/tools/gst-health-score ·
gstverify.co.in/gst-health/ · play.google.com/store/apps/details?id=com.irisgst.taxpayer.peridot
