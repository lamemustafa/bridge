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
own loaders use for a client with nothing configured there. `depreciation` reads
`[depreciation].block_by_ledger`, `.opening_wdv_paise` and `.dep_expense_ledgers` (all three
REQUIRED -- `tae.config.depreciation_config`'s own `require()` raises on a missing one, unlike the
optional tables above) and an optional `[depreciation.put_to_use_by_voucher]` (defaults to none
configured; no shipped client config uses it, but `depreciation.run()`'s own signature carries
it). With --read DIR, the [snapshot] table is replaced in memory by that read with
allow_unbracketed_read = true -- the same switch the engine's own read-format parity gate applies
-- so a legacy client config can be run against its wrapped read without editing it.

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
    ap.add_argument("--test", default="cash_44ab",
                     choices=["cash_44ab", "cash_payments_40a3", "depreciation"])
    a = ap.parse_args()

    sys.path.insert(0, str(Path(a.engine).resolve()))
    from tae.adapters import read_format
    from tae.audit_tests import cash_44ab, cash_payments_40a3, depreciation
    from tae.binding import bind_config
    from tae.config import (depreciation_config, load_rules, loan_ledgers_config, resolve_ledgers,
                             role_ledger_set)
    from tae.model import Engagement
    from tae.parity import canonical

    path = Path(a.engagement).resolve()
    cfg = tomllib.loads(path.read_text(encoding="utf-8"))
    if a.read:
        cfg["snapshot"] = {"format": "tally-read-v1", "path": str(Path(a.read).resolve()),
                           "allow_unbracketed_read": True}
    book = read_format.load_book(cfg, path.parent)
    # Mirror tae/run.py's load(): every configured ledger/group name is bound against the Book
    # before anything downstream reads it, so a renamed ledger's [ledger_ids]/[group_ids] entry
    # (or a bare name that still matches) is resolved the same way the reference runner resolves
    # it -- not the raw, possibly-stale name straight out of the TOML.
    cfg, binding = bind_config(cfg, book, path.parent)
    eng = Engagement(cfg["client"]["entity_type"], cfg["client"]["assessment_year"], book,
                      config_binding=binding.drifts)
    rules = load_rules(eng.assessment_year, eng.entity_type)
    cash = resolve_ledgers(book, cfg["roles"]["cash_groups"])
    bank = resolve_ledgers(book, cfg["roles"]["bank_groups"])
    if a.test == "cash_44ab":
        module = cash_44ab
        result = cash_44ab.run(eng, rules, cash=cash, bank=bank)
    elif a.test == "cash_payments_40a3":
        module = cash_payments_40a3
        round_off_ledgers = role_ledger_set(cfg, "round_off_ledgers") if "round_off_ledgers" in cfg.get("roles", {}) else set()
        loan_ledgers_configured = set(loan_ledgers_config(cfg))
        result = cash_payments_40a3.run(
            eng, rules, cash=cash, bank=bank,
            loan_ledgers_configured=loan_ledgers_configured,
            round_off_ledgers=frozenset(round_off_ledgers))
    else:
        module = depreciation
        block_by_ledger, opening_wdv_paise, dep_expense_ledgers = depreciation_config(cfg)
        # Same shape as tae/pack.py's own rules_dep: the AY version plus every [depreciation] key
        # bar its two prose fields, never the whole Rules table.
        rules_dep = {"version": rules.version,
                     **{k: v for k, v in rules["depreciation"].items() if k not in ("authority", "status")}}
        result = depreciation.run(eng, rules_dep, block_by_ledger, opening_wdv_paise, dep_expense_ledgers)
    doc = canonical.canonical_test_result(eng, result, module)
    Path(a.output).write_text(json.dumps(doc, indent=2, ensure_ascii=False) + "\n", encoding="utf-8")
    print(f"{a.output}: {len(doc['figures'])} figures, {len(doc['findings'])} findings, "
          f"{len(doc['book_invariant_violations'])} book invariant violations")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
