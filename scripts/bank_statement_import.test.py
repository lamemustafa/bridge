"""Offline contract tests for the bank-statement importer.

No PDF, no network, no client data. Every fixture is synthetic: the counterparty
names, references, account digits and amounts are invented, and the only thing
carried over from a real statement is the *column geometry*, which is a property
of the bank's template rather than of any customer.

Two levels of fixture, deliberately:

  * `PAGE`-shaped fixtures are `pdftotext -bbox-layout` output, built word by
    word at real x/y coordinates, and go through `parse_pages` end to end. They
    exercise page anchors, column bounds, header suppression, footer detection,
    row-start detection, multi-line row assembly and the wrap heuristic — the
    machinery a layout change actually breaks. Only `pdftotext` itself is out of
    reach without a binary PDF.
  * row-dict fixtures test the pure functions downstream of parsing.

Refusals are asserted by *category*, never by "some SystemExit was raised": a
test that only proves an error occurred passes just as happily when an unrelated
error starts firing first, which is exactly how a regression hides.
"""

import contextlib
import datetime
import decimal
import io
import importlib.util
import os
import pathlib
import stat
import tempfile

SCRIPT = pathlib.Path(__file__).resolve().parent / "bank_statement_import.py"
#: never a credential — only ever compared for identity, or fed to an
#: argument the parser is expected to reject
SENTINEL = "<placeholder-not-a-credential>"
D = decimal.Decimal


def load():
    spec = importlib.util.spec_from_file_location("bank_statement_import", SCRIPT)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


# --------------------------------------------------------------------------- #
# helpers                                                                      #
# --------------------------------------------------------------------------- #

def refuses(m, category, call, *args, **kwargs):
    """Assert `call` refuses with exactly `category`."""
    try:
        call(*args, **kwargs)
    except m.Refusal as refusal:
        assert refusal.category == category, \
            f"expected refusal {category!r}, got {refusal.category!r} ({refusal})"
        return refusal
    raise AssertionError(f"expected refusal {category!r}, call succeeded")


def word(x0, y, x1, text, height=9.0):
    return (f'<word xMin="{x0}" yMin="{y}" xMax="{x1}" yMax="{y + height}">'
            f'{text}</word>')


def page(*lines):
    """One `pdftotext -bbox-layout` page. `lines` are (y, [(x0, x1, text), ...])."""
    body = "".join(word(x0, y, x1, text)
                   for y, cells in lines for x0, x1, text in cells)
    return f'width="595" height="842">{body}</page>'


# An HDFC page in the real column geometry: date 0-70, narration 70-280 (wrap
# edge 240), ref 280-358, value date 358-400, withdrawal 400-480, deposit
# 480-560, balance 560+.
HDFC_PAGE = page(
    (60, [(70, 200, "Statement"), (205, 260, "of"), (265, 340, "account"),
          (400, 500, "00000000001234")]),
    (100, [(5, 30, "Date"), (72, 120, "Narration"), (282, 340, "Chq./Ref.No."),
           (360, 380, "Value"), (382, 396, "Dt"), (402, 452, "Withdrawal"),
           (454, 474, "Amt."), (482, 522, "Deposit"), (524, 544, "Amt."),
           (562, 600, "Closing"), (602, 640, "Balance")]),
    # row 1: the narration is hard-wrapped mid-reference at the cell edge
    (120, [(2, 60, "01/08/26"), (72, 238, "UPI-NORTH-WIND-north@zzz-ZZZZ0001-1234"),
           (282, 350, "0000123456789012"), (360, 398, "01/08/26"),
           (482, 540, "10,000.00"), (562, 620, "11,000.00")]),
    # the continuation line carries a narration fragment AND a stray fragment
    # in the reference column, which is why those columns are row-scoped
    (132, [(72, 200, "56789012-PAYMENT"), (282, 330, "CONTINUED")]),
    # row 2: single-line narration, several words in the same cell
    (150, [(2, 60, "02/08/26"), (72, 110, "NEFT"), (112, 190, "DR-ZZZZ0000001-ACME"),
           (192, 230, "EXPORTS-NETBANK,"),
           (282, 350, "ZZZZZ00000000000"), (360, 398, "02/08/26"),
           (402, 460, "2,500.50"), (562, 620, "8,499.50")]),
    (170, [(100, 140, "HDFC"), (142, 175, "BANK"), (177, 220, "LIMITED")]),
    # below the footer: must not be read
    (190, [(2, 60, "03/08/26"), (72, 200, "UPI-GHOST-g@z-ZZZZ0001-999999999999-X"),
           (402, 460, "1.00"), (562, 620, "8,498.50")]),
)

