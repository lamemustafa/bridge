# SPDX-License-Identifier: Apache-2.0
"""Write the synthetic tally-read-v1 fixture used by the CI parity test.

Every company, ledger, group, voucher, GUID and amount below is invented. Nothing is copied
from a client book. The XML follows the element layout Tally uses in its collection exports
(ENVELOPE / HEADER / BODY / DESC / CMPINFO / DATA / COLLECTION), but the bytes were never
served by Tally: this fixture proves that the Rust port and the Python reference engine
agree on the same input, and nothing about how Tally behaves. See tests/fixtures/PROVENANCE.md.

One Book exercises both ported tests end to end: `cash_44ab` (cash and bank receipts/payments)
and `cash_payments_40a3` (masterid 19-42) -- a payee over the s.40A(3) daily limit aggregated
across two same-day vouchers, a payee exactly at the limit and one under it, a goods-carriage
heuristic hit within the Rs 35,000 proviso limit, one cash payment excluded from s.40A(3) scope
for each of the five `excluded_group_roles` kinds (the loans_liability example is two group
levels below "Loans (Liability)", proving the exclusion walks the full chain, not just the
immediate parent), s.269ST receipts and payments at/over and under the limit, a real party with
a tax or round-off line folded into it, an unidentified-party cash leg (no real party line, tax
only), and a covered/uncovered pair of s.269SS/269T loan candidates in both directions.

Masterid 38-42 add one case exactly AT each of the four ">="/"or more"/inclusive comparisons a
2026-09-18 mutation review found the fixture never exercised (flipping the comparison direction
in the Rust port passed every existing test): a s.269ST receipt of exactly Rs 2,00,000 from one
party in one day (Fairfield Textiles), a s.269ST payment leg of exactly Rs 2,00,000 to one party
in one day (Ashwood Traders), a s.269SS/269T loan-ledger line of exactly Rs 20,000 (a second
Sunrise NBFC Loan voucher), and the goods-carriage proviso limit itself: one payee-day of exactly
Rs 35,000 (Coastal Freight Carriers, still within the higher limit) and one of Rs 35,000.01, one
paisa over it (Bayside Cargo Logistics, now outside it) -- both transport-name heuristic hits and
both over the plain s.40A(3) limit, so both also produce an s.40A(3) finding.

Masterid 43-49 add `depreciation` (Income-tax Act block WDV vs books) to the same book, across the
three real blocks the vendored rules carry (furniture_10 10%, plant_machinery_15 15%,
computers_40 40%) -- fully mapped throughout, so the "unmapped ledger fails loud" and DEP-1/DEP-2
paths (which would blank every block figure for the whole test) are covered by the crate's own
Rust unit tests on hand-built books instead, never by this shared fixture. One case exactly AT
each boundary the module tests: an addition put to use for exactly 180 days (Office Furniture,
full rate) beside one for exactly 179 days (Showroom Furniture, half rate, one calendar day
later); a cash-paid addition of exactly Rs 10,000 -- the s.43(1) second proviso limit itself --
not flagged (Office Computers) beside one of Rs 10,000.01, one paisa over it, flagged (Reception
Computers). Also: a deletion (Factory Machine, masterid 48) that exceeds the full-rate pool and
spills into the half-rate one (s.43(6)); a depreciation-journal voucher (masterid 49, crediting
Factory Machine, debiting "Depreciation A/c") wiring `dep_expense_ledgers` and the book-vs-Act
tie; and a GST line beside the Office Computers addition (masterid 43) proving
`gst_tcs_addition_lines_seen_count` is exercised on data. Every new ledger's TB is left to the
script's own auto-derivation (no `TB_SKEW` entry), so DEP-1 ties cleanly on all of them.

2026-09-19: every ledger master now carries its own GUID and MASTERID (`lguid`, masterid 501+,
distinct numbering from the voucher masterids 1-49 above), the way a real Tally export always
does -- needed once `cash_payments_40a3`/`depreciation` derive their row ids from
`stable_ledger_tag` (Tally GUID, bridge#510) rather than a name hash; a ledger with no GUID falls
back to a hash of its name, as the reference implementation does (`docs/tax-audit/parity-spec-v1.md` §11).

Deterministic: running it twice writes identical bytes (gzip mtime is fixed at 0).

    python3 parity/generate_fixture.py tests/fixtures
"""
from __future__ import annotations

import gzip
import hashlib
import json
import sys
from pathlib import Path

