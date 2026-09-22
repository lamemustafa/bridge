# SPDX-License-Identifier: Apache-2.0
"""Produce the reference Python implementation's canonical dumps for one edge book.

An edge book (tests/fixtures/edge-books/NAME.json) is a small invented book, written by hand to
reach a branch or boundary the synthetic read does not: a period, group masters, ledgers with
their chains, Trial Balance rows, vouchers, the engagement's cash and bank ledgers and
own-account narration terms, and the tests to run on it. It is not a Tally read -- the book is
built directly -- so it proves the Rust port and the reference agree on the same book, and
nothing about reading Tally.

    uv run -q --python 3.13 --with openpyxl --with xlrd --with python-docx --with jsonschema \
        --with striprtf --with pdfplumber python parity/edge_golden.py ENGINE \
        tests/fixtures/edge-books/NAME.json tests/fixtures/golden

writes golden/edge.NAME.TEST.json for each test the book names -- the reference's own canonical
dump (`tae.parity.canonical.canonical_test_result`), module invariant included -- and, for
`trial_balance`, golden/edge.NAME.trial_balance.order.json: the order in which the reference emits
its per-ledger rows, which the canonical dump (sorted by id) does not show. `tests/edge_books.rs`
builds the same book in Rust and compares whole dumps. Python 3.13 is pinned because its Unicode
tables (15.1.0) are the ones the crate's case mapping reproduces (`src/support.rs`).

Spec keys: `period` ([start, end], ISO; default the AY 2026-27 previous year), `groups` ({name:
parent or null}), `ledgers` ([{name, chain, guid}]), `tb` ([{ledger, opening, debit, credit,
closing}]), `vouchers` ([{guid, date, base_type, vtype?, number?, status?, narration?, lines:
[[ledger, paise], ...]}]; `number` defaults to the GUID, so pass `""` to test a voucher with no
number), `cash`, `bank`, `own_account_terms`, `rules_without` (top-level rules tables to drop, e.g.
["ledger_scrutiny"]; the Rust side must map each one, see `tests/edge_books.rs`), `tests`; per voucher
`party` (PARTYLEDGERNAME, default ""); and for `tds_tcs_26as`/`twentysixas_receipts`: `form26as`,
`ais`, `tis` (invented document rows in the shape `parity/python_golden.py --emit-traces-documents`
writes; default []) and `tds_ledgers`, `tcs_ledgers`, `advance_tax_ledgers`, `deductor_aliases`
(default empty).
"""
from __future__ import annotations

import copy
import json
import sys
from datetime import date
from pathlib import Path

STATUS = ("regular", "optional", "cancelled", "postdated")