# An SBI page: date 0-85, value date 85-140, narration 140-220 (wrap edge 220),
# ref 220-299 (wrap edge 299), branch 299-356, debit 356-441, credit 441-506.
# SBI stacks the date over the year and repeats a three-line column header on
# every page, *below* the anchor.
SBI_PAGE = page(
    (60, [(2, 40, "Account"), (45, 90, "Number:"), (95, 200, "00000000007777")]),
    (90, [(2, 20, "Txn"), (22, 45, "Date")]),
    (100, [(88, 110, "Value"), (112, 135, "Date"), (142, 190, "Description"),
           (222, 235, "Ref"), (237, 250, "No."), (300, 330, "Branch"),
           (332, 350, "Code"), (400, 430, "Debit"), (460, 490, "Credit"),
           (510, 545, "Balance")]),
    (115, [(2, 8, "1"), (10, 30, "Aug"), (88, 130, "1 Aug 2026"),
           (142, 218, "TO TRANSFER- INB NEFT UTR NO: ZZZZ1111"),
           (222, 296, "NEFT INB: ZZZZZZZZZ9 TRANSFER TO 000"),
           (360, 430, "5000.00"), (510, 570, "95000.00")]),
    (127, [(2, 25, "2026"), (142, 200, "11111- NORTH WIND TRADERS"),
           (222, 280, "0000000 / NORTH WIND TRADERS")]),
)

# Page 2 repeats the whole three-line column header below its own anchor, while
# the last row of page 1 is still the row in progress. Without header
# suppression those words are appended to that row's narration.
SBI_PAGE_2 = page(
    (90, [(2, 20, "Txn"), (22, 45, "Date")]),
    (100, [(88, 110, "Value"), (112, 135, "Date"), (142, 190, "Description"),
           (222, 235, "Ref"), (237, 250, "No."), (300, 330, "Branch"),
           (332, 350, "Code"), (400, 430, "Debit"), (460, 490, "Credit"),
           (510, 545, "Balance")]),
    (115, [(2, 8, "2"), (10, 30, "Aug"), (142, 200, "BY TRANSFER- ZEPHYR LTD"),
           (222, 280, "TRANSFER FROM 0000000 / ZEPHYR LTD"),
           (460, 500, "1000.00"), (510, 570, "96000.00")]),
    (127, [(2, 25, "2026")]),
)


# --------------------------------------------------------------------------- #
# end-to-end parsing, from bbox-layout output                                  #
# --------------------------------------------------------------------------- #

def test_parse_hdfc_page(m):
    """Anchors, column bounds, footer, row-scoped columns and the wrap heuristic,
    all through the real code path."""
    rows = m.parse_pages([HDFC_PAGE], m.HDFC())
    assert len(rows) == 2, [r["narr"] for r in rows]  # the post-footer row is not a row

    first = rows[0]
    # the 12-digit reference was split across two printed lines at the cell edge
    # and must rejoin with no space, or it is a different reference
    assert first["narr"] == "UPI-NORTH-WIND-north@zzz-ZZZZ0001-123456789012-PAYMENT"
    assert first["date"] == "01/08/26"
    assert first["dr"] == "" and first["cr"] == "10000.00" and first["bal"] == "11000.00"
    assert m.HDFC().party(first) == "NORTH-WIND"
    assert m.HDFC().reference(first) == ("UPI", "123456789012")

    second = rows[1]
    # words inside one cell on one line are space-joined, not welded
    assert second["narr"] == "NEFT DR-ZZZZ0000001-ACME EXPORTS-NETBANK,"
    assert second["dr"] == "2500.50" and second["cr"] == ""
    assert m.HDFC().party(second) == "ACME EXPORTS"

    # row-scoped columns are read only from the line carrying the date, so a
    # stray fragment on a continuation line cannot corrupt the reference
    assert first["ref"] == "0000123456789012"


