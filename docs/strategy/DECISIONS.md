# Strategy decisions — running record

**Started:** 2026-07-31. Updated as decisions are reached, not at the end.
Each entry is either **SETTLED** (decided, do not relitigate) or **OPEN** (blocked, with the
thing that would settle it).

---

## Settled

### S1. The October deadline is real, but the bar was wrong
The founder holds 6 October 2026 as a personal checkpoint. That stands.

The stated bar — 30 firms using something 2–3 times a week for over an hour — is not
reachable by any plan on the table. Retired.

**Replacement bar:** one firm, three named people, real client data, used every month without
the founder in the room, plus a five-minute recorded demo that can be shown to a CA who has
never met him. That is a pass.

### S2. Bridge does not write to Tally this year
Verified by the engineering thread: no write has ever left Bridge; there is no working
correction primitive; voucher Alter and Cancel create duplicates. Shipping writes into a
client's real books is the single worst available decision.

**But:** `REMOTEID` was verified on 2026-07-30 to *upsert* rather than duplicate on
re-import. That is a real idempotency key and a real correction path, and it de-risks the
eventual write path. It does not change S2 for this year.

### S3. The accountant performs the import
Any product ships a file the accountant imports into Tally themselves. This removes the
double-posting risk, the trust objection and the installer problem in one move. Bridge never
writes.

### S4. Code signing is deferred
Azure Trusted Signing is unavailable to Indian entities. OV certificates (~$219/yr) do not
remove the Windows SmartScreen warning; only EV (~$325/yr) does, and validity is now capped
at 460 days. Install manually at the first firm. Buy EV when reaching firms we cannot visit.

### S5. Direct bank connectivity is out of scope permanently
Account Aggregator FIU status requires RBI/SEBI/IRDAI/PFRDA regulation. Not available to us.
Clients sending PDFs is the permanent reality. See MARKET-LANDSCAPE.md §4.

### S6. Bank statement → Tally is not the wedge
Tally imports statements natively for free. At least six vendors sell the harder version from
₹2,999/year. The category's price ceiling is set by ₹2,000–₹6,000/month junior staff.
A solo founder cannot win it. It may survive as a supporting feature.

### S7a. Nothing per-company survives — Tally eats it
Added 2026-07-31 after the deep market pass. Tally has absorbed, free, in three consecutive
releases: bank statement import and Connected Banking (6.0), GSTR-2B auto-reconciliation
(6.1), **IMS inside Tally** (7.0, Dec 2025), deeper IMS reconciliation (7.1 beta, May 2026).
It does this from 95% market share and 2M+ users.

**Rule:** if a feature's value lives inside one company's books, it has a twelve-month shelf
life. Do not build there.

**The exception, and the new leading thesis:** Tally is per-company by design — you switch
companies with Alt+F1, Gold loads two at once, cross-client consolidation needs third-party
tools. A CA's unit of work is a hundred companies on one deadline day. **Tally cannot follow
us across clients, because its buyer is the business, not the practice.**

Corroborating and previously misread: the only feature of ours with repeat unprompted usage
was **obligations** — cross-client filing status. The cold CA bought TaxOne for
**multi-location sync**. ClearTax's CA pitch is **PAN-level multi-GSTIN**. Three signals, one
direction.

### S7. Breadth is dead as a strategy
Axal shipped notices, matters, tasks, documents, reconciliation, vendor intelligence, DSC and
Tally sync. Daily usage: zero. Adding thin features to thin features does not compound into
habit. One thing, used often, done properly.

### S8. Bridge's role is narrowed
Bridge is the **local connector that reads Tally** — ledgers now, vouchers later, writes
eventually. It is not the application and does not host the product UI. This makes it smaller
and more likely to actually ship.

---

## Open

### O1. What ships first
**Revised 2026-07-31.** The previous candidate — "IMS at CA-firm scale" — was written one
message before discovering that **TallyPrime 7.0 shipped IMS in December 2025**. IMS *inside a
company* is closed.

