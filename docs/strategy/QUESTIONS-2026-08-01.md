# Questions for Viniyug — 1 August 2026

**Why this list exists:** every product decision still open is blocked on facts only this firm
can give us. Reading more will not settle any of them.

**How to run it.** Do not present anything. Do not demo. Ask, listen, write down the words
they use. If they start telling you what to build, write that down too but do not agree to it —
that input channel has been wrong four times.

**Order matters.** Section A is the one that could change the whole plan. Do it first, even if
you get nothing else.

---

## A00. The ₹35,000 question — ask Yogesh first, before anything else

Added 2026-07-31. This is now the most important section in the document. See
COMPLIANCE-SEGMENT.md for why.

1. Take me through the ₹35,000 miss again, slowly. What exactly was the filing? When did the
   clock start? How did you eventually find out?
2. **In the last two years, how many other one-time or event-triggered filings have you missed
   or nearly missed?** What did each cost, and who paid?
3. When a client cancels a registration, changes a director, takes a loan, or crosses a
   turnover threshold — how do you find out? Do they tell you, or do you discover it?
4. What do you check *manually* on the GST or MCA portal, and how often?
5. **How many of your clients are companies with a CIN, versus proprietorships and firms?**
   (Critical: the MCA half of this only applies to companies, and your client base is described
   as mostly proprietorships. If the number is small, the first customer is not Viniyug.)
6. Do you know about **CCFS-2026** — the 90% ROC late-fee waiver expiring 31 August? Do any of
   your company clients have overdue AOC-4 or MGT-7?
7. What do you pay for practice management today, if anything? Have you seen Finexo, QwikCA,
   Vider or TaxAdda?

---

## A0. The cross-client question — ask Yogesh first, before anything else

**Revised 2026-07-31 after the deep market pass.** Tally has absorbed every per-company
feature — bank import, 2B reconciliation, and IMS itself (7.0, December 2025). The only thing
Tally structurally cannot do is look across a hundred clients at once. Test that.

1. Which version of TallyPrime are you and your clients on — 6.x, 7.0? Have you seen its IMS?
2. On the 10th of the month, what do you actually need to know across **all** your clients at
   once? Describe the screen you wish existed.
3. How do you track today which client is done and which isn't — paper, Excel, WhatsApp,
   someone's memory?
4. What is the thing you are most afraid of missing across your client base?
5. When something is missed, who notices first — you, the client, or the department?
6. **Obligations:** you have used it for over a year. What made you keep opening it? What
   would make you open it every week instead of twice a month?

**Why 6 matters most:** it is the only thing we have built that anyone used repeatedly and
unprompted. We should understand why before building anything else.

---

## A. IMS — ask Yogesh, and ask a staff member separately

Since 1 April 2026 IMS is mandatory. Every supplier invoice must be accepted, rejected or held
on the portal before the 14th, and from April the portal blocks GSTR-3B filing if claimed ITC
exceeds GSTR-2B. If this firm is feeling that, it is the most important thing we have found.

1. What are you doing about IMS right now, this month?
2. Who actually does it — you, or staff? For how many clients?
3. How long does it take per client? What did July cost the firm in hours?
4. Are you using any software for it, or the portal directly, or GSTN's Excel offline tool?
5. What happens when a supplier's invoice doesn't match the client's purchase register?
6. Has any client been blocked from filing 3B because of an ITC mismatch? What happened?
7. What's the worst part of it — the volume, the deadline, chasing clients, chasing suppliers,
   or deciding accept vs pending?
8. If it were done for you across all clients, what would that be worth per month?
9. **Ask the staff separately:** did IMS make your month longer? By how much?

**What settles it:** if the honest answer is "ten minutes, we accept everything," this idea is
dead and we say so. If it is "it ate three days in July and we are dreading August," we have
found the thing.

---

## B. Bills — the volume that decides the October plan

Ask the staff who type, not the partners.

1. Purchase bills per month, across how many clients?
2. Sales bills per month? How many of those are handwritten?
3. How long does one handwritten sales bill take, start to finish?
4. How long does one printed purchase bill take?
5. What makes a bill *hard* — bad handwriting, missing GSTIN, unclear item names, wrong
   totals, multiple pages?
6. Have you tried any OCR tool on the handwritten ones? Which, and what happened?
7. What do you do when you cannot read something — guess, call the client, or leave it?

**Also collect:** 20 real handwritten sales bills as photos, and 10 printed purchase bills.
These are the test set for the local-model experiment. Ask permission explicitly; do not put
them in any repository.

---

## C. Bank statements — confirming what we now believe is a commodity

1. How do statements arrive — PDF, Excel, CSV, printed paper, screenshots?
2. Which banks? Any cooperative banks, small finance banks, or regional rural banks?
3. Are they password-protected? Who holds the passwords?
4. Do you use TallyPrime 6.x? Have you tried its built-in bank statement import? What
   happened?
5. Has anyone tried Connected Banking? Did any client agree to authorise their bank?
6. How many statements a month, across how many clients?
7. When a bank line says `NEFT:KHANDELWAL BOOKS AND SPORTS`, how do you decide which ledger
   it goes to? Is it obvious, or do you have to look it up?

**Why 7 matters:** if it's obvious to them and takes two seconds, ledger matching is not worth
building. If they hunt for it, it is.

---

## D. Tools they already pay for

1. What software does the firm pay for today? Name each and the annual cost.
2. Which of those do you open every day? Which do you open once a month?
3. What did you buy and stop using? Why?
4. Have you looked at Vyapar TaxOne or Suvit? Hisabkitab? What put you off, or what stopped
   you buying?
5. When you last bought software, what made you decide — a demo, a peer's recommendation, a
   WhatsApp group, a conference, a cold call?

**Why 5 matters:** this is the distribution question. We have no channel and no idea how CAs
in this segment actually buy.

---

## E. The ledger data we need

1. Can you export the ledger list from three or four client companies for us? (Tally: Export →
   Masters → Ledgers.)
2. Ideally pick clients that are different from each other — a transport firm, a trading firm,
   a manufacturer.

**No client names or GSTINs go into any repository.** Keep these files outside the repo.

---

## F. The uncomfortable ones — ask Yogesh directly

1. You have tested everything I have built for over a year and never paid for any of it. What
   would have to be true for you to pay?
2. If I put a price on the thing you find most useful today, what is the number you would not
   argue with?
3. Who else in your circle has the same problem, and would you introduce me?
4. What have I built that you would not recommend to another CA, and why?

**Question 3 is the most valuable thing in this document.** One warm introduction is worth
more than six weeks of engineering.

---

## After the conversation

Write the answers down the same day, in their words, and add them to this file under a
"Answers" heading. Do not summarise them into conclusions yet.
