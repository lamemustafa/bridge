# bridge-tax-audit fixture provenance

No client data. Every fixture here is synthetic: the company ("Lakeview Hardware Demo"), its
GUID, groups below the reserved primaries, ledgers, voucher types, vouchers and amounts are
invented.

## What these fixtures establish, and what they do not

They establish **parity**: that this crate and the Python reference engine it ports compute
the same canonical `cash_44ab`, `cash_payments_40a3`, `depreciation`, `financial_statements`,
`applicability_44ab`, `trial_balance`, `stale_balances_41_1`, `ledger_scrutiny` and
`cash_book_integrity` results (`docs/tax-audit/parity-spec-v1.md`) from the same bytes -- the
synthetic read below, and the edge books (small invented books built directly, not read). They do **not** establish anything
about Tally. No byte here was served by Tally. The XML follows the element layout Tally uses for
collection exports (envelope, `CMPINFO` counts, UTF-16LE without a BOM for the group and ledger
masters, `&#4;` before `Primary`, flags and amounts in Tally's sign), so the parser is exercised
on that shape, but the shape is an author's reproduction. AGENTS.md P1's rule that fixtures are
captured from a real instance is met for this crate by the local parity run over real reads (see
the crate docs), which is never committed.

## The synthetic read

`synthetic-read/` is a tally-read-v1 directory: `manifest.json` plus ten parts, one of them
gzip-stored. Written by `parity/generate_fixture.py`, deterministically (gzip mtime 0);
`python3 parity/generate_fixture.py tests/fixtures` from the crate root reproduces these bytes.
Its manifest declares `producer.kind = "synthetic"` and every part `transformation = "derived"`.

The book is built to make each check fire at least once: a Contra voucher and a custom voucher
type whose base type is Contra; optional, cancelled and (through the side list) post-dated
vouchers; a second window with no status flags, decided by `voucher_status_list.json`; a cash
ledger two groups below Cash-in-Hand; a ledger under a group missing from the masters (MAP-1);
an unbalanced journal (POP-2); a TB row 5.00 away from its vouchers (POP-1) and so a TB that
does not sum to zero (POP-3); and one amount carrying half a paisa (`1234.565`), so half-up and
half-to-even rounding give different paise.

Masterid 19-37 (added for `cash_payments_40a3`) extend the same book: a payee over the s.40A(3)
daily limit aggregated across two same-day vouchers, a payee exactly at the limit and one under
it, a goods-carriage heuristic hit within the ₹35,000 proviso limit, one cash payment excluded
from s.40A(3) scope for each of the five `excluded_group_roles` kinds (the loans_liability
example, "Highway Motors Loan", sits two group levels below "Loans (Liability)", so it also
proves the exclusion walks the full chain, not just the immediate parent), s.269ST receipts and
payments at/over and under the limit, a real party with a Duties & Taxes or `round_off_ledgers`
line folded into it, an unidentified-party cash leg (a Sales/Purchase Accounts line plus a tax
line, no real party line), and a covered/uncovered pair of s.269SS/269T loan candidates in both
directions ("Sunrise NBFC Loan", configured in `[loans.loan_ledgers]`; "Highway Motors Loan",
deliberately left out of it).

Masterid 38-42 (2026-09-18 mutation review, see the crate's `src/cash_payments_40a3.rs` git
history) add one case exactly AT each of four inclusive ("or more"/"at or over"/"up to")
comparisons the fixture had never exercised at the boundary, so a flipped comparison operator in
the Rust port changes this fixture's result and not only a real client's: a s.269ST receipt of
exactly ₹2,00,000 from one party in one day ("Fairfield Textiles"), a s.269ST payment leg of
exactly ₹2,00,000 to one party in one day ("Ashwood Traders"), a s.269SS/269T loan-ledger line of
exactly ₹20,000 (a second "Sunrise NBFC Loan" voucher, cash accepted), and the goods-carriage
proviso limit itself: one payee-day of exactly ₹35,000 ("Coastal Freight Carriers", still within
the higher limit) and one of ₹35,000.01, one paisa over it ("Bayside Cargo Logistics", now
outside it) -- both transport-name heuristic hits and both over the plain s.40A(3) limit, so both
also carry an s.40A(3) finding.

`synthetic-engagement.toml` is the engagement for it: the reference-engine client-config keys
`cash_44ab`, `cash_payments_40a3` and `depreciation` read, and nothing else.

Masterid 43-49 (added for `depreciation`) extend the same book further: three real blocks the
vendored rules carry (furniture_10, plant_machinery_15, computers_40), fully mapped -- including
the pre-existing "Delivery Van" from the `cash_payments_40a3` extension above, which shares
`plant_machinery_15`'s block per the module's own rule that a motor vehicle not used in a hiring
business is general Plant & Machinery, not a separate code. One case exactly AT each boundary the
module tests: "Office Furniture" put to use for exactly 180 days (full rate) beside "Showroom
Furniture" one calendar day later, at 179 days (half rate); "Office Computers" paid exactly Rs
10,000 in cash -- the s.43(1) second proviso limit itself -- not flagged, beside "Reception
Computers" at Rs 10,000.01, one paisa over it, flagged. Also: "Factory Machine", whose Rs 5,50,000
deletion exceeds the full-rate pool and spills into the half-rate one (s.43(6)), and whose
depreciation-journal voucher (crediting it, debiting "Depreciation A/c") wires
`dep_expense_ledgers` and the book-vs-Act tie; and a GST line beside the Office Computers addition
proving `gst_tcs_addition_lines_seen_count` is exercised on data. "Comfort Furnishings" and
"Machinery Disposal Proceeds" are invented counter-ledgers only, carrying no depreciation meaning
of their own. "Owner Capital"'s opening balance was adjusted (from -Rs 1,20,000 to -Rs 5,70,000)
to offset "Factory Machine"'s new Rs 4,50,000 opening balance, so ledger openings still sum to
zero (POP-3) -- this has no effect on `cash_44ab`/`cash_payments_40a3`, which never read a
ledger's opening balance. The "unmapped Fixed Assets ledger fails loud" and DEP-1/DEP-2 paths
(which would blank every block figure for the whole test) are deliberately NOT in this shared
fixture; they are covered by the crate's own Rust unit tests on hand-built books instead
(`src/depreciation.rs`), since triggering them here would blank every other depreciation figure
this fixture is otherwise exercising.

## The goldens

`golden/synthetic.cash_44ab.json`, `golden/synthetic.cash_payments_40a3.json`,
`golden/synthetic.depreciation.json`, `golden/synthetic.financial_statements.json`,
`golden/synthetic.financial_statements.noreport.json`, `golden/synthetic.applicability_44ab.json`,
`golden/synthetic.trial_balance.json`, `golden/synthetic.stale_balances_41_1.json`,
`golden/synthetic.ledger_scrutiny.json` and `golden/synthetic.cash_book_integrity.json` are the
reference Python implementation's own canonical dumps
for that engagement and read: its `tally-read-v1` adapter built the book, the named test ran with
its AY 2026-27 rules, and its own canonical serialiser produced the result. All three regenerated
together at reference-implementation commit `04a34da8bfcb71b2093c0cc66b19d11e3d05ded1`
because masterid 43-49 changed the shared book's cash and bank totals too (the Office/Reception
Computers cash-paid additions), by

