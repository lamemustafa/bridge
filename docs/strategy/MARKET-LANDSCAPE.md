# Market landscape — deep pass

**Written:** 2026-07-31, by the strategy thread, from web research.
**Supersedes:** the shallow first pass of the same name.
**Labels:** EVIDENCE (traceable to a source) · CLAIM (vendor's own marketing, unverified) ·
OPINION (mine).

---

## 0. The one finding that matters

**Tally absorbs every feature that lives inside a single company's books. It has done this
twice a year, for free, for the last three releases. Anything built close to Tally's core has
a shelf life of about twelve months.**

EVIDENCE — the absorption timeline:

| Release | Date | What it swallowed | What that feature used to be |
| --- | --- | --- | --- |
| TallyPrime 6.0 | 2025 | bank statement import (text/Excel/CSV), auto-match, duplicate detection, Connected Banking | a paid add-on category |
| TallyPrime 6.1 | 2025 | GSTR-2B auto-download and reconciliation, GSTR-1 upload | ClearTax's core CA pitch |
| TallyPrime 7.0 | Dec 2025 | **IMS inside Tally**, JSON import/export, TallyDrive cloud backup, Connected Banking 2.0 / PrimeBanking, SmartFind | GSP/ASP territory |
| TallyPrime 7.1 beta | 20 May 2026 | deeper IMS reconciliation — matched / mismatched / pending / action-required | third-party recon tools |

EVIDENCE — the position it does this from: Tally holds **over 95% market share in India**,
**2 million+ users across 103 countries**, revenue **₹578 crore in FY23**, and told the press
it expected **30–40% growth in FY25**.

**OPINION.** This kills a whole class of ideas, including two I recommended in this thread.
Bank statement → Tally: absorbed. 2B reconciliation: absorbed. IMS: absorbed, in December,
seven months ago. If the product's value sits inside one company's books, Tally ships it free
in the next release and the add-on vendor's business evaporates.

### What Tally structurally cannot absorb

EVIDENCE:
- Tally is **per-company by design.** You switch companies with Alt+F1. Gold edition can load
  *two* simultaneously "when you need to compare data across two client entities."
- CA firms routinely keep **50–200+ client companies** in one installation, and consolidated
  cross-client reporting **requires third-party BI tools** — it is not native.
- Connected Banking requires **the account holder to authorise at their own bank**, with users
  pre-registered on the bank's portal. A CA cannot self-serve this for 100 clients.
- Tally's IMS is configured **company-wise**, each company's invoices tracked independently.

**OPINION — the durable gap.** Tally's customer is the business. Its unit of work is one
company. A CA's unit of work is **a hundred companies at once, on the second Tuesday of every
month.** Tally will never close that gap, because closing it means building for someone who
isn't their buyer.

**The corroborating evidence is already in our own history and we misread it:**
- The one feature of ours that earned repeat unprompted usage was **obligations** — filing
  status across all clients in one view. Cross-client.
- The cold CA bought Vyapar TaxOne specifically for **multi-location sync**. Cross-company.
- ClearTax's entire pitch to CA firms is **PAN-level, multi-GSTIN** activity. Cross-company.

Three independent signals, all pointing the same way, and the plan in this repository pointed
at per-company voucher-level features instead.

---

## 1. Competitor: Vyapar TaxOne (formerly Suvit)

### The numbers

| Fact | Value | Type |
| --- | --- | --- |
| Founded | 2021, Surat — Ankit Virani, Kalpesh Zalavadiya | EVIDENCE |
| Total funding raised | **$601K–670K** across 3 rounds, 7 investors | EVIDENCE |
| Acquired by Vyapar | **27 Nov 2025**, amount undisclosed | EVIDENCE |
| Price — CA plan | **₹10,000/yr** (listed ₹20,000, 50% off) | EVIDENCE |
| Price — advocate/accountant | **₹12,000/yr** | EVIDENCE |
| Customers | "10,000+ practising firms, 30,000 accountants" | CLAIM |
| Time saved | "20+ hours weekly, 30% more clients", "80% less manual work" | CLAIM |

### The reviews — this is the interesting part

EVIDENCE (Techjockey): **one written review, five ratings**, 4.1/5. For a product claiming
10,000 firms, that review volume is implausible. Treat the 10,000 figure as marketing.

EVIDENCE (Trustpilot, real customers, paraphrased):
- one reviewer reported **1.5 months with no solution and no proper support**, ticket closed
  without response, and **refund refused even after the vendor's own demo failed**
- another: promises at demo, **product failed on day one**; they raised the network conditions
  beforehand, the vendor pushed the sale anyway, then blamed the customer
- descriptions include "absolutely the worst", "extremely unprofessional, misleading and
  totally unreliable"

EVIDENCE (Research.com): **no transfer, no cancellation, no refund once purchased.**

EVIDENCE (hostile competitor, so discount it, but it matches the pattern): extraction is
template/zone-based, ~50–80% first-pass accuracy, breaks when a vendor changes invoice format
or a photo is badly lit.

**OPINION.** This is the first hard evidence for the "sync trust is the market's open wound"
hypothesis in the original briefing — but the wound is not *sync*. **It is that these products
are sold aggressively, fail on contact with real data, and then the vendor disappears.** The
complaint is about the gap between the demo and the Tuesday morning. That is a wound a solo
founder who answers his own phone can actually exploit — and it is the *only* advantage of
being small that showed up anywhere in this research.

---

## 2. The rest of the field

| Player | What it is | Scale / price | Type |
| --- | --- | --- | --- |
| **Vyapar** (parent) | SMB billing/accounting | **1.5 crore+ businesses**, **$130M raised**, 500–1000 staff | EVIDENCE |
| **Biz Analyst** (Silicon Veins) | Tally data on mobile for the owner | **500,000+ downloads**, 4.4★, acquired by **Khatabook, Mar 2021** | EVIDENCE |
| **Hisabkitab** (Surat) | AI extraction → Tally vouchers | **from ₹2,999/yr**; SoftwareSuggest shows **zero reviews** | EVIDENCE |
| **Qosh AI** (Mumbai LLP) | AI agents for CAs; documents → extract → categorise → **sync into Tally**; **runs locally or cloud**; Lite (suggest) vs Autopilot (post) | pricing not published | EVIDENCE |
| **Finsights** (Hyderabad) | Tally on mobile + auto ITC reconciliation | not published | EVIDENCE |
| **AccuBrain** | positions explicitly as the Suvit alternative | not published | EVIDENCE |
| **ClearTax** | high-volume 2B recon, PAN-level multi-GSTIN | **pricing not published — "contact sales"** | EVIDENCE |
| **Cygnet.One** | claims **first ASP-GSP live with IMS** | enterprise | CLAIM |
| **GSTHero, Masters India, IRIS** | GSP platforms with IMS modules | enterprise | EVIDENCE |
| **Winman CA-ERP** | desktop practice suite | **from ₹9,850** | EVIDENCE |
| **KDK Spectrum Cloud** | cloud practice suite | **₹6,300/yr** | EVIDENCE |
| **Saral IncomeTax** | desktop | **₹6,090/yr** | EVIDENCE |
| **CompuTax, Genius, Webtel** | legacy desktop suites, deep forms coverage, machine-bound | — | EVIDENCE |

### Read Qosh AI carefully

It is **the closest thing to what this thread was about to recommend**: Indian, CA-targeted,
AI agents, document → Tally, and explicitly *"run Qosh on your device so data stays there, or
use the cloud securely."* That is our local-first pitch, already shipped by someone else, in
Mumbai, with a free trial live now.

**OPINION.** Not fatal — no evidence they have traction — but it means "AI reads documents and
puts them in Tally, locally" is a **positioning that is already taken**. Differentiating on it
requires beating them on execution, and we would be starting behind.

---

## 3. Price ceilings — what this market will actually pay

EVIDENCE:
- Tally add-ons typically **₹2,000–₹15,000/yr**
- External automation platforms roughly **₹1,000–₹5,000 per client per month** at the top end
- Data-entry automation: TaxOne ₹10,000/yr, Hisabkitab from ₹2,999/yr
- Practice suites: ₹6,000–₹10,000/yr
- Junior data-entry staff at our reference firm: **₹2,000–₹6,000/month**

**OPINION.** Two distinct ceilings, and which side you're on decides the business:

- **Replacing a junior's typing** competes with a ₹2,000/month student. Ceiling ~₹250–850/month.
  Volume business. Needs distribution we do not have.
- **Saving the CA's own time, or preventing a penalty the CA is liable for**, competes with the
  CA's own hourly worth and their professional risk. That is where ₹6,000–₹10,000/year products
  live, and where the practice suites sit.

**Build for the second. The first is a treadmill we cannot win.**

---

## 4. Distribution — the part we have never researched, and our biggest hole

EVIDENCE:
- Tally sells through a **partner/reseller channel**. Published margin structures: **referral
  partners 10%**, **associate partners 20%** per licence sold.
- A single certified partner advertises **2,000+ clients and 500+ CA & GST practitioners across
  19 states**.
- Partners already sell TDL add-ons — WhatsApp invoice senders, Excel importers, auto
  e-invoice, GSTR-2B reconciliation — into that base.
- Vyapar's growth funding was explicitly spent on **"digital and physical distribution
  channels"**, with 500–1000 employees.
- ICAI runs an official WhatsApp channel and regional branch **study circles**; the referral
  model is described in ICAI's own material as the oldest and most natural form of networking
  between CA firms.

**OPINION.** This is the answer to "how does anything reach a CA," and it is not content
marketing and not a tools microsite:

1. **Tally partners are a ready-made salesforce with an existing CA relationship and a
   published 10–20% margin expectation.** They sell add-ons already. This is the single most
   concrete channel found in this entire research pass.
2. **CA-to-CA referral is the primary mechanism in the profession itself.** One CA vouching
   for you beats any amount of marketing. We have exactly one CA who could do that and have
   never asked.
3. **ICAI branch study circles** are physical, local, recurring, and full of the exact buyer.

Nothing here needs a marketing budget. All of it needs conversations, which is the thing that
has not been happening.

---

## 5. Technical findings that constrain the plan

### Direct bank data — closed
EVIDENCE: an Account Aggregator FIU must be regulated by RBI, SEBI, IRDAI or PFRDA; a fintech
without such a licence must partner with a regulated entity. NBFC-AA licence needs ₹2 crore net
owned funds. Commercial statement fetch runs ₹5–₹25 per successful pull (Perfios). Ecosystem:
17 AAs, 179 FIPs, 955 FIUs.
**Conclusion:** clients emailing PDFs is permanent. Remove bank connectivity from planning.

### GSP API access — cheaper than assumed
EVIDENCE: GSP API calls cost **10 paise to ₹1**; a GSTIN with up to 2,000 B2B transactions
costs **under ₹60/month** for GSTR-1/2/3. An ASP integrates a GSP's APIs rather than becoming
a GSP.
**OPINION.** The scraping-versus-GSP decision is settled and I am not reopening it — but the
record should show that **cost was not actually the binding constraint**. The constraint is the
commercial contract with a GSP. If portal reliability across 100 clients ever becomes the
product, this is worth revisiting on facts rather than on the original assumption.

### Local models for handwritten bills — plausible, untested for India
EVIDENCE: PaddleOCR-VL is 0.9B parameters and CLAIMS to run under 1 GB VRAM covering printed
and handwritten text in 109 languages, beating Qwen2.5-VL-72B on OmniDocBench. Qwen3-VL is
reported as the strongest open-source document model of 2026, the 8B competitive at low VRAM.
Other candidates: dots.ocr, InternVL 3, Mistral OCR 3, olmOCR, GLM-OCR. PP-OCRv6 reports
1.5M–34.5M parameter models beating billion-scale VLMs on OCR.
**Not established:** nothing benchmarks any of these on Indian handwritten sales bills —
mixed script, ballpoint on carbon paper, non-standard layout.
**Conclusion:** one day, twenty real bills, local versus frontier, compared on party name,
invoice number, date, taxable value, tax, total. Not a research question. An experiment.

---

## 6. What this pass changes

**Killed outright:**
- Bank statement → Tally as a product. Tally 6.0 ships it free; six vendors sell the rest from
  ₹2,999/yr.
- IMS-inside-one-company. **Tally 7.0 shipped it in December 2025 and 7.1 deepens it.** I
  flagged this as an open window one message before finding that. It is not open.
- Any per-company, voucher-level feature. Twelve-month shelf life.

**Killed by economics:**
- Anything whose value proposition is "replaces a junior's typing." The junior costs ₹2,000.

**Now the leading thesis:**
> **Build for the CA's cross-client Tuesday, not the accountant's per-company Monday.**
> One screen, a hundred clients, on the day the deadline bites. Tally cannot follow us there
> because its buyer is the business, not the practice.

This is also the only thing we have ever built that a real firm used twice a month unprompted:
obligations. **We may have already found the wedge in early 2025 and walked away from it to
build a Rust connector.**

**Distribution, first concrete plan:**
Tally partners (10–20% margin, existing CA relationships), CA-to-CA referral starting with the
one warm firm, ICAI branch study circles. No budget required. Conversations required.

**Still open:** what specific cross-client job to build first. That is decided by the answers
in QUESTIONS-2026-08-01.md, not by more reading.

---

## 7. Sources

Tally: help.tallysolutions.com/tallyprime-features-release-wise/ ·
help.tallysolutions.com/connected-banking/ · /connected-banking-faq/ · /user-access/ ·
tallysolutions.com/gst/gstr-2a-2b-reconciliation-in-tallyprime/ ·
tallysolutions.com/business-guides/how-to-auto-import-bank-statements-for-faster-reconciliation-in-tallyprime/ ·
markitsolutions.in/blog-details/whats-new-tallyprime-7-complete-feature-guide ·
antraweb.com/blog/whats-new-in-tallyprime-release-7-1 ·
business-standard.com/companies/news/tally-expects-30-40-revenue-growth-in-fy25... ·
en.wikipedia.org/wiki/Tally_Solutions

Suvit / TaxOne / Vyapar: taxone.vyapar.com/pricing · suvit.io/data-entry-automation-feature ·
techjockey.com/detail/suvit · ca.trustpilot.com/review/suvit.io ·
research.com/software/reviews/suvit · tracxn.com/d/companies/suvit ·
entrackr.com/snippets/vyapar-acquires-accounting-automation-startup-suvit-10817806 ·
inc42.com/company/vyapar-app/funding/ · accubrai.in/accubrain-vs-suvit

Others: bizanalyst.in · play.google.com/store/apps/details?id=in.bizanalyst ·
hisabkitab.co/pricing-for-software/ · softwaresuggest.com/hisabkitab · qosh.ai/pricing ·
qosh.ai/about · technologycounter.com/products/finsights · techjockey.com/detail/winman-ca-erp ·
ca-practice-management-software.in/best-income-tax-software-for-ca/ ·
finexo.in/blog/gst-compliance/best-gst-software-for-ca-tax-practitioners-2026

IMS: cleartax.in/s/invoice-management-system-ims-under-gst ·
smartgst.in/blog/gst-invoice-management-system-ims-mandatory-guide-2026 ·
taxgarden.in/blog/ims-invoice-management-system-mandatory-gst-2026 ·
caclubindia.com/articles/gstr-2b-mismatch-and-itc-protection-the-complete-2026-playbook-55807.asp ·
gsthero.com/ims/ · cygnet.one/blog/key-features-of-invoice-management-system/ ·
mastersindia.co/blog/invoice-management-system-ims-under-gst/

Channel: tallysolutions.com/partners/growth-partners/ · precisiontech.in/apps/tally/tally-tdl-addons/ ·
ujss.in/partners · tallymaster.in/excel-to-tally/partner/ · vasaibranchicai.com (networking models)

Infra/AA/GSP: taxguru.in/rbi/account-aggregator-framework-complete-application-to-compliance-guide... ·
casparser.in/blog/state-of-account-aggregator-2026/ ·
sahigst.freshdesk.com/support/solutions/articles/31000131736-understanding-gsp-charges ·
gstzen.in/a/gsp-gst-suvidha-providers.html

Local models: insiderllm.com/guides/paddleocr-vl-local-document-ocr/ ·
promptquorum.com/power-local-llm/local-vision-models-llava-ollama-2026 ·
unstract.com/blog/best-opensource-ocr-tools/