def test_parse_sbi_page(m):
    """SBI's repeated three-line header sits below the anchor and must not be
    appended to the row in progress; the date is stacked over the year."""
    rows = m.parse_pages([SBI_PAGE, SBI_PAGE_2], m.SBI())
    assert len(rows) == 2
    row = rows[0]
    assert "Description" not in row["narr"] and "Balance" not in row["bal"]
    assert m.SBI().parse_date(row["date"]) == datetime.date(2026, 8, 1)
    assert row["dr"] == "5000.00" and row["bal"] == "95000.00"
    # de-wrapped: the UTR is intact because the first fragment reached the edge
    assert "ZZZZ111111111" in row["narr"]
    # space-joined: the counterparty name is not welded to the reference
    assert "NORTH WIND TRADERS" in row["narr_spaced"]
    assert m.SBI().party(row) == "NORTH WIND TRADERS"
    # page 2's repeated header must not land in page 1's last row, nor become a row
    for furniture in ("Description", "Branch", "Credit", "Balance"):
        assert furniture not in row["narr_spaced"], furniture
    assert m.SBI().parse_date(rows[1]["date"]) == datetime.date(2026, 8, 2)
    assert rows[1]["cr"] == "1000.00"


def test_account_binding(m):
    """The running-balance proof is equally happy to certify the wrong account's
    statement, so the account digits must appear in the document."""
    m.require_account_match([HDFC_PAGE], "HDFC CA xx1234")
    refuses(m, "account_not_in_statement", m.require_account_match,
            [HDFC_PAGE], "HDFC CA xx9876")
    refuses(m, "unbindable_account", m.require_account_match, [HDFC_PAGE], "HDFC CA")


# --------------------------------------------------------------------------- #
# numeric parsing                                                              #
# --------------------------------------------------------------------------- #

def test_amount_parsing_is_strict(m):
    assert m._money("") is None and m._money("  ") is None
    assert m._money("1234.50") == D("1234.50")
    # a third decimal place must not be silently rounded into the XML
    refuses(m, "malformed_amount", m._money, "1.005")
    # nonnumeric text in an amount cell is a parse failure, not a zero: read as
    # zero it lets the balance chain "prove" a row whose amount was never read
    refuses(m, "malformed_amount", m._money, "garbage")
    # a balance may be negative on an overdrawn account
    assert m._balance("-248044.20") == D("-248044.20")
    refuses(m, "malformed_balance", m._balance, "248044.20Dr")


def test_reconcile(m):
    bank = m.HDFC()
    rows = [{"date": "01/08/26", "dr": "", "cr": "100.00", "bal": "1100.00"},
            {"date": "02/08/26", "dr": "50.00", "cr": "", "bal": "1050.00"}]
    assert m.reconcile(rows, bank, "1000.00", "1050.00") == D("1050.00")

    broken = [dict(r) for r in rows]
    broken[1]["bal"] = "1049.00"
    refuses(m, "balance_chain_broken", m.reconcile, broken, bank, "1000.00", "1049.00")

    # a self-consistent prefix reconciles perfectly and is still not the
    # statement: only the printed closing balance catches a truncated parse
    refuses(m, "extent_unproven", m.reconcile, rows[:1], bank, "1000.00", "1050.00")
    refuses(m, "empty_statement", m.reconcile, [], bank, "1000.00", "1000.00")

    # both columns populated is a column-geometry failure that can still satisfy
    # the balance chain, because reconcile nets the two sides
    two_sided = [{"date": "01/08/26", "dr": "50.00", "cr": "150.00", "bal": "1100.00"}]
    refuses(m, "two_sided_row", m.reconcile, two_sided, bank, "1000.00", "1100.00")

    overdrawn = [{"date": "01/08/26", "dr": "1500.00", "cr": "", "bal": "-500.00"}]
    assert m.reconcile(overdrawn, bank, "1000.00", "-500.00") == D("-500.00")


# --------------------------------------------------------------------------- #
# XML shape                                                                    #
# --------------------------------------------------------------------------- #

def test_dewrap(m):
    """A fragment reaching the cell edge was broken mid-token and joins with
    nothing; a shorter one ended at a real space."""
    edge = 100.0
    assert m._dewrap([("UPI/DR/1234", 99.5), ("56789012/X", 60.0)], edge) == "UPI/DR/123456789012/X"
    assert m._dewrap([("NORTH", 40.0), ("WIND", 45.0)], edge) == "NORTH WIND"
    assert m._dewrap([], edge) == ""