```
uv run -q --with openpyxl --with xlrd --with python-docx --with jsonschema --with striprtf \
    --with pdfplumber python parity/python_golden.py ENGINE \
    tests/fixtures/synthetic-engagement.toml tests/fixtures/golden/synthetic.cash_44ab.json \
    --test cash_44ab
uv run -q --with openpyxl --with xlrd --with python-docx --with jsonschema --with striprtf \
    --with pdfplumber python parity/python_golden.py ENGINE \
    tests/fixtures/synthetic-engagement.toml tests/fixtures/golden/synthetic.cash_payments_40a3.json \
    --test cash_payments_40a3
uv run -q --with openpyxl --with xlrd --with python-docx --with jsonschema --with striprtf \
    --with pdfplumber python parity/python_golden.py ENGINE \
    tests/fixtures/synthetic-engagement.toml tests/fixtures/golden/synthetic.depreciation.json \
    --test depreciation
```

(`golden/synthetic.cash_44ab.json` was first produced at commit `c2f206beb870a8fdb0775f2d1c71aa1ccfccca64`,
regenerated at `dd376ed014d922a1e2b12052763af36a565402ec` because masterid 19-37 changed the shared
book's cash and bank totals, regenerated again at `b924bf69573deb94d0781beb5e007bf89b4fb2a4` because
masterid 38-42 did too, and regenerated once more at the commit above for masterid 43-49.
`golden/synthetic.cash_payments_40a3.json` was regenerated at the same three points for the same
reasons. `golden/synthetic.depreciation.json` is new at the commit above.)