COMPANY = "Lakeview Hardware Demo"
GUID = "6f1c2a3e-8b4d-4c5e-9a7f-0d1e2f3a4b5c"
PERIOD = ("2025-04-01", "2026-03-31")
WINDOWS = (("2025-04-01", "2025-09-30"), ("2025-10-01", "2026-03-31"))

CMPINFO_TAGS = ("COMPANY", "GROUP", "LEDGER", "VOUCHERTYPE", "CURRENCY", "UNIT", "VOUCHER")

PRIMARY = ("Capital Account", "Current Assets", "Current Liabilities", "Indirect Expenses",
           "Loans (Liability)", "Purchase Accounts", "Sales Accounts")
SUB_GROUPS = (
    ("Bank Accounts", "Current Assets"),
    ("Bank OD A/c", "Loans (Liability)"),
    ("Cash-in-Hand", "Current Assets"),
    ("Counter Cash", "Cash-in-Hand"),
    ("Shop Running Costs", "Indirect Expenses"),
    ("Sundry Creditors", "Current Liabilities"),
    ("Sundry Debtors", "Current Assets"),
    # Added for cash_payments_40a3: a duties/taxes group and an asset-side loans-and-advances
    # group (neither is one of Tally's 15 reserved primaries), and a group nested two levels
    # below "Loans (Liability)" so a payee's full group CHAIN (not just its immediate parent)
    # is what s.40A(3)'s exclusion walks.
    ("Duties & Taxes", "Current Liabilities"),
    ("Loans & Advances (Asset)", "Current Assets"),
    ("Unsecured Loans", "Loans (Liability)"),
)

# name -> (parent group, opening in paise, debit positive). "Legacy Holding" is deliberately
# absent from the group masters, so this ledger's chain is incomplete (MAP-1 fires on it).
LEDGERS = {
    "Cash": ("Cash-in-Hand", 5_000_00),
    "Counter Till": ("Counter Cash", 0),
    "Lakeview Bank Current A/c": ("Bank Accounts", 1_50_000_00),
    "Ridge Bank OD A/c": ("Bank OD A/c", -40_000_00),
    "Sales - Hardware": ("Sales Accounts", 0),
    "Purchases - Hardware": ("Purchase Accounts", 0),
    "Shop Rent": ("Shop Running Costs", 0),
    "Electricity": ("Indirect Expenses", 0),
    "Owner Capital": ("Capital Account", -5_70_000_00),  # offsets Factory Machine's opening below, so TB openings still sum to zero (POP-3)
    "Pinecrest Builders": ("Sundry Debtors", 20_000_00),
    "Tidewater Fasteners": ("Sundry Creditors", -15_000_00),
    "Unmapped Holding": ("Legacy Holding", 0),
    "Profit & Loss A/c": ("\x04 Primary", 0),
    # cash_payments_40a3 ledgers below. Every name is invented (PROVENANCE.md).
    "Riverside Traders": ("Sundry Creditors", 0),  # s.40A(3): over the daily limit, two vouchers
    "Meadow Supplies": ("Shop Running Costs", 0),  # s.40A(3): one day under, one day exactly at the limit
    "Northgate Transport Carrier": ("Sundry Creditors", 0),  # s.40A(3): over the limit, goods-carriage heuristic hit
    "Staff Advances": ("Loans & Advances (Asset)", 0),  # s.40A(3) excluded: loans_advances_asset
    "Delivery Van": ("Fixed Assets", 0),  # s.40A(3) excluded: fixed_assets
    "GST Payable": ("Duties & Taxes", 0),  # s.40A(3) excluded: duties_taxes; also a 269ST tax/fold line
    "Highway Motors Loan": ("Unsecured Loans", 0),  # s.40A(3) excluded: loans_liability, via a two-level chain;
                                                     # also an uncovered s.269T candidate (cash repaid)
    "Sunrise NBFC Loan": ("Loans (Liability)", 0),  # covered s.269SS candidate (cash accepted)
    "Harborview Textiles": ("Sundry Debtors", 0),  # s.269ST receipt party, over the limit
    "Union Textiles": ("Sundry Debtors", 0),  # s.269ST receipt: one real party + a tax line folded in
    "Cobalt Fittings": ("Sundry Creditors", 0),  # s.269ST payment leg party, over the limit
    "Metro Hardware Distributors": ("Sundry Creditors", 0),  # s.269ST payment: one real party + round-off folded in
    "Round Off": ("Indirect Expenses", 0),  # round_off_ledgers role, not under Duties & Taxes
    # masterid 38-42 boundary ledgers below (2026-09-18 mutation review). Every name is invented.
    "Fairfield Textiles": ("Sundry Debtors", 0),  # s.269ST receipt: exactly at the limit
    "Ashwood Traders": ("Sundry Creditors", 0),  # s.269ST payment leg: exactly at the limit
    "Coastal Freight Carriers": ("Sundry Creditors", 0),  # goods-carriage: exactly at the Rs 35,000 proviso limit
    "Bayside Cargo Logistics": ("Sundry Creditors", 0),  # goods-carriage: Rs 0.01 over the proviso limit
    # masterid 43-49 ledgers below, for `depreciation`. Every name is invented; "Fixed Assets" is
    # one of Bridge's own reserved primary groups (book.rs PRIMARY_GROUPS), so a ledger can be
    # parented to it directly with no GROUP element, same convention as "Delivery Van" above.
    "Office Furniture": ("Fixed Assets", 0),  # furniture_10: addition at exactly 180 days used (full rate)
    "Showroom Furniture": ("Fixed Assets", 0),  # furniture_10: addition at exactly 179 days used (half rate)
    "Factory Machine": ("Fixed Assets", 4_50_000_00),  # plant_machinery_15: deletion spills into the half-rate pool (s.43(6)); also the DEP-1/book-vs-Act journal
    "Office Computers": ("Fixed Assets", 0),  # computers_40: cash-paid addition exactly AT the s.43(1) second proviso limit (not flagged)
    "Reception Computers": ("Fixed Assets", 0),  # computers_40: cash-paid addition Rs 0.01 over the limit (flagged)
    "Depreciation A/c": ("Indirect Expenses", 0),  # dep_expense_ledgers: the client's own book depreciation charge
    "Comfort Furnishings": ("Sundry Creditors", 0),  # invented supplier absorbing the non-cash addition credits above
    "Machinery Disposal Proceeds": ("Current Assets", 0),  # invented counter-ledger for the Factory Machine deletion (amount receivable)
}