def test_escaping(m):
    """9.1b: one unescaped '&' makes the whole request malformed, and
    'Duties & Taxes' is a stock group in every company."""
    assert m.escape("Ram & Sons <Ltd>") == "Ram &amp; Sons &lt;Ltd&gt;"
    xml = m.envelope("A & B", [m.voucher_xml(
        "Payment", datetime.date(2026, 8, 1), "R1", "narr & more",
        "Ram & Sons", "Bank", D("10.00"))])
    assert "&amp;" in xml and " & " not in xml
    import xml.etree.ElementTree as ET
    ET.fromstring(xml)


def test_sign_convention(m):
    """Debit is ISDEEMEDPOSITIVE Yes with a NEGATIVE amount."""
    v = m.voucher_xml("Payment", datetime.date(2026, 8, 1), "R1", "n",
                      "Party", "Bank", D("100.00"), party_ledger="Party")
    assert "<LEDGERNAME>Party</LEDGERNAME>\n    <ISDEEMEDPOSITIVE>Yes</ISDEEMEDPOSITIVE>" \
           "\n    <AMOUNT>-100.00</AMOUNT>" in v
    assert "<LEDGERNAME>Bank</LEDGERNAME>\n    <ISDEEMEDPOSITIVE>No</ISDEEMEDPOSITIVE>" \
           "\n    <AMOUNT>100.00</AMOUNT>" in v
    assert "<DATE>20260801</DATE>" in v and "<EFFECTIVEDATE>20260801</EFFECTIVEDATE>" in v
    # a Contra has no party ledger
    c = m.voucher_xml("Contra", datetime.date(2026, 8, 1), "R2", "n", "Cash", "Bank", D("5.00"))
    assert "PARTYLEDGERNAME" not in c


def test_voucher_with_identical_legs_is_refused(m):
    """It balances, imports cleanly, and moves nothing — the transaction simply
    disappears from the book. Tally's own name matching decides identity."""
    refuses(m, "self_cancelling_voucher", m.voucher_xml,
            "Payment", datetime.date(2026, 8, 1), "R1", "n",
            "HDFC BANK LTD.", "hdfc bank ltd.", D("10.00"))


def test_selfcheck_rejects_bad_xml(m):
    good = m.envelope("Co", [m.voucher_xml("Payment", datetime.date(2026, 8, 1), "R1",
                                           "n", "P", "Bank", D("10.00"))])
    count, out, inward = m.selfcheck(good, "Bank", [{"voucher_type": "Payment"}])
    assert (count, out, inward) == (1, D("10.00"), D(0))
    # each malformation must trip its OWN check, not merely some check
    for broken, category in (
        (good.replace("<AMOUNT>10.00</AMOUNT>", "<AMOUNT>11.00</AMOUNT>"),
         "unbalanced_voucher"),
        (good.replace("<ISDEEMEDPOSITIVE>Yes</ISDEEMEDPOSITIVE>",
                      "<ISDEEMEDPOSITIVE>No</ISDEEMEDPOSITIVE>"), "sign_convention"),
        (good.replace("<EFFECTIVEDATE>20260801</EFFECTIVEDATE>",
                      "<EFFECTIVEDATE>20260802</EFFECTIVEDATE>"), "date_disagreement"),
        (good.replace("<NARRATION>n</NARRATION>", "<NARRATION> </NARRATION>"),
         "empty_narration"),
    ):
        refuses(m, category, m.selfcheck, broken, "Bank", [{"voucher_type": "Payment"}])
    refuses(m, "manifest_count_mismatch", m.selfcheck, good, "Bank",
            [{"voucher_type": "Payment"}] * 2)
    # the bank leg is recognised by Tally's matching rules, not by string equality
    count, out, inward = m.selfcheck(good, "bank", [{"voucher_type": "Payment"}])
    assert out == D("10.00")


# --------------------------------------------------------------------------- #
# mapping                                                                      #
# --------------------------------------------------------------------------- #

def mapping_file(directory, text):
    path = pathlib.Path(directory) / "map.csv"
    path.write_text(text, encoding="utf-8")
    return path


def test_mapping_key_ignores_wrap_spacing(m):
    with tempfile.TemporaryDirectory() as directory:
        path = mapping_file(directory,
                            "party,ledger,treatment\n"
                            "ZEPHYR MANUFACTURING,M/s Zephyr,auto\n"
                            "OWN ACCOUNT,,skip\n")
        mapping = m.load_mapping(path)
    # the wrap heuristic can space the same name two ways; both must map
    for spelling in ("ZEPHYRM ANUFACTURING", "ZEPHYRMANUFACTURING", "Zephyr Manufacturing"):
        assert mapping[m._key(spelling)] == ("M/s Zephyr", "auto"), spelling
    assert mapping[m._key("OWN ACCOUNT")][1] == "skip"


