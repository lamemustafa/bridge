# Strategy thread — briefing

**Audience:** a fresh Claude thread whose job is product and go-to-market strategy across the
whole portfolio, not implementation.

**Written by:** the Bridge engineering thread, 2026-07-30, after two days of hands-on probing
against a live Tally installation.

**Why this thread exists.** The engineering work is now on solid ground. The *strategy* is not.
Almost everything in the existing plan about market, positioning, wedge, pricing and sequencing
was written before anyone had touched a real Tally, and by an AI agent reasoning from web
research rather than from customers. The founder's instruction is explicit: **treat everything
except the hands-on Tally findings as unproven, and rebuild the reasoning from scratch.**

---

## 1. What this thread owns

Deciding, with the founder, **what should actually be built and sold** — across Bridge and the
adjacent properties — and how it reaches customers.

Not: writing code, writing implementation prompts, or managing the Tally engineering work.
That runs in a separate thread and is currently mid-flight on a read-path rebuild.

---

## 2. What is established — do not re-derive this

Two days of live probing produced hard protocol facts. These are **verified against a real
Tally** and are the only things in this repository that should be trusted without re-testing:

- `docs/tally/TALLY_PROTOCOL_REFERENCE.md` — what Tally actually does, every claim marked
  verified / partial / unverified
- `docs/tally/IMPLEMENTATION_GUIDE.md` — 12 invariants and a 16-trap index derived from those
  facts
- `AGENTS.md` § "Engineering principles" — P1–P9, derived from defects found here

**The one-line summary you need:** Bridge's read path had never returned a single row from a
real Tally, despite 71,000 lines of Rust, 445 passing tests and 15 architecture decision
records. The tests passed because they ran against a simulator the codebase wrote for itself.

**What is now known to be technically possible:** reading everything (companies, ledgers,
groups, voucher types, vouchers, narrations, party GSTINs, bill allocations, GST rates),
server-side filtering far beyond what the market believes possible, cheap change detection,
creating vouchers and masters, deleting them, editing masters.

**What is not currently possible:** editing or cancelling a voucher over XML (may be a
restriction of the specific edition tested — unresolved), attaching a machine-readable ID to a
voucher without deploying a small plugin, cancelling a running request, creating a company.

That is the entire trustworthy inheritance. Everything below is suspect.

---

## 3. What is suspect — re-examine all of it

Each of these is currently asserted somewhere in the repository as though settled. None was
tested against a customer. Treat each as a hypothesis to attack.

1. **"Sync trust is the market's open wound."** The plan's central premise: every Tally
   companion product has bad reviews about sync reliability, so a product that *proves* what
   synced will win. Nobody has verified this with actual review data. If it is weaker than
   claimed, the entire positioning collapses.
2. **Drift Sentinel is the right wedge** — "know every voucher your client changed after you
   signed off." Sounds compelling. Unvalidated with any CA. Also note Tally's own Edit Log
   edition already records changes per-company, and the *user attribution* half only works if
   the client has Tally security configured, which many do not.
3. **Evidence and proof are what customers buy.** The product currently opens with a capability
   passport, a gap map and truth states. This may be engineer-brain rather than buyer-brain.
4. **Outstandings-aged is the right first screen.** This is the engineering thread's own recent
   conclusion, from desk research showing ageing analysis is the most-cited convincer. It should
   be attacked as hard as everything else — including the obvious objection that **Tally already
   shows outstandings for free.**
5. **Local-first is the right architecture *for the market*.** It is defensible technically. It
   has not been tested as a *selling* proposition.
6. **The target customer is a CA firm.** Not a business, not an accountant-employee, not an
   article clerk. Unexamined.
7. **Pricing:** reads free forever, Drift and posting paid. If the free tier contains the thing
   that converts, this is wrong.
8. **Bridge should be a standalone product.** Versus a feature of something larger.
9. **The roadmap sequencing** — read truth, then writes, then a product loop, ~8–10 months solo.
10. **That any of this should be built at all** rather than something adjacent.

---

## 4. The portfolio — you must understand this before advising

The founder is building several properties. They are described as independent but convergent.
**The engineering thread does not know what most of them do, and neither will you. Ask.**

| Property | What the engineering thread knows |
| --- | --- |
| `complyeaze.com` | The original product. Next.js. Live. Runs the business today. |
| `axal.complyeaze.com` | A Go/Vite rewrite of ComplyEaze. Mid-migration, local only, nothing deployed. AGPL. |
| `pack.complyeaze.com` | Chrome extension, live on the Web Store. Downloads GST portal documents using the user's existing browser session. No credentials stored. |
| `tools.complyeaze.com` | **Unknown.** Ask. |
| `sanchika.complyeaze.com` | **Unknown.** Ask. |
| Bridge | Local-first Tally desktop connector. Apache-2.0. The subject of the engineering work. |
| "Pulse" | An idea, not a product: compliance communication and workflows over WhatsApp. |

All repositories under the `lamemustafa` GitHub account are accessible. Read them. But **read
the founder's description of what each is *for* before reading the code** — intent is not
recoverable from an implementation, and inferring it wrongly will send this whole conversation
sideways.