VOUCHER_TYPES = (("Contra", "Contra"), ("Journal", "Journal"), ("Payment", "Payment"),
                 ("Purchase", "Purchase"), ("Receipt", "Receipt"), ("Sales", "Sales"),
                 ("Cash Sale", "Sales"), ("Bank Transfer", "Contra"))

# (masterid, date, voucher type, number, status flag, lines as (ledger, amount text in
# Tally's sign: debit negative)). Amount text is written as Tally writes it; "1234.565"
# carries a half paisa on purpose, so half-up and half-to-even rounding give different paise.
VOUCHERS_H1 = (
    (1, "20250405", "Cash Sale", "CS/1", None, (("Cash", "-42000.00"), ("Sales - Hardware", "42000.00"))),
    (2, "20250418", "Receipt", "R/1", None, (("Lakeview Bank Current A/c", "-118000.00"), ("Pinecrest Builders", "118000.00"))),
    (3, "20250502", "Payment", "P/1", None, (("Shop Rent", "-25000.00"), ("Lakeview Bank Current A/c", "25000.00"))),
    (4, "20250520", "Payment", "P/2", None, (("Electricity", "-3150.50"), ("Cash", "3150.50"))),
    (5, "20250603", "Purchase", "PU/1", None, (("Purchases - Hardware", "-90000.00"), ("Tidewater Fasteners", "90000.00"))),
    (6, "20250625", "Payment", "P/3", None, (("Tidewater Fasteners", "-60000.00"), ("Ridge Bank OD A/c", "60000.00"))),
    (7, "20250701", "Contra", "C/1", None, (("Lakeview Bank Current A/c", "-30000.00"), ("Cash", "30000.00"))),
    (8, "20250715", "Bank Transfer", "BT/1", None, (("Counter Till", "-2000.00"), ("Cash", "2000.00"))),
    (9, "20250809", "Receipt", "R/2", None, (("Counter Till", "-1234.565"), ("Pinecrest Builders", "1234.565"))),
    (10, "20250820", "Payment", "P/4", "ISOPTIONAL", (("Shop Rent", "-9999.00"), ("Cash", "9999.00"))),
    (11, "20250902", "Cash Sale", "CS/2", "ISCANCELLED", ()),
    (12, "20250928", "Journal", "J/1", None, (("Electricity", "-500.00"), ("Unmapped Holding", "498.00"))),
    # ---- cash_payments_40a3 vouchers below (masterid 19-37). Every name and amount is invented.
    # s.40A(3): Riverside Traders paid Rs 6,000 + Rs 5,700 = Rs 11,700 cash same day (over Rs 10,000).
    (19, "20250610", "Payment", "P/8", None, (("Riverside Traders", "-6000.00"), ("Cash", "6000.00"))),
    (20, "20250610", "Payment", "P/9", None, (("Riverside Traders", "-5700.00"), ("Cash", "5700.00"))),
    # Meadow Supplies: Rs 4,000 one day (under the limit), Rs 10,000 another (exactly at it, not over).
    (21, "20250611", "Payment", "P/10", None, (("Meadow Supplies", "-4000.00"), ("Cash", "4000.00"))),
    (22, "20250612", "Payment", "P/11", None, (("Meadow Supplies", "-10000.00"), ("Cash", "10000.00"))),
    # Northgate Transport Carrier: Rs 32,000 cash, transport-name heuristic within the Rs 35,000 goods limit.
    (23, "20250613", "Payment", "P/12", None, (("Northgate Transport Carrier", "-32000.00"), ("Cash", "32000.00"))),
    # s.40A(3) exclusions, one cash payment per excluded_group_roles kind:
    (24, "20250616", "Payment", "P/13", None, (("Owner Capital", "-12000.00"), ("Cash", "12000.00"))),  # capital
    (25, "20250617", "Payment", "P/14", None, (("Highway Motors Loan", "-25000.00"), ("Cash", "25000.00"))),  # loans_liability, two-level chain; also an uncovered s.269T candidate
    (26, "20250618", "Payment", "P/15", None, (("Staff Advances", "-8000.00"), ("Cash", "8000.00"))),  # loans_advances_asset
    (27, "20250619", "Payment", "P/16", None, (("Delivery Van", "-45000.00"), ("Cash", "45000.00"))),  # fixed_assets
    (28, "20250620", "Payment", "P/17", None, (("GST Payable", "-3000.00"), ("Cash", "3000.00"))),  # duties_taxes
    # s.269ST receipt leg: Rs 2,10,000 cash from Harborview Textiles (over the limit); Rs 50,000
    # from Pinecrest Builders (under it, a control); Rs 2,15,000 from Union Textiles with a GST
    # tax line folded into the one real party; an unidentified-party cash sale (Sales Accounts +
    # tax, no real party line) at Rs 2,20,000.
    (29, "20250623", "Receipt", "R/5", None, (("Cash", "-210000.00"), ("Harborview Textiles", "210000.00"))),
    (30, "20250624", "Receipt", "R/6", None, (("Cash", "-50000.00"), ("Pinecrest Builders", "50000.00"))),
    (31, "20250625", "Receipt", "R/7", None, (("Cash", "-215000.00"), ("Union Textiles", "200000.00"), ("GST Payable", "15000.00"))),
    (32, "20250626", "Cash Sale", "CS/4", None, (("Cash", "-220000.00"), ("Sales - Hardware", "200000.00"), ("GST Payable", "20000.00"))),
    # s.269ST payment leg: mirrors the receipt leg above, Purchase Accounts in place of Sales.
    (33, "20250627", "Payment", "P/18", None, (("Cobalt Fittings", "-205000.00"), ("Cash", "205000.00"))),
    (34, "20250630", "Payment", "P/19", None, (("Tidewater Fasteners", "-9000.00"), ("Cash", "9000.00"))),
    (35, "20250702", "Payment", "P/20", None, (("Metro Hardware Distributors", "-185000.00"), ("Round Off", "-20000.00"), ("Cash", "205000.00"))),
    (36, "20250703", "Purchase", "PU/2", None, (("Purchases - Hardware", "-190000.00"), ("GST Payable", "-20000.00"), ("Cash", "210000.00"))),
    # s.269SS candidate, covered by [loans] configuration: Rs 30,000 cash accepted against Sunrise NBFC Loan.
    (37, "20250704", "Receipt", "R/8", None, (("Cash", "-30000.00"), ("Sunrise NBFC Loan", "30000.00"))),
    # ---- masterid 38-42: one case exactly at each ">="/inclusive comparison (2026-09-18 mutation review).
    # s.269ST receipt leg, exactly at the Rs 2,00,000 per-person-per-day limit (limb (i), "or more").
    (38, "20250707", "Receipt", "R/9", None, (("Cash", "-200000.00"), ("Fairfield Textiles", "200000.00"))),
    # s.269ST payment leg, exactly at the Rs 2,00,000 per-person-per-day threshold.
    (39, "20250708", "Payment", "P/21", None, (("Ashwood Traders", "-200000.00"), ("Cash", "200000.00"))),
    # s.269SS/269T candidate, exactly at the Rs 20,000 limit ("or more"): a second Sunrise NBFC Loan
    # voucher, cash accepted.
    (40, "20250709", "Receipt", "R/10", None, (("Cash", "-20000.00"), ("Sunrise NBFC Loan", "20000.00"))),
    # Coastal Freight Carriers: Rs 35,000 cash, exactly at the goods-carriage proviso limit (still
    # within it, transport-name heuristic hit, also over the plain Rs 10,000 s.40A(3) limit).
    (41, "20250710", "Payment", "P/22", None, (("Coastal Freight Carriers", "-35000.00"), ("Cash", "35000.00"))),
    # Bayside Cargo Logistics: Rs 35,000.01 cash, one paisa over the goods-carriage proviso limit
    # (now outside it; same transport-name heuristic and s.40A(3) over-limit as the row above).
    (42, "20250711", "Payment", "P/23", None, (("Bayside Cargo Logistics", "-35000.01"), ("Cash", "35000.01"))),
    # ---- masterid 43-49: `depreciation` (see the module docstring above for the full scenario map).
    # Office Computers (computers_40): addition Rs 2,00,000 with a GST input-tax line alongside it
    # (gst_tcs_addition_lines_seen_count) and a cash payment of exactly Rs 10,000 -- the s.43(1)
    # second proviso limit itself -- NOT flagged (the limit is "over", not "at or over").
    (43, "20250401", "Purchase", "PU/3", None,
     (("Office Computers", "-200000.00"), ("GST Payable", "-36000.00"), ("Cash", "10000.00"), ("Comfort Furnishings", "226000.00"))),
    # Reception Computers (computers_40): addition Rs 1,50,000 with a cash payment of Rs 10,000.01,
    # one paisa over the s.43(1) limit -- flagged.
    (44, "20250401", "Purchase", "PU/4", None,
     (("Reception Computers", "-150000.00"), ("Cash", "10000.01"), ("Comfort Furnishings", "139999.99"))),
    # Factory Machine deletion of Rs 5,50,000, exceeding the full-rate pool (opening only, no >=180
    # addition here) and spilling into the half-rate pool (s.43(6)).
    (48, "20250601", "Journal", "J/2", None, (("Machinery Disposal Proceeds", "-550000.00"), ("Factory Machine", "550000.00"))),
)
# The second window carries no ISOPTIONAL/ISPOSTDATED tags (the shape of an export whose
# status came from a separate side list); the voucher_status_list part decides them.
VOUCHERS_H2 = (
    (13, "20251010", "Cash Sale", "CS/3", None, (("Cash", "-55500.00"), ("Sales - Hardware", "55500.00"))),
    (14, "20251111", "Payment", "P/5", None, (("Purchases - Hardware", "-12000.00"), ("Cash", "12000.00"))),
    (15, "20251212", "Receipt", "R/3", None, (("Ridge Bank OD A/c", "-75000.00"), ("Pinecrest Builders", "75000.00"))),
    (16, "20260105", "Payment", "P/6", None, (("Shop Rent", "-8000.00"), ("Cash", "8000.00"))),
    (17, "20260320", "Receipt", "R/4", None, (("Lakeview Bank Current A/c", "-10000.00"), ("Pinecrest Builders", "10000.00"))),
    (18, "20260214", "Payment", "P/7", None, (("Tidewater Fasteners", "-4000.00"), ("Counter Till", "4000.00"))),
    # ---- masterid 45-47, 49: `depreciation` (continued from VOUCHERS_H1's masterid 43/44/48).
    # Office Furniture (furniture_10): addition put to use exactly 180 days before period end
    # (2026-03-31), inclusive -- the s.32(1) second proviso boundary itself: full rate.
    (45, "20251003", "Purchase", "PU/6", None, (("Office Furniture", "-100000.00"), ("Comfort Furnishings", "100000.00"))),
    # Showroom Furniture (furniture_10): one calendar day later, exactly 179 days used: half rate.
    (46, "20251004", "Purchase", "PU/7", None, (("Showroom Furniture", "-60000.00"), ("Comfort Furnishings", "60000.00"))),
    # Factory Machine (plant_machinery_15): a mid-year addition well under 180 days used, paid on credit.
    (47, "20260201", "Purchase", "PU/5", None, (("Factory Machine", "-80000.00"), ("Comfort Furnishings", "80000.00"))),
    # Depreciation journal: the client's own book depreciation for the year on Factory Machine,
    # wiring dep_expense_ledgers and the book-vs-Act tie (book_dep_total, book_dep_tie_diff_paise).
    (49, "20260331", "Journal", "J/3", None, (("Depreciation A/c", "-45000.00"), ("Factory Machine", "45000.00"))),
)
SIDE_LIST = ({"masterid": "16", "optional": True, "cancelled": False, "postdated": False, "void": False},
             {"masterid": "17", "optional": False, "cancelled": False, "postdated": True, "void": False})
