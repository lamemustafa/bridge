# `company_currencyname_live`: provenance

The `Company` collection that names each loaded company's `CURRENCYNAME` (bridge#551;
TALLY_PROTOCOL_REFERENCE §9.10a.2), captured as received.

- **Host / gateway:** TallyPrime **Silver (licensed)** 7.1, `education_mode` false.
- **Date:** 2026-09-24, 11:01–11:05 +0530, read-only, one request at a time.
- **Loaded companies:** only the four synthetic books below. A client-derived demo copy listed by the
  opening `/status` was closed before this request; the closing `/status` lists only these four.
- **Request:** `render_company_base_currency_request("BRIDGE CORPUS FOREX")`, as the production
  renderer produces it. The collection lists every loaded company whatever `SVCURRENTCOMPANY` names.
  The same request for `BRIDGE SHAPE LAB`, sent next, returned the same bytes.
- **Encoding:** BOM-less UTF-16LE, the undecoded wire bytes. `STATUS 1`, 4 `COMPANY` rows.

| file | bytes | sha256 |
|---|---|---|
| `company_currencyname_live.utf16le.xml` | 4,914 | `83ad785d1d7d6f0e42c930ea8585461aeb217f6269987dd700d87c3fe5805def` |

| company | GUID | `CURRENCYNAME` |
|---|---|---|
| `Bridge Billwise Lab` | `75f7566d-7a4f-431a-9642-e93a9d06d57d` | `Rs.` |
| `BRIDGE CORPUS FOREX` | `b14e9b2d-8a63-4779-804d-25d59eb787eb` | `₹` |
| `BRIDGE SHAPE LAB` | `3a6bd6e1-b835-4bff-89dd-8a6af138c346` | `₹` |
| `Bridge Validation Lab` | `c6afd306-00e1-4f51-802a-babe44daddd3` | `₹` |

- The `CMPINFO` counters describe the current company, not the collection (`<COMPANY>0</COMPANY>`);
  the parser reads rows only from `<DATA>`, which a test checks.
- Missing, duplicated, blank and ambiguous rows are tested as labelled edits of this capture, and the
  runtime's refusal cases edit FOREX's row (`$`, or a value naming no master).
- It does **not** show a company whose base is not INR: no such book was loaded.

## `company_list_synthetic_live`, `company_extents_synthetic_live`: identity for SHAPE LAB

Captured so that SHAPE LAB can be read end to end in tests (the older committed company fixtures do
not list it).

- **Host / date:** TallyPrime 7.1 Silver, licensed, `education_mode` false; 2026-09-24, 11:35
  +0530, read-only, one request at a time.
- **Loaded companies:** only synthetic books. The opening `/status` listed the four above; the owner
  created a fifth synthetic book, `BRIDGE READS LAB`, during the block, and both responses list it.
- **Requests:** the production company-list request and the production company book-extent
  request for `BRIDGE SHAPE LAB`, as master `2db6a9c5`'s renderers produce them. Both list every
  loaded company.
- **Encoding:** BOM-less UTF-16LE, the undecoded wire bytes.

| file | bytes | sha256 |
|---|---|---|
| `company_list_synthetic_live.utf16le.xml` | 8,324 | `326801874dec020c5ab6c3677e5a7dcc984ff39877a1b0576ab50b1fa2d3292b` |
| `company_extents_synthetic_live.utf16le.xml` | 7,416 | `daa78388f1f9e7e870e7c56c03e5bbfd03feec72f74659cf288b9d90e74205aa` |

Companies in both: `Bridge Billwise Lab`, `BRIDGE CORPUS FOREX`, `BRIDGE READS LAB`, `BRIDGE SHAPE
LAB`, `Bridge Validation Lab`.
