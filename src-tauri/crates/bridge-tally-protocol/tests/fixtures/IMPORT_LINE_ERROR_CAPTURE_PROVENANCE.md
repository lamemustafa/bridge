# `import_line_error_partial_commit_live` — provenance

One captured import response in which Tally committed 49 of 50 vouchers and reported the fiftieth
only as `EXCEPTIONS 1` with one `LINEERROR`. It is the fixture for keeping `LINEERROR` text for a
person to read, and for a multi-voucher response that is not clean.

## Provenance

- **Host / gateway:** TallyPrime **Silver (licensed)**, `http://localhost:9001`, TallyPrime 7.1.
- **Date:** 2026-09-24, one request, `/status` healthy before and after.
- **Company:** a synthetic company created for amendment tests; no client or client-derived book
  was written.
- **Request:** one import of 50 Journals in Bridge's voucher shape, sent by a lab script straight to
  the gateway, **not** through Bridge's post path. Voucher 25 of 50 named a ledger that does not
  exist, `Lane A No Such Ledger` (synthetic). The request is not committed.
- **Encoding:** **BOM-less UTF-16LE**, exactly as received. These are undecoded wire bytes, not
  decoded text. `.gitattributes` does not normalise this tree.

| file | bytes | sha256 |
|---|---|---|
| `import_line_error_partial_commit_live.utf16le.xml` | 708 | `0cd8feac1ef3cd029ec024ff63b75996e0034679a99692b4aa95eeae6815831d` |

## What it establishes

- `CREATED 49`, `ERRORS 0`, `EXCEPTIONS 1` and one `LINEERROR` reading
  `Ledger 'Lane A No Such Ledger' does not exist!` (escaped as `&apos;` on the wire). A read-back
  found 49 of the 50 vouchers, with exactly voucher 25 missing.
- The response names **no voucher**: it has no index, no `REMOTEID` and no voucher number. Only a
  read-back can say which voucher failed. The text is Tally's, for a person to read; Bridge never
  decides on it.
- `ERRORS 0` on a partial commit, again: success needs every counter condition, not `ERRORS` alone.
