# Batch E1b fixture provenance: `book_keeping_quality`

Lane E, 2026-09-22. Every book here is invented; none is a Tally read and none holds client data.

## What these fixtures establish, and what they do not

- `golden/synthetic.book_keeping_quality.json` is the reference implementation's canonical dump of
  `book_keeping_quality` on the shared synthetic read, with the five `[roles]` keys this batch adds
  to `synthetic-engagement.toml`, all empty (the reference requires every one). That read carries
  no MASTERID and no inventory, so most figures there are zero; it establishes that the port and the
  reference agree on it, and the edge books carry the branches. Regenerating every other synthetic
  golden with the new engagement file gives the same bytes as with the old one.
- Each `edge-books/bkq_*.json` names, in its own `comment`, the boundaries and branches it
  reaches, and each `golden/edge.bkq_*.book_keeping_quality.json` is the reference's dump of it.
  Every claim in a comment is one some figure, finding or module check exposes: the batch's
  mutations (`parity/mutations.json`, E1B-*) each change one of those behaviours and each is
  killed by these books, the module's unit tests or the binding tests.
  - `bkq_order`: the MASTERID clock (lag 30 not over, 31 over; -31 not over; a sale dated before
    the clock leaves it), a sale with a negative MASTERID starting the clock before a purchase
    numbered `-2`, equal MASTERIDs kept in population order, a GUID seen twice keeping its last
    creation-order lag, MASTERID text as `int()` reads it, and base-type slugs from a non-ASCII and
    a symbols-only name;
  - `bkq_channel`: the greedy receipt match (date before GUID for both invoices and receipts, a
    receipt used once, the 0- and 3-day edges), a Sales voucher crediting the channel ledger taken as
    neither invoice nor receipt, an invoice's channel credit line not reducing its amount, a channel
    sale with a cash line priced in the channel bucket, purchase rates as a year average, a sale line
    a float epsilon under cost (0.3 units at 1234 paise cost 1234.0000000000002) counted below cost
    and one exactly at cost not, and a half-paisa cost rounded half to even (769 bp where half-up
    would give 0);
  - `bkq_channel_alone`: channel lines with no named-customer line (the no-comparison limit), a GST
    head with no payment ledger (no GST finding), and purchase quantities whose float sum depends on
    order (1, 1, 1e16 in population order), deciding one sale line's below-cost result;
    and a named-customer line priced but worth zero, so no comparison margin is claimed;
  - `bkq_channel_zero`: channel lines worth zero, so the margin section reports nothing;
  - `bkq_misc`: untouched Stock-in-Hand ledgers, one with a TB movement of 201 paise so that BKQ-1
    fires and its message is compared, one moved exactly 100 paise (within tolerance), one under a
    Stock-in-Hand subgroup; GST heads and payment ledgers with and without TB rows; re-issue terms
    through Python's `upper()` (a sharp s matching its SS spelling); write-offs (one row per
    voucher and debtor with that debtor's credit lines summed, two journals sharing a GUID kept as
    two rows but cited once, a zero line skipped, a journal whose only debtor line is zero giving
    no row, a zero write-off-ledger line not qualifying); every
    Contra direction case (a narration naming both keywords is a mismatch when either disagrees),
    including a zero net cash line skipped;
  - `bkq_quiet`: nothing to report, every input empty.
- At the pinned commit the reference cites each write-off by its voucher's GUID, the debtor named
  in the label, so EVID-1 resolves every citation. (An earlier reference cited `"<guid>:<ledger>"`,
  which its own EVID-1 reported as unresolved; that is fixed at the pin, and no golden here carries
  it.) The same commit's CA-facing wording is ported as written: no definition names a file, a path
  or a function, or refers to a row above it.
- They do not establish anything about reading Tally, or about a config the port refuses where the
  reference reads it: a well-formed MASTERID holding a non-ASCII digit (which `int()` reads) or
  beyond i64, and a ledger listed under two GST heads (see `src/book_keeping_quality.rs` and
  `BookKeepingQualityConfig`). A MASTERID `int()` rejects is unparseable on both sides, non-ASCII
  digits or not; `int()`'s whitespace is Rust's `char::is_whitespace` (measured over every code
  point), not `str.strip()`'s; more than 4,300 digits is unparseable, as `int()` refuses it.

## Real books (local only; nothing from them is in this repository)

