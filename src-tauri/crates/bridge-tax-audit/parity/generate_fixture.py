# SPDX-License-Identifier: Apache-2.0
"""Write the synthetic tally-read-v1 fixture used by the CI parity test.

Every company, ledger, group, voucher, GUID and amount below is invented. Nothing is copied
from a client book. The XML follows the element layout Tally uses in its collection exports
(ENVELOPE / HEADER / BODY / DESC / CMPINFO / DATA / COLLECTION), but the bytes were never
served by Tally: this fixture proves that the Rust port and the Python reference engine
agree on the same input, and nothing about how Tally behaves. See tests/fixtures/PROVENANCE.md.

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
    "Owner Capital": ("Capital Account", -1_20_000_00),
    "Pinecrest Builders": ("Sundry Debtors", 20_000_00),
    "Tidewater Fasteners": ("Sundry Creditors", -15_000_00),
    "Unmapped Holding": ("Legacy Holding", 0),
    "Profit & Loss A/c": ("\x04 Primary", 0),
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
)
SIDE_LIST = ({"masterid": "16", "optional": True, "cancelled": False, "postdated": False, "void": False},
             {"masterid": "17", "optional": False, "cancelled": False, "postdated": True, "void": False})
EXCLUDED = {10, 11, 16, 17}
# Tally's TB for this ledger is written 5.00 away from its vouchers, so POP-1 fires on it.
TB_SKEW = {"Electricity": 500}
ALTER_BASE = 100
HIGH_WATER = (ALTER_BASE + 18, 57)


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
        f'    </LEDGER>\n' for n, (p, o) in LEDGERS.items())
    return envelope(MASTER_ATTRS, body)


def voucher_types_xml() -> str:
    body = "".join(
        f'    <VOUCHERTYPE NAME="{esc(n)}" RESERVEDNAME="">\n'
        f'     <PARENT TYPE="String">{esc(p)}</PARENT>\n'
        f'    </VOUCHERTYPE>\n' for n, p in VOUCHER_TYPES)
    return envelope(MASTER_ATTRS, body)


def vguid(masterid: int) -> str:
    return f"{GUID}-{masterid:08x}"


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
