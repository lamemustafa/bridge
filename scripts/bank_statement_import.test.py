"""Offline contract tests for the bank-statement importer.

No PDF, no network, no client data. Every fixture is synthetic: the counterparty
names, references, account digits and amounts are invented, and the only thing
carried over from a real statement is the *column geometry*, which is a property
of the bank's template rather than of any customer.

Three levels of fixture, deliberately:

  * **Captures** — `fixtures/*-bbox-capture.xml` are real `pdftotext
    -bbox-layout` output from real statements, sanitised. Every coordinate,
    word break, line break and entity encoding is the producer's; every
    customer value is fabricated. These are the only fixtures that can catch a
    change in the bank's template or in `pdftotext`'s serialisation, because
    they are the only ones this repository did not write. See the banner
    comment in each file, and `../sanitise-bbox-capture.py` for how they
    were made.
  * **Constructed pages** — `PAGE`-shaped fixtures built word by word at the
    same geometry. They exist for cases a capture happens not to contain and
    cannot be made to contain on demand: a row printed below the page footer, a
    stray fragment in a row-scoped column. Losing these would lose the negative
    cases; keeping them alone would prove only that the parser agrees with
    itself.
  * **Row dicts** — for the pure functions downstream of parsing.

Refusals are asserted by *category*, never by "some SystemExit was raised": a
test that only proves an error occurred passes just as happily when an unrelated
error starts firing first, which is exactly how a regression hides.
"""

import contextlib
import datetime
import decimal
import hashlib
import io
import importlib.util
import inspect
import os
import pathlib
import signal
import stat
import subprocess
import sys
import tempfile
import types

SCRIPT = pathlib.Path(__file__).resolve().parent / "bank_statement_import.py"
#: never a credential — only ever compared for identity, or fed to an
#: argument the parser is expected to reject
SENTINEL = "<placeholder-not-a-credential>"
D = decimal.Decimal


def load():
    # No .pyc, ever. A mutation that keeps the file's size — swapping one
    # column bound for another of the same width, say — can leave the mtime
    # granular enough that Python reuses a cached module, and the suite then
    # tests the code you did not write. Several mutations read as "not caught"
    # for exactly that reason before this line existed.
    sys.dont_write_bytecode = True
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
    (52, [(340, 380, "Account"), (382, 396, "No"), (397, 400, ":"),
          (403, 470, "00000000001234")]),
    # a second header number, to prove the binding does not accept just any of
    # them: this is where a customer id or a phone number sits
    (56, [(340, 380, "Cust"), (382, 396, "ID"), (397, 400, ":"),
          (403, 470, "00000000004230")]),
    (60, [(70, 200, "Statement"), (205, 260, "of"), (265, 340, "account")]),
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
           (192, 230, "EXPORTS-MUM-ZZZZZ00000000000-BB"),
           (282, 350, "ZZZZZ00000000000"), (360, 398, "02/08/26"),
           (402, 460, "2,500.50"), (562, 620, "8,499.50")]),
    (170, [(28, 60, "HDFC"), (62, 95, "BANK"), (97, 140, "LIMITED")]),
    # below the footer: must not be read
    (190, [(2, 60, "03/08/26"), (72, 200, "UPI-GHOST-g@z-ZZZZ0001-999999999999-X"),
           (402, 460, "1.00"), (562, 620, "8,498.50")]),
)

# The same HDFC geometry, carrying ACH rows whose narration wraps at the cell
# edge in each of the three places a wrap can land. The point of parsing these
# rather than handing `party()` a narration string is that the **spacing is
# produced by the parser**, not by the test: `narr_spaced` is built in
# `parse_pages` by space-joining a wrapped cell's lines, so a test that writes
# `"...-12345 67890"` itself proves only that the regex matches what the test
# thinks the parser emits. Neither capture contains an ACH narration and one
# cannot be obtained on demand, which is the case the constructed level exists
# for.
ACH_WRAP_HEADER = (
    (52, [(340, 380, "Account"), (382, 396, "No"), (397, 400, ":"),
          (403, 470, "00000000001234")]),
    (100, [(5, 30, "Date"), (72, 120, "Narration"), (282, 340, "Chq./Ref.No."),
           (360, 380, "Value"), (382, 396, "Dt"), (402, 452, "Withdrawal"),
           (454, 474, "Amt."), (482, 522, "Deposit"), (524, 544, "Amt."),
           (562, 600, "Closing"), (602, 640, "Balance")]),
)


