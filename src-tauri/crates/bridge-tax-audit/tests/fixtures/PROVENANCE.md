# bridge-tax-audit fixture provenance

No client data. Every fixture here is synthetic: the company ("Lakeview Hardware Demo"), its
GUID, groups below the reserved primaries, ledgers, voucher types, vouchers and amounts are
invented.

## What these fixtures establish, and what they do not

They establish **parity**: that this crate and the Python reference engine it ports compute
the same canonical `cash_44ab` result (PARITY-SPEC-v1) from the same bytes. They do **not**
establish anything about Tally. No byte here was served by Tally. The XML follows the element
layout Tally uses for collection exports (envelope, `CMPINFO` counts, UTF-16LE without a BOM
for the group and ledger masters, `&#4;` before `Primary`, flags and amounts in Tally's sign),
so the parser is exercised on that shape, but the shape is an author's reproduction. AGENTS.md
P1's rule that fixtures are captured from a real instance is met for this crate by the local
parity run over real reads (see the crate docs), which is never committed.

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

`synthetic-engagement.toml` is the engagement for it: the reference-engine client-config keys
`cash_44ab` reads, and nothing else.

## The golden

`golden/synthetic.cash_44ab.json` is the reference engine's own canonical dump for that
engagement and read: its `read_format` adapter built the book, its `cash_44ab` ran with its
AY 2026-27 rules, and its `tae.parity.canonical` serialised the result. Produced at reference
engine commit `c2f206beb870a8fdb0775f2d1c71aa1ccfccca64` by

```
uv run -q --with openpyxl --with xlrd --with python-docx --with jsonschema --with striprtf \
    --with pdfplumber python parity/python_golden.py ENGINE \
    tests/fixtures/synthetic-engagement.toml tests/fixtures/golden/synthetic.cash_44ab.json
```

## Bytes

| Fixture | Bytes | SHA-256 | Path |
| --- | ---: | --- | --- |
| `synthetic.cash_44ab.json` | 5,824 | `5c40880fcd737139b17882a70337cbb740951c6a6242eefb2f6963a48b3f7eb1` | `golden/synthetic.cash_44ab.json` |
| `synthetic-engagement.toml` | 503 | `e542e7dbe0bfd7ddf73902e476a5647993158cffbec01211785af476c6c32a28` | `synthetic-engagement.toml` |
| `manifest.json` | 9,704 | `b694fc67dc55d3cfc78b5a565e5bb18f78f40869dfa7115f2e0c13878ead9b89` | `synthetic-read/manifest.json` |
| `company_object.xml` | 606 | `f1b6fe4e6b6cc406a4ae92ce0ef62a6c79a88a99ac83b1888989a98fbee967b4` | `synthetic-read/parts/company_object.xml` |
| `groups.xml` | 5,428 | `7f87422759a3e70fe62c55fc91712ef19fbf7ffdc0a03ec8bb9afd80217b9a0e` | `synthetic-read/parts/groups.xml` |
| `high_water_after.xml` | 643 | `5207edc8a183d7ff35cd454e165d5384d064bcd157f4182bc92599a8ef17a584` | `synthetic-read/parts/high_water_after.xml` |
| `high_water_before.xml` | 643 | `5207edc8a183d7ff35cd454e165d5384d064bcd157f4182bc92599a8ef17a584` | `synthetic-read/parts/high_water_before.xml` |
| `ledgers.xml` | 5,362 | `733c887fb4c7134760e62c2a5ec1b5181822b73f8cf218c9535ef91e7f4e6b23` | `synthetic-read/parts/ledgers.xml` |
| `tb_fy.xml` | 4,030 | `ba8b65d832c76710ffb70519a078f07e7587a0e79ba408ba860535ed24403c66` | `synthetic-read/parts/tb_fy.xml` |
| `voucher_status_list.json` | 249 | `c7959e4e91a445f774ff2a5eaad4f8968f6fc82e21622d5168ececf9b0a1aa78` | `synthetic-read/parts/voucher_status_list.json` |
| `vouchers_h1.xml` | 13,728 | `535395a112dc59e1f14b76da10bc2ee48d0230c8277369685e38d2623bb86f5b` | `synthetic-read/parts/vouchers_h1.xml` |
| `vouchers_h2.xml.gz` | 913 | `d2561801c2c360a29c658db2dcce389b05014f05d4766c48592249a28be5f4fb` | `synthetic-read/parts/vouchers_h2.xml.gz` |
| `vouchertypes.xml` | 1,307 | `56a74424d30240ea24c2e0858e854ac3e199123d92f6fb8b8cfc640b068b42da` | `synthetic-read/parts/vouchertypes.xml` |

Sensitivity review: invented names, GUIDs and amounts only; no client, person, path or host
appears in any file.
