# Bank-allocation STATUS field shape

Project-authored synthetic fixture. It carries reserved synthetic names and
fabricated identifiers; no customer export is included or copied.

## What established the shape

A supervised read-only observation on 2026-09-15 against a licensed TallyPrime
7.1 endpoint read a real trading book's vouchers with the shipped agent voucher
FETCH list (`ALLLEDGERENTRIES.*`). Across twelve monthly windows and thirty-one
single-day windows, `STATUS` occurred 971 times against 53 responses: one
protocol `ENVELOPE/HEADER/STATUS` per response, and 918 data fields at
`ENVELOPE/BODY/DATA/COLLECTION/VOUCHER/ALLLEDGERENTRIES.LIST/BANKALLOCATIONS.LIST/STATUS`.

`BANKALLOCATIONS.LIST` is returned for Payment, Receipt and Contra vouchers that
carry a bank allocation. In the same corpus `ERROR`, `LINEERROR` and `RESPONSE`
never occurred as data element names, and none of the protocol-shadowing names
carried XML attributes.

This fixture reproduces that nesting with synthetic values. It establishes the
element path only. It does not establish field semantics, completeness of the
bank-allocation shape, behaviour on another Tally release or edition, or any
compatibility-cell promotion. The observation was read-only; no write was sent.

The captured responses that establish the counts above contain customer data and
are retained privately by the maintainer. They are not part of this repository.
