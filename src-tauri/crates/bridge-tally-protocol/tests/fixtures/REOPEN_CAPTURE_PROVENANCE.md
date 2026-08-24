# `vouchers_settle_then_reopen_live` — provenance

The one ageing path no other capture reaches: a bill **settled to zero and then reopened**.

`vouchers_agst_ref_reopen_live` (from `BRIDGE CORPUS SETTLED`) stops at `New Ref → Agst Ref(full)`,
so a bill's balance reaches zero and nothing follows it. In `outstandings/compute.rs` the
`previous_balance.is_zero()` arm therefore fires only on a bill's **first** allocation in that book,
never on a genuine reopening. This capture adds the third allocation that makes the arm fire for the
reason it exists.

## Provenance

- **Host / gateway:** TallyPrime **Silver (licensed)** 7.1, `http://localhost:9001`
- **Date:** 2026-08-23
- **Company:** `BRIDGE PROBE B SANDBOX`, written in an otherwise-unused date window
  (2026-08-10 … 2026-08-14; the book's prior last voucher was 2026-08-01) so the capture isolates
  cleanly — no non-`RO-` bill reference appears in it.
- **Encoding:** BOM-less UTF-16LE, undecoded wire bytes. `.gitattributes` marks this tree `-text`.
- **Request shape:** the production `BridgeVoucherExport` collection with its `SYSTEM TYPE="Formulae"`
  filter.
- **`/status`:** healthy before and after. Import reported `CREATED=19 ERRORS=0 EXCEPTIONS=0`.

| file | bytes | sha256 |
|---|---|---|
| `vouchers_settle_then_reopen_live.utf16le.xml` | 101,132 | `02a93b8a12c1ba80445ee136c690e72236ed372c25b333491a90e1fcd72f2737` |

`STATUS 1`, 19 vouchers, 7 bill references — five reopened and two settled-but-never-reopened
controls.

## What it establishes

**A reopening `Agst Ref` carries the original bill's `BILLDATE` and `BILLCREDITPERIOD`, supplied by
Tally.** Each of the three allocations for a bill:

```
RO-INV-003  20260810  New Ref   BILLDATE 20260810  '2 Months'  -2000.00   opened
RO-INV-003  20260812  Agst Ref  BILLDATE 20260810  '2 Months'   2000.00   settled to ZERO
RO-INV-003  20260814  Agst Ref  BILLDATE 20260810  '2 Months'   -750.00   REOPENED
```

Five distinct credit periods were used deliberately — `30 Days`, `45 Days`, `2 Months`, `3 Weeks`,
`60 Days` — and **each is echoed exactly on its own reopening**, so this is Tally resolving the
referenced bill rather than substituting a default.

The committed generator record [`generators/build_reopen.py`](./generators/build_reopen.py)
calls `entry(party, True, reopen_amount, ref, "Agst Ref")` for the reopening.
[`generators/lib.py`](./generators/lib.py) defines
`entry(led, debit, amount, bill=None, billtype=None, billdate=None, credit_period=None)` and
emits `BILLDATE` and `BILLCREDITPERIOD` only when those optional arguments are present. The
generator therefore sends **neither** field on the reopening allocation:

```python
entry(party, True, reopen_amount, ref, "Agst Ref")   # bill reference and type only
```

so the returned values cannot be an echo of its input.

These generator files are committed as an auditable record of what was sent, **not as runnable
scripts**. In particular, `lib.py` imports the session-local `tq.post` probe helper, which is
not part of this repository; running `python generators/build_reopen.py` is not a reproduction
procedure.

**Consequence:** recomputing the age date from a reopening allocation recovers the original due date
by construction. Under `AgeingAnchor::DueDate` the reopened balance of `RO-INV-003` must age from
2026-10-10 (bill date + 2 months), not from the reopening date and not from the bare bill date.

This settles the PR #171 review finding *"Preserve the original due date when an against-ref
reopens"* on the actual reopen arm rather than by inference from a book that never reopens anything.

## Controls

`RO-CTL-001` (`30 Days`) and `RO-CTL-002` (`2 Months`) are opened and settled to zero with **no**
reopening, so a test can distinguish "closed stays closed" from "reopened balance re-ages".

## Known limits

- One release (7.1), one machine.
- Reopening is by a `Sales` voucher allocating `Agst Ref` against the settled reference. A credit
  note, journal, or a *partial* settlement followed by reopening are not represented.
- Every reopening here happens after an **exact** settlement to zero. A bill driven negative and
  back is not covered.
- No reopening of a bill whose original credit period was empty appears here; that case would
  correctly age from the bill date, but it is not measured.