EXCLUDED = {10, 11, 16, 17}
# Tally's TB for this ledger is written 5.00 away from its vouchers, so POP-1 fires on it.
TB_SKEW = {"Electricity": 500}
ALTER_BASE = 100
HIGH_WATER = (ALTER_BASE + 49, 57)  # closing high-water must be >= the max ALTERID across both windows (masterid 49, H2)


def paise(text: str) -> int:
    neg = text.startswith("-")
    whole, _, frac = text.lstrip("-").partition(".")
    frac = (frac + "000")[:3]
    p = int(whole) * 100 + int(frac[:2]) + (1 if int(frac[2]) >= 5 else 0)
    return -p if neg else p


def amount_text(paise_dr_positive: int) -> str:
    """Canonical paise (debit positive) back to Tally amount text (debit negative); empty for zero."""
    if paise_dr_positive == 0:
        return ""
    v = -paise_dr_positive
    sign = "-" if v < 0 else ""
    return f"{sign}{abs(v) // 100}.{abs(v) % 100:02d}"


def envelope(collection_attrs: str, body: str) -> str:
    cmpinfo = "".join(f"    <{t}>0</{t}>\n" for t in CMPINFO_TAGS)
    return ("<ENVELOPE>\n <HEADER>\n  <VERSION>1</VERSION>\n  <STATUS>1</STATUS>\n </HEADER>\n"
            f" <BODY>\n  <DESC>\n   <CMPINFO>\n{cmpinfo}   </CMPINFO>\n  </DESC>\n  <DATA>\n"
            f"   <COLLECTION {collection_attrs}>\n{body}   </COLLECTION>\n  </DATA>\n </BODY>\n</ENVELOPE>\n")