def test_mapping_refuses_ambiguous_input(m):
    with tempfile.TemporaryDirectory() as directory:
        # 'party' missing: every record would be skipped and the run would still
        # report a clean, balanced, entirely suspense-bound import
        refuses(m, "mapping_headers_missing", m.load_mapping,
                mapping_file(directory, "name,ledger,treatment\nA,L,auto\n"))
        # two different counterparties collapsing to one key: last row would win
        refuses(m, "mapping_key_collision", m.load_mapping,
                mapping_file(directory, "party,ledger,treatment\n"
                                        "A & B,Ledger One,auto\n"
                                        "AB,Ledger Two,auto\n"))
        # ... but an identical instruction spelled two ways is not a conflict
        mapping = m.load_mapping(mapping_file(
            directory, "party,ledger,treatment\nA & B,One,auto\nAB,One,auto\n"))
        assert mapping[m._key("AB")] == ("One", "auto")
        # a Contra's other leg must be a real bank/cash ledger
        refuses(m, "contra_without_ledger", m.load_mapping,
                mapping_file(directory, "party,ledger,treatment\nOWN,,contra\n"))
        refuses(m, "unknown_treatment", m.load_mapping,
                mapping_file(directory, "party,ledger,treatment\nA,L,transfer\n"))


# --------------------------------------------------------------------------- #
# voucher construction                                                         #
# --------------------------------------------------------------------------- #

def test_build_treatments(m):
    """skip emits nothing; contra emits a Contra; unmapped falls to suspense."""
    bank = m.HDFC()
    rows = [{"date": "01/08/26", "narr": "UPI-ALPHA-9@x-ABCD0001-111111111111-P", "ref": "1",
             "dr": "10.00", "cr": "", "bal": "990.00"},
            {"date": "02/08/26", "narr": "UPI-OWN ACCT-9@x-ABCD0001-222222222222-P", "ref": "2",
             "dr": "20.00", "cr": "", "bal": "970.00"},
            {"date": "03/08/26", "narr": "UPI-GHOST-9@x-ABCD0001-333333333333-P", "ref": "3",
             "dr": "", "cr": "30.00", "bal": "1000.00"}]
    mapping = {m._key("ALPHA"): ("Alpha Ledger", "contra"),
               m._key("OWN ACCT"): ("", "skip")}
    vouchers, manifest = m.build(rows, bank, "Co", "Bank", "SUSPENSE ACC", mapping, "ACC")
    assert len(vouchers) == 2 and len(manifest) == 3
    assert 'VCHTYPE="Contra"' in vouchers[0] and "PARTYLEDGERNAME" not in vouchers[0]
    assert [r["voucher_type"] for r in manifest] == ["Contra", "SKIPPED", "Receipt"]
    assert manifest[2]["suspense"] == "YES"
    assert "reallocate from Suspense" in manifest[2]["narration"]
    # an unmapped party keeps the statement's own spelling in the narration so it
    # can still be identified later
    assert "GHOST" in manifest[2]["narration"]


def test_suspense_is_compared_the_way_tally_compares_it(m):
    """3.3b: Tally resolves 'suspense-acc' and 'SUSPENSE ACC' to one master. An
    exact compare would post to suspense while reporting the row as resolved and
    dropping the operator's warning — the row would vanish from the suspense
    count it exists to appear in."""
    bank = m.HDFC()
    rows = [{"date": "01/08/26", "narr": "UPI-ALPHA-9@x-ABCD0001-111111111111-P",
             "ref": "1", "dr": "10.00", "cr": "", "bal": "990.00"}]
    mapping = {m._key("ALPHA"): ("suspense-acc", "auto")}
    _, manifest = m.build(rows, bank, "Co", "Bank", "SUSPENSE ACC", mapping, "ACC")
    assert manifest[0]["suspense"] == "YES"
    assert "UNIDENTIFIED" in manifest[0]["narration"]


