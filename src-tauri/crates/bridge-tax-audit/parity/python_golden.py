# SPDX-License-Identifier: Apache-2.0
"""Produce the reference Python implementation's canonical dump (docs/tax-audit/parity-spec-v1.md)
for one engagement, for either ported test.

The reference engine is the Python tax-audit engine this crate ports; it is not in this
repository. Pass its `engine/` directory (the one holding the `tae` package) as ENGINE. Run it
the way that engine's porting contract runs everything, e.g.:

    uv run -q --with openpyxl --with xlrd --with python-docx --with jsonschema --with striprtf \
        --with pdfplumber python parity/python_golden.py ENGINE ENGAGEMENT_TOML OUT_JSON \
        --test cash_payments_40a3

ENGAGEMENT_TOML is a client config with [client], [period], [roles] and a [snapshot] naming a
tally-read-v1 directory (relative to the TOML), e.g. tests/fixtures/synthetic-engagement.toml.
`--test` selects the module (default `cash_44ab`, for backward compatibility with every existing
invocation of this script). `cash_payments_40a3` additionally reads `[roles].round_off_ledgers`
(optional, defaults to none configured) and an optional `[loans.loan_ledgers.<ledger>]` table
(defaults to no ledger configured) -- the same optional-table conventions the reference engine's
own loaders use for a client with nothing configured there. With --read DIR, the [snapshot]
table is replaced in memory by that read with allow_unbracketed_read = true -- the same switch
the engine's own read-format parity gate applies -- so a legacy client config can be run against
its wrapped read without editing it.

The book is built by the reference implementation's own read-format adapter and the dump by its
own canonical serialiser, so this script adds no logic of its own beyond choosing the inputs.
"""
from __future__ import annotations

import argparse
import json
import sys
import tomllib
from pathlib import Path


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("engine")
    ap.add_argument("engagement")
    ap.add_argument("output")
    ap.add_argument("--read", help="override [snapshot] with this tally-read-v1 directory")
    ap.add_argument("--test", default="cash_44ab", choices=["cash_44ab", "cash_payments_40a3"])
    a = ap.parse_args()

    sys.path.insert(0, str(Path(a.engine).resolve()))
    from tae.adapters import read_format
    from tae.audit_tests import cash_44ab, cash_payments_40a3
    from tae.config import load_rules, loan_ledgers_config, resolve_ledgers, role_ledger_set
    from tae.model import Engagement
    from tae.parity import canonical

    path = Path(a.engagement).resolve()
    cfg = tomllib.loads(path.read_text(encoding="utf-8"))
    if a.read:
        cfg["snapshot"] = {"format": "tally-read-v1", "path": str(Path(a.read).resolve()),
                           "allow_unbracketed_read": True}
    book = read_format.load_book(cfg, path.parent)
    eng = Engagement(cfg["client"]["entity_type"], cfg["client"]["assessment_year"], book)
    rules = load_rules(eng.assessment_year, eng.entity_type)
    cash = resolve_ledgers(book, cfg["roles"]["cash_groups"])
    bank = resolve_ledgers(book, cfg["roles"]["bank_groups"])
    if a.test == "cash_44ab":
        module = cash_44ab
        result = cash_44ab.run(eng, rules, cash=cash, bank=bank)
    else:
        module = cash_payments_40a3
        round_off_ledgers = role_ledger_set(cfg, "round_off_ledgers") if "round_off_ledgers" in cfg.get("roles", {}) else set()
        loan_ledgers_configured = set(loan_ledgers_config(cfg))
        result = cash_payments_40a3.run(
            eng, rules, cash=cash, bank=bank,
            loan_ledgers_configured=loan_ledgers_configured,
            round_off_ledgers=frozenset(round_off_ledgers))
    doc = canonical.canonical_test_result(eng, result, module)
    Path(a.output).write_text(json.dumps(doc, indent=2, ensure_ascii=False) + "\n", encoding="utf-8")
    print(f"{a.output}: {len(doc['figures'])} figures, {len(doc['findings'])} findings, "
          f"{len(doc['book_invariant_violations'])} book invariant violations")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
