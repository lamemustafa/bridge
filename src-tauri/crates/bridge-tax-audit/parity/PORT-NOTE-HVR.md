# Port note: `high_value_register`, the party side against the money line

The reference changed after this port's first goldens (reference commit `1038dc05`). The goldens
`golden/synthetic.high_value_register.json` and `golden/edge.hvr_paths.high_value_register.json`
are regenerated at reference commit `140bc7d3`; `golden/edge.hvr_bare.high_value_register.json`
is byte-identical at both commits. This note states the change as behaviour, for the port.

## Why

A (party, day) row's amount is the party's side of each voucher, not the money that moved in the
row's mode. A payment of Cash 50,000 + Bank 2,00,000 to one supplier was reported as 2,50,000 of
cash at or over the s.269ST(a) limit, and the same 2,50,000 as a bank row. No row is added or
dropped, and no amount changes: where a voucher's own money line differs from the party's share of
it, the finding says so and shows that line.

## Behaviour

**Per voucher, per row.** While building the rows (both grains), each row keeps, for every voucher
that contributes to it, keyed by the voucher's GUID:

- `share`: this party's amount from that voucher (what the row already adds; the unidentified-party
  row's share is the voucher's fallback total);
- `line`: the voucher's own money line in the row's mode and direction. Receipt: the sum of the
  voucher's positive amounts on the mode's ledgers (cash or bank debited). Payment: the sum of the
  magnitudes of its negative amounts on the mode's ledgers (credited). Lines of the other direction
  and of the other mode are not counted.

Both are **added per voucher occurrence as it is read**, so two vouchers that share a GUID (an empty
GUID reads as `""`) add their lines and shares together and never pair one voucher's line with two
vouchers' shares.

**When the row is marked.** Only for the (party, day) rows that raise a finding (at or over the row
threshold). The row *differs* when, for at least one GUID, `line != share`. `line_total` is the sum
of the row's lines. `below` is `line_total < row_threshold` (the s.269ST(a) limit for cash rows, the
CA vouching threshold for bank rows).

With `mode` = `cash` or `bank`, `other_mode` the other one, `verb` = `debited` for a receipt and
`credited` for a payment, and `moved` = `received from` for a receipt and `paid to` for a payment:

1. The row amount's figure definition gains, only when the row differs, this suffix (after the
   existing text, separated by one space):
   `This is the party's side of each voucher; on one or more of the row's vouchers it differs from the {mode} line, which is shown with it.`
2. A new figure, only when the row differs: id `{prefix}_row_{mode}_line_{rid}` (the same `prefix`
   and `rid` as the row's amount figure), value `line_total`, unit `paise`, definition
   `The {mode} {verb} on the vouchers in this row, summed, whichever parties it was for.`, evidence
   the same voucher references as the row's amount figure.
3. The finding's facts gain, only when the row differs, the key `{mode}_line` naming that figure.
   The canonical dump orders a finding's facts by key, as for every other finding, so the dump reads
   `amount`, `{mode}_line`, `date`; where the reference inserts the key does not show.
4. The finding's limits gain, only when the row differs, one line appended after the existing
   mode-specific limits and **before** the unidentified-party limit (when that one applies):
   `The amount is this party's own side of each voucher, not the voucher's own {mode} line. On one or more of the row's vouchers the two differ, so such a voucher carries other lines as well (such as money moving by {other_mode}, a discount or deduction, a round-off, a loan, a counterparty this register leaves out, several parties sharing the line, or money moving the other way).`
   then, only when `below`, one space and
   `The {mode} {verb} on these vouchers is below the threshold, so the {mode} {moved} this party on them is below it too.`
   then one space and
   `The {mode} {verb} on these vouchers is shown with this row.`

Nothing else changes: titles, clauses, asks, the other figures and the invariants are as before.
Neither regenerated golden has a row that both differs and is the unidentified-party row, so the
order of limit 4 and the unidentified-party limit is stated from the reference's source, not shown
by a golden.

## Why the sentences are true (for a reviewer of the port)

- The party's money in this mode on these vouchers can never exceed `line_total`, because both are
  taken in the row's own direction; so the threshold sentence, emitted only when `line_total` is
  under the threshold, is a certainty.
- `line != share` on a balanced voucher means the voucher carries lines beyond this mode's line and
  this party's side; the list after "such as" gives examples, and no cause is asserted.

## In the regenerated goldens

- `synthetic.high_value_register.json`: one cash payment row (2025-04-01) differs: one new figure,
  its amount definition, and its finding's facts and limits.
- `edge.hvr_paths.high_value_register.json`: the cash-and-bank payment (2025-06-02) differs on both
  its cash and its bank row, and one cash receipt row (2025-05-17) differs: three new figures, three
  amount definitions, and three findings' facts and limits.
- No figure value changes, and no finding is added or removed.
