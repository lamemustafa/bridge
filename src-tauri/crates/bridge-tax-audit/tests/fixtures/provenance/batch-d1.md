# Batch D1 fixture provenance: `loans_interest`

Lane B, 2026-09-23. Every book here is invented: no fixture is a Tally read of any real assessee, and every
lender and ledger name is invented ("Invented Finance Ltd", "NBFC Loan", ...).

## What these fixtures establish, and what they do not

- `golden/synthetic.loans_interest.json` is the reference's dump on the synthetic read, whose config names
  one loan with no interest ledger: two Clause 31(a) cash receipts and the deductor-status finding of an
  individual with no previous-year turnover.
- `edge-books/loans_interest_core.json` (a firm) reaches every mode and Note 1 code (cash, bank, journal,
  other; A, B, I, J, K, and none for bank); s.194A over the threshold for an NBFC lender and not for a
  bank; the running balance, so that a receipt below the limit on its own is flagged once the balance
  reaches it; the s.269SS and s.269T flags; an insurer (reportable and flagged), a co-operative bank
  (reportable, never flagged) and two reporting-exempt lenders (a bank and a Government body); a clipped
  debit opening; a repayment reportable only through interest credited and not yet paid; a narration
  with both quote characters; a Contra; and an optional voucher.
- `edge-books/loans_interest_individual_over.json` and `..._individual_at.json`: an individual one paisa
  over and exactly at the previous-year turnover limit (a deductor; not a deductor).
- `edge-books/loans_interest_shared.json` and `..._shared_reversals.json` are one book: a declared-shared
  interest ledger with a paired loan's interest journal (left out), overdraft interest, a reversal-shaped
  Journal credit, a Receipt credit, a same-voucher credit, a Contra and an EMI split. The first carries no
  `net_reversals` key, so it runs through `run()` and `check_invariants()` and pins the rule in force
  (no credit reduces the figure); the second sets `net_reversals`, reaching the dormant
  reversal rule in `run` and in LOAN-3 alike (one credit of 300000 paise nets, matched to the earlier
  of two equal debits on the same day by entry order).
- `edge-books/loans_interest_invariants.json` makes the module invariants fire: LOAN-1 (a Contra into a
  loan; a loan with no Trial Balance row), LOAN-2 (an EMI split, an interest journal net of TDS, a GUID
  shared by two population vouchers, an unbalanced interest journal) and LOAN-3 (more than five interest
  debits outside any loan on a ledger not declared shared; a shared ledger whose TB debit does not tie).
- Not reached here: LOAN-2's "cites vouchers outside the books population", and the unresolvable-tag
  messages, which need a result the builder never produces. Two vouchers with one GUID on one loan in
  one direction repeat a figure id: the reference raises and the port refuses; a unit test in
  `src/loans_interest.rs` covers it, as no golden can.

## Real books (local only; nothing from them is in this repository)

`examples/local_parity` compared the port with the reference at `76310f60` on three real client reads,
each with that client's own reference-engine config: byte-identical dumps and 0 differences on the two
books with configured loans (210 figures and 42 findings, with 8 interest ledgers and one shared ledger
bound by identity; 49 figures and 6 findings), and 0 module-invariant violations on both. The third
has no configured loan: its 5 figures are identical, and the floor refuses it as vacuous.

The figure floor in `registry.rs` is structural: every book gives 5 figures and each configured loan
6 more, so a run with fewer than 11 compared no loan.

## Reference commit and invocations

Produced at the reference engine commit `76310f60a300d6172efdf11a9c7158cd51f38497` (loans_interest's
never-subtract rule of `02487f42` and bind_config's re-keyed interest ledger), from an archive of that
commit with no client data, under Python 3.13:

    uv run -q --python 3.13 --with openpyxl --with xlrd --with python-docx --with jsonschema \
        --with striprtf --with pdfplumber python parity/python_golden.py ENGINE \
        tests/fixtures/synthetic-engagement.toml tests/fixtures/golden/synthetic.loans_interest.json \
        --test loans_interest
    uv run -q --python 3.13 --with openpyxl --with xlrd --with python-docx --with jsonschema \
        --with striprtf --with pdfplumber python parity/edge_golden.py ENGINE \
        tests/fixtures/edge-books/loans_interest_NAME.json tests/fixtures/golden

Re-checked at reference commit `9d64c7436deedd8136b71d87d41dd36eb82733e1` (CA-facing wording in other
tests): `loans_interest.py` and every module it imports are unchanged between `76310f60` and `9d64c743`,
and regenerating all seven goldens there gives byte-identical files.

## Bytes

| File | Bytes | SHA-256 | Path |
| --- | ---: | --- | --- |
| `loans_interest_core.json` | 8,960 | `d94365352c3225d2ab9acc8213836237911923472d8dc22b07c17d2178ae420d` | `edge-books/loans_interest_core.json` |
| `loans_interest_individual_at.json` | 2,246 | `23f1d587147d0e8163b80cf91c96193c86f086f2b1b8ed7288d37910f9a83c0e` | `edge-books/loans_interest_individual_at.json` |
| `loans_interest_individual_over.json` | 2,250 | `2fb2d55b6a6c1522a1dc094ec37e1bc3d962bf0e84b427de0a3fa9efaec8d875` | `edge-books/loans_interest_individual_over.json` |
| `loans_interest_invariants.json` | 6,570 | `5ddaf6edd94c6fc0dfe087c7eba3f35fe54c7139e1dc5d2092837e1c7b346e87` | `edge-books/loans_interest_invariants.json` |
| `loans_interest_shared.json` | 3,883 | `f714219e6bf0aa29a66e109cc1be1523b6549e686c5bf4521a60b871b238b90b` | `edge-books/loans_interest_shared.json` |
| `loans_interest_shared_reversals.json` | 3,907 | `3e58ab5517e605380cbf4a2f502e700f62fd87ee9ecac4441149286790f88208` | `edge-books/loans_interest_shared_reversals.json` |
| `edge.loans_interest_core.loans_interest.json` | 59,132 | `67491ec6be7db312a1af52885daed4fab7be7008ad0f7ac46626593dfd243aea` | `golden/edge.loans_interest_core.loans_interest.json` |
| `edge.loans_interest_individual_at.loans_interest.json` | 8,931 | `51063638d289b69461391785eb43e7a6d89586f382fd08c3f436e6f9573b0743` | `golden/edge.loans_interest_individual_at.loans_interest.json` |
| `edge.loans_interest_individual_over.loans_interest.json` | 10,591 | `96e619adc40aa0caca6567bd8417e4cd557694bb946166ca2715ddfa7dc8f57d` | `golden/edge.loans_interest_individual_over.loans_interest.json` |
| `edge.loans_interest_invariants.loans_interest.json` | 25,096 | `8e00c88b76f811c59aca89352f9e83d8ff3321b0f85594c7bba788e6a515651a` | `golden/edge.loans_interest_invariants.loans_interest.json` |
| `edge.loans_interest_shared.loans_interest.json` | 11,547 | `3347c094619f1c355dfe6a9dcfa8e7b70425981ff60eb8f086f013ef945b8bca` | `golden/edge.loans_interest_shared.loans_interest.json` |
| `edge.loans_interest_shared_reversals.loans_interest.json` | 12,536 | `bfae3f31fced42cd3653d02cbebf160b85057c29d9c26101d19a1978ff142734` | `golden/edge.loans_interest_shared_reversals.loans_interest.json` |
| `synthetic.loans_interest.json` | 15,254 | `462ad5b40fa8742899489d8f70e7122cd040123277ccae0a085ed97d82016657` | `golden/synthetic.loans_interest.json` |
