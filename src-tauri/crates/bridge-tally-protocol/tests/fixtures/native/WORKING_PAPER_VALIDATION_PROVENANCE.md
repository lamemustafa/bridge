# Completed working-paper capture provenance — 2026-08-26

This fixture set is one byte-exact, completed native outstandings read from the
purpose-built synthetic `Bridge Validation Lab` on the loopback TallyPrime
endpoint at port 9001. The company was pinned by GUID
`c6afd306-00e1-4f51-802a-babe44daddd3`; its target extent was unchanged before
and after the data sequence:

- `BOOKSFROM=20250401`
- `LASTVOUCHERDATE=20260801`
- `ALTVCHID=7`
- `ALTMSTID=219`

The data capture ran from 01:26:32 through 01:26:34 IST on 2026-08-26. A
paired closing extent completed at 01:27:33. Tally returns all loaded companies
for this collection despite `SVCURRENTCOMPANY`, so neither full extent response
is committed. The target row was selected by its exact GUID. The paired closing
extent responses were byte-identical at 7,063 bytes with SHA-256
`99ac3602a7932ed34c5237aa19633ff00282e5a4c49420d781ebd2c28f8689f3`.

Every data request used the production native request shape, was dispatched
alone, and was sent twice. Successful `/status` reads bracketed the first and
second responses. All fifteen status bodies were byte-identical, contained
`TallyPrime Server is Running`, and had SHA-256
`655415972de8e54d65743a548e00c2218810683fc0c4cab76cf6ea97ff1d3800`.
Both response copies for every request were byte-identical.

| Fixture | Production read | Request SHA-256 | Bytes | Response SHA-256 |
| --- | --- | --- | ---: | --- |
| `bills_receivable_validation_lab.xml` | Bills Receivable, `SVTODATE=20260801` | `f459e775977b0a5f76d611373f7dc5bb658e0a9d5cdf30c75e199ff67ec5bea8` | 1,170 | `a7f4ff5209c98b145970112a3ba1be9e6d303008b270786e7bfb286c3a99697b` |
| `bills_payable_validation_lab.xml` | Bills Payable, `SVTODATE=20260801` | `e0e93b2e14399ec53a7a8051c43ba41c05f1dd595370a1b6a36a58d15d023157` | 257 | `62063a77ebaccdaebdae42a431bc8859388f415035812e82e212808c64ee83fd` |
| `group_snapshot_validation_lab.xml` | Native Group snapshot | `eb5c9fd7144b4de247de976c65c28e6def9ba6fbe06d2b2f1f759b6108eed251` | 24,452 | `de722a9e6f0800279cc5fc9bf7d1f81a01d052de0f7f09433b55a2dfdd515840` |
| `ledger_snapshot_validation_lab.xml` | Native Ledger snapshot, `20250401–20260801` | `ef6bf48da3a4ee17e4272bbb4d4347f1ca7cb62bc175ed064da121ad29f95adf` | 7,696 | `64cc585f6bfa2bdc076c2fc28e8732c26e931819dac8f53086e932daeb053a3a` |

The retained fixture set totals 33,575 source bytes and contains five
receivable bill rows, one payable bill row, 28 Group rows, and 13 Ledger rows.
The existing bill and ledger fixture files compared byte-for-byte with this
new capture; only the missing Group response had to be added. With
`as_of=20260801`, the positive overdue counter independently identifies the
requested effective date, the native computation reaches `Complete`, its six
non-zero bill rows and unallocated controls reconcile, and the working-paper
renderer produces an XLSX archive. That complete end-to-end claim is pinned by
the runtime regression test; the older 2026-08-17 capture note records its
request date but does not by itself establish a completed 2026-08-17 read.

A bounded privacy audit found no email, address, phone, pincode, GSTIN, or PAN
fields and no GSTIN- or PAN-shaped values. Party, ledger, bill, group, and
company labels belong to the repository's synthetic validation corpus. The
source and fixture copies compared byte-for-byte; `.gitattributes` disables
text normalization and the fixture-byte-integrity gate protects committed
objects.
