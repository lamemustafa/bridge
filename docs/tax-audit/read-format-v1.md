# Tally read format, version 1 (`tally-read`, major 1)

This is the public specification of `tally-read-v1`: the directory format a books-based
tax-audit engine reads instead of talking to Tally directly. It is the contract between three
parties: a **producer** that captures a company's Tally responses to disk, the **reference
implementation** (a Python engine) that has consumed it in production, and this repository's
`bridge-tax-audit` crate, which is the first Rust consumer. `bridge-tax-audit`'s `read` module
implements the consumer rules in section 7 below; see its doc comments for the exact mapping.

A JSON Schema (2020-12) formalises the manifest shape this document describes; the reference
implementation and `bridge-tax-audit` each validate independently against the rules below rather
than sharing one schema validator, so a drift between the two is a parity failure, not a silent
pass.

## 1. What a read is

A **read** holds one Tally company's raw responses to a named set of requests, all taken for one
audit period, plus a manifest that says what each byte is.

- The manifest carries the facts every consumer needs before trusting a byte: identity, licence,
  windows, hashes and a consistency bracket. A consumer never has to re-derive them from the XML.
- A producer writes reads to its own private storage, never to a location the caller chooses, and
  a read reaches a shared folder only through an explicit, user-confirmed export step.
- Client reads — real Tally data — never enter a product repository or CI. CI uses synthetic
  reads only (see this crate's `tests/fixtures/PROVENANCE.md`).

A read is a directory:

```
<read>/manifest.json          the manifest (this spec); UTF-8 JSON
<read>/parts/<file>           one file per stored request or response, e.g. vouchers_202504.xml.gz
```

**Handle.** A producer returns a `read_id` plus the sha256 of `manifest.json`. A later call that
names the read passes both, and the producer refuses the call if the manifest has changed since.

## 2. Hashing and storage (definitions every producer and consumer share)

- **Content** means the bytes exactly as Tally returned them. For a request, it means the bytes
  exactly as sent, including the UTF-16LE BOM.
- **`sha256` / `bytes`** are the sha256 and length of the content.
- **`storage`** is how the content sits on disk:
  - `gzip` means the file is the gzip of the content. Its name must end in `.gz`.
  - `identity` means the file is the content itself. Its name must not end in `.gz`.
- **`stored_sha256` / `stored_bytes`** are the sha256 and length of the file on disk. Recording
  both hashes lets a consumer detect a corrupted file without decompressing it, and lets a
  format-converting tool prove it copied the source file unchanged.
- **`wire_exact`** is true only when a record made at read time shows that the content is the wire
  bytes. When it is false, `transformation` says what is known:
  - `reencoded_utf8`: the producer decoded Tally's UTF-16LE response and stored it as UTF-8.
  - `derived`: the part is not a Tally response at all.
  - `unknown`: no record establishes either way.
- Producers store bytes as received. They do not strip, normalise or re-encode anything. A parser
  must tolerate what Tally actually sends on the wire, including control characters a naive XML
  parser rejects.

## 3. Manifest fields

| Field | Meaning |
|---|---|
| `format` | always `"tally-read"` |
| `format_version` | `"MAJOR.MINOR"`, here `"1.0"` (section 8) |
| `read_id` | producer-assigned, unique |
| `created_at` | when the manifest was written (ISO 8601 with offset) |
| `read_at` | when the read represents the books. Book.read_at comes from here, and post-dated status is relative to it |
| `producer` | `name`, `version`, `kind` ∈ {`bridge`, `legacy-wrap`, `capture-wrap`, `synthetic`}, `source` |
| `company` | `guid` (identity), `name` (display only), `books_from` |
| `tally` | `basis` ∈ {`observed`, `not_recorded`}, `product`, `release`, `license_tier` ∈ {Silver, Gold, Education}, `education_mode`, `observed_at`, `evidence_part` (the `company_list` part that shows it). When `basis` is `observed`, the tier, the flag and the evidence part are all required |
| `period` | `{from, to}`: the audit period the read covers |
| `consistency` | `status` ∈ {`unchanged`, `moved`, `not_recorded`}, plus `before` and `after` high-water marks `{alter_voucher_id, alter_master_id, observed_at, part}`, and `attempts` |
| `elapsed_ms` | total wall time of the read |
| `parts[]` | one entry per stored part (the part table below) |
| `notes[]` | free-text provenance statements a person should read |

**Part fields.**

| Field | Meaning |
|---|---|
| `id` | unique within the read (`[a-z0-9][a-z0-9._-]*`) |
| `kind` | the request kind (section 5) |
| `name` | the file's display and provenance name |
| `window {from,to}` | required for `vouchers`, `trial_balance`, `report_profit_and_loss` and `report_balance_sheet` |
| `as_of` | required for `stock_summary` |
| `request` | the stored request blob. For a part whose request hash is known but whose bytes were not kept: `{sha256, stored:false}`. `null` needs `request_absent_reason` |
| `response` | the stored blob, plus `media_type`, `text_encoding`, `wire_exact`, `transformation` and `transformation_note` |
| `fetch_profile` | the request profile that produced this part |
| `requested_at`, `elapsed_ms`, `tally_status` | per request |
| `rows`, `alter_id_max` | required for `vouchers`. `rows` counts the `VOUCHER` elements that carry a `REMOTEID` attribute or a `GUID` child; a template row has neither. `alter_id_max` is the largest `ALTERID` among them, or null |
| `scope {from,to,exhaustive}` | required for `voucher_status_list` |
| `derived_from` | the part ids a derived part came from |

## 4. Consistency: the high-water bracket

A read brackets itself against concurrent posting with a Company-collection request that fetches
`GUID, ALTVCHID, ALTMSTID` with the current company pinned. A company that has never held a
voucher omits `ALTVCHID`.

**Producer procedure.**

1. Read the high-water mark H0 and store it as a `company_high_water` part.
2. Read every other part.
3. Read the high-water mark H1 and store it as well.
4. If H0 equals H1 on both axes, set `status = "unchanged"`.
5. Otherwise, discard the parts and re-read from step 1, up to `attempts = 3`.
6. If the bracket still moves, refuse. A producer may keep the refused manifest as evidence with
   `status = "moved"`, but no consumer may use it.

The bracket catches any posting or alteration made during the read, including one that nets to
zero within a ledger, which a trial-balance tie alone cannot see.

**Consumer rule (C6).** The consumer recomputes the status from the recorded values and never
trusts the label:

- The master axis is present on both sides and every axis is equal: `unchanged`.
- The master axis is present on both sides but some axis differs: `moved`. A voucher axis present
  on one side only also counts as `moved`.
- Anything else: `not_recorded`.

The consumer refuses in three cases:

- the declared status disagrees with the recomputed one (`C6-status`);
- the status is `moved` (`C6-moved`);
- the status is `not_recorded` and the client config does not set `allow_unbracketed_read = true`
  (`C6-unbracketed`). Only reads taken before brackets existed may set that flag.

**Additional check (C9).** When `after.alter_voucher_id` is known, no voucher `ALTERID` in the
read may exceed it.

## 5. Request kinds v1

| Kind | Cardinality | Window | What a consumer reads |
|---|---|---|---|
| `company_list` | ≤1 | – | per company: GUID, name, books-from, licence tier, Education mode |
| `company_high_water` | 0 or 2 (before/after) | – | COMPANY: GUID, ALTVCHID, ALTMSTID |
| `company` | 1 (required) | – | COMPANY: GUID (identity, C5), ISINTEGRATED (stock test) |
| `groups` | 1 (required) | – | GROUP@NAME, PARENT, plus classification flags (ISREVENUE, AFFECTSGROSSPROFIT, ISDEEMEDPOSITIVE, ISSUBLEDGER, RESERVEDNAME) |
| `ledgers` | 1 (required) | request with SVFROMDATE = period start | LEDGER@NAME, PARENT, OPENINGBALANCE, PARTYGSTIN, GST registration history, INCOMETAXNUMBER, ISBILLWISEON |
| `trial_balance` | 1 (required) | = period | LEDGER@NAME, TBALOPENING, TBALCLOSING, DEBITTOTALS, CREDITTOTALS |
| `voucher_types` | ≤1 | – | VOUCHERTYPE@NAME, PARENT (base-type resolution) |
| `vouchers` | ≥1 (required); windows contiguous over the period | per part | VOUCHER (GUID or @REMOTEID), DATE, VOUCHERTYPENAME/@VCHTYPE, VOUCHERNUMBER, REFERENCE, PARTYLEDGERNAME, PARTYGSTIN, NARRATION, MASTERID, ALTERID, ISCANCELLED, ISOPTIONAL, ISPOSTDATED; ledger and inventory entry lists |
| `stock_items` | ≤1 | – | STOCKITEM@NAME, GUID, PARENT, BASEUNITS, OPENINGBALANCE, OPENINGVALUE, CLOSINGBALANCE, CLOSINGVALUE |
| `stock_summary` | 0..n, distinct `as_of` | `as_of` | STOCKITEM@NAME, GUID, CLOSINGBALANCE, CLOSINGVALUE, CLOSINGRATE |
| `report_profit_and_loss` | ≤1 | = period | Tally's own report line items |
| `report_balance_sheet` | ≤1 | = period | Tally's own report line items |
| `voucher_status_list` | ≤1 | `scope` | derived JSON `{vouchers:[{masterid, optional, cancelled, postdated, void}]}` |
| `supplementary` | any | – | evidence a v1 reader ignores (probe and mirror variants, timing reads) |

Any kind a reader does not know is ignored. This is how v1.x can add kinds.

## 6. Producer rules

- **P1 — Admission.** A part enters the manifest only if the producer's own typed parser accepted
  it. The read then keeps a parse-at-the-boundary rule even though it stores raw bytes.
- **P2 — Identity.** Set the current-company pin on every company-scoped request. Re-verify the
  company GUID before the first part and after the last. `company.guid` is the identity; the name
  is display only.
- **P3 — Windows.** Size voucher windows up front: monthly by default, with smaller windows
  remembered per company. The windows are contiguous, disjoint and cover the period exactly. Never
  issue an unwindowed whole-book voucher read.
- **P4 — Education mode.** If `education_mode` is true, every window end and `as_of` date must
  fall on day 1, 2 or 31 — Tally silently widens any other date. Record the licence tier and
  Education flag from `company_list` in `tally`, with `basis = "observed"`.
- **P5 — Evidence.** Record `requested_at`, `elapsed_ms` and `tally_status` per part, and
  `elapsed_ms` for the whole read. Record `rows` and `alter_id_max` for every vouchers part.
- **P6 — Bytes.** Store bytes as received, and requests as sent. Store no derived parts.
  Compression with gzip is allowed.
- **P7 — At rest.** A whole-year book stored in plaintext is a new at-rest data class for any
  producer that previously kept hashes only. Encryption, retention and deletion policy for that
  class need their own design decision; this spec does not decide them.

## 7. Consumer rules

`bridge-tax-audit`'s `read` module enforces C1–C7 and C10; see its source for the exact code
path.

| Code | Rule |
|---|---|
| C1 | Schema-valid; `format = "tally-read"`; major version 1 (any minor). Unknown fields and kinds are ignored |
| C2 | Paths are relative and inside the read, with no `.` or `..` segment, no backslash and no symlink anywhere below the read root. `storage` matches the `.gz` suffix. `manifest.json` is a regular file |
| C3 | For every part, consumed or not: the stored bytes and the decoded content both match their sha256 and length. Decompression is capped at 512 MiB. Each parser's own returned sha256 must equal the manifest's, which closes the gap between verifying a file and parsing it |
| C4 | Part ids are unique. Singleton kinds appear at most once. `company`, `groups`, `ledgers`, `trial_balance` and at least one `vouchers` part are present. No reference points at an unknown part. A `stock_summary` is selected by exact `as_of` |
| C5 | The company part's GUID equals `company.guid`. The read's period equals the client's period |
| C6 | High-water bracket (section 4) |
| C7 | Voucher windows are sorted, disjoint and contiguous, and their union is exactly the period |
| C8 | Every voucher's DATE lies inside its own part's window. This catches Education-mode widening and a wrongly declared window |
| C9 | Each vouchers part's declared `rows` and `alter_id_max` equal what the consumer's own parser found. No ALTERID exceeds the closing high-water |
| C10 | If `education_mode` is true, every window end and `as_of` falls on day 1, 2 or 31 |
| CFG | A client config that names a read must not also name raw, unwrapped files (`CFG-mixed`) |

**Configuration.** A client config selects a read like this (paths and names below are
illustrative, not literal):

```toml
[snapshot]
format = "tally-read-v1"
path = "relative/path/to/read-v1"
allow_unbracketed_read = true   # only for a read taken before high-water brackets existed

[stock]
opening_summary = "from_masters"
opening_date = "2025-04-01"
closing_summary = "from_masters"
closing_date = "2026-03-31"
```

## 8. Versioning, and how v2 arrives without breaking v1 readers

- **`format_version` is `MAJOR.MINOR`.** A reader accepts only its own major version, and any
  minor version within it.
- **What a minor version may do.** It may only add optional fields and new part kinds. It may
  never change the meaning, units, sign or required status of an existing field or kind. The
  schema therefore allows additional properties everywhere, and v1 readers ignore what they do not
  know.
- **What forces a major version.** Changing a hash definition, making a field required, changing a
  kind's meaning (for example, a vouchers part without inventory entries), or changing the
  directory layout.
- **How v2 is added.** It gets its own schema file and its own manifest file, written next to
  `manifest.json` (v1). Both manifests point at the same `parts/` files; content addressing means
  no byte is stored twice. v1 readers never open the v2 manifest, and v2 readers prefer it. A
  producer stops writing `manifest.json` only after every consumer has a v2 reader.

## 9. Real-world validation

This format and its consumer rules have been validated against real Tally exports from several
client engagements, converted into this directory layout and read back through both the reference
Python implementation and (for the rules `bridge-tax-audit` implements) this crate. Results are
kept privately — no client name, figure or byte from that validation appears in this repository or
its CI, per this project's rule that client data never enters a public repo. CI validates this
format exclusively against synthetic, invented fixtures (see this crate's
`tests/fixtures/PROVENANCE.md`).

One general protocol observation from that validation is worth recording here because it affects
every consumer, not one client: a live group-master response can carry raw ASCII control
characters (observed: `U+0003`) inside `PARENTSTRUCTURE` elements. A strict XML parser may reject
these outright, so a `groups` part reader must tolerate them rather than assume well-formed XML.
Section 10 says how.

## 10. Text decoding: the Tally text rule

Tally's responses are not always well-formed XML 1.0, and the parts of a read keep them exactly as
sent (section 2). Every consumer turns a part's content into text by this one rule, so that two
consumers of the same bytes get the same strings. Bridge's committed group captures
(`src-tauri/crates/bridge-tally-protocol/tests/fixtures/native/group_snapshot_*`) show both of
the cases it exists for:

- raw `U+0003` characters inside `PARENTSTRUCTURE`, which Tally sends even when the request's
  `FETCH` list does not ask for that field. Each name in the list is wrapped in a pair of them:
  `U+0003 Sundry Debtors U+0003 U+0003 Current Assets U+0003` (spaces added for reading);
- the character reference `&#4;` in front of Tally's reserved values: `&#4; Primary` in `PARENT`
  and `PRIMARYGRPPARENT` for the top-level root, and `&#4; Not Applicable`, `&#4; Any` and similar
  in other master and voucher fields. XML 1.0 forbids the code point it names.

The rule, in order:

1. **Bytes to text.** Content whose second byte is `0x00` is UTF-16LE without a BOM. Otherwise a
   UTF-8, UTF-16LE or UTF-16BE byte-order mark selects that encoding, and anything else is UTF-8.
   Decoding is strict; the XML declaration's `encoding` is not consulted.
2. **No DTD.** A document that declares a DTD or an entity is refused.
3. **Forbidden references are marked, not deleted.** A numeric character reference, decimal or
   hexadecimal (`x` or `X`), whose code point XML 1.0 forbids (C0 controls other than tab, LF and
   CR; the surrogates; `U+FFFE`; `U+FFFF`; beyond `U+10FFFF`) is replaced, before parsing, by the
   text `U+FFFD` `#` *n* `;` with *n* in decimal. `&#4; Primary` therefore reads as the string
   `"\u{FFFD}#4; Primary"`, and `&#x1F;` as `"\u{FFFD}#31;"`.
   - A `U+FFFD` already in the document, literal or written as a legal reference, that is directly
     followed by `#`, one to ten ASCII digits and `;` is replaced by `U+FFFD` `#65533;`. The
     rewrite is therefore injective: every decoded value maps back to the code points Tally sent.
   - The scan for a reference's `;` covers at most twelve bytes, starting at its `#`. The rewrite
     ends at a `&#` whose `;` lies further on; what follows is left to the XML parser, which
     refuses any reference to a forbidden code point.
   - This is the rewrite `bridge-tally-protocol` already applies before its native group, ledger,
     voucher, trial-balance and outstandings parsers read a response (`tolerant_xml`), and
     `TALLY_SANITIZED_ROOT_MARKER` in that crate is the `U+FFFD#4;` prefix.
4. **Raw characters are kept.** Every raw character stays as itself, C0 controls included, in
   element text and attribute values. `PARENTSTRUCTURE` keeps its `U+0003` separators. A raw
   `U+0000`, `U+FFFE` or `U+FFFF` is refused: Tally has not been seen to send one, and a NUL
   usually means the bytes were decoded with the wrong encoding. Legal character references and
   the five predefined entities resolve as XML 1.0 says.
5. **Nothing else is transformed.** No value is trimmed, case-folded or normalised while it is
   decoded. A test that needs a trimmed view (is this empty? is this the root?) trims a copy.

**The reserved root.** A `PARENT` is Tally's reserved top-level root when, after trimming
whitespace, it starts with `U+FFFD#4;` and the rest, trimmed again, is `Primary` (ASCII case
ignored). Only the marked form is the root. A `PARENT` of plain `Primary` names a group a user
called that, and a chain walks through it like any other group. The same test with another word
(`U+FFFD#4; Not Applicable`) recognises Tally's other reserved values. A consumer that renders a
reserved value for a person may show it without the marker, as Tally does; a decision may not
depend on the unmarked spelling.

**`PARENTSTRUCTURE`.** No consumer rule reads it, and ancestry comes from `PARENT` one hop at a
time. A consumer that does read it treats it as a list: split on `U+0003` and drop the empty
items. It is never compared or stored as one string with its separators.

**A parser that refuses raw C0 controls** (expat, the parser behind Python's `ElementTree`, is
one) may carry them through the parse as other characters, provided the carriage is reversible
for every input and every value is mapped back before use. It must not delete them.

`bridge-tax-audit`'s `xml` module implements this rule. `tests/tally_text_rule.rs` reads every
committed Bridge group capture with it, and checks each decoded `PARENT` against
`bridge-tally-protocol`'s own native group parser on the captures that carry a company GUID and
on every `PARENT` spelled from up to four of fifteen reference and marker atoms.
`tests/reserved_root.rs` covers the reserved root and a group named `Primary`.

**Where Bridge's own decoders differ from this rule today.** The rule follows
`bridge-tally-protocol`, because it is the decoder in front of that crate's native collection
parsers and it is lossless. Bridge's agent-facing parsers resolve `&#4;` with a plain XML
unescape, to the raw `U+0004` character, so the same wire text has two spellings inside Bridge.
The protocol crate's `is_tally_reserved_root` also accepts a plain `Primary`. A consumer of a
read follows this section, not either of those behaviours.