def esc(s: str) -> str:
    return s.replace("&", "&amp;").replace("<", "&lt;").replace(">", "&gt;").replace('"', "&quot;")


def tally_text(s: str) -> str:
    """Escape, then write U+0004 the way Tally does: as the character reference &#4;."""
    return esc(s).replace("\x04", "&#4;")


MASTER_ATTRS = 'ISMSTDEPTYPE="Yes" MSTDEPTYPE="1"'


def groups_xml() -> str:
    rows = [(g, "\x04 Primary") for g in PRIMARY] + list(SUB_GROUPS)
    body = "".join(
        f'    <GROUP NAME="{esc(n)}" RESERVEDNAME="">\n'
        f'     <PARENT TYPE="String">{tally_text(p)}</PARENT>\n'
        f'     <ISSUBLEDGER TYPE="Logical">No</ISSUBLEDGER>\n'
        f'    </GROUP>\n' for n, p in rows)
    return envelope(MASTER_ATTRS, body)


def ledgers_xml() -> str:
    body = "".join(
        f'    <LEDGER NAME="{esc(n)}" RESERVEDNAME="">\n'
        f'     <PARENT TYPE="String">{tally_text(p)}</PARENT>\n'
        f'     <OPENINGBALANCE TYPE="Amount">{amount_text(o)}</OPENINGBALANCE>\n'
        f'     <GUID>{lguid(i)}</GUID>\n'
        f'     <MASTERID TYPE="Number"> {i}</MASTERID>\n'
        f'    </LEDGER>\n' for i, (n, (p, o)) in enumerate(LEDGERS.items(), start=501))
    return envelope(MASTER_ATTRS, body)