def ach_wrap_page(first_line, continuation):
    """One ACH row whose narration wraps onto a second line at the 240 edge."""
    return page(
        *ACH_WRAP_HEADER,
        (120, [(2, 60, "01/08/26"), (72, 238, first_line),
               (282, 350, "0000123456789012"), (360, 398, "01/08/26"),
               (402, 460, "1,000.00"), (562, 620, "9,000.00")]),
        (132, [(72, 200, continuation)]),
        (170, [(28, 60, "HDFC"), (62, 95, "BANK"), (97, 140, "LIMITED")]),
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

def digest(values):
    """A tripwire over every row, not a spot check on the ones I thought to name.

    Both of the defects a capture is here to catch were *in* the capture and
    passed anyway, because the assertions named specific rows or a property too
    weak to separate right from wrong. The wrap heuristic decides per printed
    line whether to insert a space; the party extractor decides where a name
    ends. Neither is checkable one row at a time.

    In particular "no row is UNRESOLVED" is not enough: a name that has run on
    into the next field is resolved, just wrong. Only pinning the values
    catches that.

    When this fails, print the list and read the diff — the digest says
    something changed, never what.
    """
    return hashlib.sha256("\n".join(values).encode("utf-8")).hexdigest()[:16]


def narration_digest(rows):
    return digest(row["narr"] for row in rows)


def party_digest(rows, bank):
    return digest(bank.party(row) for row in rows)


def reference_digest(rows, bank):
    return digest(f"{mode}:{value}" for mode, value in
                  (bank.reference(row) for row in rows))


def capture(name):
    return (pathlib.Path(__file__).resolve().parent / "fixtures" / name
            ).read_text(encoding="utf-8").split("<page ")[1:]


def test_parse_real_hdfc_capture(m):
    """Real geometry, real template, fabricated customer.

    This is the fixture that fails when HDFC changes its statement or poppler
    changes its serialisation. Nothing here was written by this repository
    except the substituted text.
    """
    bank = m.HDFC()
    pages = capture("hdfc-bbox-capture.xml")
    rows = m.parse_pages(pages, bank)
    assert len(rows) == 14, len(rows)

    # every row resolves to a counterparty. On the unsanitised capture this is
    # 32 of 32 with no UNRESOLVED, which is the property that matters: a
    # narration shape the parsers do not recognise silently becomes a suspense
    # voucher, and nothing downstream can tell that apart from a genuinely
    # unidentifiable payer.
    unresolved = [r["narr"] for r in rows if bank.party(r) == "UNRESOLVED"]
    assert not unresolved, unresolved
    # ... and the resolved names are pinned, because a name that has run on into
    # the following field is resolved too. That defect was in this very capture
    # and survived the assertion above; it took running the tool against the
    # unsanitised statement to see it.
    assert party_digest(rows, bank) == "f079dbf8cc126ee0", [bank.party(r) for r in rows]
    assert reference_digest(rows, bank) == "25ddb159d0c3e2b7", \
        [bank.reference(r) for r in rows]

    # a narration wrapped across four printed lines, rejoined in full. Asserted
    # whole rather than by prefix: the wrap heuristic decides, per line, whether
    # to insert a space, and only the complete string pins every one of those
    # decisions.
    assert rows[4]["narr"] == (
        'UPI-ZZZZW ZZZZZK ZZZZZZW-ZZZZZB.ZZZZZK@K ZQ-ZZZZ1111114-111111111113-ZZZZZZV FROMZZZZG')
    assert bank.party(rows[4]) == 'ZZZZW ZZZZZK ZZZZZZW'
    # ... and its 12-digit reference survived the wrap intact
    assert bank.reference(rows[4]) == ('UPI', '111111111113')
    # and every other row's wrap decisions, which no readable assertion reaches
    assert narration_digest(rows) == "4c1a76b6a6f582c5", [r["narr"] for r in rows]

    # row-scoped columns land where the geometry says, not one column over
    assert rows[0]["ref"] == '1111111111111111'
    assert rows[0]["vdt"] == "04/08/26"

    # row-scoped columns: every row has exactly one amount side and a balance
    for index, row in enumerate(rows, 1):
        assert bool(row["dr"]) != bool(row["cr"]), (index, row["dr"], row["cr"])
        assert row["bal"], index
        assert bank.parse_date(row["date"]).year == 2026, index

    # page 2 ends at STATEMENT SUMMARY and page 3 is never read. Page 3 carries a
    # summary line below its own top anchor with amounts and no date, so without
    # the end anchor it is appended to the last row as a phantom narration.
    assert len(pages) == 3
    assert all("SUMMARY" not in r["narr"] for r in rows)
    assert not rows[-1]["narr"].endswith(" "), rows[-1]["narr"]
    assert m.parse_pages(pages[:2], bank) == rows, "page 3 must contribute nothing"

    # the account number is bound from the header block, not from the table
    m.require_account_match(pages, bank, "HDFC CA xx1111")
    # 1112 is the captured MICR tail, not an account-number value. 1113-1115
    # occur in captured transaction-table references, a separate negative
    # case: a table reference must not stand in for the account. 9876 is not
    # printed in the capture.
    for wrong in ("xx1112", "xx1113", "xx1114", "xx1115", "xx9876"):
        refuses(m, "account_not_in_statement", m.require_account_match,
                pages, bank, f"HDFC CA {wrong}")


def test_real_hdfc_capture_binds_the_account_no_geometry_only(m):
    """The unchanged capture pins the header field, not a convenient tail.

    Customer values are sanitised, so several header numbers intentionally end
    alike.  Its captured labels and coordinates still prove which field the
    production selector reads.  A postcode label is not present in this
    capture; this test makes no claim about an absent field.
    """
    bank = m.HDFC()
    pages = capture("hdfc-bbox-capture.xml")

    selected = [
        [(round(x0, 3), round(y0, 3), round(x1, 3), round(y1, 3), text)
         for x0, y0, x1, y1, text in group
         if text in {"Account", "No"}]
        for _, group in m._lines(pages[0])
        if m._matches(group, bank.account_anchors)
    ]
    assert selected == [[
        (340.157, 149.001, 367.261, 156.201, "Account"),
        (369.261, 149.001, 379.037, 156.201, "No"),
    ]], selected
    account = m.require_account_match(pages, bank, "xx1111111")
    assert account == "11111111111111"

    # Mutation controls select existing captured header geometry.  They prove
    # that the production Account/No selector excludes phone, customer-id,
    # IFSC and MICR rows even where their sanitised numeric tails overlap.
    original = bank.account_anchors
    try:
        bank.account_anchors = (("Phone", "no."),)
        assert m.require_account_match(pages, bank, "xx1112") == "11111112"

        bank.account_anchors = (("Cust", "ID"),)
        assert m.require_account_match(pages, bank, "xx111111111") == "111111111"

        bank.account_anchors = (("RTGS/NEFT", "IFSC"),)
        assert m.require_account_match(pages, bank, "xx1111111") == "1111111"

        bank.account_anchors = (("MICR",),)
        assert m.require_account_match(pages, bank, "xx1112") == "111111112"
    finally:
        bank.account_anchors = original


def test_parse_real_sbi_capture(m):
    """SBI stacks the date over the year, repeats a three-line column header on
    every page, and wraps the narration mid-token across five lines. All three
    are here as the producer emitted them."""
    bank = m.SBI()
    pages = capture("sbi-bbox-capture.xml")
    rows = m.parse_pages(pages, bank)
    assert len(rows) == 3, len(rows)
    assert not [r for r in rows if bank.party(r) == "UNRESOLVED"]
    assert party_digest(rows, bank) == "9182a433650d104c", [bank.party(r) for r in rows]
    assert reference_digest(rows, bank) == "52f8f2b555195dae", \
        [bank.reference(r) for r in rows]

    for row in rows:
        # "31 Jul" over "2026" in one cell, concatenated without a separator
        assert bank.parse_date(row["date"]) == datetime.date(2026, 7, 1)
        # the repeated header did not land in the row in progress
        for furniture in ("Description", "No./Cheque", "Balance"):
            assert furniture not in row["narr_spaced"], furniture

    # the reference is space-tolerant because the producer breaks it mid-token
    assert bank.reference(rows[0])[0] == "UPI"
    assert bank.reference(rows[0])[1].startswith('111111111111')
    assert narration_digest(rows) == "949f0d93d6c532ab", [r["narr"] for r in rows]
    assert rows[0]["ref"] == 'TRANSFER TO 1111111111203 /'
    m.require_account_match(pages, bank, "SBI CA xx1111")
    refuses(m, "account_not_in_statement", m.require_account_match,
            pages, bank, "SBI CA xx9876")


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
    assert second["narr"] == "NEFT DR-ZZZZ0000001-ACME EXPORTS-MUM-ZZZZZ00000000000-BB"
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
    statement, so the binding is the only thing tying the document to the ledger.

    It reads the line the statement labels as its account number, and nothing
    else. Reading the whole document lets a transaction reference stand in for
    the account; reading the whole header block is barely better because it
    includes non-account identifiers. The captured HDFC contract below keeps
    that evidence tied to the bank-produced geometry.
    """
    hdfc = m.HDFC()
    m.require_account_match([HDFC_PAGE], hdfc, "HDFC CA xx1234")
    refuses(m, "account_not_in_statement", m.require_account_match,
            [HDFC_PAGE], hdfc, "HDFC CA xx9876")
    refuses(m, "unbindable_account", m.require_account_match, [HDFC_PAGE], hdfc, "HDFC CA")

    # a transaction reference inside the table must not satisfy the binding —
    # 9012 ends the UPI reference on row 1
    refuses(m, "account_not_in_statement", m.require_account_match,
            [HDFC_PAGE], hdfc, "HDFC CA xx9012")
    # nor may the other constructed header value; fixture-specific account
    # binding against real header geometry remains in test_parse_real_hdfc_capture.
    refuses(m, "account_not_in_statement", m.require_account_match,
            [HDFC_PAGE], hdfc, "HDFC CA xx4230")
    # and a document with no account-number line fails closed
    refuses(m, "no_account_number_line", m.require_account_match,
            [page((10, [(2, 60, "nothing")]))], hdfc, "HDFC CA xx1234")


def test_account_identity_comes_from_the_statement(m):
    """The REMOTEID is keyed on the account number the statement prints, not on
    the operator's label.

    A label is free-form: `HDFC CA xx1234`, `HDFC xx1234` and `xx001234` all name
    one account and reduce to three different strings. Any of them keying the
    digest gives every transaction a new REMOTEID, and re-importing duplicates
    the whole statement — the failure the digest exists to prevent, reached
    through the label.
    """
    bank = m.HDFC()
    pages = capture("hdfc-bbox-capture.xml")
    # different valid tails, same account, same returned number
    numbers = {m.require_account_match(pages, bank, tail)
               for tail in ("HDFC CA xx1111", "xx11111", "1111111111111")}
    assert numbers == {"11111111111111"}, numbers

    row = {"date": "01/08/26", "narr": "UPI-ALPHA-9@x-ABCD0001-111111111111-P",
           "ref": "1", "dr": "10.00", "cr": "", "bal": "990.00"}
    keys = {m.build([row], bank, "Co", "Bank", "SUSP", {}, tail,
                    account=m.require_account_match(pages, bank, tail))[1][0]["remoteid"]
            for tail in ("HDFC CA xx1111", "xx11111", "1111111111111")}
    assert len(keys) == 1, keys

    # a tail short enough to match two numbers on that line is refused rather
    # than resolved to whichever came first. HDFC prints a product code beside
    # the account number, so two numbers on that line is the documented residual
    # of this binding rather than a hypothetical.
    refuses(m, "unbindable_account", m.require_account_match, pages, bank, "xx55")
    two_numbers = page(
        (52, [(340, 380, "Account"), (382, 396, "No"), (397, 400, ":"),
              (403, 470, "00000000001234"), (474, 520, "99001234")]),
        (100, [(5, 30, "Date"), (72, 120, "Narration")]),
    )
    refuses(m, "ambiguous_account_match", m.require_account_match,
            [two_numbers], bank, "xx1234")
    # ... and a tail long enough to pick one of them is accepted
    assert m.require_account_match([two_numbers], bank, "xx0000001234") == "00000000001234"


# --------------------------------------------------------------------------- #
# numeric parsing                                                              #
# --------------------------------------------------------------------------- #

def test_hyphenated_counterparties_survive_every_narration_shape(m):
    """Every HDFC narration field is hyphen-delimited, so a counterparty called
    ACME-INDUSTRIES occupies two fields. Cutting at the first hyphen either
    misses its mapping or silently posts to an unrelated ledger called ACME."""
    hdfc = m.HDFC()
    for narration, expected in (
        # UPI: bounded by the VPA
        ("UPI-ACME-INDUSTRIES-acme@ok-HDFC0001-123456789012-P", "ACME-INDUSTRIES"),
        # UPI without a VPA: bounded by the 12-digit reference, less the bank code
        ("UPI-XXXXXX0000-ZZZZ0000001-888888888888-PAYMENT", "UNNAMED"),
        # IMPS: bounded by the masked account. NOT by the four-letter bank code,
        # which a name component can also be (ACME is four capitals too).
        ("IMPS-999999999999-ACME INDUSTRIES-ZZZZ-XXXXXXXX0000-US", "ACME INDUSTRIES"),
        ("IMPS-999999999999-ACME-INDUSTRIES-ZZZZ-XXXXXXXX0000-US", "ACME-INDUSTRIES"),
        # NEFT: bounded by the UTR, skipping the leading IFSC — which has the
        # same shape as the UTR and would otherwise terminate the name at once
        ("NEFT DR-ZZZZ0000000-ACME INTL-MUM-ZZZZZ00000000000-BB", "ACME INTL"),
        ("NEFT DR-ZZZZ0000000-ACME-INTL-MUM-ZZZZZ00000000000-BB", "ACME-INTL"),
        # a long all-capitals name is not a reference: the UTR test requires
        # digits, or INTERNATIONAL terminates its own name
        ("NEFT CR-ZZZZ0000000-INTERNATIONAL-MUM-ZZZZZ00000000000-B", "INTERNATIONAL"),
        # the cell wrap lands inside the UTR itself, so the marker arrives
        # split. Found against a real statement, where a two-character payee
        # name absorbed the branch field because the boundary went unrecognised.
        ("NEFT DR-ZZZZ0ZZZZZZ-GST-MUM-ZZZ ZZ00000000000-BB0", "GST"),
        # shapes the rules were not written for go to suspense, never to a guess
        ("UPI-NOSTRUCTURE-HERE", "UNRESOLVED"),
        ("IMPS-1-WEIRD", "UNRESOLVED"),
        ("NEFT DR-ONLY-TWO", "UNRESOLVED"),
    ):
        assert hdfc.party({"narr": narration}) == expected, narration


def test_control_values_take_the_operator_at_their_word(m):
    """These are copied off a printed page, so they arrive with separators."""
    assert m.control_value("1,00,000.00", "--opening") == D("100000.00")
    assert m.control_value("-248044.20", "--expect-closing", signed=True) == D("-248044.20")
    # a total of withdrawals may not be negative
    refuses(m, "malformed_control_value", m.control_value, "-1.00", "--expect-debits")
    refuses(m, "malformed_control_value", m.control_value, "1.005", "--opening")
    refuses(m, "malformed_control_value", m.control_value, "abc", "--opening")


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


def test_closing_balance_alone_cannot_prove_extent(m):
    """The closing balance is the NET, so a dropped tail whose two sides cancel
    lands on the printed figure and the parse looks complete. Only the printed
    debit and credit totals see it — which is why they are required."""
    bank = m.HDFC()
    full = [{"date": "01/08/26", "dr": "", "cr": "100.00", "bal": "1100.00"},
            {"date": "02/08/26", "dr": "50.00", "cr": "", "bal": "1050.00"},
            {"date": "03/08/26", "dr": "", "cr": "50.00", "bal": "1100.00"}]
    truncated = full[:1]
    # both close at 1100.00, so reconcile cannot tell them apart
    assert m.reconcile(full, bank, "1000.00", "1100.00") == D("1100.00")
    assert m.reconcile(truncated, bank, "1000.00", "1100.00") == D("1100.00")
    # the debit total does: 50.00 against the truncated parse's 0
    assert sum(m._money(r["dr"], "dr", i) or D(0) for i, r in enumerate(full, 1)) == D("50.00")
    assert sum(m._money(r["dr"], "dr", i) or D(0)
               for i, r in enumerate(truncated, 1)) == D(0)


def test_impossible_dates_are_typed(m):
    bank = m.HDFC()
    rows = [{"date": "31/02/26", "narr": "UPI-A-9@x-ABCD0001-111111111111-P",
             "ref": "1", "dr": "10.00", "cr": "", "bal": "990.00"}]
    refuses(m, "unparseable_date", m.build, rows, bank, "Co", "Bank", "SUSP", {}, "AC1234")


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


def test_mapping_key_survives_non_ascii_scripts(m):
    """An ASCII-only key reduces a name written entirely in Devanagari, Tamil or
    Bengali to the empty string, so every such party shares one key and a book
    with two of them posts both to whichever was mapped first — with no
    collision left for `load_mapping` to refuse. The demo company this project
    reads carries ledgers in all three scripts."""
    assert m._key("पार्टी")
    # distinct names stay distinct, including ones differing only in their
    # vowel signs — dropping combining marks would be the same bug one layer down
    names = ["पार्टी", "पारटी", "ஏபிசி", "কোম্পানি"]
    assert len({m._key(n) for n in names}) == len(names)


def test_mapping_key_keeps_punctuation_significant(m):
    """`_key` folds whitespace and nothing else.

    This assertion used to read `_key("A & B") == _key("AB") == "AB"` — the
    defect written down as a contract. Dropping punctuation collapsed genuinely
    different names onto one mapping row, and while `load_mapping` refuses two
    *mapping rows* that collide, nothing refuses a **statement** party colliding
    with a row written for somebody else: one candidate, no ambiguity to reject,
    and the transaction posts to a ledger the operator never chose for it.

    Removing it was measured rather than assumed. Across the 23 distinct parties
    in the delivered manifests, none carried punctuation the old key dropped,
    and the one real merge — the cell-wrap case below — is unaffected.
    """
    apart = [("A & B", "AB"), ("S.K. Minerals", "SK Minerals"),
             ("M/s Mercury", "Ms Mercury"), ("Shree-Ram Traders", "Shree Ram Traders")]
    for left, right in apart:
        assert m._key(left) != m._key(right), f"{left!r} and {right!r} must stay apart"

    # ...while the reason the key is loose at all still holds: one payee, split
    # two ways by the PDF cell wrap, measured on the delivered HDFC statement.
    assert m._key("MERCURYM ANUFACTURERS") == m._key("MERCURYMANUFACTURERS")
    assert m._key("ZEPHYR MANUFACTURING") == m._key("ZEPHYRMANUFACTURING")

    # Case still folds, and nothing else is touched.
    assert m._key("m/s mercury") == m._key("M/S MERCURY")
    assert m._key("A&B") == "A&B"


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
        # two different counterparties collapsing to one key: last row would win.
        # They must collide under the key `_key` actually computes — spacing
        # only. `A & B` / `AB` was the old example and no longer collides,
        # which is the point of the change, not a gap here.
        refuses(m, "mapping_key_collision", m.load_mapping,
                mapping_file(directory, "party,ledger,treatment\n"
                                        "ZEPHYR MANUFACTURING,Ledger One,auto\n"
                                        "ZEPHYRMANUFACTURING,Ledger Two,auto\n"))
        # ... but an identical instruction spelled two ways is not a conflict
        mapping = m.load_mapping(mapping_file(
            directory, "party,ledger,treatment\n"
                       "ZEPHYR MANUFACTURING,One,auto\nZEPHYRMANUFACTURING,One,auto\n"))
        assert mapping[m._key("ZEPHYRMANUFACTURING")] == ("One", "auto")
        # and two names that differ only in punctuation are now simply two rows
        two = m.load_mapping(mapping_file(
            directory, "party,ledger,treatment\nA & B,One,auto\nAB,Two,auto\n"))
        assert two[m._key("A & B")] == ("One", "auto")
        assert two[m._key("AB")] == ("Two", "auto")
        # a Contra's other leg must be a real bank/cash ledger
        refuses(m, "contra_without_ledger", m.load_mapping,
                mapping_file(directory, "party,ledger,treatment\nOWN,,contra\n"))
        refuses(m, "unknown_treatment", m.load_mapping,
                mapping_file(directory, "party,ledger,treatment\nA,L,transfer\n"))
        # `---` used to reduce to an empty key and was refused for it. With the
        # key folding whitespace only it is an ordinary name, and the empty-key
        # case is unreachable — `_squash` drops a whitespace-only party first.
        assert m.load_mapping(
            mapping_file(directory, "party,ledger,treatment\n---,L,auto\n"))[m._key("---")] \
            == ("L", "auto")
        # two columns normalising to one name: the later silently wins, and if
        # it is blank the row is skipped and its transactions fall to suspense
        refuses(m, "mapping_headers_duplicated", m.load_mapping,
                mapping_file(directory, "party,Party,ledger,treatment\nA,,L,auto\n"))
        # capitalised or padded headers pass the header check; the records must
        # be normalised too, or every row reads empty and the whole mapping is
        # discarded into suspense while the run reports success
        mapping = m.load_mapping(mapping_file(
            directory, " Party , Ledger , Treatment \nAcme,Acme Ledger,auto\n"))
        assert mapping[m._key("ACME")] == ("Acme Ledger", "auto")


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
    # a skipped row still records its REMOTEID: if the statement was already
    # imported before the mapping said skip, that key is the only way to remove
    # the voucher still standing in the book. Omission is not deletion.
    assert manifest[1]["remoteid"]
    assert manifest[2]["suspense"] == "YES"
    # names the suspense ledger the operator configured, not the word "Suspense"
    assert "reallocate from SUSPENSE ACC" in manifest[2]["narration"]
    # an unmapped party keeps the statement's own spelling in the narration so it
    # can still be identified later
    assert "GHOST" in manifest[2]["narration"]


def test_a_row_landing_in_suspense_is_flagged_loosely_and_named_exactly(m):
    """Two separate contracts, and the second is the one that was wrong.

    **Flagging is loose on purpose.** A mapping naming the suspense ledger in a
    different spelling must still raise the operator's warning; an exact compare
    would report the row as resolved and drop it from the suspense count it
    exists to appear in. Over-flagging costs a look.

    **The message must name the ledger actually written.** This used to say
    "reallocate from Suspense" unconditionally, which is an instruction that
    cannot be followed when the fold over-flags: `_ledger_key` is looser than
    §9.4b in six ways, so a mapping to `A-B` against a suspense master named
    `A B` is flagged while the voucher is posted to `A-B`. The operator was sent
    to search a ledger the voucher had never been in. Naming the real
    destination makes a false positive cost a look rather than a wrong search.

    This test no longer claims Tally resolves the two spellings to one master.
    §9.4b marks that direction UNVERIFIED, and nothing offline can know it.
    """
    bank = m.HDFC()
    rows = [{"date": "01/08/26", "narr": "UPI-ALPHA-9@x-ABCD0001-111111111111-P",
             "ref": "1", "dr": "10.00", "cr": "", "bal": "990.00"}]
    mapping = {m._key("ALPHA"): ("suspense-acc", "auto")}
    _, manifest = m.build(rows, bank, "Co", "Bank", "SUSPENSE ACC", mapping, "ACC")
    assert manifest[0]["suspense"] == "YES"
    assert "UNIDENTIFIED" in manifest[0]["narration"]
    # the ledger the voucher was actually written to, not the word "Suspense"
    assert "reallocate from suspense-acc" in manifest[0]["narration"], manifest[0]["narration"]
    assert manifest[0]["dr_ledger"] == "suspense-acc"

    # the over-flagging case the old wording could not describe: an unverified
    # fold equates the mapped ledger with the suspense master, and the voucher
    # goes to the mapped one
    mapping = {m._key("ALPHA"): ("A-B", "auto")}
    _, manifest = m.build(rows, bank, "Co", "Bank", "A B", mapping, "ACC")
    assert manifest[0]["suspense"] == "YES"
    assert "reallocate from A-B" in manifest[0]["narration"], manifest[0]["narration"]
    assert manifest[0]["dr_ledger"] == "A-B"


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

def test_hard_links_are_the_same_file(m):
    """`--out` naming a second hard link to the input PDF destroys the statement
    on the O_TRUNC, and two links to one inode keep different names, so a
    lexical path compare passes."""
    with tempfile.TemporaryDirectory() as directory:
        source = pathlib.Path(directory) / "statement.pdf"
        source.write_text("pdf", encoding="utf-8")
        link = pathlib.Path(directory) / "other-name.pdf"
        os.link(source, link)
        args = m.build_parser().parse_args(
            cli(m, **{"--pdf": str(source), "--dry-run": None, "--out": str(link)}))
        refuses(m, "path_collision", m.preflight, args)


def test_empty_selection_is_not_a_successful_import(m):
    """An ordered window that does not overlap the statement selects nothing,
    and every downstream check accepts an empty file."""
    bank = m.HDFC()
    rows = [{"date": "01/08/26", "narr": "UPI-A-9@x-ABCD0001-111111111111-P",
             "ref": "1", "dr": "10.00", "cr": "", "bal": "990.00"}]
    refuses(m, "empty_selection", m.build, rows, bank, "Co", "Bank", "SUSP", {}, "AC1234",
            date_from=datetime.date(2025, 1, 1), date_to=datetime.date(2025, 12, 31))


def test_a_transaction_naming_the_bank_is_not_the_footer(m):
    """The footer anchor is a subset test, so any line carrying its words ends
    the page — including a payment whose counterparty is the bank itself, which
    drops that row and every row after it.

    The guard is that a line opening a transaction is a transaction whatever
    else it says, so the date wins and the anchor only breaks ties.

    The tokenisation below is contrived: these narrations join fields with
    hyphens, so a payee "HDFC BANK LIMITED" usually tokenises as "…-HDFC",
    "BANK", "LIMITED-…" and only the middle word is bare. A bare trailing-hyphen
    token followed by a space does occur (the real capture carries "ACH D- TP
    ACH …"), so this is reachable rather than impossible — and the guard costs
    one comparison either way.
    """
    bank = m.HDFC()
    tricky = page(
        (100, [(5, 30, "Date"), (72, 120, "Narration"), (282, 340, "Chq./Ref.No."),
               (402, 452, "Withdrawal"), (562, 600, "Closing")]),
        (120, [(2, 60, "01/08/26"), (72, 110, "TPT-"), (112, 150, "HDFC"),
               (152, 185, "BANK"), (187, 230, "LIMITED"),
               (282, 350, "0000000000000001"), (402, 460, "10.00"),
               (562, 620, "990.00")]),
        (140, [(2, 60, "02/08/26"), (72, 200, "UPI-BETA-b@z-ZZZZ1-222222222222-P"),
               (282, 350, "0000000000000002"), (402, 460, "20.00"),
               (562, 620, "970.00")]),
        (170, [(28, 60, "HDFC"), (62, 95, "BANK"), (97, 140, "LIMITED")]),
        (190, [(2, 60, "03/08/26"), (72, 200, "UPI-GHOST-g@z-ZZZZ1-333333333333-X"),
               (402, 460, "1.00"), (562, 620, "969.00")]),
    )
    rows = m.parse_pages([tricky], bank)
    assert len(rows) == 2, [r["narr"] for r in rows]
    assert "HDFC BANK LIMITED" in rows[0]["narr_spaced"]
    # the real footer, printed from the left margin, still ended the page
    assert "GHOST" not in " ".join(r["narr"] for r in rows)


def test_dry_run_groups_by_mapping_key(m):
    """The dry run is the list the operator writes the mapping from, so it must
    group the way the mapping is read.

    One counterparty reaches it under two spellings whenever the wrap heuristic
    decides differently on two lines — a real statement showed one payee as both
    `MERCURY MANUFACTURERS` and `MERCURY M ANUFACTURERS`. Those are one mapping
    row, and listing them as two invites the operator to write two rows and then
    wonder why the second never fires.
    """
    manifest = [
        m._manifest_row(party="MERCURY MANUFACTURERS", voucher_type="Receipt",
                        amount="60000.00", suspense="YES"),
        m._manifest_row(party="MERCURY M ANUFACTURERS", voucher_type="Receipt",
                        amount="220000.00", suspense="YES"),
        m._manifest_row(party="AMBIKA INDUSTRIES", voucher_type="Receipt",
                        amount="1000.00", suspense="YES"),
    ]
    printed = io.StringIO()
    with contextlib.redirect_stdout(printed):
        m._print_dry_run(manifest)
    report = printed.getvalue()

    # one row for the two spellings, carrying their combined total
    assert "280,000.00" in report
    assert "220,000.00" not in report and "60,000.00" not in report
    # ... labelled with the spelling the bank printed. A wrap can only insert a
    # space, never remove one, so the lower word count is the original — even
    # though the other spelling carries more value.
    lines = [line for line in report.splitlines() if line.startswith("MERCURY")]
    assert lines and lines[0].startswith("MERCURY MANUFACTURERS"), lines
    # and the variant is still named, so it stays findable in the statement
    assert "also printed as MERCURY M ANUFACTURERS" in report
    assert "AMBIKA INDUSTRIES" in report


def test_output_files_are_owner_only(m):
    """The XML and manifest carry counterparties, amounts and every narration;
    the default 022 umask would publish them as 0644 on a shared host."""
    with tempfile.TemporaryDirectory() as directory:
        path = pathlib.Path(directory) / "out.xml"
        path.write_text("stale", encoding="utf-8")
        os.chmod(path, 0o644)
        m.write_outputs([(path, "<ENVELOPE/>")])
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
            "--expect-debits": "0", "--expect-credits": "0", "--dry-run": True}
    args.update(overrides)
    argv = []
    for flag, value in args.items():
        if value is True:
            argv.append(flag)
        elif value is not None:
            argv += [flag, value]
    return argv


def test_preflight_runs_before_the_pdf_is_opened(m):
    """Split out of main so it is reachable on its own: a mistyped flag should
    cost nothing, and none of these checks needs the statement."""
    parser = m.build_parser()
    args = parser.parse_args(cli(m, **{"--from": "2026-08-01", "--to": "2026-08-31"}))
    window, expected = m.preflight(args)
    assert window == (datetime.date(2026, 8, 1), datetime.date(2026, 8, 31))
    assert set(expected) == {"opening", "closing", "debits", "credits"}
    assert expected["opening"] == D("0")
    # a window open at either end is legal — it means "no bound on that side"
    assert m.preflight(parser.parse_args(cli(m)))[0] == (None, None)


def test_control_totals_are_all_three_compared(m):
    bank = m.HDFC()
    rows = [{"date": "01/08/26", "dr": "", "cr": "100.00", "bal": "1100.00"},
            {"date": "02/08/26", "dr": "50.00", "cr": "", "bal": "1050.00"}]
    expected = {"debits": D("50.00"), "credits": D("100.00")}
    assert m.verify_against_statement(rows, bank, expected) == expected
    for wrong in ({"debits": D("60.00"), "credits": D("100.00")},
                  {"debits": D("50.00"), "credits": D("110.00")}):
        refuses(m, "control_total_mismatch", m.verify_against_statement, rows, bank, wrong)


def test_manifest_rows_always_carry_every_column(m):
    """csv.DictWriter raises on an unexpected key — after the XML is written."""
    row = m._manifest_row(row=1, voucher_type="Payment")
    assert set(row) == set(m.MANIFEST_COLUMNS)
    assert row["narration"] == "" and row["row"] == 1


def test_cli_refuses_before_reading_anything(m):
    """Every one of these produced a successful-looking run that wrote nothing
    useful, wrote to the wrong place, or wrote an empty import."""
    refuses(m, "company_unconfirmed", m.main,
            cli(m, **{"--confirm-open-company": "Co Ltd"}))
    # two unset shell variables agree with each other. An empty company name is
    # the most dangerous value this flag can take, not the most harmless:
    # 9.11d means Tally imports into whichever company is open.
    refuses(m, "company_blank", m.main,
            cli(m, **{"--company": "  ", "--confirm-open-company": "  "}))
    refuses(m, "malformed_cli_date", m.main, cli(m, **{"--from": "2026-02-30"}))
    refuses(m, "malformed_cli_date", m.main, cli(m, **{"--to": "not-a-date"}))
    refuses(m, "no_output_requested", m.main, cli(m, **{"--dry-run": None}))
    refuses(m, "reversed_date_window", m.main,
            cli(m, **{"--from": "2026-08-31", "--to": "2026-08-01"}))
    refuses(m, "path_collision", m.main,
            cli(m, **{"--dry-run": None, "--out": "s.pdf"}))
    refuses(m, "path_collision", m.main,
            cli(m, **{"--dry-run": None, "--out": "x.csv", "--manifest": "x.csv"}))


def test_ach_party_ends_at_the_final_bank_reference(m):
    """A non-greedy boundary stopped at the first hyphen followed by digits, so
    `STUDIO-54 INDUSTRIES` resolved to `STUDIO` — and a mapping for `STUDIO`
    then silently posts an unrelated counterparty to that ledger. Hyphenated
    numbers inside a name are ordinary."""
    def party(narr):
        return m.HDFC.party(m.HDFC, {"narr": narr, "narr_spaced": narr})
    assert party("ACH D- TP ACH STUDIO-54 INDUSTRIES-1234567890") == "STUDIO-54 INDUSTRIES"
    assert party("ACH D- TP ACH ACME TRADERS-1234567890") == "ACME TRADERS"
    # The reference is still the delimiter, not part of the name.
    assert "1234567890" not in party("ACH D- TP ACH UNIT-7 METALS-1234567890")

    # This branch reads `narr_spaced`, which keeps the PDF's spacing, and a cell
    # wrap lands wherever the column edge falls — inside the reference as
    # readily as between fields, the same way `HDF CH12345678901` wraps in the
    # UTR branch. Anchoring on `\d+$` made every wrapped reference UNRESOLVED,
    # which the earlier over-greedy pattern had handled: the first fix for the
    # boundary traded one failure for another.
    assert party("ACH D- TP ACH ACME TRADERS-12345 67890") == "ACME TRADERS"
    assert party("ACH D- TP ACH STUDIO-54 INDUSTRIES-12345 67890") == "STUDIO-54 INDUSTRIES"
    assert party("ACH D- TP ACH UNIT-7 METALS-12 345 67890") == "UNIT-7 METALS"


def test_a_wrapped_ach_reference_survives_the_parser(m):
    """The assertions above hand `party()` a narration the *test* spaced, so on
    their own they prove only that the regex matches what the test believes the
    parser emits. Here the spacing is the parser's: the row is bbox words at the
    real column geometry, `parse_pages` splits the narration cell at the 240
    edge and space-joins the lines into `narr_spaced`, and only then does
    `party()` see it.

    Measured through `parse_pages`, a wrap lands in three distinguishable
    places, and the two readings of the cell disagree about which is right:

        wrap inside the reference   narr_spaced 'ACME TRADERS-12345 67890'
        wrap between name words     narr        'NORTHWINDTRADERS-1234567890'  (welded)
        wrap inside a name word     narr_spaced 'NORTHWIND TRAD ERS-...'       (split)

    The first two are why this branch reads `narr_spaced` *and* why the
    reference pattern has to tolerate internal spaces: `narr` keeps the
    reference intact but welds two words of a name together, so neither reading
    alone resolves both rows. The third is a residual — a wrap inside a single
    word leaves a space `narr_spaced` cannot distinguish from a real one — and
    it is asserted here so it is a recorded limitation rather than a surprise.
    """
    def one(first, continuation):
        rows = m.parse_pages([ach_wrap_page(first, continuation)], m.HDFC())
        assert len(rows) == 1, rows
        return rows[0]

    # 1. the wrap falls inside the reference
    row = one("ACH D- TP ACH ACME TRADERS-12345", "67890")
    assert row["narr_spaced"] == "ACH D- TP ACH ACME TRADERS-12345 67890", row
    assert m.HDFC.party(m.HDFC, row) == "ACME TRADERS"
    # the de-wrapped reading is the one that keeps the reference whole
    assert row["narr"] == "ACH D- TP ACH ACME TRADERS-1234567890", row

    # 2. the wrap falls between two words of the name — `narr` welds them, which
    #    is why this branch cannot simply read `narr` and anchor on `\d+$`
    row = one("ACH D- TP ACH NORTHWIND", "TRADERS-1234567890")
    assert row["narr"] == "ACH D- TP ACH NORTHWINDTRADERS-1234567890", row
    assert m.HDFC.party(m.HDFC, row) == "NORTHWIND TRADERS"

    # 3. residual: a wrap inside one word of the name leaves a space that
    #    `narr_spaced` cannot tell from a real one. Recorded, not fixed — the
    #    reading that would get this right is the one that fails case 2.
    row = one("ACH D- TP ACH NORTHWIND TRAD", "ERS-1234567890")
    assert m.HDFC.party(m.HDFC, row) == "NORTHWIND TRAD ERS", row


def test_a_zero_in_one_amount_column_is_still_two_sided(m):
    """`if debit and credit` asked whether both were **non-zero**. A row filling
    both columns with one of them `0.00` is a column-geometry failure, and it
    used to pass: the balance replay still matched, so `build()` emitted a
    voucher from a structurally invalid row."""
    rows = [{"date": "01/08/26", "narr": "x", "ref": "1",
             "dr": "0.00", "cr": "10.00", "bal": "10.00"}]
    refusal = refuses(m, "two_sided_row", m.reconcile, rows, m.HDFC, "0", "10.00")
    assert "both amount columns" in str(refusal)


def test_a_mapping_may_not_claim_the_unresolved_sentinel(m):
    """`UNRESOLVED` is what `party()` prints when it could not identify anyone.
    Every unrecognised narration shape reports the same word, so one mapping row
    for it would gather unrelated transactions into one ledger — or, with
    `skip`, drop all of them — instead of letting them reach suspense where an
    operator can see them."""
    with tempfile.TemporaryDirectory() as directory:
        for sentinel in sorted(m.PARSER_SENTINELS):
            path = mapping_file(directory, f"party,ledger,treatment\n{sentinel},Some Ledger,auto\n")
            refusal = refuses(m, "mapping_claims_a_sentinel", m.load_mapping, path)
            assert "could NOT identify" in str(refusal)
        # and the name is matched the way every other party name is
        path = mapping_file(directory, "party,ledger,treatment\nun resolved,L,skip\n")
        refuses(m, "mapping_claims_a_sentinel", m.load_mapping, path)
        # an ordinary name that merely contains the word is fine
        path = mapping_file(directory, "party,ledger,treatment\nUNRESOLVED TRADING CO,L,auto\n")
        assert m.load_mapping(path)


def test_a_blank_bank_ledger_is_refused_by_the_parser(m):
    """`required=True` asserts the flag was given, not that it says anything.
    An empty value reached every voucher as an empty `<LEDGERNAME>`, and
    `selfcheck` compared it against the same empty argument and agreed."""
    parser = m.build_parser()
    for blank in ("", "   ", "\t"):
        try:
            parser.parse_args(cli(m, **{"--bank-ledger": blank}))
        except SystemExit:
            pass
        else:
            raise AssertionError(f"--bank-ledger {blank!r} was accepted")
    assert parser.parse_args(cli(m, **{"--bank-ledger": "HDFC Bank"}))


def test_case_only_output_aliases_are_caught_once_the_xml_exists(m):
    """On a case-insensitive volume `--out Result.xml --manifest result.XML` are
    one file. The pre-run check cannot see it — `samefile` needs both paths to
    exist, so it falls back to a case-*sensitive* lexical compare — and the
    manifest then truncated the XML with both success lines printed.

    The fix is to ask the filesystem again once the XML exists, before the
    manifest is written. Skipped where the volume is case-sensitive, because
    there the two names really are two files and there is nothing to catch.
    """
    class Args:
        pdf = mapping = None

    with tempfile.TemporaryDirectory() as directory:
        probe = pathlib.Path(directory, "Aa.probe")
        probe.write_text("")
        if not pathlib.Path(directory, "aa.probe").exists():
            return  # case-sensitive volume
        probe.unlink()

        args = Args()
        args.out = str(pathlib.Path(directory, "Result.xml"))
        args.manifest = str(pathlib.Path(directory, "result.XML"))

        # Before either exists the lexical compare cannot distinguish them.
        m._check_paths(args)

        pathlib.Path(args.out).write_text("<xml/>")
        refusal = refuses(m, "path_collision", m._check_paths, args)
        assert "same file" in str(refusal)
        # and the XML the run already wrote is still intact
        assert pathlib.Path(args.out).read_text() == "<xml/>"


@contextlib.contextmanager
def pretending_windows(m):
    """Run a block with the importer believing it is on Windows.

    Every Windows guard in this file is dead code on the machine that runs CI
    for it, and a guard whose branch never executes reports the same zero
    failures whether it works or is broken. These tests drive the branch. What
    they do *not* prove is ACL behaviour — no POSIX host can — only that the
    refusals fire, in the right order, and that the create is exclusive.

    This rebinds the importer module's own `os`, rather than setting
    `os.name = "nt"` on the real module: that is global, and `pathlib` reads it
    to decide whether `Path` is a `WindowsPath`, so every path this tool builds
    became uninstantiable on a POSIX host. Yields the shim, so a test can also
    make one call on it lie.
    """
    class WindowsOs:
        name = "nt"

        def __getattr__(self, attribute):
            return getattr(os, attribute)

    was, shim = m.os, WindowsOs()
    m.os = shim
    try:
        yield shim
    finally:
        m.os = was


def test_windows_refuses_to_claim_a_privacy_it_cannot_deliver(m):
    """POSIX modes do not restrict a file on Windows, so writing one and
    reporting `mode 0600` would be a false claim rather than a weaker one."""
    with tempfile.TemporaryDirectory() as directory, pretending_windows(m):
        target = str(pathlib.Path(directory, "out.xml"))
        refusal = refuses(m, "cannot_restrict_on_windows",
                          m.write_outputs, [(target, "<xml/>")], False)
        assert "--accept-inherited-permissions" in str(refusal)
        assert not pathlib.Path(target).exists(), "refused, so nothing may be written"
        # with the acknowledgement, a fresh path is written
        m.write_outputs([(target, "<xml/>")], True)
        assert pathlib.Path(target).read_text() == "<xml/>"
        # ...and the second attempt refuses, because an overwrite would keep the
        # existing file's ACL rather than inheriting the directory's
        refuses(m, "existing_target_on_windows", m.write_outputs, [(target, "<new/>")], True)
        assert pathlib.Path(target).read_text() == "<xml/>", "refused, so not truncated"


def test_windows_cleanup_closes_a_new_output_before_unlinking(m):
    """Windows cannot unlink an open exclusive output, so caught cleanup
    releases its pin before removal. The branch is simulated; ACL/filesystem
    behavior still needs an affected Windows host."""
    with tempfile.TemporaryDirectory() as directory, pretending_windows(m) as shim:
        target = pathlib.Path(directory, "out.xml")
        events = []
        real_close = shim.close
        real_unlink = m._unlink_for_cleanup

        def observe_close(handle):
            events.append("close")
            return real_close(handle)

        def observe_unlink(path, identity, failures):
            assert events == ["close"]
            events.append("unlink")
            return real_unlink(path, identity, failures)

        shim.close = observe_close
        m._unlink_for_cleanup = observe_unlink
        try:
            try:
                m.write_outputs([(str(target), "<xml/>")], True,
                                after_claim=lambda: (_ for _ in ()).throw(
                                    OSError("controlled failure")))
                raise AssertionError("the controlled failure must escape")
            except OSError as error:
                assert "controlled failure" in str(error)
        finally:
            shim.close = real_close
            m._unlink_for_cleanup = real_unlink

        assert events == ["close", "unlink"]
        assert not target.exists()


def test_a_windows_target_appearing_after_the_check_is_not_truncated(m):
    """The check and the create must be one operation.

    `os.path.exists(path)` followed by an `O_CREAT | O_TRUNC` open is a race: a
    file arriving in the gap was truncated anyway, destroying a file whose ACL
    the operator never acknowledged, and the run reported success. Driven here
    by making the existence check lie, which is what losing the race looks like
    from inside the process.
    """
    with tempfile.TemporaryDirectory() as directory, pretending_windows(m) as shim:
        target = pathlib.Path(directory, "out.xml")
        target.write_text("someone else's file")
        # the gap between check and create: the check says the path is free and
        # the filesystem disagrees
        shim.path = types.SimpleNamespace(exists=lambda path: False)
        refusal = refuses(m, "existing_target_on_windows",
                          m.write_outputs, [(str(target), "<xml/>")], True)
        assert "already exists" in str(refusal)
        assert target.read_text() == "someone else's file", "O_EXCL must not truncate"


def test_both_windows_destinations_are_checked_before_either_is_written(m):
    """A new `--out` with an existing `--manifest` used to write the XML and
    then refuse the manifest, leaving a partial result — and the refusal's
    remedy ("remove it and re-run") then failed on the XML the failed run had
    just created. Preflight judges both before anything is written."""
    class Args:
        pdf = mapping = None
        company = confirm_open_company = "Some Company"
        dry_run = False
        date_from = date_to = None
        accept_inherited_permissions = True

    with tempfile.TemporaryDirectory() as directory, pretending_windows(m):
        args = Args()
        args.out = str(pathlib.Path(directory, "new.xml"))
        args.manifest = str(pathlib.Path(directory, "existing.csv"))
        pathlib.Path(args.manifest).write_text("old manifest")

        refusal = refuses(m, "existing_target_on_windows", m.preflight, args)
        assert "existing.csv" in str(refusal), refusal
        # the whole point: the XML was not written first
        assert not pathlib.Path(args.out).exists(), \
            "preflight refused, so the run must not have written the other target"
        assert pathlib.Path(args.manifest).read_text() == "old manifest"


def test_a_dry_run_does_not_judge_destinations_it_will_not_write(m):
    """`--dry-run` returns before opening either destination, so refusing the
    run over an ACL acknowledgement or an existing file made the preview
    unusable on Windows for the one command an operator wants to preview —
    their real one, flags and all."""
    class Args:
        pdf = mapping = None
        company = confirm_open_company = "Some Company"
        date_from = date_to = None
        opening = expect_closing = "1000.00"
        expect_debits = expect_credits = "0.00"
        accept_inherited_permissions = False  # the flag a real preview omits

    with tempfile.TemporaryDirectory() as directory, pretending_windows(m):
        args = Args()
        args.out = str(pathlib.Path(directory, "result.xml"))
        args.manifest = str(pathlib.Path(directory, "manifest.csv"))
        pathlib.Path(args.manifest).write_text("already here")

        args.dry_run = True
        m.preflight(args)  # must not raise: nothing will be written
        assert not pathlib.Path(args.out).exists()

        # ...and the same command without --dry-run is still refused, or this
        # test would pass against a preflight that checks nothing at all.
        args.dry_run = False
        refuses(m, "cannot_restrict_on_windows", m.preflight, args)


def test_no_output_is_written_unless_every_destination_was_claimed(m):
    """Creating each file at its own write site left a partial result: with a
    new `--out` and the manifest taken between preflight and the write, the XML
    was written and the manifest then refused. Both destinations are claimed
    before either payload is written, and a failed claim rolls the set back."""
    with tempfile.TemporaryDirectory() as directory, pretending_windows(m):
        first = pathlib.Path(directory, "first.xml")
        second = pathlib.Path(directory, "second.csv")
        second.write_text("someone else's file")
        refuses(m, "existing_target_on_windows", m.write_outputs,
                [(str(first), "<xml/>"), (str(second), "row\n")], True)
        assert not first.exists(), "the first target must be rolled back, not left behind"
        assert second.read_text() == "someone else's file"

    # The rollback removes only what this call created. A POSIX run overwrites
    # by design, so the second claim succeeds and both are written.
    with tempfile.TemporaryDirectory() as directory:
        first = pathlib.Path(directory, "first.xml")
        second = pathlib.Path(directory, "second.csv")
        m.write_outputs([(str(first), "<xml/>"), (str(second), "row\n")])
        assert first.read_text() == "<xml/>" and second.read_text() == "row\n"
        assert stat.S_IMODE(first.stat().st_mode) == 0o600


def test_a_failed_run_does_not_destroy_the_previous_output(m):
    """The rollback must not be worse than the failure it cleans up after.

    Claiming an existing POSIX destination with `O_TRUNC` emptied it *at claim
    time*, so a later failure rolled back by unlinking a file whose contents the
    run had already destroyed — leaving the operator with neither the previous
    output nor a new one. An existing destination is staged and renamed into
    place only once every payload is written.
    """
    with tempfile.TemporaryDirectory() as directory:
        existing = pathlib.Path(directory, "previous.xml")
        existing.write_text("the previous run's output")
        unwritable = str(pathlib.Path(directory, "missing-dir", "second.csv"))

        try:
            m.write_outputs([(str(existing), "<new/>"), (unwritable, "row\n")])
            raise AssertionError("the second destination cannot be opened")
        except (OSError, m.Refusal):
            pass

        assert existing.exists(), "the previous output was deleted by the rollback"
        assert existing.read_text() == "the previous run's output", \
            "the previous output was truncated before the run could commit"
        # nothing staged is left lying around next to it
        assert sorted(p.name for p in pathlib.Path(directory).iterdir()) == \
            ["previous.xml"], sorted(p.name for p in pathlib.Path(directory).iterdir())

    # and when every target does succeed, an existing destination is replaced
    with tempfile.TemporaryDirectory() as directory:
        existing = pathlib.Path(directory, "previous.xml")
        existing.write_text("old")
        other = pathlib.Path(directory, "new.csv")
        m.write_outputs([(str(existing), "<new/>"), (str(other), "row\n")])
        assert existing.read_text() == "<new/>"
        assert other.read_text() == "row\n"
        assert stat.S_IMODE(existing.stat().st_mode) == 0o600
        assert sorted(p.name for p in pathlib.Path(directory).iterdir()) == \
            ["new.csv", "previous.xml"]


def test_write_outputs_writes_through_a_symlinked_destination(m):
    """`os.replace` does not follow a symlink at the destination -- it
    replaces the link itself, the same as `unlink` would. Renaming the staged
    payload onto the link's own name would silently turn a live symlink into
    a plain file, leaving whatever else reads through that link looking at
    stale content forever. The swap must resolve through the link and land on
    its target instead."""
    with tempfile.TemporaryDirectory() as directory:
        real_target = pathlib.Path(directory, "real-target.xml")
        real_target.write_text("old")
        link = pathlib.Path(directory, "out.xml")
        link.symlink_to(real_target)

        m.write_outputs([(str(link), "<new/>")])

        assert link.is_symlink(), "the link itself must survive the write"
        assert pathlib.Path(os.readlink(link)) == real_target
        assert real_target.read_text() == "<new/>", \
            "the payload must reach the link's target, not replace the link"


def test_write_outputs_refuses_a_retargeted_symlink_before_commit(m):
    """The destination observed after claiming must still name the same target
    at the commit boundary. Otherwise a successful command updates a stale path
    while the operator's requested path continues to expose old bytes."""
    with tempfile.TemporaryDirectory() as directory:
        root = pathlib.Path(directory)
        first = root / "first.xml"
        second = root / "second.xml"
        link = root / "out.xml"
        first.write_text("first old")
        second.write_text("second old")
        link.symlink_to(first)

        def retarget():
            link.unlink()
            link.symlink_to(second)

        refuses(m, "output_path_changed", m.write_outputs,
                [(str(link), "new bytes")], False, retarget)
        assert first.read_text() == "first old"
        assert second.read_text() == "second old"
        assert link.read_text() == "second old"
        assert sorted(p.name for p in root.iterdir()) == ["first.xml", "out.xml", "second.xml"]


def test_write_outputs_revalidates_a_symlink_after_backup_preparation(m):
    """The check belongs directly before the swap, not before a backup syscall
    which can itself be used to retarget the operator's path."""
    with tempfile.TemporaryDirectory() as directory:
        root = pathlib.Path(directory)
        first = root / "first.xml"
        second = root / "second.xml"
        link = root / "out.xml"
        first.write_text("first old")
        second.write_text("second old")
        link.symlink_to(first)
        real_copy = m._copy_private_backup

        def retarget_after_backup(src, identity, backup_handle):
            result = real_copy(src, identity, backup_handle)
            link.unlink()
            link.symlink_to(second)
            return result

        m._copy_private_backup = retarget_after_backup
        try:
            refuses(m, "output_path_changed", m.write_outputs, [(str(link), "new bytes")])
        finally:
            m._copy_private_backup = real_copy

        assert first.read_text() == "first old"
        assert second.read_text() == "second old"
        assert link.read_text() == "second old"
        assert sorted(p.name for p in root.iterdir()) == ["first.xml", "out.xml", "second.xml"]


def test_write_outputs_keeps_destination_during_private_backup_preparation(m):
    """The requested output remains readable while its private backup is made.

    `os.link` deliberately fails here: writable filesystems without hard-link
    support still need the same caught-exception rollback behavior.
    """
    with tempfile.TemporaryDirectory() as directory:
        destination = pathlib.Path(directory, "previous.xml")
        destination.write_text("old bytes")
        real_copy = m._copy_private_backup
        real_link = m.os.link
        observed = []

        def copy_while_observing(src, identity, backup_handle):
            result = real_copy(src, identity, backup_handle)
            observed.append((destination.exists(), destination.read_text()))
            return result

        m._copy_private_backup = copy_while_observing
        m.os.link = lambda *_: (_ for _ in ()).throw(OSError("hard links unavailable"))
        try:
            m.write_outputs([(str(destination), "new bytes")])
        finally:
            m._copy_private_backup = real_copy
            m.os.link = real_link

        assert observed == [(True, "old bytes")]
        assert destination.read_text() == "new bytes"


def test_an_interrupt_after_private_backup_preserves_the_previous_output(m):
    """The exclusive backup is private while interruption can still occur, and
    cleanup removes only that owned copy while leaving the destination intact."""
    with tempfile.TemporaryDirectory() as directory:
        destination = pathlib.Path(directory, "previous.xml")
        destination.write_text("old bytes")
        root = pathlib.Path(directory)
        real_copy = m._copy_private_backup
        modes = []

        def interrupt_after_backup_copy(src, identity, backup_handle):
            result = real_copy(src, identity, backup_handle)
            backups = list(root.glob("*.bak"))
            assert len(backups) == 1
            modes.append(stat.S_IMODE(backups[0].stat().st_mode))
            raise KeyboardInterrupt("controlled interrupt after private backup")

        m._copy_private_backup = interrupt_after_backup_copy
        try:
            try:
                m.write_outputs([(str(destination), "new bytes")])
                raise AssertionError("the controlled interrupt must escape")
            except KeyboardInterrupt:
                pass
        finally:
            m._copy_private_backup = real_copy

        assert modes == [0o600]
        assert destination.read_text() == "old bytes"
        assert sorted(p.name for p in pathlib.Path(directory).iterdir()) == ["previous.xml"]


def test_an_interrupt_after_a_swap_restores_the_previous_output(m):
    """A caught interrupt may arrive after rename(2) took effect. Pending swap
    state must therefore be restored, not discarded as if the call had failed
    before touching the filesystem."""
    with tempfile.TemporaryDirectory() as directory:
        destination = pathlib.Path(directory, "previous.xml")
        destination.write_text("old bytes")
        real_replace = m.os.replace

        def interrupt_after_swap(src, dst):
            result = real_replace(src, dst)
            if str(src).endswith(".part"):
                raise KeyboardInterrupt("controlled interrupt after swap")
            return result

        m.os.replace = interrupt_after_swap
        try:
            try:
                m.write_outputs([(str(destination), "new bytes")])
                raise AssertionError("the controlled interrupt must escape")
            except KeyboardInterrupt:
                pass
        finally:
            m.os.replace = real_replace

        assert destination.read_text() == "old bytes"
        assert sorted(p.name for p in pathlib.Path(directory).iterdir()) == ["previous.xml"]


def test_line_interrupt_after_recording_a_swap_restores_it_once(m):
    """Trace the real line between append and clearing pending ownership."""
    with tempfile.TemporaryDirectory() as directory:
        root = pathlib.Path(directory)
        destination = root / "previous.xml"
        destination.write_text("old bytes")
        real_restore = m._restore_backup
        restores = []
        _, start = inspect.getsourcelines(m.write_outputs)
        clear_line = start + [
            index for index, line in enumerate(inspect.getsource(m.write_outputs).splitlines())
            if line.strip() == "pending_swap = None"
        ][-1]
        old_trace = sys.gettrace()
        fired = False

        def interrupt_after_append(frame, event, _arg):
            nonlocal fired
            if (not fired and event == "line" and frame.f_code is m.write_outputs.__code__
                    and frame.f_lineno == clear_line):
                fired = True
                raise KeyboardInterrupt("controlled interrupt after swap append")
            return interrupt_after_append

        def observe_restore(*args, **kwargs):
            restores.append(args[0])
            return real_restore(*args, **kwargs)

        m._restore_backup = observe_restore
        sys.settrace(interrupt_after_append)
        try:
            try:
                m.write_outputs([(str(destination), "new bytes")])
                raise AssertionError("the controlled interrupt must escape")
            except KeyboardInterrupt:
                pass
        finally:
            sys.settrace(old_trace)
            m._restore_backup = real_restore

        assert fired
        assert len(restores) == 1, "one swap must have one rollback owner"
        assert destination.read_text() == "old bytes"
        assert sorted(path.name for path in root.iterdir()) == ["previous.xml"]


def test_line_interrupt_before_clearing_pending_backup_has_one_cleanup_owner(m):
    """An interrupt in the alias window must not clean one backup twice."""
    with tempfile.TemporaryDirectory() as directory:
        root = pathlib.Path(directory)
        destination = root / "previous.xml"
        destination.write_text("old bytes")
        _, start = inspect.getsourcelines(m.write_outputs)
        clear_line = start + [
            index for index, line in enumerate(inspect.getsource(m.write_outputs).splitlines())
            if line.strip() == "pending_backup = None"
        ][-1]
        real_cleanup = m._cleanup_owned_path
        cleanup_paths = []
        old_trace = sys.gettrace()
        fired = False

        def interrupt_before_clear(frame, event, _arg):
            nonlocal fired
            if (not fired and event == "line" and frame.f_code is m.write_outputs.__code__
                    and frame.f_lineno == clear_line):
                fired = True
                raise KeyboardInterrupt("controlled pending-backup interrupt")
            return interrupt_before_clear

        def observe_cleanup(record, failures):
            cleanup_paths.append(str(record["path"]))
            return real_cleanup(record, failures)

        m._cleanup_owned_path = observe_cleanup
        sys.settrace(interrupt_before_clear)
        try:
            try:
                m.write_outputs([(str(destination), "new bytes")])
                raise AssertionError("the controlled interrupt must escape")
            except KeyboardInterrupt:
                pass
        finally:
            sys.settrace(old_trace)
            m._cleanup_owned_path = real_cleanup

        assert fired
        assert destination.read_text() == "old bytes"
        assert not any(path.endswith(".bak") for path in cleanup_paths)
        assert sorted(path.name for path in root.iterdir()) == ["previous.xml"]


def test_ownership_registration_failure_reconciles_a_created_path_and_closes_its_pin(m):
    """The creator is not in an outer cleanup list until fstat succeeds."""
    with tempfile.TemporaryDirectory() as directory:
        root = pathlib.Path(directory)
        real_identity = m._fd_identity
        real_close = m.os.close
        closed = []
        for position, failure in enumerate((OSError("fstat I/O"),
                                            KeyboardInterrupt("fstat interrupt"))):
            path = root / f"fresh-{position}.xml"
            handle = m._open_private(path)
            calls = 0

            def fail_once(candidate):
                nonlocal calls
                calls += 1
                if calls == 1:
                    raise failure
                return real_identity(candidate)

            def observe_close(candidate):
                closed.append(candidate)
                return real_close(candidate)

            m._fd_identity = fail_once
            m.os.close = observe_close
            try:
                try:
                    m._owned_path(path, handle, created=True)
                    raise AssertionError("the original identity failure must escape")
                except BaseException as error:
                    assert error is failure
            finally:
                m._fd_identity = real_identity
                m.os.close = real_close

            assert not path.exists(), "a reconciled fresh output must not strand a refusal"
            assert handle in closed


def test_ownership_registration_preserves_an_unproven_reclaimed_path(m):
    """When fstat cannot establish ownership, foreign bytes survive visibly."""
    with tempfile.TemporaryDirectory() as directory:
        root = pathlib.Path(directory)
        path = root / "fresh.xml"
        foreign = root / "foreign.xml"
        handle = m._open_private(path)
        foreign.write_text("foreign writer bytes")
        os.replace(foreign, path)
        real_identity = m._fd_identity

        def fail_identity(_handle):
            raise OSError("persistent fstat I/O")

        m._fd_identity = fail_identity
        try:
            try:
                m._owned_path(path, handle, created=True)
                raise AssertionError("the identity failure must escape")
            except OSError as error:
                notes = "\n".join(getattr(error, "__notes__", []))
        finally:
            m._fd_identity = real_identity

        assert path.read_text() == "foreign writer bytes"
        assert str(path) in notes


def test_original_pin_registration_failure_preserves_existing_output(m):
    """The original pin is not newly created cleanup authority."""
    with tempfile.TemporaryDirectory() as directory:
        root = pathlib.Path(directory)
        destination = root / "previous.xml"
        destination.write_text("old bytes")
        real_identity = m._fd_identity
        calls = 0

        def fail_original_pin(handle):
            nonlocal calls
            calls += 1
            # Staged output and private backup register first. The third pin
            # is the old destination opened for backup and metadata capture.
            if calls == 3:
                raise OSError("controlled original pin fstat failure")
            return real_identity(handle)

        m._fd_identity = fail_original_pin
        try:
            try:
                m.write_outputs([(str(destination), "new bytes")])
                raise AssertionError("the controlled original-pin failure must escape")
            except OSError as error:
                assert "controlled original pin fstat failure" in str(error)
        finally:
            m._fd_identity = real_identity

        assert destination.read_text() == "old bytes"
        assert sorted(path.name for path in root.iterdir()) == ["previous.xml"]


def test_restore_reconciles_a_backup_replace_that_raised_after_effect(m):
    """A restore rename can report an error after it has moved the private
    backup. Its new identity then proves recovery completed and must not be
    reported as a missing retained backup."""
    with tempfile.TemporaryDirectory() as directory:
        root = pathlib.Path(directory)
        first = root / "first.xml"
        second = root / "second.csv"
        first.write_text("first old")
        second.write_text("second old")
        real_replace = m.os.replace

        def fail_second_swap_and_restore(src, dst):
            if str(src).endswith(".part") and os.path.basename(dst) == "second.csv":
                raise OSError("simulated second swap failure")
            result = real_replace(src, dst)
            if str(src).endswith(".bak"):
                raise OSError("simulated restore failure after effect")
            return result

        m.os.replace = fail_second_swap_and_restore
        try:
            try:
                m.write_outputs([(str(first), "new first"),
                                 (str(second), "new second")])
                raise AssertionError("the second target's swap must fail")
            except OSError as error:
                notes = getattr(error, "__notes__", [])
                assert len(notes) == 1
                assert str(first) in notes[0]
                assert "extended ACLs and file flags were not verified" in notes[0]
                assert ".bak" not in notes[0]
        finally:
            m.os.replace = real_replace

        assert first.read_text() == "first old"
        assert second.read_text() == "second old"
        assert sorted(path.name for path in root.iterdir()) == ["first.xml", "second.csv"]


def test_refusal_reports_rollback_metadata_scope_in_its_visible_code(m):
    """Refusal is SystemExit, so rollback scope must be added to `code`, not
    only an exception note that an unhandled process would omit."""
    with tempfile.TemporaryDirectory() as directory:
        root = pathlib.Path(directory)
        first = root / "first.xml"
        second = root / "second.csv"
        first.write_text("first old")
        second.write_text("second old")
        real_replace = m.os.replace

        def replace_first_then_refuse_second(src, dst):
            if str(src).endswith(".part") and os.path.basename(dst) == "second.csv":
                raise m.Refusal("controlled_refusal", "controlled second swap refusal")
            return real_replace(src, dst)

        m.os.replace = replace_first_then_refuse_second
        try:
            refusal = refuses(m, "controlled_refusal", m.write_outputs,
                              [(str(first), "new first"),
                               (str(second), "new second")])
        finally:
            m.os.replace = real_replace

        assert str(first) in str(refusal.code)
        assert "extended ACLs and file flags were not verified" in str(refusal.code)
        assert first.read_text() == "first old"
        assert second.read_text() == "second old"


def test_write_outputs_reports_a_retained_backup_after_commit(m):
    """Successful replacement is not a successful command when cleanup leaves
    prior bank-statement bytes at an undisclosed random backup path."""
    with tempfile.TemporaryDirectory() as directory:
        root = pathlib.Path(directory)
        destination = root / "previous.xml"
        destination.write_text("old bytes")
        real_unlink = m.os.unlink

        def fail_committed_backup(path):
            if str(path).endswith(".bak") and destination.read_text() == "new bytes":
                raise OSError("controlled backup cleanup failure")
            return real_unlink(path)

        m.os.unlink = fail_committed_backup
        try:
            try:
                m.write_outputs([(str(destination), "new bytes")])
                raise AssertionError("a retained backup must be reported")
            except m.OutputCleanupFailure as failure:
                assert len(failure.retained_paths) == 1
                backup = pathlib.Path(failure.retained_paths[0])
                assert backup.exists()
                assert backup.read_text() == "old bytes"
        finally:
            m.os.unlink = real_unlink
            for path in root.glob("*.bak"):
                path.unlink()

        assert destination.read_text() == "new bytes"


def test_interrupt_before_committed_cleanup_keeps_new_output_and_reports_backup(m):
    """The committed flag covers the line before old-copy cleanup starts."""
    with tempfile.TemporaryDirectory() as directory:
        root = pathlib.Path(directory)
        destination = root / "previous.xml"
        destination.write_text("old bytes")
        real_close = m._close_owned_path
        closed_paths = []
        _, start = inspect.getsourcelines(m._cleanup_committed_outputs)
        cleanup_line = start + next(
            index for index, line in enumerate(
                inspect.getsource(m._cleanup_committed_outputs).splitlines())
            if line.strip() == "for swap in replaced:")
        old_trace = sys.gettrace()
        fired = False

        def interrupt_before_cleanup(frame, event, _arg):
            nonlocal fired
            if (not fired and event == "line"
                    and frame.f_code is m._cleanup_committed_outputs.__code__
                    and frame.f_lineno == cleanup_line):
                fired = True
                raise KeyboardInterrupt("controlled interrupt before cleanup")
            return interrupt_before_cleanup

        def observe_close(record, failures):
            if record.get("pin") is not None:
                closed_paths.append(str(record["path"]))
            return real_close(record, failures)

        m._close_owned_path = observe_close
        sys.settrace(interrupt_before_cleanup)
        try:
            try:
                m.write_outputs([(str(destination), "new bytes")])
                raise AssertionError("the controlled interrupt must escape")
            except KeyboardInterrupt as error:
                notes = "\n".join(getattr(error, "__notes__", []))
                backups = list(root.glob("*.bak"))
                assert len(backups) == 1
                assert backups[0].read_text() == "old bytes"
                assert "retained path(s):" in notes
                assert os.path.realpath(backups[0]) in notes
        finally:
            sys.settrace(old_trace)
            m._close_owned_path = real_close
            for path in root.glob("*.bak"):
                path.unlink()

        assert fired
        assert destination.read_text() == "new bytes"
        assert os.path.realpath(destination) in closed_paths, closed_paths
        assert any(path.endswith(".bak") for path in closed_paths)
        assert any(path.endswith(".part") for path in closed_paths)


def test_interrupt_after_backup_unlink_does_not_report_a_phantom_path(m):
    """A cleanup syscall may interrupt after deletion; name no absent backup."""
    with tempfile.TemporaryDirectory() as directory:
        root = pathlib.Path(directory)
        destination = root / "previous.xml"
        destination.write_text("old bytes")
        real_unlink = m.os.unlink
        real_close = m._close_owned_path
        closed_paths = []
        interrupted_backup = None

        def interrupt_after_backup_unlink(path):
            nonlocal interrupted_backup
            result = real_unlink(path)
            if str(path).endswith(".bak"):
                interrupted_backup = str(path)
                raise KeyboardInterrupt("controlled interrupt after backup unlink")
            return result

        def observe_close(record, failures):
            if record.get("pin") is not None:
                closed_paths.append(str(record["path"]))
            return real_close(record, failures)

        m.os.unlink = interrupt_after_backup_unlink
        m._close_owned_path = observe_close
        try:
            try:
                m.write_outputs([(str(destination), "new bytes")])
                raise AssertionError("the controlled interrupt must escape")
            except KeyboardInterrupt as error:
                notes = "\n".join(getattr(error, "__notes__", []))
                assert interrupted_backup is not None
                assert not pathlib.Path(interrupted_backup).exists()
                assert os.path.realpath(interrupted_backup) not in notes
        finally:
            m.os.unlink = real_unlink
            m._close_owned_path = real_close

        assert destination.read_text() == "new bytes"
        assert not list(root.glob("*.bak"))
        assert os.path.realpath(destination) in closed_paths, closed_paths
        assert any(path.endswith(".bak") for path in closed_paths)
        assert any(path.endswith(".part") for path in closed_paths)


def test_legacy_oserror_cleanup_diagnostic_reaches_stderr(m):
    """Python 3.10's OSError rendering ignores args mutations and needs stderr."""
    program = f'''\
import errno
import importlib.util

script = {str(SCRIPT)!r}
spec = importlib.util.spec_from_file_location("bank_statement_import_subprocess", script)
module = importlib.util.module_from_spec(spec)
spec.loader.exec_module(module)
class LegacyOSError(OSError):
    add_note = None
error = LegacyOSError(errno.EIO, "controlled original failure", "/tmp/legacy-output.xml")
module._note_cleanup_failures(error, ["/tmp/owned-backup.bak"])
raise error
'''
    done = subprocess.run([sys.executable, "-c", program], text=True,
                          capture_output=True, check=False)
    assert done.returncode != 0
    assert "controlled original failure" in done.stderr
    assert "/tmp/legacy-output.xml" in done.stderr
    assert "retained path(s): /tmp/owned-backup.bak" in done.stderr


def test_refusal_reports_a_retained_backup_on_stderr(m):
    """Refusal inherits SystemExit, whose unhandled rendering ignores
    `BaseException.add_note`. Assert the CLI-visible error rather than the
    in-process exception object so a sensitive retained backup is not hidden."""
    program = f'''\
import importlib.util
import pathlib
import tempfile

script = {str(SCRIPT)!r}
spec = importlib.util.spec_from_file_location("bank_statement_import_subprocess", script)
module = importlib.util.module_from_spec(spec)
spec.loader.exec_module(module)
with tempfile.TemporaryDirectory() as directory:
    root = pathlib.Path(directory)
    first = root / "first.xml"
    second = root / "second.xml"
    link = root / "out.xml"
    first.write_text("first old")
    second.write_text("second old")
    link.symlink_to(first)
    real_copy = module._copy_private_backup
    real_cleanup = module._unlink_for_cleanup
    def retarget_after_backup(src, identity, backup_handle):
        result = real_copy(src, identity, backup_handle)
        link.unlink()
        link.symlink_to(second)
        return result
    def retain_backup(path, identity, failures):
        if str(path).endswith(".bak"):
            failures.append(path)
        else:
            real_cleanup(path, identity, failures)
    module._copy_private_backup = retarget_after_backup
    module._unlink_for_cleanup = retain_backup
    module.write_outputs([(str(link), "new bytes")])
'''
    done = subprocess.run([sys.executable, "-c", program], text=True,
                          capture_output=True, check=False)
    assert done.returncode != 0
    assert "output_path_changed" in done.stderr, done.stderr
    assert "retained path(s):" in done.stderr, done.stderr
    assert ".bak" in done.stderr, done.stderr


def test_refusal_reports_rollback_metadata_scope_on_stderr(m):
    """The ACL/file-flag limitation must survive unhandled SystemExit
    rendering, where Python omits ordinary exception notes."""
    program = f'''\
import importlib.util
import os
import pathlib
import tempfile

script = {str(SCRIPT)!r}
spec = importlib.util.spec_from_file_location("bank_statement_import_subprocess", script)
module = importlib.util.module_from_spec(spec)
spec.loader.exec_module(module)
with tempfile.TemporaryDirectory() as directory:
    root = pathlib.Path(directory)
    first = root / "first.xml"
    second = root / "second.csv"
    first.write_text("first old")
    second.write_text("second old")
    real_replace = module.os.replace
    def replace_first_then_refuse_second(src, dst):
        if str(src).endswith(".part") and os.path.basename(dst) == "second.csv":
            raise module.Refusal("controlled_refusal", "controlled second swap refusal")
        return real_replace(src, dst)
    module.os.replace = replace_first_then_refuse_second
    module.write_outputs([(str(first), "new first"), (str(second), "new second")])
'''
    done = subprocess.run([sys.executable, "-c", program], text=True,
                          capture_output=True, check=False)
    assert done.returncode != 0
    assert "controlled_refusal" in done.stderr, done.stderr
    assert "extended ACLs and file flags were not verified" in done.stderr, done.stderr


def test_a_failed_swap_rolls_back_every_staged_replacement(m):
    """Two existing destinations are both staged; the first's swap succeeds
    and the second's fails. The rollback used to run only the un-staged
    cleanup, so the first destination was left holding the new run's content
    with no way back -- a mid-sequence failure must undo every replacement
    this call already committed, not only the one in progress when it failed.
    """
    with tempfile.TemporaryDirectory() as directory:
        first = pathlib.Path(directory, "first.xml")
        second = pathlib.Path(directory, "second.csv")
        first.write_text("first old")
        second.write_text("second old")

        real_replace = m.os.replace
        calls = {"n": 0}

        def flaky_replace(src, dst):
            calls["n"] += 1
            # The first destination's swap lands, then the second swap fails.
            if calls["n"] == 2:
                raise OSError("simulated failure mid-sequence")
            return real_replace(src, dst)

        m.os.replace = flaky_replace
        try:
            try:
                m.write_outputs([(str(first), "<new-first/>"),
                                  (str(second), "new-second\n")])
                raise AssertionError("the second target's swap must fail")
            except OSError:
                pass
        finally:
            m.os.replace = real_replace

        assert first.read_text() == "first old", \
            "the first destination's already-committed swap must be rolled back"
        assert second.read_text() == "second old"
        # no stray .part/.bak file is left behind by either destination
        assert sorted(p.name for p in pathlib.Path(directory).iterdir()) == \
            ["first.xml", "second.csv"], \
            sorted(p.name for p in pathlib.Path(directory).iterdir())


def test_backup_copy_refuses_a_replaced_original_before_commit(m):
    """Rollback bytes need the original inode's provenance, not whatever
    happened to occupy its name while the private copy was being prepared."""
    with tempfile.TemporaryDirectory() as directory:
        root = pathlib.Path(directory)
        destination = root / "previous.xml"
        replacement = root / "foreign.xml"
        destination.write_text("old bytes")
        replacement.write_text("foreign writer bytes")
        real_copy = m._copy_private_backup
        real_replace = m.os.replace

        def replace_before_copy(src, identity, backup_handle):
            real_replace(replacement, destination)
            return real_copy(src, identity, backup_handle)

        m._copy_private_backup = replace_before_copy
        try:
            refuses(m, "output_path_changed", m.write_outputs,
                    [(str(destination), "new bytes")])
        finally:
            m._copy_private_backup = real_copy

        assert destination.read_text() == "foreign writer bytes"
        assert sorted(path.name for path in root.iterdir()) == ["previous.xml"]


def test_claim_refuses_a_foreign_replacement_after_committing_original_identity(m):
    """The descriptor, rather than a pre-open stat, commits the old inode.

    An attacker can replace the path before this call opens it; without a
    lock, that is the file the call is asked to replace. This regression covers
    the actionable interval: a foreign replacement after the old descriptor
    establishes identity but before the path is verified must remain untouched.
    """
    if os.name == "nt":
        return  # Existing destinations are deliberately refused on Windows.
    with tempfile.TemporaryDirectory() as directory:
        root = pathlib.Path(directory)
        destination = root / "previous.xml"
        foreign = root / "foreign.xml"
        destination.write_text("old bytes")
        foreign.write_text("foreign writer bytes")
        real_owned_path = m._owned_path
        real_replace = m.os.replace

        def replace_after_identity(path, handle, *, created):
            record = real_owned_path(path, handle, created=created)
            if not created and os.path.realpath(path) == os.path.realpath(destination):
                real_replace(foreign, destination)
            return record

        m._owned_path = replace_after_identity
        try:
            refuses(m, "output_path_changed", m.write_outputs,
                    [(str(destination), "new bytes")])
        finally:
            m._owned_path = real_owned_path

        assert destination.read_text() == "foreign writer bytes"
        assert sorted(path.name for path in root.iterdir()) == ["previous.xml"]


def test_line_interrupt_during_final_validation_rolls_back_replacements(m):
    """A real line-traced SIGINT at the final validation stays recoverable."""
    if os.name == "nt":
        return  # Existing destinations are deliberately refused on Windows.
    with tempfile.TemporaryDirectory() as directory:
        root = pathlib.Path(directory)
        first = root / "first.xml"
        second = root / "second.csv"
        first.write_text("first old")
        second.write_text("second old")
        _, start = inspect.getsourcelines(m.write_outputs)
        validation_line = start + next(
            index for index, line in enumerate(
                inspect.getsource(m.write_outputs).splitlines())
            if line.strip() == "if _claimed_output_changed(")
        old_trace = sys.gettrace()
        old_signal_handler = signal.getsignal(signal.SIGINT)
        fired = False

        def interrupt_final_validation(frame, event, _arg):
            nonlocal fired
            if (not fired and event == "line" and frame.f_code is m.write_outputs.__code__
                    and frame.f_lineno == validation_line):
                fired = True
                signal.raise_signal(signal.SIGINT)
            return interrupt_final_validation

        signal.signal(signal.SIGINT, signal.default_int_handler)
        sys.settrace(interrupt_final_validation)
        try:
            try:
                m.write_outputs([(str(first), "new first"),
                                 (str(second), "new second")])
                raise AssertionError("the controlled interrupt must escape")
            except KeyboardInterrupt:
                pass
        finally:
            sys.settrace(old_trace)
            signal.signal(signal.SIGINT, old_signal_handler)

        assert fired
        assert first.read_text() == "first old"
        assert second.read_text() == "second old"
        assert sorted(path.name for path in root.iterdir()) == ["first.xml", "second.csv"]


def test_backup_copy_refuses_a_fifo_before_reading_it(m):
    """An existing output is data only when it is a regular file; opening a
    FIFO for its rollback copy would otherwise wait for an unrelated writer."""
    if not hasattr(os, "mkfifo"):
        return
    with tempfile.TemporaryDirectory() as directory:
        root = pathlib.Path(directory)
        destination = root / "previous.xml"
        os.mkfifo(destination)

        refuses(m, "output_not_regular", m.write_outputs,
                [(str(destination), "new bytes")])

        assert destination.is_fifo()
        assert sorted(path.name for path in root.iterdir()) == ["previous.xml"]


def test_duplicate_failure_after_claim_closes_and_removes_new_output(m):
    """The claimed private descriptor is already recorded before a duplicate
    is needed for writing, so descriptor exhaustion cannot leak the path."""
    with tempfile.TemporaryDirectory() as directory:
        destination = pathlib.Path(directory, "new.xml")
        real_open_private = m._open_private
        real_dup = m.os.dup
        real_close = m.os.close
        opened, closed = [], []

        def observe_open(path, accept_inherited=False):
            handle = real_open_private(path, accept_inherited)
            opened.append(handle)
            return handle

        def exhausted_dup(_):
            raise OSError("controlled descriptor exhaustion")

        def observe_close(handle):
            closed.append(handle)
            return real_close(handle)

        m._open_private = observe_open
        m.os.dup = exhausted_dup
        m.os.close = observe_close
        try:
            try:
                m.write_outputs([(str(destination), "new bytes")])
                raise AssertionError("the duplicate failure must escape")
            except OSError as error:
                assert "controlled descriptor exhaustion" in str(error)
        finally:
            m._open_private = real_open_private
            m.os.dup = real_dup
            m.os.close = real_close

        assert len(opened) == 1
        assert opened[0] in closed
        assert not destination.exists()


def test_backup_refuses_when_original_metadata_cannot_be_recorded(m):
    """A rollback cannot claim to restore metadata it was unable to capture."""
    if not hasattr(m.os, "listxattr"):
        return
    with tempfile.TemporaryDirectory() as directory:
        root = pathlib.Path(directory)
        destination = root / "previous.xml"
        destination.write_text("old bytes")
        real_listxattr = m.os.listxattr

        def unavailable_xattrs(_):
            raise OSError("controlled xattr metadata failure")

        m.os.listxattr = unavailable_xattrs
        try:
            refusal = refuses(m, "output_metadata_unavailable", m.write_outputs,
                              [(str(destination), "new bytes")])
        finally:
            m.os.listxattr = real_listxattr

        assert "controlled xattr metadata failure" in str(refusal.code)
        assert destination.read_text() == "old bytes"
        assert sorted(path.name for path in root.iterdir()) == ["previous.xml"]


def test_cleanup_keeps_a_reclaimed_owned_path(m):
    """A cleanup record proves only the path our run made. If that pathname
    changes identity, reporting it is safe; unlinking it is not."""
    with tempfile.TemporaryDirectory() as directory:
        root = pathlib.Path(directory)
        owned = root / "output.xml.pending"
        foreign = root / "foreign.xml"
        owned.write_text("our temporary bytes")
        identity = m._entry_identity(owned)
        foreign.write_text("foreign writer bytes")
        os.replace(foreign, owned)
        failures = []

        m._unlink_for_cleanup(owned, identity, failures)

        assert owned.read_text() == "foreign writer bytes"
        assert failures == [str(owned)]


def test_pinned_backup_reclaimed_path_retains_cleanup_diagnostic(m):
    if os.name == "nt":
        return
    with tempfile.TemporaryDirectory() as directory:
        path = pathlib.Path(directory) / "rollback.bak"
        moved = pathlib.Path(directory) / "moved.bak"
        path.write_text("prior output")
        identity = m._entry_identity(path)
        handle = os.open(path, os.O_RDONLY)
        record = {"path": path, "identity": identity, "pin": handle}
        path.rename(moved)
        path.write_text("foreign bytes")
        failures = []
        m._cleanup_owned_path(record, failures)
        assert failures == [str(path)]
        assert record["pin"] is None
        assert path.read_text() == "foreign bytes"
        assert moved.read_text() == "prior output"


def test_pinned_backup_parent_rename_reports_unlocated_copy(m):
    if os.name == "nt":
        return
    with tempfile.TemporaryDirectory() as directory:
        root = pathlib.Path(directory)
        before, after = root / "before", root / "after"
        before.mkdir()
        path = before / "rollback.bak"
        path.write_text("prior output")
        identity = m._entry_identity(path)
        handle = os.open(path, os.O_RDONLY)
        record = {"path": path, "identity": identity, "pin": handle}
        try:
            before.rename(after)
            failures = []
            m._cleanup_owned_path(record, failures)
        finally:
            if record["pin"] is not None:
                os.close(record["pin"])
        assert failures == [
            f"owned output could not be located after cleanup: {path}"
        ]
        assert (after / "rollback.bak").read_text() == "prior output"


def test_new_output_symlink_loop_is_a_typed_path_refusal(m):
    with tempfile.TemporaryDirectory() as directory:
        destination = pathlib.Path(directory) / "output.xml"
        real_resolve = m.pathlib.Path.resolve

        def loop_resolve(path, *args, **kwargs):
            if path == destination:
                raise RuntimeError("controlled symlink loop")
            return real_resolve(path, *args, **kwargs)

        m.pathlib.Path.resolve = loop_resolve
        try:
            refusal = refuses(m, "output_path_changed", m.write_outputs,
                              [(str(destination), "new bytes")])
        finally:
            m.pathlib.Path.resolve = real_resolve
        assert "could not be resolved safely" in str(refusal.code)
        assert not destination.exists()


def test_new_output_final_revalidation_loop_cleans_all_claimed_outputs(m):
    """A loop discovered after claiming still rolls back every fresh output."""
    with tempfile.TemporaryDirectory() as directory:
        root = pathlib.Path(directory)
        first, second = root / "first.xml", root / "second.xml"
        real_resolve = m.pathlib.Path.resolve
        resolve_calls = 0

        def loop_after_claim(path):
            nonlocal resolve_calls
            if pathlib.Path(path) == first:
                resolve_calls += 1
                # Claiming resolves once to choose the canonical path and once
                # to verify the new inode. The third call is final
                # revalidation, after both outputs have been written.
                if resolve_calls == 3:
                    raise RuntimeError("controlled post-claim symlink loop")
            return real_resolve(path)

        m.pathlib.Path.resolve = loop_after_claim
        try:
            refusal = refuses(
                m,
                "output_path_changed",
                m.write_outputs,
                [(str(first), "first bytes"), (str(second), "second bytes")],
            )
        finally:
            m.pathlib.Path.resolve = real_resolve
        assert resolve_calls == 3, "RuntimeError must be injected during final revalidation"
        assert "could not be resolved safely" in str(refusal.code)
        assert not first.exists(), "the first claimed output must be cleaned"
        assert not second.exists(), "the earlier claimed output must be cleaned too"


def test_unlinked_pinned_backup_does_not_claim_a_hard_link_alias(m):
    if os.name == "nt":
        return
    with tempfile.TemporaryDirectory() as directory:
        path = pathlib.Path(directory) / "rollback.bak"
        path.write_text("prior output")
        identity = m._entry_identity(path)
        handle = os.open(path, os.O_RDONLY)
        try:
            path.unlink()
            refusal = refuses(m, "output_path_changed", m._pinned_backup_still_has_one_link,
                              {"path": path, "identity": identity, "pin": handle})
            assert "hard-link alias" not in str(refusal.code)
        finally:
            os.close(handle)


def test_fresh_output_parent_rename_reports_an_unlocated_owned_descriptor(m):
    """A parent rename preserves a newly created inode under a name cleanup
    cannot discover. The failure must disclose that fact rather than calling
    stale-path ENOENT a successful rollback."""
    if os.name == "nt":
        return
    with tempfile.TemporaryDirectory() as directory:
        root = pathlib.Path(directory) / "before"
        moved = pathlib.Path(directory) / "after"
        root.mkdir()
        destination = root / "output.xml"

        def rename_parent():
            os.rename(root, moved)

        try:
            m.write_outputs([(str(destination), "new bytes")], after_claim=rename_parent)
            raise AssertionError("a moved fresh output must refuse before commit")
        except m.Refusal as refusal:
            assert refusal.category == "output_path_changed"
            assert "owned output could not be located after cleanup" in str(refusal.code)

        retained = moved / "output.xml"
        assert retained.read_text() == "new bytes"


def test_fresh_output_parent_rename_with_foreign_replacement_reports_only_owned_uncertainty(m):
    """A stale name can be reclaimed after its parent moves. Cleanup must not
    remove or present that foreign file as the location of our pinned output."""
    if os.name == "nt":
        return
    with tempfile.TemporaryDirectory() as directory:
        root = pathlib.Path(directory) / "before"
        moved = pathlib.Path(directory) / "after"
        root.mkdir()
        destination = root / "output.xml"

        def rename_parent_and_reclaim_old_name():
            os.rename(root, moved)
            root.mkdir()
            destination.write_text("foreign bytes")

        try:
            m.write_outputs(
                [(str(destination), "new bytes")], after_claim=rename_parent_and_reclaim_old_name)
            raise AssertionError("a moved fresh output must refuse before commit")
        except m.Refusal as refusal:
            assert refusal.category == "output_path_changed"
            detail = str(refusal.code)
            assert "owned output could not be located after cleanup" in detail
            assert "retained path(s): " + str(destination) not in detail

        assert destination.read_text() == "foreign bytes"
        assert (moved / "output.xml").read_text() == "new bytes"


def test_writer_refuses_when_the_private_backup_path_is_reclaimed(m):
    """The commit boundary must still name this run's backup; if it does not,
    do not overwrite the destination without a recoverable owned copy."""
    with tempfile.TemporaryDirectory() as directory:
        root = pathlib.Path(directory)
        destination = root / "previous.xml"
        foreign = root / "foreign.xml"
        destination.write_text("old bytes")
        real_copy = m._copy_private_backup

        def reclaim_backup_after_copy(src, identity, backup_handle):
            result = real_copy(src, identity, backup_handle)
            backup, = root.glob("previous.xml.*.bak")
            foreign.write_text("foreign writer bytes")
            os.replace(foreign, backup)
            return result

        m._copy_private_backup = reclaim_backup_after_copy
        try:
            refusal = refuses(m, "output_path_changed", m.write_outputs,
                              [(str(destination), "new bytes")])
        finally:
            m._copy_private_backup = real_copy

        backups = list(root.glob("previous.xml.*.bak"))
        assert destination.read_text() == "old bytes"
        assert len(backups) == 1
        assert backups[0].read_text() == "foreign writer bytes"
        assert str(backups[0]) in str(refusal.code)


def test_rollback_keeps_a_foreign_destination_and_private_backup(m):
    """When an external writer replaces an already-swapped destination before
    another target fails, rollback must retain the owned backup rather than
    overwriting that writer's bytes."""
    with tempfile.TemporaryDirectory() as directory:
        root = pathlib.Path(directory)
        first = root / "first.xml"
        second = root / "second.csv"
        foreign = root / "foreign.xml"
        first.write_text("first old")
        second.write_text("second old")
        real_replace = m.os.replace
        real_open_regular = m._open_regular_output
        real_owned_path = m._owned_path
        real_close = m.os.close
        swaps = []
        first_handles, closed_handles = [], []
        owned_handles = {}

        def observe_first_handles(path, identity):
            handle = real_open_regular(path, identity)
            if os.path.samefile(path, first):
                first_handles.append(handle)
            return handle

        def observe_closes(handle):
            closed_handles.append(handle)
            return real_close(handle)

        def observe_owned_path(path, handle, *, created):
            record = real_owned_path(path, handle, created=created)
            if os.path.basename(path).startswith("first.xml."):
                owned_handles[pathlib.Path(path).suffix] = record["pin"]
            return record

        def replace_then_conflict(src, dst):
            if str(src).endswith(".part") and os.path.basename(dst) == "first.xml":
                swaps.append("first")
                assert len(first_handles) == 2
                # The first open is the ownership pin captured before the
                # backup read. It must survive the first swap and the foreign
                # replacement so that its inode cannot be recycled.
                assert first_handles[0] not in closed_handles
                assert first_handles[1] in closed_handles
                assert owned_handles[".part"] not in closed_handles
                assert owned_handles[".bak"] not in closed_handles
                result = real_replace(src, dst)
                foreign.write_text("foreign writer bytes")
                real_replace(foreign, first)
                return result
            if str(src).endswith(".part") and os.path.basename(dst) == "second.csv":
                swaps.append("second")
                raise OSError("simulated failure after foreign writer")
            return real_replace(src, dst)

        m.os.replace = replace_then_conflict
        m._open_regular_output = observe_first_handles
        m._owned_path = observe_owned_path
        m.os.close = observe_closes
        diagnostic_notes = ""
        try:
            try:
                m.write_outputs([(str(first), "new first"),
                                 (str(second), "new second")])
                raise AssertionError("the second target's swap must fail")
            except OSError as error:
                diagnostic_notes = "\n".join(
                    str(note) for note in getattr(error, "__notes__", [])
                )
        finally:
            m.os.replace = real_replace
            m._open_regular_output = real_open_regular
            m._owned_path = real_owned_path
            m.os.close = real_close

        backups = list(root.glob("first.xml.*.bak"))
        assert len(backups) == 1
        assert str(backups[0]) in diagnostic_notes
        assert swaps == ["first", "second"]
        assert first_handles[0] in closed_handles
        assert owned_handles[".part"] in closed_handles
        assert owned_handles[".bak"] in closed_handles
        assert stat.S_IMODE(backups[0].stat().st_mode) == 0o600
        assert backups[0].read_text() == "first old"
        assert first.read_text() == "foreign writer bytes"
        assert second.read_text() == "second old"


def test_original_inode_pin_blocks_after_claim_replacement_before_backup(m):
    """A same-name replacement after claim cannot make backup copy foreign bytes."""
    if os.name == "nt":
        return  # Existing destinations are deliberately refused on Windows.
    with tempfile.TemporaryDirectory() as directory:
        root = pathlib.Path(directory)
        destination = root / "output.xml"
        foreign = root / "foreign.xml"
        destination.write_text("original bytes")
        real_open = m._open_regular_output
        original_handles = []

        def observe_original(path, identity):
            handle = real_open(path, identity)
            if os.path.realpath(path) == os.path.realpath(destination):
                original_handles.append(handle)
            return handle

        def replace_after_claim():
            assert original_handles, "the original inode must be pinned before the hook"
            assert os.pread(original_handles[0], 32, 0) == b"original bytes"
            foreign.write_text("foreign bytes")
            os.replace(foreign, destination)
            assert os.pread(original_handles[0], 32, 0) == b"original bytes"

        m._open_regular_output = observe_original
        try:
            refusal = refuses(
                m,
                "output_path_changed",
                m.write_outputs,
                [(str(destination), "new bytes")],
                False,
                replace_after_claim,
            )
        finally:
            m._open_regular_output = real_open
        assert "changed" in str(refusal.code)
        assert destination.read_text() == "foreign bytes"
        assert original_handles
        try:
            os.fstat(original_handles[0])
        except OSError:
            pass
        else:
            raise AssertionError("the original pin must be closed during recovery")


def test_committed_close_after_effect_does_not_report_missing_backup(m):
    with tempfile.TemporaryDirectory() as directory:
        destination = pathlib.Path(directory) / "output.xml"
        destination.write_text("old bytes")
        real_close = m.os.close
        real_owned = m._owned_path
        backup_handles = set()
        fired = False

        def observe_owned(path, handle, *, created):
            record = real_owned(path, handle, created=created)
            if str(path).endswith(".bak"):
                backup_handles.add(handle)
            return record

        def close_then_error(handle):
            nonlocal fired
            real_close(handle)
            if handle in backup_handles and not fired:
                fired = True
                raise OSError("controlled close after effect")

        m._owned_path, m.os.close = observe_owned, close_then_error
        try:
            m.write_outputs([(str(destination), "new bytes")])
        finally:
            m._owned_path, m.os.close = real_owned, real_close
        assert fired
        assert destination.read_text() == "new bytes"
        assert list(pathlib.Path(directory).iterdir()) == [destination]


def test_existing_hard_link_output_is_refused_with_topology_unchanged(m):
    with tempfile.TemporaryDirectory() as directory:
        destination = pathlib.Path(directory) / "output.xml"
        alias = pathlib.Path(directory) / "alias.xml"
        destination.write_text("old bytes")
        os.link(destination, alias)
        refuses(m, "output_has_multiple_links", m.write_outputs,
                [(str(destination), "new bytes")])
        assert os.path.samefile(destination, alias)
        assert destination.read_text() == alias.read_text() == "old bytes"
        assert sorted(p.name for p in pathlib.Path(directory).iterdir()) == ["alias.xml", "output.xml"]


def test_preswap_abort_restores_access_time_on_the_original_pin(m):
    # Inject the access-time effect explicitly so this covers noatime hosts too.
    for abort_before_replace in (True, False):
        with tempfile.TemporaryDirectory() as directory:
            destination = pathlib.Path(directory) / "output.xml"
            destination.write_text("old bytes")
            old_atime = 1_600_000_000_000_000_000
            old_mtime = 1_700_000_000_000_000_000
            os.utime(destination, ns=(old_atime, old_mtime))
            real_copy, real_replace = m._copy_private_backup, m.os.replace

            def copy_with_access_time_effect(*args):
                real_copy(*args)
                os.utime(destination, ns=(old_mtime, old_mtime))
                if abort_before_replace:
                    raise OSError("controlled copy abort")

            def refuse_replace(*_args):
                raise OSError("controlled replace before effect")

            m._copy_private_backup, m.os.replace = copy_with_access_time_effect, refuse_replace
            try:
                try:
                    m.write_outputs([(str(destination), "new bytes")])
                    raise AssertionError("the controlled abort must escape")
                except OSError as error:
                    assert str(error).startswith("controlled")
            finally:
                m._copy_private_backup, m.os.replace = real_copy, real_replace
            final_stat = destination.stat()
            assert final_stat.st_atime_ns == old_atime
            assert final_stat.st_mtime_ns == old_mtime
            assert destination.read_text() == "old bytes"
            assert list(pathlib.Path(directory).iterdir()) == [destination]


def test_rollback_restores_original_output_metadata(m):
    """A caught later swap failure restores the former bytes and portable
    metadata, while a retained pre-rollback backup stays private."""
    with tempfile.TemporaryDirectory() as directory:
        root = pathlib.Path(directory)
        first = root / "first.xml"
        second = root / "second.csv"
        first.write_text("first old")
        second.write_text("second old")
        first.chmod(0o640)
        old_atime_ns = 1_600_000_000_123_456_789
        old_mtime_ns = 1_700_000_000_123_456_789
        os.utime(first, ns=(old_atime_ns, old_mtime_ns))
        xattr_name = "user.bridge_rollback_test"
        xattr_value = b"original metadata"
        preserves_xattr = False
        if hasattr(os, "setxattr"):
            try:
                os.setxattr(first, xattr_name, xattr_value)
                preserves_xattr = True
            except OSError:
                pass
        real_replace = m.os.replace

        def fail_second_swap(src, dst):
            if str(src).endswith(".part") and os.path.basename(dst) == "second.csv":
                raise OSError("controlled second swap failure")
            return real_replace(src, dst)

        m.os.replace = fail_second_swap
        try:
            try:
                m.write_outputs([(str(first), "new first"),
                                 (str(second), "new second")])
                raise AssertionError("the controlled swap failure must escape")
            except OSError as error:
                assert "controlled second swap failure" in str(error)
        finally:
            m.os.replace = real_replace

        restored = first.stat()
        assert first.read_text() == "first old"
        assert stat.S_IMODE(restored.st_mode) == 0o640
        assert restored.st_atime_ns == old_atime_ns
        assert restored.st_mtime_ns == old_mtime_ns
        if preserves_xattr:
            assert os.getxattr(first, xattr_name) == xattr_value
        assert second.read_text() == "second old"
        assert sorted(path.name for path in root.iterdir()) == ["first.xml", "second.csv"]


def test_a_case_insensitive_collision_is_refused_before_anything_is_written(m):
    """`--out Result.xml --manifest result.XML` is one file on a case-insensitive
    volume. The lexical preflight cannot see it and `samefile` needs both paths
    to exist, so this used to be caught only *after* the XML had been written
    and the manifest had truncated it. Both destinations are now created empty
    first, which is when the filesystem can answer."""
    class Args:
        pdf = mapping = None

    with tempfile.TemporaryDirectory() as directory:
        probe = pathlib.Path(directory, "Aa.probe")
        probe.write_text("")
        if not pathlib.Path(directory, "aa.probe").exists():
            return  # case-sensitive volume
        probe.unlink()

        args = Args()
        args.out = str(pathlib.Path(directory, "Result.xml"))
        args.manifest = str(pathlib.Path(directory, "result.XML"))
        refuses(m, "path_collision", m.write_outputs,
                [(args.out, "<xml/>"), (args.manifest, "row\n")], False,
                lambda: m._check_paths(args))
        # neither payload reached the disk, and the rollback left nothing
        assert not pathlib.Path(args.out).exists(), "rolled back"


def test_an_ach_reference_must_be_reference_shaped(m):
    """"Hyphen then digits" does not distinguish a bank reference from a name.
    `STUDIO-54` resolved to `STUDIO`, and so did `STUDIO-5 4` once the pattern
    tolerated a wrap — so a mapping for `STUDIO` silently posted a `STUDIO-54`
    transaction to the wrong ledger. A reference-length run of digits is
    required; anything shorter is UNRESOLVED and reaches suspense, where an
    operator sees it."""
    def party(narr):
        return m.HDFC.party(m.HDFC, {"narr": narr, "narr_spaced": narr})

    # a name's own hyphenated number is never a delimiter.
    #
    # The last four are the ones that killed a *reasoned* threshold. An earlier
    # version required six digits, arguing that a number inside a name is a unit
    # or a year and so at most four — which overlooked the most ordinary six
    # digits in an Indian address. A PIN code is six, and is routinely printed
    # with a space, so `ACME-400 001` resolved to `ACME`. The rule is now the
    # observed reference length, not an argument about where a gap ought to sit.
    for tail in ["STUDIO-54", "UNIT-7", "SHOP-2024", "STUDIO-5 4", "SHOP-1 2 3",
                 "ACME-400 001", "ACME-400001", "TRADERS-560 034", "CORP-110001"]:
        assert party(f"ACH D- TP ACH {tail}") == "UNRESOLVED", tail

    # ...and a run that is merely *long* is not a reference either. Only the
    # observed length is one; anything else reaches suspense.
    for digits in ["123456789", "12345678901", "123456789012"]:
        assert party(f"ACH D- TP ACH ACME-{digits}") == "UNRESOLVED", digits

    # a real reference still is, wrapped or not — including a wrap immediately
    # after the delimiter, where the line ends at the hyphen itself
    for tail, expected in [
        ("STUDIO-54 INDUSTRIES-1234567890", "STUDIO-54 INDUSTRIES"),
        ("ACME TRADERS-12345 67890", "ACME TRADERS"),
        ("ACME TRADERS- 1234567890", "ACME TRADERS"),
        ("STUDIO-54 INDUSTRIES- 1234567890", "STUDIO-54 INDUSTRIES"),
        ("UNIT-7 METALS-12 345 67890", "UNIT-7 METALS"),
    ]:
        assert party(f"ACH D- TP ACH {tail}") == expected, tail


def test_ledger_key_folds_exactly_what_its_docstring_claims(m):
    """`_ledger_key` is looser than §9.4b, and the docstring says so with a
    table. Pin the table, because the hazard here is not the behaviour — it
    fails safe at all three call sites — but somebody copying the function into
    a binder on the strength of a docstring that used to call it "Tally's own
    master-name identity".

    A change in either direction should be deliberate: tightening it breaks the
    verified rows, loosening it adds a row Tally has never been shown to fold.
    """
    same = lambda a, b: m._ledger_key(a) == m._ledger_key(b)

    # VERIFIED by 9.4b — these must keep folding.
    assert same("bridge probe ledger a", "BRIDGE PROBE LEDGER A")
    assert same("BRIDGE PROBE LEDGER A", "BRIDGE-PROBE-LEDGER-A")
    assert same("A B ", "A B")

    # UNVERIFIED by 9.4b, folded here anyway. Safe only because nothing in this
    # tool resolves against a master list; recorded so it cannot drift silently.
    assert same("A-B", "A B"), "the reverse hyphen direction"
    assert same("A  B", "A B"), "internal whitespace run"
    assert same("  A B", "A B"), "leading whitespace"
    # ...and the three the first version of this table missed, because they are
    # properties of `.upper()` and `_squash` rather than of anything written in
    # `_ledger_key`. A table that lists only the deliberate folds understates
    # the function to exactly the reader most likely to copy it.
    assert same("A   ", "A"), "arbitrary trailing whitespace, not one space"
    assert same("straße", "STRASSE"), "str.upper() is Unicode, not ASCII, and changes length"
    assert same("A\tB", "A B"), "tab folds to a space"
    assert same("A\u00a0B", "A B"), "NBSP folds to a space"

    # Must stay apart. Only the first was measured as rejected; the rest were
    # never sent at all, and this fold happening to keep them apart is not
    # evidence that Tally does.
    assert not same("ZZ Ram AND Sons", "ZZ Ram & Sons"), "measured: rejected"
    assert not same("AB", "A & B"), "deleting & was never measured either way"
    assert not same("A_B", "A B")
    assert not same("A/B", "A B")
    assert not same("A\u2013B", "A B"), "en dash is not an ASCII hyphen"


def test_write_outputs_refuses_a_hard_link_added_after_backup_copy(m):
    """The commit check must see a link created by a backup-time hook."""
    with tempfile.TemporaryDirectory() as directory:
        root = pathlib.Path(directory)
        destination = root / "output.xml"
        alias = root / "alias.xml"
        destination.write_text("old bytes")
        real_copy = m._copy_private_backup

        def add_link_after_backup(*args):
            result = real_copy(*args)
            os.link(destination, alias)
            return result

        m._copy_private_backup = add_link_after_backup
        try:
            refuses(
                m,
                "output_has_multiple_links",
                m.write_outputs,
                [(str(destination), "new bytes")],
            )
        finally:
            m._copy_private_backup = real_copy

        assert os.path.samefile(destination, alias)
        assert destination.read_text() == alias.read_text() == "old bytes"
        assert sorted(path.name for path in root.iterdir()) == ["alias.xml", "output.xml"]


def test_write_outputs_refuses_a_hard_link_added_to_the_private_backup(m):
    """The generated backup is ownership-only until commit. A link made after
    its copy finishes leaves an unknown alias with old statement bytes, so the
    run must refuse and describe the retention rather than report success."""
    with tempfile.TemporaryDirectory() as directory:
        root = pathlib.Path(directory)
        destination = root / "output.xml"
        alias = root / "backup-alias.xml"
        destination.write_text("old bytes")
        real_copy = m._copy_private_backup

        def add_link_after_backup_copy(*args):
            result = real_copy(*args)
            backup, = root.glob("output.xml.*.bak")
            os.link(backup, alias)
            return result

        m._copy_private_backup = add_link_after_backup_copy
        try:
            refusal = refuses(
                m,
                "rollback_backup_has_multiple_links",
                m.write_outputs,
                [(str(destination), "new bytes")],
            )
        finally:
            m._copy_private_backup = real_copy

        assert "unknown hard-link alias" in str(refusal.code)
        assert destination.read_text() == "old bytes"
        assert alias.read_text() == "old bytes"
        assert not list(root.glob("output.xml.*.bak"))


def test_committed_new_output_close_failure_is_not_a_retained_backup(m):
    """A failed ownership-pin close does not create a prior-output backup."""
    with tempfile.TemporaryDirectory() as directory:
        destination = pathlib.Path(directory) / "output.xml"
        real_close = m.os.close
        real_owned = m._owned_path
        output_handles = set()
        fired = False

        def observe_owned(path, handle, *, created):
            record = real_owned(path, handle, created=created)
            if created and pathlib.Path(path).resolve() == destination.resolve():
                output_handles.add(handle)
            return record

        def close_then_error(handle):
            nonlocal fired
            real_close(handle)
            if handle in output_handles and not fired:
                fired = True
                raise OSError("controlled close after effect")

        m._owned_path, m.os.close = observe_owned, close_then_error
        try:
            try:
                m.write_outputs([(str(destination), "new bytes")])
                raise AssertionError("the controlled close failure must escape")
            except m.OutputDescriptorCloseFailure as failure:
                assert failure.output_paths == (str(destination.resolve()),)
                assert "prior output retained" not in str(failure)
        finally:
            m._owned_path, m.os.close = real_owned, real_close

        assert fired
        assert destination.read_text() == "new bytes"
        assert list(pathlib.Path(directory).iterdir()) == [destination]


def test_new_output_unlink_after_claim_refuses_and_cleans_owned_canonical_path(m):
    with tempfile.TemporaryDirectory() as directory:
        destination = pathlib.Path(directory) / "output.xml"
        def unlink_after_claim():
            destination.unlink()
        refusal = refuses(m, "output_path_changed", m.write_outputs,
                          [(str(destination), "new bytes")], False, unlink_after_claim)
        assert "owned output could not be located after cleanup" not in str(refusal.code)
        assert not destination.exists()


def test_new_output_parent_retarget_refuses_and_preserves_foreign_path(m):
    with tempfile.TemporaryDirectory() as directory:
        root = pathlib.Path(directory)
        original, foreign = root / "original", root / "foreign"
        original.mkdir()
        foreign.mkdir()
        foreign_destination = foreign / "output.xml"
        foreign_destination.write_text("foreign bytes")
        destination = root / "linked" / "output.xml"
        (root / "linked").symlink_to(original, target_is_directory=True)
        def retarget_parent():
            (root / "linked").unlink()
            (root / "linked").symlink_to(foreign, target_is_directory=True)
        refuses(m, "output_path_changed", m.write_outputs,
                [(str(destination), "new bytes")], False, retarget_parent)
        assert foreign_destination.read_text() == "foreign bytes"
        assert original.exists() and not (original / "output.xml").exists()
        assert (root / "linked").is_symlink(), "foreign retarget must not be deleted"
        assert (root / "linked").joinpath("output.xml").read_text() == "foreign bytes"


def test_new_output_replace_after_claim_preserves_foreign_bytes(m):
    with tempfile.TemporaryDirectory() as directory:
        root = pathlib.Path(directory)
        destination, foreign = root / "output.xml", root / "foreign.xml"
        def replace_after_claim():
            foreign.write_text("foreign bytes")
            os.replace(foreign, destination)
        refuses(m, "output_path_changed", m.write_outputs,
                [(str(destination), "new bytes")], False, replace_after_claim)
        assert destination.read_text() == "foreign bytes"


def test_new_output_claim_inspection_failure_cleans_owned_path(m):
    with tempfile.TemporaryDirectory() as directory:
        destination = pathlib.Path(directory) / "output.xml"
        real_identity = m._file_identity
        def fail_identity(path):
            if pathlib.Path(path).resolve() == destination.resolve():
                raise OSError("controlled claim inspection failure")
            return real_identity(path)
        m._file_identity = fail_identity
        try:
            refuses(m, "output_path_changed", m.write_outputs,
                    [(str(destination), "new bytes")])
        finally:
            m._file_identity = real_identity
        assert not destination.exists()


def test_new_output_parent_retarget_during_open_cleans_actual_created_path(m):
    with tempfile.TemporaryDirectory() as directory:
        root = pathlib.Path(directory)
        original, foreign = root / "original", root / "foreign"
        original.mkdir()
        foreign.mkdir()
        link = root / "linked"
        link.symlink_to(original, target_is_directory=True)
        foreign_destination = foreign / "output.xml"
        foreign_destination.write_text("foreign bytes")
        real_open = m._open_private
        def retarget_open(path, accept_inherited):
            link.unlink()
            link.symlink_to(foreign, target_is_directory=True)
            return real_open(path, accept_inherited)
        m._open_private = retarget_open
        try:
            refuses(m, "output_path_changed", m.write_outputs,
                    [(str(link / "output.xml"), "new bytes")])
        finally:
            m._open_private = real_open
        assert not (original / "output.xml").exists()
        assert foreign_destination.read_text() == "foreign bytes"


def test_new_output_changed_during_later_swap_rolls_back_existing_output(m):
    with tempfile.TemporaryDirectory() as directory:
        root = pathlib.Path(directory)
        fresh, existing = root / "fresh.xml", root / "existing.xml"
        existing.write_text("old bytes")
        real_replace = m.os.replace
        def replace_then_unlink(source, destination):
            result = real_replace(source, destination)
            if pathlib.Path(destination).resolve() == existing.resolve() and str(source).endswith(".part"):
                fresh.unlink()
            return result
        m.os.replace = replace_then_unlink
        try:
            refuses(m, "output_path_changed", m.write_outputs,
                    [(str(fresh), "new fresh"), (str(existing), "new existing")])
        finally:
            m.os.replace = real_replace
        assert not fresh.exists()
        assert existing.read_text() == "old bytes"
        assert list(root.iterdir()) == [existing]


def test_staged_output_hard_link_before_commit_refuses_and_reports_alias(m):
    with tempfile.TemporaryDirectory() as directory:
        root = pathlib.Path(directory)
        destination, alias = root / "output.xml", root / "alias.xml"
        destination.write_text("old bytes")
        def link_staged():
            staged, = root.glob("output.xml.*.part")
            os.link(staged, alias)
        refusal = refuses(m, "staged_output_has_multiple_links", m.write_outputs,
                          [(str(destination), "new bytes")], False, link_staged)
        assert "unknown hard-link alias" in str(refusal.code)
        assert destination.read_text() == "old bytes"
        assert alias.read_text() == "new bytes"
        assert not list(root.glob("*.part"))
        assert not list(root.glob("*.bak"))


def test_fresh_output_hard_link_before_commit_refuses_and_reports_alias(m):
    with tempfile.TemporaryDirectory() as directory:
        root = pathlib.Path(directory)
        destination, alias = root / "output.xml", root / "alias.xml"
        refusal = refuses(m, "output_has_multiple_links", m.write_outputs,
                          [(str(destination), "new bytes")], False,
                          lambda: os.link(destination, alias))
        assert "unknown hard-link alias" in str(refusal.code)
        assert not destination.exists()
        assert alias.read_text() == "new bytes"


def test_staged_parent_rename_reports_unlocated_output(m):
    if os.name == "nt":
        return
    with tempfile.TemporaryDirectory() as directory:
        root, moved = pathlib.Path(directory) / "before", pathlib.Path(directory) / "after"
        root.mkdir()
        destination = root / "output.xml"
        destination.write_text("old bytes")
        stderr = io.StringIO()
        with contextlib.redirect_stderr(stderr):
            try:
                m.write_outputs([(str(destination), "new bytes")],
                                after_claim=lambda: root.rename(moved))
                raise AssertionError("missing backup parent must fail")
            except FileNotFoundError as error:
                detail = stderr.getvalue() + "\n".join(getattr(error, "__notes__", []))
                assert "owned output could not be located after cleanup" in detail
        staged, = moved.glob("output.xml.*.part")
        assert staged.read_text() == "new bytes"
        assert (moved / "output.xml").read_text() == "old bytes"


def test_earlier_replacement_is_revalidated_after_later_swap(m):
    for change in ("replace", "unlink", "symlink"):
        with tempfile.TemporaryDirectory() as directory:
            root = pathlib.Path(directory)
            first, second, foreign = root / "first.xml", root / "second.xml", root / "foreign.xml"
            first.write_text("old first")
            second.write_text("old second")
            foreign.write_text("foreign bytes")
            supplied = root / "first-link.xml" if change == "symlink" else first
            if change == "symlink":
                supplied.symlink_to(first)
            real_replace = m.os.replace
            changed_first = []
            def replace_then_change_first(source, destination):
                result = real_replace(source, destination)
                if (str(source).endswith(".part")
                        and pathlib.Path(destination).resolve() == second.resolve()):
                    changed_first.append(change)
                    if change == "replace":
                        real_replace(foreign, first)
                    elif change == "unlink":
                        first.unlink()
                    else:
                        supplied.unlink()
                        supplied.symlink_to(foreign)
                return result
            m.os.replace = replace_then_change_first
            try:
                refusal = refuses(m, "output_path_changed", m.write_outputs,
                                  [(str(supplied), "new first"), (str(second), "new second")])
            finally:
                m.os.replace = real_replace
            assert changed_first == [change], "interleave must run after the second swap"
            assert second.read_text() == "old second"
            assert "owned output could not be located after cleanup" not in str(refusal.code)
            if change == "replace":
                assert first.read_text() == "foreign bytes"
            elif change == "unlink":
                assert not first.exists()
            else:
                assert first.read_text() == "old first"
                assert supplied.read_text() == "foreign bytes"


def test_earlier_backup_alias_is_revalidated_after_later_swap(m):
    with tempfile.TemporaryDirectory() as directory:
        root = pathlib.Path(directory)
        first, second, alias = root / "first.xml", root / "second.xml", root / "alias.xml"
        first.write_text("old first")
        second.write_text("old second")
        real_replace = m.os.replace
        linked = []
        def replace_then_link_first_backup(source, destination):
            result = real_replace(source, destination)
            if (str(source).endswith(".part")
                    and pathlib.Path(destination).resolve() == second.resolve()):
                backup, = root.glob("first.xml.*.bak")
                os.link(backup, alias)
                linked.append(True)
            return result
        m.os.replace = replace_then_link_first_backup
        try:
            refusal = refuses(m, "rollback_backup_has_multiple_links", m.write_outputs,
                              [(str(first), "new first"), (str(second), "new second")])
        finally:
            m.os.replace = real_replace
        assert linked == [True]
        assert "unknown hard-link alias" in str(refusal.code)
        assert first.read_text() == "old first"
        assert second.read_text() == "old second"
        assert alias.read_text() == "old first"
        assert not list(root.glob("*.bak"))


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