def main() -> int:
    engine, spec_path, out_dir = sys.argv[1], Path(sys.argv[2]), Path(sys.argv[3])
    sys.path.insert(0, str(Path(engine).resolve()))
    from tae.adapters.traces_documents import AisRow, TisRow
    from tae.audit_tests import (cash_book_integrity, ledger_scrutiny, stale_balances_41_1, tds_tcs_26as,
                                 trial_balance, twentysixas_receipts)
    from tae.model import Form26ASRow
    from tae.config import load_rules
    from tae.model import Book, Engagement, Group, Ledger, LedgerLine, Period, TBRow, Voucher, VoucherStatus
    from tae.parity import canonical

    spec = json.loads(spec_path.read_text(encoding="utf-8"))
    name = spec_path.stem
    status = {s: getattr(VoucherStatus, s.upper()) for s in STATUS}
    start, end = spec.get("period", ["2025-04-01", "2026-03-31"])
    groups = {n: Group(name=n, parent=p) for n, p in spec["groups"].items()}
    ledgers = {l["name"]: Ledger(name=l["name"], parent=l["chain"][0] if l["chain"] else "",
                                 chain=tuple(l["chain"]), chain_complete=True, guid=l.get("guid", ""))
               for l in spec["ledgers"]}
    vouchers = [Voucher(guid=v["guid"], masterid=None, alterid=None, date=date.fromisoformat(v["date"]),
                        vtype=v.get("vtype", v["base_type"]), base_type=v["base_type"],
                        number=v.get("number", v["guid"]), reference="", party_field=v.get("party", ""), party_gstin="",
                        narration=v.get("narration", ""), status=status[v.get("status", "regular")],
                        status_source="edge-book",
                        lines=tuple(LedgerLine(ledger=l, amount_paise=a) for l, a in v["lines"]))
                for v in spec["vouchers"]]
    tb = {t["ledger"]: TBRow(ledger=t["ledger"], opening_paise=t["opening"], debit_paise=t["debit"],
                             credit_paise=t["credit"], closing_paise=t["closing"]) for t in spec["tb"]}
    book = Book(company_name="Invented edge book",
                period=Period(date.fromisoformat(start), date.fromisoformat(end)), groups=groups,
                ledgers=ledgers, vouchers=vouchers, tb=tb, company_guid="invented-edge-company")
    eng = Engagement("individual", "2026-27", book)
    rules = load_rules("2026-27", "individual")
    for table in spec.get("rules_without", []):
        rules = copy.copy(rules)
        rules.pop(table)
    cash, bank = set(spec.get("cash", [])), set(spec.get("bank", []))
    day = lambda s: None if s is None else date.fromisoformat(s)
    form26as = [Form26ASRow(**{**r, "txn_date": day(r["txn_date"])}) for r in spec.get("form26as", [])]
    ais = [AisRow(**{**r, "txn_date": day(r["txn_date"])}) for r in spec.get("ais", [])]
    tis = [TisRow(**r) for r in spec.get("tis", [])]
    eng.form26as = form26as
    aliases = dict(spec.get("deductor_aliases", {}))
    terms = frozenset(spec.get("own_account_terms", []))

    # One runner per test an edge book may name: the module and its result, run as the reference's
    # pack runs it.
    runners = {
        "cash_book_integrity": lambda: (cash_book_integrity,
                                        cash_book_integrity.run(eng, rules, cash, bank, terms)),
        "ledger_scrutiny": lambda: (ledger_scrutiny, ledger_scrutiny.run(eng, rules, cash)),
        "stale_balances_41_1": lambda: (stale_balances_41_1, stale_balances_41_1.run(eng, rules)),
        "tds_tcs_26as": lambda: (tds_tcs_26as, tds_tcs_26as.run(
            eng, rules, form26as=form26as, ais_rows=ais, tis_rows=tis,
            tds_ledgers=set(spec.get("tds_ledgers", [])), tcs_ledgers=set(spec.get("tcs_ledgers", [])),
            deductor_aliases=aliases, advance_tax_ledgers=set(spec.get("advance_tax_ledgers", [])))),
        "trial_balance": lambda: (trial_balance, trial_balance.run(eng, rules)),
        "twentysixas_receipts": lambda: (twentysixas_receipts, twentysixas_receipts.run(eng, rules, aliases)),
    }
    for test in spec["tests"]:
        if test not in runners:
            raise SystemExit(f"{spec_path}: no edge runner for test {test!r}")
        module, result = runners[test]()
        if test == "trial_balance":
            order = [fid for fid in result.figures if fid.startswith("trial_balance.tb_group_")]
            (out_dir / f"edge.{name}.trial_balance.order.json").write_text(
                json.dumps(order, indent=1) + "\n", encoding="utf-8")
        doc = canonical.canonical_test_result(eng, result, module)
        out = out_dir / f"edge.{name}.{test}.json"
        out.write_text(json.dumps(doc, indent=2, ensure_ascii=False) + "\n", encoding="utf-8")
        print(f"{out}: {len(doc['figures'])} figures, {len(doc['findings'])} findings, "
              f"{len(doc['module_invariant_violations'])} module invariant violations")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