`examples/local_parity` compared the port with the reference on three real client reads, each with
that client's own reference-engine config: identical JSON, 0 differences on all three. Real
(non-zero) work, per section: entry order and GST set-off on all three; the payment-channel section
and stock-ledger touch counts on two; re-issue narrations, debtor write-offs and Contra direction on
one only. Between the previous reference commit and the pinned one, no figure value changes on any
of the three reads; on the one with write-offs, the reference's EVID-1 violations go from 76 to 0.

## Reference commit and invocations

The goldens were produced at the reference engine commit `9d64c7436deedd8136b71d87d41dd36eb82733e1`,
from an archive of that commit with no client data, under Python 3.13:

    uv run -q --python 3.13 --with openpyxl --with xlrd --with python-docx --with jsonschema \
        --with striprtf --with pdfplumber python parity/python_golden.py ENGINE \
        tests/fixtures/synthetic-engagement.toml tests/fixtures/golden/synthetic.book_keeping_quality.json \
        --test book_keeping_quality
    uv run -q --python 3.13 --with openpyxl --with xlrd --with python-docx --with jsonschema \
        --with striprtf --with pdfplumber python parity/edge_golden.py ENGINE \
        tests/fixtures/edge-books/bkq_NAME.json tests/fixtures/golden

## Bytes

| File | Bytes | SHA-256 | Path |
| --- | ---: | --- | --- |
| `synthetic.book_keeping_quality.json` | 35,978 | `e44ece2c3af4ccee7d3e6bb205b8966a51045b37ccd888719bac831e66aaf358` | `golden/synthetic.book_keeping_quality.json` |
| `bkq_channel.json` | 9,054 | `db4c43d3f64d2f6515676769b4ed6a170207bf52816dcf8beed786f2a11c5f9c` | `edge-books/bkq_channel.json` |
| `bkq_channel_alone.json` | 3,845 | `7f705760290ce9ef8e3ff3d55d85b929d43855a02e49b311ee4be48e83286164` | `edge-books/bkq_channel_alone.json` |
| `bkq_channel_zero.json` | 2,489 | `118e1112f554d9bc3bb3964288a7d6a4b1edb0453d234fa09e6a2c37a0510c25` | `edge-books/bkq_channel_zero.json` |
| `bkq_misc.json` | 10,356 | `e7e23ce5751b1479c0c40f5f8b557aac39e1dcac54e9bd678238d53a749348a2` | `edge-books/bkq_misc.json` |
| `bkq_order.json` | 6,758 | `06c6500c322d5a8b7b49cf4b8cd947d9677bc917d22ed9e72b9128fa8d6a0e0b` | `edge-books/bkq_order.json` |
| `bkq_quiet.json` | 2,141 | `502057ad49326bd5d72350dcafd2ae5c209879a4f9076786031637afc608765f` | `edge-books/bkq_quiet.json` |
| `edge.bkq_channel.book_keeping_quality.json` | 16,610 | `0fa8e387bf850d4b108b00433b436be01a66de2b972c22d473b307ae6d954bce` | `golden/edge.bkq_channel.book_keeping_quality.json` |
| `edge.bkq_channel_alone.book_keeping_quality.json` | 12,715 | `af10348dc851a5e419d730402af8ddd3d819bc43c9b7adad175d35ca30c372ad` | `golden/edge.bkq_channel_alone.book_keeping_quality.json` |
| `edge.bkq_channel_zero.book_keeping_quality.json` | 9,207 | `60ed70f9f6580c5169d18ae3d43fb6ff254702f37164e850771a7b2e0453a5ff` | `golden/edge.bkq_channel_zero.book_keeping_quality.json` |
| `edge.bkq_misc.book_keeping_quality.json` | 25,864 | `b926a90e9a1384ee04e3ea1d2912d1a050470f1a39acc03c9020ab4d7050f5ea` | `golden/edge.bkq_misc.book_keeping_quality.json` |
| `edge.bkq_order.book_keeping_quality.json` | 18,878 | `d0a731eb1b5bd7df16f6d0ad15657a51f886b851419b3c7ac9093a85d8a05ea3` | `golden/edge.bkq_order.book_keeping_quality.json` |
| `edge.bkq_quiet.book_keeping_quality.json` | 4,843 | `bcb7170ec8d81338e6e5f2656ec910c4259bb83f00d1073a6efa411af791c6df` | `golden/edge.bkq_quiet.book_keeping_quality.json` |
