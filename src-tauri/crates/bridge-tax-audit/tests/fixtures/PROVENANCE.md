# bridge-tax-audit fixture provenance

No client data. Every fixture here is synthetic: the company ("Lakeview Hardware Demo"), its
GUID, groups below the reserved primaries, ledgers, voucher types, vouchers and amounts are
invented.

## What these fixtures establish, and what they do not

They establish **parity**: that this crate and the Python reference engine it ports compute
the same canonical `cash_44ab` and `cash_payments_40a3` results
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
`cash_44ab` and `cash_payments_40a3` read, and nothing else.

## The goldens

`golden/synthetic.cash_44ab.json` and `golden/synthetic.cash_payments_40a3.json` are the
reference Python implementation's own canonical dumps for that engagement and read: its
`tally-read-v1` adapter built the book, the named test ran with its AY 2026-27 rules, and its
own canonical serialiser produced the result. Produced at reference-implementation commit
`b924bf69573deb94d0781beb5e007bf89b4fb2a4` by

```
uv run -q --with openpyxl --with xlrd --with python-docx --with jsonschema --with striprtf \
    --with pdfplumber python parity/python_golden.py ENGINE \
    tests/fixtures/synthetic-engagement.toml tests/fixtures/golden/synthetic.cash_44ab.json \
    --test cash_44ab
uv run -q --with openpyxl --with xlrd --with python-docx --with jsonschema --with striprtf \
    --with pdfplumber python parity/python_golden.py ENGINE \
    tests/fixtures/synthetic-engagement.toml tests/fixtures/golden/synthetic.cash_payments_40a3.json \
    --test cash_payments_40a3
```

(`golden/synthetic.cash_44ab.json` was first produced at commit `c2f206beb870a8fdb0775f2d1c71aa1ccfccca64`,
regenerated at `dd376ed014d922a1e2b12052763af36a565402ec` because masterid 19-37 changed the shared
book's cash and bank totals, and regenerated again at the commit above because masterid 38-42 did
too. Both goldens were regenerated at the commit above for the same masterid 38-42 change.)

## Bytes

| Fixture | Bytes | SHA-256 | Path |
| --- | ---: | --- | --- |
| `synthetic.cash_44ab.json` | 5,828 | `d023d87f6b2d59224c505f2fca54f512a233913caaf383bf9d6e482864a542f0` | `golden/synthetic.cash_44ab.json` |
| `synthetic.cash_payments_40a3.json` | 69,094 | `49d04faa59d68b8fba06dd6757535ee98085bde01ecf2444b4851b1848917eb6` | `golden/synthetic.cash_payments_40a3.json` |
| `synthetic-engagement.toml` | 1,167 | `2dd1b7f2acd0736d1663d417a3dcdf7957a69e17f8aad45f5c4fa88c69a47ef8` | `synthetic-engagement.toml` |
| `manifest.json` | 9,706 | `7d3357e9b12e1afe2667a52eeea98b6471dd19c47787d8abc0730faaf23dfb3b` | `synthetic-read/manifest.json` |
| `company_object.xml` | 606 | `f1b6fe4e6b6cc406a4ae92ce0ef62a6c79a88a99ac83b1888989a98fbee967b4` | `synthetic-read/parts/company_object.xml` |
| `groups.xml` | 6,466 | `d314bbcea1fb8a70e5e3e1a25008da57031f872da06d971be6014af82f950395` | `synthetic-read/parts/groups.xml` |
| `high_water_after.xml` | 643 | `737c8f46527674e4075f2c1c6d0754e0c3edfd16e2a9096ee675af6bdfc3cecf` | `synthetic-read/parts/high_water_after.xml` |
| `high_water_before.xml` | 643 | `737c8f46527674e4075f2c1c6d0754e0c3edfd16e2a9096ee675af6bdfc3cecf` | `synthetic-read/parts/high_water_before.xml` |
| `ledgers.xml` | 11,306 | `b081ef22bceeb4e0f8f174a4994a0202b725bf59765689bd45b868b3ee0ddaf0` | `synthetic-read/parts/ledgers.xml` |
| `tb_fy.xml` | 8,752 | `6efdefbc8aeb7709bc9c314012ebbeb30732d9075f957f34bc5d40a94df6cbb6` | `synthetic-read/parts/tb_fy.xml` |
| `voucher_status_list.json` | 249 | `c7959e4e91a445f774ff2a5eaad4f8968f6fc82e21622d5168ececf9b0a1aa78` | `synthetic-read/parts/voucher_status_list.json` |
| `vouchers_h1.xml` | 42,016 | `1660ff7e5aff5c3c17b6cdfc70457733fbebc5117e10490949399da9d2565591` | `synthetic-read/parts/vouchers_h1.xml` |
| `vouchers_h2.xml.gz` | 913 | `d2561801c2c360a29c658db2dcce389b05014f05d4766c48592249a28be5f4fb` | `synthetic-read/parts/vouchers_h2.xml.gz` |
| `vouchertypes.xml` | 1,307 | `56a74424d30240ea24c2e0858e854ac3e199123d92f6fb8b8cfc640b068b42da` | `synthetic-read/parts/vouchertypes.xml` |

Sensitivity review: invented names, GUIDs and amounts only; no client, person, path or host
appears in any file.
