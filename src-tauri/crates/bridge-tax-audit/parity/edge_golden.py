# SPDX-License-Identifier: Apache-2.0
"""Produce the reference Python implementation's canonical dumps for one edge book.

An edge book (tests/fixtures/edge-books/NAME.json) is a small invented book, written by hand to
reach a branch or boundary the synthetic read does not: a period, group masters, ledgers with
their chains, Trial Balance rows, vouchers, the engagement's cash and bank ledgers and
own-account narration terms, and the tests to run on it. It is not a Tally read -- the book is
built directly -- so it proves the Rust port and the reference agree on the same book, and
nothing about reading Tally.

    uv run -q --with openpyxl --with xlrd --with python-docx --with jsonschema --with striprtf \
        --with pdfplumber python parity/edge_golden.py ENGINE tests/fixtures/edge-books/NAME.json \
        tests/fixtures/golden

writes golden/edge.NAME.TEST.json for each test the book names -- the reference's own canonical
dump (`tae.parity.canonical.canonical_test_result`), module invariant included -- and, for
`trial_balance`, golden/edge.NAME.trial_balance.order.json: the order in which the reference emits
its per-ledger rows, which the canonical dump (sorted by id) does not show. `tests/edge_books.rs`
builds the same book in Rust and compares whole dumps.

Spec keys: `period` ([start, end], ISO; default the AY 2026-27 previous year), `groups` ({name:
parent or null}), `ledgers` ([{name, chain, guid}]), `tb` ([{ledger, opening, debit, credit,
closing}]), `vouchers` ([{guid, date, base_type, vtype?, number?, status?, narration?, lines:
[[ledger, paise], ...]}]), `cash`, `bank`, `own_account_terms`, `rules_without` (rules tables to
drop, e.g. ["ledger_scrutiny"]), `tests`.
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
    from tae.audit_tests import cash_book_integrity, ledger_scrutiny, stale_balances_41_1, trial_balance
    from tae.config import load_rules
    from tae.model import Book, Engagement, Group, Ledger, LedgerLine, Period, TBRow, Voucher, VoucherStatus
    from tae.parity import canonical

    spec = json.loads(spec_path.read_text(encoding="utf-8"))
    name = spec_path.stem
    status = {s: getattr(VoucherStatus, s.upper()) for s in STATUS}
    start, end = spec.get("period", ["2025-04-01", "2026-03-31"])
    groups = {n: Group(n, p) for n, p in spec["groups"].items()}
    ledgers = {l["name"]: Ledger(l["name"], l["chain"][0] if l["chain"] else "", tuple(l["chain"]), True,
                                 guid=l.get("guid", "")) for l in spec["ledgers"]}
    vouchers = [Voucher(v["guid"], None, None, date.fromisoformat(v["date"]), v.get("vtype", v["base_type"]),
                        v["base_type"], v.get("number", v["guid"]), "", "", "", v.get("narration", ""),
                        status[v.get("status", "regular")], "edge-book",
                        tuple(LedgerLine(l, a) for l, a in v["lines"])) for v in spec["vouchers"]]
    tb = {t["ledger"]: TBRow(t["ledger"], t["opening"], t["debit"], t["credit"], t["closing"]) for t in spec["tb"]}
    book = Book("Invented edge book", Period(date.fromisoformat(start), date.fromisoformat(end)), groups,
                ledgers, vouchers, tb, company_guid="invented-edge-company")
    eng = Engagement("individual", "2026-27", book)
    rules = load_rules("2026-27", "individual")
    for table in spec.get("rules_without", []):
        rules = copy.copy(rules)
        rules.pop(table)
    cash, bank = set(spec.get("cash", [])), set(spec.get("bank", []))
    terms = frozenset(spec.get("own_account_terms", []))

    for test in spec["tests"]:
        if test == "trial_balance":
            module, result = trial_balance, trial_balance.run(eng, rules)
            order = [fid for fid in result.figures if fid.startswith("trial_balance.tb_group_")]
            (out_dir / f"edge.{name}.trial_balance.order.json").write_text(
                json.dumps(order, indent=1) + "\n", encoding="utf-8")
        elif test == "stale_balances_41_1":
            module, result = stale_balances_41_1, stale_balances_41_1.run(eng, rules)
        elif test == "ledger_scrutiny":
            module, result = ledger_scrutiny, ledger_scrutiny.run(eng, rules, cash)
        elif test == "cash_book_integrity":
            module, result = cash_book_integrity, cash_book_integrity.run(eng, rules, cash, bank, terms)
        else:
            raise SystemExit(f"{spec_path}: unknown test {test!r}")
        doc = canonical.canonical_test_result(eng, result, module)
        out = out_dir / f"edge.{name}.{test}.json"
        out.write_text(json.dumps(doc, indent=2, ensure_ascii=False) + "\n", encoding="utf-8")
        print(f"{out}: {len(doc['figures'])} figures, {len(doc['findings'])} findings, "
              f"{len(doc['module_invariant_violations'])} module invariant violations")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