def test_remoteid_is_derived_from_the_transaction(m):
    """3.3a makes a repeated REMOTEID an upsert, so the key must not depend on
    where the row happened to fall in this particular download."""
    bank = m.HDFC()
    alpha = {"date": "01/08/26", "narr": "UPI-ALPHA-9@x-ABCD0001-111111111111-P",
             "ref": "1", "dr": "10.00", "cr": "", "bal": "990.00"}
    beta = {"date": "01/08/26", "narr": "UPI-BETA-9@x-ABCD0001-222222222222-P",
            "ref": "2", "dr": "20.00", "cr": "", "bal": "970.00"}

    _, one = m.build([alpha, beta], bank, "Co", "Bank", "SUSP", {}, "ACC")
    # the same transaction preceded by an extra row in an overlapping export:
    # its ordinal moves, its identity does not
    earlier = {"date": "31/07/26", "narr": "UPI-GAMMA-9@x-ABCD0001-333333333333-P",
               "ref": "0", "dr": "5.00", "cr": "", "bal": "1000.00"}
    _, two = m.build([earlier, alpha, beta], bank, "Co", "Bank", "SUSP", {}, "ACC")
    assert one[0]["remoteid"] == two[1]["remoteid"], "same transaction, same key"
    assert one[1]["remoteid"] == two[2]["remoteid"]
    # two different transactions at the same date and ordinal must not collide,
    # or the second import silently overwrites the first
    assert one[0]["remoteid"] != one[1]["remoteid"]

    # a genuinely repeated line is caught rather than silently upserted away
    refuses(m, "duplicate_remoteid", m.build,
            [alpha, dict(alpha)], bank, "Co", "Bank", "SUSP", {}, "ACC")


# --------------------------------------------------------------------------- #
# command line                                                                 #
# --------------------------------------------------------------------------- #

def test_output_files_are_owner_only(m):
    """The XML and manifest carry counterparties, amounts and every narration;
    the default 022 umask would publish them as 0644 on a shared host."""
    with tempfile.TemporaryDirectory() as directory:
        path = pathlib.Path(directory) / "out.xml"
        path.write_text("stale", encoding="utf-8")
        os.chmod(path, 0o644)
        m._write_private(path, "<ENVELOPE/>")
        assert path.read_text(encoding="utf-8") == "<ENVELOPE/>"
        assert stat.S_IMODE(path.stat().st_mode) == 0o600


def test_password_is_never_an_argument(m):
    """A password on the command line lands in shell history and in the process
    list, and these are commonly derived from personal identifiers."""
    assert m.read_password({m.PASSWORD_ENV: SENTINEL}) == SENTINEL
    # argparse must reject it outright rather than quietly ignoring it
    try:
        with contextlib.redirect_stderr(io.StringIO()):
            m.main(cli(m) + ["--password", SENTINEL])
    except SystemExit as exit_:
        assert not isinstance(exit_, m.Refusal)
    else:
        raise AssertionError("--password was accepted")
    # with no environment variable and no tty there is nowhere safe to read it
    refuses(m, "no_password", m.read_password, {}, interactive=False)


def cli(m, **overrides):
    args = {"--pdf": "s.pdf", "--bank": "hdfc", "--company": "Co",
            "--confirm-open-company": "Co", "--bank-ledger": "Bank",
            "--account-tail": "xx1234", "--opening": "0", "--expect-closing": "0",
            "--dry-run": True}
    args.update(overrides)
    argv = []
    for flag, value in args.items():
        if value is True:
            argv.append(flag)
        elif value is not None:
            argv += [flag, value]
    return argv


def test_cli_refuses_before_reading_anything(m):
    """Every one of these produced a successful-looking run that wrote nothing
    useful, wrote to the wrong place, or wrote an empty import."""
    refuses(m, "company_unconfirmed", m.main,
            cli(m, **{"--confirm-open-company": "Co Ltd"}))
    refuses(m, "no_output_requested", m.main, cli(m, **{"--dry-run": None}))
    refuses(m, "reversed_date_window", m.main,
            cli(m, **{"--from": "2026-08-31", "--to": "2026-08-01"}))
    refuses(m, "path_collision", m.main,
            cli(m, **{"--dry-run": None, "--out": "s.pdf"}))
    refuses(m, "path_collision", m.main,
            cli(m, **{"--dry-run": None, "--out": "x.csv", "--manifest": "x.csv"}))


def main():
    module = load()
    for name, test in sorted(globals().items()):
        if name.startswith("test_"):
            test(module)
            print(f"ok  {name}")
    print("all offline contract tests passed")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