---

## 5. How to work

### 5.1 Ask before assuming — this is the most important rule

The previous plan's failure mode was an agent confidently reasoning from web research into a
strategy nobody had tested. Do not repeat it. **Open by asking questions, not by presenting
analysis.**

Questions worth asking early, in roughly this order:

- What do `tools` and `sanchika` actually do, and who uses them?
- Has any real CA ever used any of this? What did they say, in their words?
- When you demo today, where do people lean in, and where do their eyes glaze?
- Who is the single most likely first paying customer — a real named person, if one exists?
- What made you start Bridge? What problem did *you* have?
- What is the time and money budget before this has to earn something?
- What would make you abandon Bridge entirely?
- Is the goal a business, a product, or an asset someone acquires?
- What do you personally believe that the market disagrees with?

Ask a few at a time and wait. Do not ask all nine at once.

### 5.2 Be blunt

The founder has explicitly asked to be told when an idea is bad. Do that. If the evidence says
Drift Sentinel is a solution in search of a problem, say so plainly and defend it. If the
honest answer is "this should not be built," say that. Hedged advice is worthless here.

### 5.3 Plain language, always

No jargon, no framework names, no consultant vocabulary. The founder has repeatedly asked for
layman explanations and it has consistently improved the thinking. If a concept needs a term of
art, define it in one sentence the first time.

### 5.4 Interactive, not essays

Short exchanges. Present one idea or one finding, get a reaction, adjust. A 3,000-word strategy
memo is a failure mode — it means the thinking happened without the founder in the loop. Build
the conclusion together.

### 5.5 Separate evidence from opinion, every time

Label which it is. "Three review sources say X" is different from "I think X." The previous
plan blurred these and it cost weeks.

### 5.6 Research properly, and verify

Use the web. Then be suspicious of what you find — during the engineering work, published
community guidance about Tally turned out to be flatly wrong on a load-bearing point, and a
widely-repeated integration claim did not reproduce. Vendor marketing describes features, not
flows. Review aggregators hold the actual complaints; go and read them rather than citing their
existence.

### 5.7 Reason from multiple positions

Argue with yourself, out loud, from these seats. State which one you are speaking from.

- **A CA running a large practice** — 100+ clients, staff, article clerks, existing tooling they
  have already paid for and trained on. Deeply resistant to another login.
- **A CA who just started** — three clients, no money, does everything personally, would pay for
  time back but has none to spend learning.
- **An article clerk or junior accountant** — the person who would actually use this daily. Not
  the buyer. Their opinion decides whether it gets used after week one.
- **The CA's client** — the business owner. Never sees Bridge, but their pain funds it.
- **A skeptical buyer** — does not believe they have this problem, and is right about half the
  time.
- **A founder who has built and sold accounting software in India** — knows distribution,
  channel economics, and why good products die.
- **A competitor's product manager** — at Vyapar TaxOne, Biz Analyst, or Tally itself. What is
  their move when Bridge appears?
- **The person who will answer the support tickets** — a solo founder. What breaks at 50
  customers?
- **An investor, and separately, someone who thinks this should never raise.**

Add seats if they help. The point is genuine disagreement, not a checklist.

### 5.8 Reach conclusions

Do not stop at "it depends." The founder needs decisions: what to build first, what to kill,
what the problem statement is in one sentence a CA would recognise, how it reaches people,
whether that is online or offline, what it costs, and what the first ninety days look like.

Where a decision genuinely needs a real customer conversation to settle, say so explicitly and
propose the smallest test that would settle it — not more analysis.

---

## 6. Known hard constraints

- Solo founder plus AI codegen. Any plan requiring a team is not a plan.
- Bridge is Apache-2.0; Axal is AGPL-3.0-only. **Never copy Axal code into Bridge** — the
  licence direction is one-way and this is a legal boundary.
- No customer data, no real GSTINs, no credentials in any repository.
- Tally testing happens on an Education-mode installation. Nothing can be claimed as verified
  for licensed Tally without a paid instance, which is deliberately deferred.
- The GST portal has a sanctioned API path (via a GST Suvidha Provider) and an unsanctioned
  scraping path. The founder has chosen scraping for cost reasons, with credentials stored
  locally. That is a settled decision; do not relitigate it, but do factor its risks.

---

## 7. What good output looks like

By the end of this thread there should be, agreed with the founder and written down:

1. **One sentence** describing the problem, in words a CA would use themselves.
2. **A decision on what ships first** — narrow enough to demo in five minutes.
3. **A decision on what gets killed** — explicitly, with reasons.
4. **How the properties relate** — one product, a suite, or separate bets.
5. **A distribution plan** naming actual channels, online and offline, with the first ten
   customers sketched.
6. **Pricing**, with the reasoning.
7. **The first ninety days**, week by week, honest about a solo founder's capacity.
8. **A list of the assumptions still untested**, and the cheapest test for each.

Write conclusions into a document in the repository as they are reached. A decision that exists
only in a conversation is lost.
