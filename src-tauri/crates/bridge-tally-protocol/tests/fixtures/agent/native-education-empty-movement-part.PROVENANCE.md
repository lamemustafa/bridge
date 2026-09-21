# Education-mode empty movement part

Covers `native-education-empty-movement-part.utf16le.xml`.

| File | Bytes | SHA-256 |
| --- | ---: | --- |
| `native-education-empty-movement-part.utf16le.xml` | 3022 | `947fbf369d0fcf74e53bff1a3802a927c72737735dbb3064b3cfe34dd70fa974` |

Captured byte for byte, UTF-16LE with no byte-order mark, from the maintainer's lab
gateway (TallyPrime 7.1) on 2026-09-22 while the instance was in Education mode. It
answers Bridge's `Bridge Agent Vouchers` movement request for a single day whose
`SVFROMDATE` and `SVTODATE` were both the third of the month, against a synthetic
laboratory company. The response is `STATUS 1`, a `CMPINFO` block of zero counts and
an empty `COLLECTION`: it carries no company name, identifier or voucher data.

## What it establishes

Nine other single-day requests whose date was not the first, second or thirty-first
of the month returned this same body, byte for byte, although an independent census
of the same book counted vouchers on every one of those days; the first and second of
the month returned their counted vouchers. In Education mode, therefore, a movement
read starting on such a day is served empty rather than refused (bridge#581).

It does not establish the behaviour of an end date on its own: every empty request
had equal start and end dates. It establishes nothing about licensed mode, another
release or another request shape.
