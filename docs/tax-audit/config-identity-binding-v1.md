# Engagement config identity binding, version 1

This is the public specification of how an engagement config's ledger and group names are
resolved against a Book before a test reads them. The reference implementation (a Python engine)
established this rule in production; `bridge-tax-audit`'s `binding` module is the first Rust
implementation, and its doc comments are the exact mapping.

## 1. The problem

An engagement config names ledgers and groups by display text: a cash-group list, a round-off
ledger set, a table of loan ledgers, a depreciation block map. Staff rename ledgers in Tally
between one read and the next. Before this rule existed, a name that stopped matching the Book
simply dropped out of whatever set or map it belonged to — the figures moved, and nothing said
why.

## 2. The rule

Two optional tables in the config, `[ledger_ids]` and `[group_ids]`, map a LABEL — the display
name the rest of the config uses in every other location — to the master's Tally identity:

```toml
[ledger_ids]
"Round Off" = "1753...-000000b1"                       # a bare GUID
"Cartage Inward" = { guid = "...", masterid = 193 }    # GUID and MASTERID, both checked
"Office Rent" = { masterid = 177 }                     # MASTERID alone
```

For every configured location that names a ledger (or, in `[group_ids]`, a group):

- **A label with an identity entry** is resolved by that identity, never by its text, and every
  occurrence of the label in the config is rewritten to the master's CURRENT name in the Book
  being read. A GUID that resolves to no master, to more than one master, or a MASTERID that
  disagrees with the GUID's own master, refuses (see the codes below) rather than guessing.
- **A bare label** (no identity entry) resolves exactly as it always has: it must equal a ledger
  (or group) name in the Book exactly. A bare label that matches nothing refuses.
- **Nothing is ever case-folded, whitespace-folded or fuzzy-matched.** The only thing that
  follows a rename is the identity.

A label bound by identity whose master now carries a different name than the label is a reported
**drift**, not a refusal: the identity already proves which master the config meant, so the
figures are correct, but the rename might also mean the ledger was repurposed, which only a
person reading the drift can judge.

## 3. Refusal codes

| Code | Fires when |
| --- | --- |
| `BIND-NAME-UNKNOWN` | A bare ledger label matches no ledger in the Book. |
| `BIND-GROUP-UNKNOWN` | A bare group label matches no group in the Book. |
| `BIND-GUID-UNKNOWN` | An identity entry's GUID matches no master in the Book. |
| `BIND-MASTERID-UNKNOWN` | An identity entry's MASTERID (with no GUID, or a GUID that already resolved) matches no master. |
| `BIND-MASTERID-MISMATCH` | An identity entry names both a GUID and a MASTERID, and the GUID's master carries a different MASTERID. |
| `BIND-NO-MASTERID` | An identity entry binds by MASTERID alone, but the Book carries no MASTERIDs at all. |
| `BIND-BOOK-DUPLICATE-ID` | A GUID or MASTERID an identity entry names is carried by more than one master in the Book. |
| `BIND-ID-MALFORMED` | `[ledger_ids]`/`[group_ids]` (or an entry in them) is not a GUID string or `{ guid, masterid }`, or names neither. |
| `BIND-ID-UNUSED` | An identity entry binds a label no configured location this implementation reads actually uses. |
| `BIND-COLLISION` | Two different labels in a table keyed by ledger name (e.g. a loan-ledger table, a depreciation block map) resolve to the same current ledger — merging their values would be a guess. |

## 4. Scope of this port

`bridge-tax-audit` ports three tests (`cash_44ab`, `cash_payments_40a3`, `depreciation`), and its
`Engagement` config type reads only the locations those three need: `roles.cash_groups`,
`roles.bank_groups`, `roles.round_off_ledgers`, `loans.loan_ledgers`'s keys,
`depreciation.block_by_ledger`'s keys and `depreciation.dep_expense_ledgers`. This is a strict
subset of the reference implementation's own registry, which also binds names inside `tds`,
`gst_outward`, `related_parties`, `statutory_dues`, a legacy trade-creditor source and more.

`BIND-ID-UNUSED` is therefore checked only against the locations this port reads. A production
client config's `[ledger_ids]`/`[group_ids]` are written for the reference implementation's full
pack and will typically carry labels this port never looks at; a caller feeding such a config to
this crate should first narrow those two tables to the labels the six locations above actually
use, the same way this crate's own local parity harness does, or `BIND-ID-UNUSED` will fire on a
label that is genuinely used, just not by this port.

## 5. What binding does not do

Binding never reads or verifies the Book itself — it only resolves names already loaded by
[`read-format-v1`](read-format-v1.md) and [`book`](../../src-tauri/crates/bridge-tax-audit/src/book.rs).
It runs once, at config load, before any test; a test module never resolves a configured name on
its own.
