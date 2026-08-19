# Tally XML encoding captures

These response bodies were captured byte-for-byte from a real TallyPrime EDU
instance on 2026-08-19 against a synthetic Bridge Validation Lab book. Each
request was bracketed by healthy status checks. No customer data, GSTIN, or
real company identity is present.

The paired requests differed only in their XML wire encoding and
`Content-Type` charset. UTF-16 requests were `FF FE` followed by UTF-16LE.
Tally's UTF-16 responses declared `text/xml; charset=utf-16` and carried no
BOM.

| File | Bytes | SHA-256 |
| --- | ---: | --- |
| `led-ascii.bin` | 7,114 | `71ff5511eb8ebd84bf50fbff2c074e7372ec652cf26d2de61c47feca5b1acc7a` |
| `led-utf16.bin` | 14,228 | `786d6c3335dffd93b5e3a65b45d7b36e40a261ba8040318da17d1dc7ca7592c8` |
| `bills-ascii.bin` | 1,170 | `a7f4ff5209c98b145970112a3ba1be9e6d303008b270786e7bfb286c3a99697b` |
| `bills-utf16.bin` | 2,340 | `e8c2cb5aa44a62c1166efe2bf2bcedf69e09f1d9e192215ec8a49e75e62c1df1` |

Do not edit, re-encode, or normalize the `.bin` files. They are covered by
the repository's `fixtures/** -text` policy and byte-integrity gate.
