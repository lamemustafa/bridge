# `company_currencyname_forex_edited`: provenance (EDITED capture)

The `Company` collection that names each loaded company's `CURRENCYNAME` (bridge#551;
TALLY_PROTOCOL_REFERENCE §9.10a.2). **This fixture is an edited capture, not a byte-exact one:**
rows were removed from the captured response, as recorded below.

## Why it is edited

The collection lists **every loaded company**, whatever `SVCURRENTCOMPANY` names. The lab had other
companies loaded when it was captured, including client-derived copies, so the response as captured
cannot be published. The edit keeps the fewest rows that test what the parser needs: the company
the read is for, and one other synthetic company, so that choosing the row by GUID is tested against
a real second row.

## Source capture

- **Host / gateway:** TallyPrime **Silver (licensed)** 7.1, `education_mode` false; `/status`
  byte-identical before and after.
- **Date:** 2026-09-23, 10:57–10:59 +0530, read-only.
- **Request:** `render_company_base_currency_request("BRIDGE CORPUS FOREX")`: byte-identical to the
  request sent.
- **Source response:** 14,464 bytes, sha256
  `2f69596ea921c254b0f327e397d60546271db1a650bea1a00143fe36569c20dd`, BOM-less UTF-16LE, `STATUS 1`,
  23 `COMPANY` rows. Not committed.

## The edit

- **Removed:** 21 of the 23 `COMPANY` rows, each from its leading indentation through the CRLF after
  its `</COMPANY>`. Nothing else was changed.
- **Kept, byte for byte and in captured order:** the rows of `BRIDGE CORPUS FOREX` (GUID
  `b14e9b2d-8a63-4779-804d-25d59eb787eb`) and `BRIDGE SHAPE LAB` (GUID
  `3a6bd6e1-b835-4bff-89dd-8a6af138c346`), both synthetic, with `CURRENCYNAME` `₹`, and all bytes
  before the first row and after the last.
- **`CMPINFO` counters are as captured.** They describe the current company, not the collection:
  `<COMPANY>0</COMPANY>` was 0 before the edit too, and `<CURRENCY>3</CURRENCY>` and
  `<LEDGER>11</LEDGER>` are FOREX's own. The parser reads rows only from `<DATA>` and ignores
  `CMPINFO`, which a test checks.
- **Scanned after the edit** for every company name and GUID in the source response: only the two
  kept companies remain.

| file | bytes | sha256 |
|---|---|---|
| `company_currencyname_forex_edited.utf16le.xml` | 3,912 | `f44ff5795891ec4ba180da8d65a16e574385e6cc69ddd75c51b170144f53ec05` |

## What it establishes, and what it does not

- The row shape: a `COMPANY` element with `NAME` and `RESERVEDNAME` attributes, and `NAME`, `GUID` and
  `CURRENCYNAME` children, `CURRENCYNAME` carrying the base master's `ORIGINALNAME` (`₹`, where the
  master's `NAME` is `I₹`).
- It does **not** show the collection's full membership, row order among other companies, or how a
  company whose base is not INR reports (a USD-based control book reported `$`; that row was
  removed).
- Missing, duplicated, empty and ambiguous rows are tested as labelled edits of this fixture, not
  captured.