**Leading thesis now:** a cross-client job — one screen, a hundred clients, on the day the
deadline bites. Which job specifically is undecided. Candidates: IMS action status across all
client GSTINs (Tally's version is per-company and requires opening each), filing status
(already proven with obligations), ITC-block exposure before the 14th.

**Settled by:** QUESTIONS-2026-08-01.md. One conversation, not more research.

### O1a. The leading candidate, revised again — 2026-07-31 (see COMPLIANCE-SEGMENT.md)
The obligations story was corrected: 3–4 months of usage, not a year, and it began with
Viniyug paying **~₹35,000 out of their own pocket** for a missed post-cancellation filing
(almost certainly GSTR-10) discovered 6–7 months late. They asked to be onboarded on the spot.

**Thesis:** recurring compliance calendars are a solved ₹500/month commodity — Finexo
(₹5,999–24,999/yr, 1,500 firms), QwikCA (₹7,500–27,000/yr), Vider (₹1,788/user), TaxAdda.
Notice monitoring is solved (OptoTax, My Notice Track). Point-in-time GSTIN health is solved
and free (ClearTax, IRIS Peridot, GSTVerify).

**What is not solved:** continuously watching a whole client base and raising an obligation
*when a state changes on a government portal*. That is the exact shape of the ₹35,000 miss.

**Two facts that make it attractive:** MCA master data is **public, free, no login**, and most
ROC forms carry **₹100/day with no upper cap** — unlike GST, which caps.

**Must be checked before any code:** free trials of Finexo and QwikCA, to see whether they
detect events or merely schedule them. Two hours.

### S9. MCA data is NOT an accessible source — tested 2026-07-31
Retracts an earlier claim in this file's own history. Tests recorded in
COMPLIANCE-SEGMENT.md §5:
- mca.gov.in returns **HTTP 403 from Akamai** to every automated request; a real browser
  navigation was also refused; MCA's CSP loads `disable-devtool`; its data centre burned on
  5 June 2026.
- The data.gov.in bulk mirror is reachable but is **the wrong data**: 4,065,191 records, **last
  updated 2024-12-13**, 13 fields, containing **no filing dates, no AGM, no directors, no
  charges, no compliance status** — and "NA" in several fields of the very first record.

**Consequence:** the CCFS-2026 free tool below cannot be built from open data. Commercial
providers (Probe42, Surepass, Karza, Signzy) all publish "contact sales". **Do not plan around
MCA data until someone quotes a price.**

**What survives:** the GST half — registration status, cancellation, LUT, composition, filing
history — observable with the ~60 client credentials already held and the working `pack`
extension.

### O1d. A dated GTM opportunity — CCFS-2026, expires 31 August 2026 — NOW BLOCKED, see S9
MCA's Companies Compliance Facilitation Scheme gives a **90% waiver on ROC additional fees**;
deadline extended from 15 July to **31 August 2026** (General Circular 03/2026, after the MCA
data-centre fire of 5 June 2026). Companies only, not LLPs.

A free public tool — paste CINs, get overdue ROC filings and the rupee value of the waiver —
needs no credentials, has a real deadline four weeks out, and targets exactly CAs and CSs.
**Deliverable is a list of CA firms, not revenue.** Risk: four weeks is tight and it is a
one-off.

### O1b. Distribution — first concrete options found
Not a decision yet, but no longer a blank. **Tally's partner channel** publishes 10% referral
and 20% associate margins, and individual partners claim 500+ CA relationships. **CA-to-CA
referral** is the profession's own primary mechanism. **ICAI branch study circles** are
physical, recurring, and full of the buyer. None needs a budget. All need conversations.

### O1c. "AI reads documents into Tally, locally" is already taken
Qosh AI (Mumbai LLP) ships exactly this positioning today, including the local-or-cloud
choice. No evidence of traction, but the line is not ours to claim.

### O2. Where the product is built — web or Bridge
Web is roughly 2–3 weeks; Bridge is 6–10 weeks by its own thread's estimate, with an untested
Rust encrypted-PDF story as the largest unknown.

**Settled by:** what O1 resolves to. An IMS product is portal work and barely touches Tally,
which would make this moot.

### O3. Whether local extraction of handwritten bills is viable
PaddleOCR-VL claims 0.9B parameters in under 1 GB VRAM with handwriting support. Nothing tests
this on Indian handwritten sales bills.

**Settled by:** one day, twenty real bills, local models against a frontier model, compared on
party name, invoice number, date, taxable value, tax, total.

### O4. Pricing
Nothing decided. What is known: the data-entry category clears at ₹250–₹850/month because it
competes with a ₹2,000/month student. Anything sold to the CA personally — rather than to
replace a junior — is not bound by that ceiling. Which of those we are in depends on O1.

### O5. The positioning line
"Verified, not trusted" is directionally right and reads flat. Needs to be written after O1,
not before.

### O6. How this reaches anyone beyond Viniyug
No channel exists. Zero brand, zero trust, one warm contact. Section D of the questions
document starts on this. It is the largest unsolved problem in the entire plan and it is not
an engineering problem.

---

## Assumptions still untested, and the cheapest test for each

| Assumption | Cheapest test |
| --- | --- |
| IMS is painful for a 60-client practice | Ask Yogesh. Tomorrow. |
| Ledger matching from bank narration is slow for staff | Ask the staff who do it. Tomorrow. |
| Handwritten bills are the real hour sink | Get the monthly bill count. Tomorrow. |
| Local models can read Indian handwritten bills | 20 real bills, one day of testing |
| A CA will pay for anything we build | Ask Yogesh for a number, directly |
| There is any channel to CAs beyond one warm firm | Ask for one introduction |
