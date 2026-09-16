# Bank-statement fixture provenance

No client data. Every fixture here is synthetic.

## Generated PDFs

Written by `scripts/generate-bank-statement-fixtures.py`, deterministically;
CI runs it with `--check`, so these bytes are that script's output and nothing
else. Every name, account digit, reference and amount is invented. What is
carried over from real statements is only the template geometry the SBI and
HDFC profiles were calibrated on, which is already public in
`scripts/bank_statement_import.py`.

| Fixture | Bytes | SHA-256 |
| --- | ---: | --- |
| `hdfc-synthetic.pdf` | 4,810 | `3eb43c5376ca6588e492747ab64d1494c7f59a380f9ac3ad0d41a073adacfd0f` |
| `sbi-owner-password-only.pdf` | 3,545 | `64bb53bf392ec9cc52aede9085390b29b7cac0ba7908d678ba933551414d14c0` |
| `hdfc-rotated.pdf` | 2,921 | `655d670aa5513a6386d723611c61db19555196ea034e85e611846a8f6f904d21` |

- `hdfc-synthetic.pdf` — three pages, user password `synthetic-user-4321`. A
  12-digit UPI reference and an ACH reference broken mid-token at the 240pt
  narration edge, a wrap that ends at a real space, a transaction naming the
  bank that must not be read as the footer, a row printed below the footer,
  `STATEMENT SUMMARY`, and a third page that must not be read. The phone line
  also ends in the account digits; only the account-number line binds.
- `sbi-owner-password-only.pdf` — two pages. Its user password is one the
  operator never has; only the owner password `synthetic-owner-7788` is
  supplied. `pdftotext -upw synthetic-owner-7788` rejects it ("Incorrect
  password") and `-opw` opens it, measured with poppler 26.04.0 on 2026-09-16:
  the case the reference tried both flags for. The date is stacked over the
  year, the column header repeats on page 2 while page 1's last row is still
  open, and a 12-digit reference wraps mid-token.
- `hdfc-rotated.pdf` — one page with `/Rotate 90`, refused before parsing.

**Limits.** Encryption is RC4 128-bit (standard security handler revision 3);
real statements commonly use AES, which is not exercised here. The text is the
non-embedded base-14 Courier font; embedded and proportional fonts are not.

## Poppler captures of the generated PDFs

`pdftotext -bbox-layout` output of the two PDFs above, byte-exact, from poppler
26.04.0 (Homebrew) on macOS arm64, 2026-09-16:

```
pdftotext -bbox-layout -upw synthetic-user-4321 hdfc-synthetic.pdf hdfc-synthetic.pdftotext.xml
pdftotext -bbox-layout -opw synthetic-owner-7788 sbi-owner-password-only.pdf sbi-owner-password-only.pdftotext.xml
```

| Fixture | Bytes | SHA-256 |
| --- | ---: | --- |
| `hdfc-synthetic.pdftotext.xml` | 31,469 | `dc31411303e974361de3895258a7f1e80f26914331906d49b6c87aa6ddc2c448` |
| `sbi-owner-password-only.pdftotext.xml` | 23,420 | `cfe5a7144facde8d3bd5e4ff82b9f2ca634bbc1815543ee76b84c20b13aa2965` |

They are the reference the PDFium path is compared against: `pdf_tests.rs`
requires PDFium's words to produce exactly the rows these produce, and pins the
digests the Python reference (`scripts/bank_statement_import.py`) computes over
them. They were captured from the first generation of the PDFs; adding
`/Rotate 0` to the page dictionaries later changed the PDF bytes but not the
text, and re-running the commands above reproduces these files byte for byte.
