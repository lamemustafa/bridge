"""Settle-then-reopen — the arm no committed capture exercises.

BRIDGE CORPUS SETTLED has New Ref -> Agst Ref(full) and stops there, so a bill's
balance reaches zero and nothing follows. compute.rs's `previous_balance.is_zero()`
arm therefore fires only on a bill's FIRST allocation in that book, never on a
genuine reopening.

This adds a THIRD allocation: New Ref -> Agst Ref(settles to zero) -> Agst Ref(reopens).
The question it answers: does the REOPENING allocation carry the original bill's
BILLCREDITPERIOD, or does it arrive empty? If empty, recomputing the age date from it
collapses due-date ageing to bill-date ageing for the reopened balance.

Written into the sandbox in an unused date window so a capture can isolate it.
"""
import datetime
from lib import send, envelope, ledger, voucher, entry

C  = "BRIDGE PROBE B SANDBOX"
d1 = datetime.date(2026, 8, 10)   # invoice
d2 = datetime.date(2026, 8, 12)   # receipt settles to zero
d3 = datetime.date(2026, 8, 14)   # reopening

parties = [f"RO Party {i:02d}" for i in (1, 2, 3)]
send(envelope(C, "All Masters",
    "".join(ledger(p, "Sundry Debtors") for p in parties)
    + ledger("RO Sales", "Sales Accounts", billwise=False)
    + ledger("RO Bank", "Bank Accounts", billwise=False)), "reopen masters")

# Vary the credit period so an echoed value is unmistakably the ORIGINAL one,
# not a default Tally might substitute.
CASES = [("RO-INV-001", "30 Days",  1000.00, 400.00),
         ("RO-INV-002", "45 Days",  1500.00, 600.00),
         ("RO-INV-003", "2 Months", 2000.00, 750.00),
         ("RO-INV-004", "3 Weeks",  2500.00, 900.00),
         ("RO-INV-005", "60 Days",  3000.00, 1200.00)]
CONTROLS = [("RO-CTL-001", "30 Days", 1100.00),   # settled, never reopened
            ("RO-CTL-002", "2 Months", 1300.00)]

vs = []
for ref, cp, amt, reopen_amt in CASES:
    p = parties[int(ref[-1]) % 3]
    vs.append(voucher("Sales", d1,
        [entry(p, True, amt, ref, "New Ref", d1, cp),
         entry("RO Sales", False, amt)], f"reopen {ref} open {cp}"))
    vs.append(voucher("Receipt", d2,
        [entry(p, False, amt, ref, "Agst Ref"),
         entry("RO Bank", True, amt)], f"reopen {ref} settle to zero"))
    # The reopening: no credit period sent, so anything present on readback is Tally's.
    vs.append(voucher("Sales", d3,
        [entry(p, True, reopen_amt, ref, "Agst Ref"),
         entry("RO Sales", False, reopen_amt)], f"reopen {ref} REOPEN after zero"))

for ref, cp, amt in CONTROLS:
    p = parties[0]
    vs.append(voucher("Sales", d1,
        [entry(p, True, amt, ref, "New Ref", d1, cp),
         entry("RO Sales", False, amt)], f"control {ref} open {cp}"))
    vs.append(voucher("Receipt", d2,
        [entry(p, False, amt, ref, "Agst Ref"),
         entry("RO Bank", True, amt)], f"control {ref} settle, no reopen"))

send(envelope(C, "Vouchers", "".join(vs)), f"{len(vs)} vouchers")
print(f"  -> 5 reopened bills + 2 settled controls, window {d1}..{d3}")