def voucher_types_xml() -> str:
    body = "".join(
        f'    <VOUCHERTYPE NAME="{esc(n)}" RESERVEDNAME="">\n'
        f'     <PARENT TYPE="String">{esc(p)}</PARENT>\n'
        f'    </VOUCHERTYPE>\n' for n, p in VOUCHER_TYPES)
    return envelope(MASTER_ATTRS, body)


def vguid(masterid: int) -> str:
    return f"{GUID}-{masterid:08x}"


def lguid(ledger_masterid: int) -> str:
    """A ledger master's own Tally identity (bridge#510 / tae/ledger_ids.py): distinct numbering
    from `vguid`'s voucher masterids (1-49) so a ledger and a voucher never share a GUID by
    coincidence. Every ledger in this fixture carries one -- 2026-09-19, switching
    cash_payments_40a3/depreciation's row ids to stable_ledger_tag (GUID-based) needs every
    ledger this book's ledgers.xml describes to carry a GUID, exactly as a real Tally export
    always does (tae/ledger_ids.py's own MissingGuid docstring: 0 missing across three real
    client reads)."""
    return f"{GUID}-ldg-{ledger_masterid:04d}"


def voucher_xml(rows, with_flags: bool) -> str:
    out = []
    for mid, date, vtype, number, flag, lines in rows:
        g = vguid(mid)
        x = [f'    <VOUCHER REMOTEID="{g}" VCHKEY="{g}:00000008" VCHTYPE="{esc(vtype)}" OBJVIEW="Accounting Voucher View">\n',
             f'     <DATE TYPE="Date">{date}</DATE>\n',
             f'     <GUID>{g}</GUID>\n',
             f'     <NARRATION TYPE="String">Synthetic voucher {number}</NARRATION>\n',
             f'     <VOUCHERTYPENAME>{esc(vtype)}</VOUCHERTYPENAME>\n',
             f'     <VOUCHERNUMBER>{esc(number)}</VOUCHERNUMBER>\n']
        flags = ("ISCANCELLED", "ISOPTIONAL", "ISPOSTDATED") if with_flags else ()
        for f in flags:
            x.append(f'     <{f} TYPE="Logical">{"Yes" if flag == f else "No"}</{f}>\n')
        x.append(f'     <ALTERID TYPE="Number"> {ALTER_BASE + mid}</ALTERID>\n')
        x.append(f'     <MASTERID TYPE="Number"> {mid}</MASTERID>\n')
        for ledger, amt in lines:
            dp = "Yes" if amt.startswith("-") else "No"
            x.append('     <ALLLEDGERENTRIES.LIST>\n'
                     f'      <LEDGERNAME TYPE="String">{esc(ledger)}</LEDGERNAME>\n'
                     f'      <ISDEEMEDPOSITIVE TYPE="Logical">{dp}</ISDEEMEDPOSITIVE>\n'
                     f'      <AMOUNT TYPE="Amount">{amt}</AMOUNT>\n'
                     '     </ALLLEDGERENTRIES.LIST>\n')
        x.append('    </VOUCHER>\n')
        out.append("".join(x))
    return envelope('ISCMPDEPTYPE="Yes" CMPLOCUS="1" CMPDEPTYPE="1"', "".join(out))