2026-09-19: `cash_payments_40a3` and `depreciation` switched their per-row figure/finding ids from
hashing a ledger's display NAME to `tae.ledger_ids.stable_ledger_tag` (Tally GUID) at reference-
implementation commit `84386b14b77a291c0bf5b1ae70e7e8afb9b609a3` -- the same rename-churn fix
`creditor_ageing_43bh` and the rest of `audit_tests` already had; `cash_44ab` needed no change (it
carries no name-derived id). `synthetic-read/parts/ledgers.xml` (and its `manifest.json` sha256/
bytes) gained a `<GUID>`/`<MASTERID>` per ledger master (`parity/generate_fixture.py`'s `lguid`,
masterid 501+, distinct from the voucher masterids 1-49 above) -- a real Tally ledger export always
carries one; a ledger with none falls back to a hash of its name (`docs/tax-audit/parity-spec-v1.md` §11). `golden/synthetic.
cash_payments_40a3.json` and `golden/synthetic.depreciation.json` were regenerated at the commit
above (id-tag text only; every figure/finding value is byte-identical to the previous goldens).
`golden/synthetic.cash_44ab.json` is untouched (same reason it needed no source change). Fixture
regenerated first, deterministically, by `python3 parity/generate_fixture.py tests/fixtures`, then
the two goldens by the same `python_golden.py` invocations above (ENGINE pointed at the reference-
implementation commit above), before the two SHA-256 rows here were updated by hand from that
output.

2026-09-21: the reference implementation's `depreciation` changed its `book_vs_act` finding at
commit `7598ebffcdce8ff2004ce2d93ce96b0340ab633c` (this crate follows in the same change). The title
"Book depreciation differs from Income-tax Act depreciation" asserted a difference even when it was
zero; it is now "Book vs Income-tax Act depreciation". And when no block is mapped, no opening WDV is
configured and no Fixed Assets ledger carries a balance or movement, the finding is no longer a
vacuous `computed` (its "every ledger mapped" check passed on an empty set): it is
`judgement_required`, titled "Depreciation not computed: ...", citing no zero total (only a non-zero
book charge, when there is one). `golden/synthetic.depreciation.json` was regenerated by the
`python_golden.py --test depreciation` invocation above; it differs from the previous golden only in
that finding's `title_text`/`title_sha256_16`. This fixture maps blocks, so it exercises the new
title, not the new branch; the branch is covered by the crate's unit tests on hand-built books and,
on real data, by the third client engagement below. The other two goldens are unchanged.

