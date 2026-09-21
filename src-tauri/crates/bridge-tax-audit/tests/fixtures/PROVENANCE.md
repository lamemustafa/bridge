# bridge-tax-audit fixture provenance

No client data. Every fixture here is synthetic: the company ("Lakeview Hardware Demo"), its
GUID, groups below the reserved primaries, ledgers, voucher types, vouchers and amounts are
invented.

## What these fixtures establish, and what they do not

They establish **parity**: that this crate and the Python reference engine it ports compute
the same canonical `cash_44ab`, `cash_payments_40a3`, `depreciation`, `financial_statements` and `applicability_44ab` results
(`docs/tax-audit/parity-spec-v1.md`) from the same bytes. They do **not** establish anything
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
`golden/synthetic.financial_statements.noreport.json` and `golden/synthetic.applicability_44ab.json` are the reference Python implementation's own canonical dumps
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

### What the three-client parity does and does not evidence

The local parity run (never committed) compares this crate with the reference implementation on
three real client engagements. A test counts as having run on a client only if it is non-vacuous
there, meaning at least one figure is non-zero. For `depreciation`, the third engagement is vacuous:
7 figures, all zero, because it maps no block, no opening WDV and no depreciation-expense ledger.
Its "identical" row compared zeros (and, since 2026-09-21, the "not computed" finding). So the real-
data evidence for `depreciation`'s arithmetic rests on the first two engagements (61 of 68 and 37 of
48 figures non-zero). `cash_44ab` and `cash_payments_40a3` are non-vacuous on all three.

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

## Bytes

| Fixture | Bytes | SHA-256 | Path |
| --- | ---: | --- | --- |
| `synthetic.cash_44ab.json` | 5,828 | `c424096e2accfab877b68d5391f81a5e5319698637e2b68d64cae75314d3ac8f` | `golden/synthetic.cash_44ab.json` |
| `synthetic.cash_payments_40a3.json` | 72,284 | `9dc98414ef0271c1ca2544a7e7042716471ec7f5c260c52f3ab0193d947f3269` | `golden/synthetic.cash_payments_40a3.json` |
| `synthetic.depreciation.json` | 27,613 | `0c100a919eee8443b4622a1aac568707c7e3a5849a496759816c4e1c56520ae8` | `golden/synthetic.depreciation.json` |
| `synthetic.financial_statements.json` | 16,686 | `e0f5038fa515c3e39c6af49dcfc6ccf66ff04172b7da6b770c48eefdc9725cc5` | `golden/synthetic.financial_statements.json` |
| `synthetic.financial_statements.noreport.json` | 15,688 | `9d63143f038a78e936237b622fe203796c47eef6cff03cd95630511e25aa7800` | `golden/synthetic.financial_statements.noreport.json` |
| `synthetic-report-totals.json` | 134 | `e772509bd6ebc52afc23ef9742b6b1f2a090737533abe3761a7448124411b7e3` | `synthetic-report-totals.json` |
| `synthetic.applicability_44ab.json` | 10,666 | `3ecd7d2ce69b076833a0c3c41e8b64906fd1286d3b1f076b9e57f5bcb400a2b9` | `golden/synthetic.applicability_44ab.json` |
| `synthetic-turnover-inputs.json` | 153 | `970500728d9d0447cb3fe1b6e870d5fea2c3a1bb919da05f1d601d4ba6f66929` | `synthetic-turnover-inputs.json` |
| `synthetic-engagement.toml` | 2,821 | `c179b7ebcc9a03c9a4d836c62298aaa5841ee2bf20f7df51bd1010f83a06c68b` | `synthetic-engagement.toml` |
| `manifest.json` | 9,711 | `6872873e193231239b7e2bf38ef7d482dd8cf7c43fb00ab76b0bc0dd07e8833a` | `synthetic-read/manifest.json` |
| `company_object.xml` | 606 | `f1b6fe4e6b6cc406a4ae92ce0ef62a6c79a88a99ac83b1888989a98fbee967b4` | `synthetic-read/parts/company_object.xml` |
| `groups.xml` | 8,102 | `12e4d994960ecd768cd33fb4565f19b140a765d3f9f4982dbcfe1a108d9e5214` | `synthetic-read/parts/groups.xml` |
| `high_water_after.xml` | 643 | `0d482540a4ac4beebfe19e5f5dd695e748084ee1779bf80b012b5bd489a17eaa` | `synthetic-read/parts/high_water_after.xml` |
| `high_water_before.xml` | 643 | `0d482540a4ac4beebfe19e5f5dd695e748084ee1779bf80b012b5bd489a17eaa` | `synthetic-read/parts/high_water_before.xml` |
| `ledgers.xml` | 26,356 | `824dc80ffdc97e0b396c5d810b553e55f978fa9c14c65c19b9da0452a85df896` | `synthetic-read/parts/ledgers.xml` |
| `tb_fy.xml` | 12,942 | `8640ebe7761fffab0b9232cc19fb83ae82c9a2500e37b59a5aa090614057f1e4` | `synthetic-read/parts/tb_fy.xml` |
| `voucher_status_list.json` | 249 | `c7959e4e91a445f774ff2a5eaad4f8968f6fc82e21622d5168ececf9b0a1aa78` | `synthetic-read/parts/voucher_status_list.json` |
| `vouchers_h1.xml` | 50,762 | `993dc982021b41efb7303a736d70a6c846de87a8cd815ec8c7d049bdad759884` | `synthetic-read/parts/vouchers_h1.xml` |
| `vouchers_h2.xml.gz` | 1,156 | `cd9cb1f339141bc1af8585fd16c445f21016dccbbccd5cd21d1bd82c1feb41b1` | `synthetic-read/parts/vouchers_h2.xml.gz` |
| `vouchertypes.xml` | 1,307 | `56a74424d30240ea24c2e0858e854ac3e199123d92f6fb8b8cfc640b068b42da` | `synthetic-read/parts/vouchertypes.xml` |

Sensitivity review: invented names, GUIDs and amounts only; no client, person, path or host
appears in any file.