def trial_balance_xml() -> str:
    dr = {n: 0 for n in LEDGERS}
    cr = {n: 0 for n in LEDGERS}
    for mid, _d, _t, _n, _f, lines in VOUCHERS_H1 + VOUCHERS_H2:
        if mid in EXCLUDED:
            continue
        for ledger, amt in lines:
            p = -paise(amt)
            if p > 0:
                dr[ledger] += p
            else:
                cr[ledger] -= p
    body = []
    for n, (_p, opening) in LEDGERS.items():
        closing = opening + dr[n] - cr[n] + TB_SKEW.get(n, 0)
        body.append(f'    <LEDGER NAME="{esc(n)}" RESERVEDNAME="">\n'
                    f'     <DEBITTOTALS TYPE="Amount">{amount_text(dr[n])}</DEBITTOTALS>\n'
                    f'     <CREDITTOTALS TYPE="Amount">{amount_text(-cr[n])}</CREDITTOTALS>\n'
                    f'     <TBALCLOSING TYPE="Amount">{amount_text(closing)}</TBALCLOSING>\n'
                    f'     <TBALOPENING TYPE="Amount">{amount_text(opening)}</TBALOPENING>\n'
                    f'    </LEDGER>\n')
    return envelope(MASTER_ATTRS, "".join(body))


def company_xml() -> str:
    body = (f'    <COMPANY NAME="{esc(COMPANY)}" RESERVEDNAME="">\n'
            f'     <GUID TYPE="String">{GUID}</GUID>\n'
            f'     <ISINTEGRATED TYPE="Logical">No</ISINTEGRATED>\n'
            f'    </COMPANY>\n')
    return envelope(MASTER_ATTRS, body)


def high_water_xml() -> str:
    body = (f'    <COMPANY NAME="{esc(COMPANY)}" RESERVEDNAME="">\n'
            f'     <GUID TYPE="String">{GUID}</GUID>\n'
            f'     <ALTVCHID TYPE="Number"> {HIGH_WATER[0]}</ALTVCHID>\n'
            f'     <ALTMSTID TYPE="Number"> {HIGH_WATER[1]}</ALTMSTID>\n'
            f'    </COMPANY>\n')
    return envelope(MASTER_ATTRS, body)


def utf16le(text: str) -> bytes:
    return text.encode("utf-16-le")  # no BOM, as Tally serves master collections