Masterid 50-53 and seven ledgers appended after every earlier one (so no earlier ledger's
masterid or GUID moved), for `financial_statements` (2026-09-21): three journals between
non-cash, non-bank ledgers -- "Freight Inward" under a "Carriage Inward" subgroup of Direct
Expenses (the primary group is the chain's last element, not the immediate parent), "Job Work
Income" (Direct Incomes) and "Interest Received" (Indirect Incomes), both credit-balance groups
that are sign-flipped -- and a fourth debiting "Interest to Partners", the `[partners.partner_a]`
interest ledger in `synthetic-engagement.toml` (its `capital_ledgers` entry is there only because
the reference's own binding binds every `[partners.*]` location). Two Stock-in-Hand ledgers carry
no voucher: "Hardware Stock", whose TB debit column carries Rs 75,000 and credit column Rs 5,000
while its closing field stays a copy of its Rs 60,000 opening (`STALE_TB_DEBIT`/`STALE_TB_CREDIT`
in the generator: the non-integrated inventory quirk the test counts; the credit makes closing
stock depend on the "- credit" term; POP-1 still ties because no voucher moves it), and "Packing
Material Stock", opening Rs 10,000 with no movement, not stale. "Owner Capital"'s opening moved
from -Rs 5,70,000 to -Rs 6,40,000 to offset the two stock openings, so TB openings still sum as
before (POP-3). The three new P&L primaries and two subgroups are added to the group masters. The
closing high-water mark rose to ALTERID 153 (masterid 53). No cash, bank or Fixed Assets ledger is
touched: the `cash_44ab`, `cash_payments_40a3` and `depreciation` goldens regenerated byte-identical
after this change.

`synthetic-report-totals.json` is invented report totals for the tie branch, each exactly Rs 1
from the derived figure (net profit -Rs 30,304.25 against -Rs 30,305.25; closing stock
Rs 1,40,001.00 against Rs 1,40,000.00): FS-1's tolerance is inclusive, so the committed golden is
`computed` and a comparison that used `<` would disagree with it. This crate takes report totals
as caller data and never parses a Tally report (a native-report reader needs its own ADR first).

`applicability_44ab` (2026-09-21) needs no new book data: it combines `financial_statements`'
`sales` and `cash_44ab`'s cash share, as the reference's own pack does. `synthetic-engagement.toml`
gains an invented `[presumptive_history]` (one year, `opted_44ad = true`), and
`synthetic-turnover-inputs.json` is invented comparison turnover: a full-coverage GSTR-1 figure
Rs 2,500 below books turnover (differenced), a GSTR-3B figure marked as covering the B2B section
only (never differenced), and no AIS figure. The five earlier goldens regenerated byte-identical.

Both `financial_statements` goldens were regenerated at reference-implementation commit
`c85b8162ec9f45a40f2e55ab770158f6556d1e48` for the "excluded_voucher" evidence kind: the figures
that list excluded vouchers now cite them as `excluded_voucher`, which POP-4 checks is really
excluded instead of flagging, so the four POP-4 violations those figures used to raise on this
fixture are gone. Nothing else in either golden changed; the other four goldens are byte-identical.

Both `financial_statements` goldens were regenerated again at reference-implementation commit
`41a392a2adaa9a19128e0baa65dc8138ef1d7a10` for the report tie's status: each gains the text figure
`report_tie_status` ("performed: net profit and closing stock" with the synthetic report totals,
"not performed: no report part in this read" without them), cited as a fact of the report-tie
finding. In the no-report golden that finding's title now says the tie was not performed and its
confidence is `needs_document` instead of `computed`. Nothing else in either golden changed; the
other four goldens are byte-identical.

All six goldens were regenerated at reference-implementation commit
`b559cad6bebd68fcfe84dd59e590359572c2a7f2` for the book invariant MAP-0 (every in-books voucher
line posts to a ledger the book's masters carry): each gains only `"MAP-0"` in
`book_invariants_evaluated`. The fixture has no such line, so no violation is added.

Batch 1 of the port (2026-09-21) adds `trial_balance`, `stale_balances_41_1`, `ledger_scrutiny`
and `cash_book_integrity`. Two ledgers are appended after every earlier one (so no earlier ledger's
masterid or GUID moved): "Quarry Lane Stores" (Sundry Debtors, opening Rs 12,345) and "Harbour Mill
Supplies" (Sundry Creditors, opening -Rs 12,345), which no voucher touches, so the Trial Balance
carries each opening unchanged with nil movement: `stale_balances_41_1`'s stale path, its two
findings and its per-ledger figures run in CI. Their openings are equal and opposite, so TB
openings still sum as before (POP-3). Only `parts/ledgers.xml`, `parts/tb_fy.xml` and
`manifest.json` changed. All ten goldens were regenerated at reference-implementation commit
`57f2619b686b9fea7a8f1f39f8c547012e66757e`, from an archive of that commit, by the invocations
above (with `--test trial_balance`, `--test stale_balances_41_1`, `--test ledger_scrutiny` and
`--test cash_book_integrity` for the four new ones, each with no further arguments). The six
existing goldens came out byte-identical. `ledger_scrutiny` and `cash_book_integrity` already
reach their finding paths on the unchanged vouchers (7 and 2 findings).

What the synthetic read reaches for batch 1, and what it does not. `cash_book_integrity` has five
parts; the synthetic golden is non-zero only in part 1 (negative cash) -- its openings sum to zero,
no own-account terms are configured, and no receipt credits an expense or journal pays cash to one
-- so parts 2 to 5 are evidenced by the edge books below, not by the synthetic read.
`ledger_scrutiny`'s large-entry and contra-nature indicators do not fire on it either. Parsing a
voucher's NARRATION, which the `Book` now carries for `cash_book_integrity`, is tested on a copy of
the synthetic read in `tests/consumer_rules.rs`.

## The edge books

`edge-books/*.json` are small invented books, each written by hand to reach boundaries and
branches the synthetic read does not, and each carrying a `comment` naming what it reaches:
`tb_rows.json` (`trial_balance`: closing-only, zero and ledger-less rows, an empty chain, the
reserved Primary marker, and the row order), `stale.json` (`stale_balances_41_1`: the Re 1 active
boundary, a credit-only movement, a debtor with no TB row, zero-amount and optional lines, STL-1
firing), `scrutiny.json`, `scrutiny_default.json` and `scrutiny_short.json` (`ledger_scrutiny`:
the Rs 50,000 and 30% boundaries, the last-days window's first day, cash legs, journal-only and
contra-nature, LSC-1 and its tolerance, no rules table, a 4-day period), and `cash_book.json`
(`cash_book_integrity`: all five parts, equal minima, CBI-2 firing); and two written by an
independent reviewer to catch what those miss, `cash_book_misc.json` (own-account terms repeated
in different case, a Contra between two bank ledgers, zero-amount expense lines, one voucher
matching on several lines) and `scrutiny_misc.json` (a debit cash leg, a large credit entry, a
ledger with no TB row, a ledger under both expense groups); and `cash_book_unicode.json`
(narrations and a term Unicode 17 and 15.1 upper-case differently). Two vouchers have no number
and a non-ASCII GUID whose 12-byte cut splits a character, so the voucher label must take the last
12 characters, as the reference does. None of these is a Tally read: they establish that the port
and the reference agree on the same book, and nothing about reading Tally.

One divergence is known and not fixed here: case mapping. Rust 1.96 carries Unicode 17.0 and the
reference's Python 3.13 carries 15.1.0, and upper- and lower-casing each differ at 55 code points
(measured over every code point). All 55 for lower-casing, and 51 for upper-casing, are unassigned in
15.1; the other 4 (U+019B, U+0264, U+A7D3, U+A7D5) are older lowercase letters whose uppercase
partner was assigned later. A narration holding one could change `cash_book_integrity`'s parts
3 and 5, so every case mapping in the crate goes through `support::py_upper` / `py_lower`, pinned
to 15.1 (parity spec §4.1). `cash_book_unicode.json` holds such narrations and such a term, and its
reference golden -- 0 repeated narrations, 0 own-account matches -- is what the pinned mapping
gives; unpinned Rust mapping gives a repeated pair and a deposit.

`golden/edge.NAME.TEST.json` is the reference implementation's own canonical dump for that book,
built with its own model (`tae.model`) by `parity/edge_golden.py` -- company GUID
`invented-edge-company`, an individual's AY 2026-27 engagement, its own rules with any table the
book's `rules_without` names removed -- and `golden/edge.tb_rows.trial_balance.order.json` is the
order in which it emits `trial_balance`'s per-ledger rows, which the canonical dump (sorted by id)
does not show. All were produced at reference-implementation commit
`57f2619b686b9fea7a8f1f39f8c547012e66757e`, from an archive of that commit, by

```
uv run -q --with openpyxl --with xlrd --with python-docx --with jsonschema --with striprtf \
    --with pdfplumber python parity/edge_golden.py ENGINE tests/fixtures/edge-books/NAME.json \
    tests/fixtures/golden
```

once per book. `tests/edge_books.rs` builds each book in Rust, runs each named test with its module
check, compares the whole dump with `compare` -- every field of it, the spec and test versions
included -- and compares the row order. Every mutation written for this crate is recorded in
`parity/mutations.json` with its author, and `parity/mutations.py` re-runs them. Of 52 hand-written
mutations of the four modules and the NARRATION parse (32 from an independent reviewer, 20 from the
author; see the PR), the crate's suite fails on every one, and the edge books alone on 50: the other
two alter how the read's XML is parsed, which the edge books bypass by building the book directly,
and `tests/consumer_rules.rs` catches both.

### What the three-client parity does and does not evidence

The local parity run (never committed) compares this crate with the reference implementation on
three real client engagements. A test counts as having run on a client only if it is non-vacuous
there, meaning at least one figure is non-zero. For `depreciation`, the third engagement is vacuous:
7 figures, all zero, because it maps no block, no opening WDV and no depreciation-expense ledger.
Its "identical" row compared zeros (and, since 2026-09-21, the "not computed" finding). So the real-
data evidence for `depreciation`'s arithmetic rests on the first two engagements (61 of 68 and 37 of
48 figures non-zero). `cash_44ab` and `cash_payments_40a3` are non-vacuous on all three.

Batch 1 (2026-09-21: the Rust port at this branch's head against dumps from the reference
implementation's engine worktree at `145622de`, whose four batch-1 modules are those of `57f2619b`) is identical
on all twelve test-client pairs. `trial_balance` and `ledger_scrutiny` are non-vacuous on all three.
`stale_balances_41_1` finds stale ledgers on the first two only; on the third its figures are
counts with no stale ledger, so its stale path is evidenced there only by the synthetic read and
the edge books. `cash_book_integrity` is non-zero only in part 4 (receipts credited to expenses)
on the first two, and in all five parts on the third, which is the only one configuring
own-account terms. `ledger_scrutiny`'s contra-nature indicator fires on none of the three: only the
edge books reach it.

The two `financial_statements` goldens were produced at reference-implementation commit
`7598ebffcdce8ff2004ce2d93ce96b0340ab633c`, after the fixture change above, by

```
uv run -q --with openpyxl --with xlrd --with python-docx --with jsonschema --with striprtf \
    --with pdfplumber python parity/python_golden.py ENGINE \
    tests/fixtures/synthetic-engagement.toml tests/fixtures/golden/synthetic.financial_statements.json \
    --test financial_statements --report-totals tests/fixtures/synthetic-report-totals.json
uv run -q --with openpyxl --with xlrd --with python-docx --with jsonschema --with striprtf \
    --with pdfplumber python parity/python_golden.py ENGINE \
    tests/fixtures/synthetic-engagement.toml tests/fixtures/golden/synthetic.financial_statements.noreport.json \
    --test financial_statements
```

`golden/synthetic.applicability_44ab.json`, at the same reference commit, by

```
uv run -q --with openpyxl --with xlrd --with python-docx --with jsonschema --with striprtf \
    --with pdfplumber python parity/python_golden.py ENGINE \
    tests/fixtures/synthetic-engagement.toml tests/fixtures/golden/synthetic.applicability_44ab.json \
    --test applicability_44ab --turnover-inputs tests/fixtures/synthetic-turnover-inputs.json
```

## Regenerating, and where later batches record their fixtures

From 2026-09-22 every golden is produced under Python 3.13 (`uv run --python 3.13 ...`), the
version whose Unicode tables the crate reproduces; the ten synthetic goldens and every edge golden
above regenerate byte-identical under it. `parity/python_golden.py`'s runner table names exactly the tests in `src/registry.rs`
(`tests/registry.rs` checks it), and `parity/edge_golden.py`'s runners name exactly the tests
`tests/edge_books.rs` dispatches, all of them registered (`edge_runners_agree_across_the_two_sides`).
`tests/edge_books.rs` builds every book in `edge-books/`, with no hand-kept list, and fails on a
golden that no book names or no registered test owns. `tests/provenance_rows.rs` fails on a golden
or edge book whose byte row is missing, names another file, or does not match the file's size
and SHA-256 (it checks them itself, since the repository's gate skips a row it cannot parse).

From batch 2 on, each batch records its own fixtures -- prose and byte table -- in
`provenance/<batch>.md` under this directory, not in this file, so two lanes porting in parallel
never edit the same table. The fixture-provenance gate reads every Markdown file under
`tests/fixtures`.

## Bytes

| Fixture | Bytes | SHA-256 | Path |
| --- | ---: | --- | --- |
| `synthetic.cash_44ab.json` | 5,841 | `cf1a9f74e3622fdd969cb4250a9c5ac46c1b42737fe51460c9dffc286d408cc7` | `golden/synthetic.cash_44ab.json` |
| `synthetic.cash_payments_40a3.json` | 72,297 | `f12b0ac35ccd09fc406037579442afeb47d3a2e26ee540acc4bd114b023a3364` | `golden/synthetic.cash_payments_40a3.json` |
| `synthetic.depreciation.json` | 27,626 | `0ccb72fb5c4c65f046bd6358878a717b1fd57ec46d33c4f7ffed06cd4296ea36` | `golden/synthetic.depreciation.json` |
| `synthetic.financial_statements.json` | 16,705 | `b40e6fac2ee98cb37528ecc2fe0f42c6afacf31974eec7ba5be30f74201a324f` | `golden/synthetic.financial_statements.json` |
| `synthetic.financial_statements.noreport.json` | 15,682 | `d407d2e72b09ad9348e8cfb19dbeff90a3617742ef8a348eb9e29018d1fa0c93` | `golden/synthetic.financial_statements.noreport.json` |
| `synthetic-report-totals.json` | 134 | `e772509bd6ebc52afc23ef9742b6b1f2a090737533abe3761a7448124411b7e3` | `synthetic-report-totals.json` |
| `synthetic.applicability_44ab.json` | 10,679 | `b90cf74038dcb1b26e4a9e8236861447e4b10d303027fbeb2e7359ead81778f5` | `golden/synthetic.applicability_44ab.json` |
| `synthetic.trial_balance.json` | 92,608 | `6236b033586f46684545d189b9e98cf93872775250f296fa60b5adf9bf5fd305` | `golden/synthetic.trial_balance.json` |
| `synthetic.stale_balances_41_1.json` | 9,820 | `0e58840903da8234858a209606305d1a1d1aa7a33f89ea7441ab98fd633c0e48` | `golden/synthetic.stale_balances_41_1.json` |
| `synthetic.ledger_scrutiny.json` | 51,342 | `6b0358464627bc7e7d4cc9fd206cc2f4564ed7a35a1b769d303e5d0986d1470e` | `golden/synthetic.ledger_scrutiny.json` |
| `synthetic.cash_book_integrity.json` | 15,221 | `2ee1cbc0de9d5087956c4115610c74a8cdd15cc361296dd09b685363bdf74289` | `golden/synthetic.cash_book_integrity.json` |
| `cash_book.json` | 8,499 | `1ae602f22368e3b549ce1430770f097758f13efb716025bcb2bab2a2e4a34f11` | `edge-books/cash_book.json` |
| `cash_book_misc.json` | 5,333 | `23bd18a9b41d3768ce4cba9c9b5243b0822316e5fa7ab0308b83226c581c93f0` | `edge-books/cash_book_misc.json` |
| `cash_book_unicode.json` | 2,262 | `ec61a34edc11214da0c5cac2b7b7d40abe148c0fbcfec1582379c57a21ff3bc3` | `edge-books/cash_book_unicode.json` |
| `scrutiny.json` | 7,211 | `047e7ca918611b43b7480fef16841fadb54016360b16ebca30e9c94df05c3fb4` | `edge-books/scrutiny.json` |
| `scrutiny_default.json` | 1,108 | `04bf6e5efb9c45601803624f8931ee563f23fbc502580fcc3c28b6870c9dd37e` | `edge-books/scrutiny_default.json` |
| `scrutiny_misc.json` | 4,300 | `258aae0ba8d75656870d57638345b10d465edabd797f9cce4bab999f39812a8d` | `edge-books/scrutiny_misc.json` |
| `scrutiny_short.json` | 1,212 | `e60f643456d1b812bb82c24ad581a030c4ae0dcc1c9701560ef1ace44d54b46e` | `edge-books/scrutiny_short.json` |
| `stale.json` | 3,134 | `3e896344abf36b0469d289d69dabfdc5206ee4998c2df505bed449fb79255059` | `edge-books/stale.json` |
| `tb_rows.json` | 2,362 | `5e3df2c81094e5ea7577309b48597bc03067a4f9ba09175a610b627b62fc31cd` | `edge-books/tb_rows.json` |
| `edge.cash_book.cash_book_integrity.json` | 32,683 | `510282f185b850cec19cacb06ec08a82c4d8d9bf3b88fd12e8aae37247584078` | `golden/edge.cash_book.cash_book_integrity.json` |
| `edge.cash_book_misc.cash_book_integrity.json` | 21,454 | `c955eeca88f6880d43ffb93c5642a37c625c55a334323a6e15f0981097de747c` | `golden/edge.cash_book_misc.cash_book_integrity.json` |
| `edge.cash_book_unicode.cash_book_integrity.json` | 8,864 | `6c65cd4aafc2ed0a4935ade0d222dbbbe013cd6bfc917cd6832b04b7df101bd7` | `golden/edge.cash_book_unicode.cash_book_integrity.json` |
| `edge.scrutiny.ledger_scrutiny.json` | 69,427 | `52ba969e677ef12c09cf26fb118b6ebca4aace4a4ed5a76f0063e2409475f666` | `golden/edge.scrutiny.ledger_scrutiny.json` |
| `edge.scrutiny_default.ledger_scrutiny.json` | 8,407 | `03c41d14a83cd4da0bb94e026f93dc36438270b9a430d84f67967ed7339e58b7` | `golden/edge.scrutiny_default.ledger_scrutiny.json` |
| `edge.scrutiny_misc.ledger_scrutiny.json` | 44,609 | `920f462e1642ea33f8249f3bece1fded847ee83886bc678a26f46678628dcf45` | `golden/edge.scrutiny_misc.ledger_scrutiny.json` |
| `edge.scrutiny_short.ledger_scrutiny.json` | 8,638 | `9cd03cab0d2c3daebf60368faf9130366d19726963b0815dc72758fa62ef1112` | `golden/edge.scrutiny_short.ledger_scrutiny.json` |
| `edge.stale.stale_balances_41_1.json` | 9,839 | `39a7bef9ed505d7ec78806d1b6ab524eeaaccc23aac95721a1f65a6b6bc37319` | `golden/edge.stale.stale_balances_41_1.json` |
| `edge.tb_rows.trial_balance.json` | 14,783 | `870dd45e33e6350394c42d3e4c0ed5ec5be3f875b5a131e4e812682d88c8f9b6` | `golden/edge.tb_rows.trial_balance.json` |
| `edge.tb_rows.trial_balance.order.json` | 219 | `2ace3be45ee1cd6ae4757727e947e07fd2ca3600a83f480b279bdda181d5f43d` | `golden/edge.tb_rows.trial_balance.order.json` |
| `synthetic-turnover-inputs.json` | 153 | `970500728d9d0447cb3fe1b6e870d5fea2c3a1bb919da05f1d601d4ba6f66929` | `synthetic-turnover-inputs.json` |
| `synthetic-engagement.toml` | 2,821 | `c179b7ebcc9a03c9a4d836c62298aaa5841ee2bf20f7df51bd1010f83a06c68b` | `synthetic-engagement.toml` |
| `manifest.json` | 9,711 | `d3948085c8466002c269133fd59c5f6361acab028ff6db74bd1fd6d23f7271a7` | `synthetic-read/manifest.json` |
| `company_object.xml` | 606 | `f1b6fe4e6b6cc406a4ae92ce0ef62a6c79a88a99ac83b1888989a98fbee967b4` | `synthetic-read/parts/company_object.xml` |
| `groups.xml` | 8,102 | `12e4d994960ecd768cd33fb4565f19b140a765d3f9f4982dbcfe1a108d9e5214` | `synthetic-read/parts/groups.xml` |
| `high_water_after.xml` | 643 | `0d482540a4ac4beebfe19e5f5dd695e748084ee1779bf80b012b5bd489a17eaa` | `synthetic-read/parts/high_water_after.xml` |
| `high_water_before.xml` | 643 | `0d482540a4ac4beebfe19e5f5dd695e748084ee1779bf80b012b5bd489a17eaa` | `synthetic-read/parts/high_water_before.xml` |
| `ledgers.xml` | 27,528 | `89fce4866d4cb45934f50764c6de618421adab52ea0c8b543af02f1e1a7f944e` | `synthetic-read/parts/ledgers.xml` |
| `tb_fy.xml` | 13,497 | `261c76d9756be12783e98ec27f19bc4d7960cd47fa62c511639bf285f4bb4213` | `synthetic-read/parts/tb_fy.xml` |
| `voucher_status_list.json` | 249 | `c7959e4e91a445f774ff2a5eaad4f8968f6fc82e21622d5168ececf9b0a1aa78` | `synthetic-read/parts/voucher_status_list.json` |
| `vouchers_h1.xml` | 50,762 | `993dc982021b41efb7303a736d70a6c846de87a8cd815ec8c7d049bdad759884` | `synthetic-read/parts/vouchers_h1.xml` |
| `vouchers_h2.xml.gz` | 1,156 | `cd9cb1f339141bc1af8585fd16c445f21016dccbbccd5cd21d1bd82c1feb41b1` | `synthetic-read/parts/vouchers_h2.xml.gz` |
| `vouchertypes.xml` | 1,307 | `56a74424d30240ea24c2e0858e854ac3e199123d92f6fb8b8cfc640b068b42da` | `synthetic-read/parts/vouchertypes.xml` |

Sensitivity review: invented names, GUIDs and amounts only; no client, person, path or host
appears in any file.
