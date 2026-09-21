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
it). `financial_statements` reads each `[partners.<key>].interest_ledger` (optional; none when there is
no [partners] table) and, only when asked, Tally's own Profit & Loss report totals: `--report-totals
FILE` reads them from a JSON file ({"net_profit_paise", "closing_stock_paise", "source"}), and
`--emit-report-totals FILE` takes them from the read's own report part through the reference
engine's `read_format.report_pl` and writes that JSON, so the Rust side can be fed the same numbers
(the crate takes these totals as caller data and does not parse Tally reports). With neither, the
test runs with no report, as it does for a read that carries none. `applicability_44ab` runs as
the reference's own pack runs it: books turnover is financial_statements' `sales`, the cash share
is cash_44ab's two figures and finding limits, presumptive history is the optional
[presumptive_history] table. GSTR-1/GSTR-3B/AIS turnover is caller data: `--turnover-inputs FILE`
reads {"gstr1": {"turnover_paise", "coverage"} | null, "gstr3b": ..., "ais": ...};
`--emit-turnover-inputs FILE` takes GSTR-1 from the reference's own full pack for an engagement
that is one of the engine's own client configs (named by its file stem, as the pack names it),
with the configured gstr1_coverage and no GSTR-3B/AIS, exactly as that pack passes them, and
writes that JSON. With neither, no comparison source is supplied. With --read DIR, the [snapshot] table is replaced in memory by that read with
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
                     choices=["cash_44ab", "cash_payments_40a3", "depreciation", "financial_statements",
                              "applicability_44ab"])
    ap.add_argument("--turnover-inputs", help="applicability_44ab: comparison turnover JSON to use")
    ap.add_argument("--emit-turnover-inputs",
                    help="applicability_44ab: take GSTR-1 turnover from the reference's own pack and write it here")
    ap.add_argument("--report-totals", help="financial_statements: report totals JSON to use")
    ap.add_argument("--emit-report-totals",
                    help="financial_statements: take report totals from the read and write them here")
    a = ap.parse_args()

    sys.path.insert(0, str(Path(a.engine).resolve()))
    from tae.adapters import read_format
    from tae.audit_tests import (applicability_44ab, cash_44ab, cash_payments_40a3, depreciation,
                                 financial_statements)
    from tae.binding import bind_config
    from tae.config import (depreciation_config, gstr1_coverage, load_rules, loan_ledgers_config,
                             partner_interest_ledgers, presumptive_history_config, resolve_ledgers,
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
    elif a.test == "financial_statements":
        module = financial_statements
        report_totals = None
        if a.report_totals and a.emit_report_totals:
            ap.error("--report-totals and --emit-report-totals are exclusive")
        if a.report_totals:
            report_totals = json.loads(Path(a.report_totals).read_text(encoding="utf-8"))
        elif a.emit_report_totals:
            pl = read_format.report_pl(cfg, path.parent)
            if pl is None:
                raise SystemExit("--emit-report-totals: the read carries no Profit & Loss report")
            # The same three keys, with the same source text, that the reference's own pack builds.
            report_totals = {"net_profit_paise": pl["net_profit_paise"],
                             "closing_stock_paise": pl["closing_stock_paise"],
                             "source": "Tally Profit & Loss report export"}
            Path(a.emit_report_totals).write_text(json.dumps(report_totals, indent=2) + "\n", encoding="utf-8")
        result = financial_statements.run(eng, rules, partner_interest_ledgers(cfg), report_totals)
    elif a.test == "applicability_44ab":
        module = applicability_44ab
        if a.turnover_inputs and a.emit_turnover_inputs:
            ap.error("--turnover-inputs and --emit-turnover-inputs are exclusive")
        comparisons = {"gstr1": None, "gstr3b": None, "ais": None}
        if a.turnover_inputs:
            comparisons = json.loads(Path(a.turnover_inputs).read_text(encoding="utf-8"))
        elif a.emit_turnover_inputs:
            from tae import pack
            from tae.audit_tests import gst_outward_gstr1
            _cfg, _eng, results, _inv = pack._compute(path.stem)
            fig = results[gst_outward_gstr1.TEST_ID].figures[
                f"{gst_outward_gstr1.TEST_ID}.annual_gstr1_taxable_total_paise"]
            comparisons = {"gstr1": None if fig.value is None else
                           {"turnover_paise": fig.value, "coverage": gstr1_coverage(cfg)},
                           "gstr3b": None, "ais": None}
            Path(a.emit_turnover_inputs).write_text(json.dumps(comparisons, indent=2) + "\n", encoding="utf-8")
        fs = financial_statements.run(eng, rules, partner_interest_ledgers(cfg), None)
        c44 = cash_44ab.run(eng, rules, cash=cash, bank=bank)
        turnover_inputs = {"books_turnover_paise": fs.figures[f"{financial_statements.TEST_ID}.sales"].value}
        for source in ("gstr1", "gstr3b", "ais"):
            c = comparisons.get(source)
            turnover_inputs[f"{source}_turnover_paise"] = None if c is None else c["turnover_paise"]
            if c is not None:
                turnover_inputs[f"{source}_coverage"] = c["coverage"]
        cash_share = {"receipts_bp": c44.figures[f"{cash_44ab.TEST_ID}.cash_share_receipts"].value,
                      "payments_bp": c44.figures[f"{cash_44ab.TEST_ID}.cash_share_payments"].value,
                      "limits": c44.findings[0].limits}
        result = applicability_44ab.run(eng, rules, turnover_inputs, cash_share, presumptive_history_config(cfg))
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
