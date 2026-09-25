# Company-split capture — 2026-08-26

Company-collection responses from licensed TallyPrime Silver (`EDUMODE No`,
`SILVER Yes` in the rows), taken before and after a year-end split of the
synthetic `BRIDGE PROBE B SANDBOX` book. Added in `48bce0a6` (#188).
`docs/tally/TALLY_PROTOCOL_REFERENCE.md` §9.11b and
`docs/adr/0006-tally-incremental-authority.md` record what they establish: the
pre-split capture has 14 companies, the post-split capture 15, and the added
child kept its parent's `GUID`. Port and request are not recorded, and no
transformation is recorded; the `.utf8` suffix names the stored encoding.

| Fixture | Bytes | SHA-256 |
| --- | ---: | --- |
| `company_list_before_split.utf8.xml` | 6775 | `ac46c6e5fefcf700577a4eb39ccb827d3fe11b12d1f166ba52f1281c6db8d36c` |
| `company_list_after_split.utf8.xml` | 7204 | `9faa4304130904e37dd49faf34c4ca2637f0ea79e8771c9ce98664e41ddd10ef` |
| `company_identity_fields_after_split.utf8.xml` | 7470 | `d247dd885f4109087856960f3cba9c304b75fa29f195064f862587aa7146733d` |

`company_identity_fields_after_split.utf8.xml` is the post-split read carrying
the identity fields the composite key uses. §9.11b's `ALTVCHID` collision
claim rests on a response that is **not** committed; none of these three files
contains `ALTVCHID`. Used by the split tests in `../simulator_corpus.rs`.