def blob(root: Path, rel: str, content: bytes, gz: bool, encoding: str, media: str = "application/xml") -> dict:
    stored = gzip.compress(content, compresslevel=9, mtime=0) if gz else content
    path = root / rel
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_bytes(stored)
    return {"path": rel, "storage": "gzip" if gz else "identity",
            "stored_sha256": hashlib.sha256(stored).hexdigest(), "stored_bytes": len(stored),
            "sha256": hashlib.sha256(content).hexdigest(), "bytes": len(content),
            "media_type": media, "text_encoding": encoding, "wire_exact": False,
            "transformation": "derived",
            "transformation_note": "synthetic: written by parity/generate_fixture.py, never served by Tally"}


def part(root, pid, kind, name, content, gz=False, encoding="utf-8", media="application/xml", **extra) -> dict:
    p = {"id": pid, "kind": kind, "name": name,
         "request": None, "request_absent_reason": "synthetic fixture: no request was sent",
         "response": blob(root, f"parts/{name}", content, gz, encoding, media),
         "fetch_profile": "synthetic", "requested_at": None, "elapsed_ms": None, "tally_status": None}
    p.update(extra)
    return p


def main(out_dir: str) -> None:
    root = Path(out_dir) / "synthetic-read"
    h1 = voucher_xml(VOUCHERS_H1, with_flags=True).encode()
    h2 = voucher_xml(VOUCHERS_H2, with_flags=False).encode()
    parts = [
        part(root, "high-water-before", "company_high_water", "high_water_before.xml", high_water_xml().encode()),
        part(root, "company", "company", "company_object.xml", company_xml().encode()),
        part(root, "groups", "groups", "groups.xml", utf16le(groups_xml()), encoding="utf-16le"),
        part(root, "ledgers", "ledgers", "ledgers.xml", utf16le(ledgers_xml()), encoding="utf-16le"),
        part(root, "trial-balance", "trial_balance", "tb_fy.xml", trial_balance_xml().encode(),
             window={"from": PERIOD[0], "to": PERIOD[1]}),
        part(root, "voucher-types", "voucher_types", "vouchertypes.xml", voucher_types_xml().encode()),
        part(root, "vouchers-2025-04-01", "vouchers", "vouchers_h1.xml", h1,
             window={"from": WINDOWS[0][0], "to": WINDOWS[0][1]}, rows=len(VOUCHERS_H1),
             alter_id_max=ALTER_BASE + max(v[0] for v in VOUCHERS_H1)),
        part(root, "vouchers-2025-10-01", "vouchers", "vouchers_h2.xml.gz", h2, gz=True,
             window={"from": WINDOWS[1][0], "to": WINDOWS[1][1]}, rows=len(VOUCHERS_H2),
             alter_id_max=ALTER_BASE + max(v[0] for v in VOUCHERS_H2)),
        part(root, "voucher-status-list", "voucher_status_list", "voucher_status_list.json",
             (json.dumps({"vouchers": list(SIDE_LIST)}, indent=1) + "\n").encode(), media="application/json",
             scope={"from": PERIOD[0], "to": PERIOD[1], "exhaustive": True}),
        part(root, "high-water-after", "company_high_water", "high_water_after.xml", high_water_xml().encode()),
    ]
    mark = {"alter_voucher_id": HIGH_WATER[0], "alter_master_id": HIGH_WATER[1], "observed_at": None}
    manifest = {
        "format": "tally-read", "format_version": "1.0", "read_id": "synthetic-lakeview-1",
        "created_at": "2026-09-18T00:00:00+05:30", "read_at": "2026-09-18T00:00:00+05:30",
        "producer": {"name": "bridge-tax-audit parity/generate_fixture.py", "version": "1", "kind": "synthetic",
                     "source": None},
        "company": {"guid": GUID, "name": COMPANY, "books_from": PERIOD[0]},
        "tally": {"basis": "not_recorded", "product": None, "release": None, "license_tier": None,
                  "education_mode": None, "observed_at": None, "evidence_part": None},
        "period": {"from": PERIOD[0], "to": PERIOD[1]},
        "consistency": {"status": "unchanged", "attempts": 1,
                        "before": {**mark, "part": "high-water-before"},
                        "after": {**mark, "part": "high-water-after"}},
        "elapsed_ms": None,
        "parts": parts,
        "notes": ["Synthetic read: every name, GUID and amount is invented; no byte was served by Tally."],
    }
    (root / "manifest.json").write_text(json.dumps(manifest, indent=1) + "\n", encoding="utf-8")


if __name__ == "__main__":
    main(sys.argv[1])
