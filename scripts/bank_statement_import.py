"""Turn a password-protected bank-statement PDF into TallyPrime import XML.

OPERATOR TOOL — offline. It shells out to `pdftotext` and writes XML to disk.
It never contacts Tally, a bank, or any network service; the resulting file is
imported by hand through Gateway of Tally > Import > Vouchers.

No client data lives in this repository. The statement PDF, its password and the
ledger mapping are all supplied at run time.

Why the parsing is done from word coordinates rather than `pdftotext -layout`:
bank statements render the narration into a narrow fixed-width cell, so the PDF
producer wraps long tokens *mid-token*. A 12-digit UPI reference arrives split
across two lines and a naive join corrupts it in one of two directions — glue
every line and real spaces vanish, insert a space everywhere and the reference
breaks. The producer only breaks mid-token when the token itself fills the cell,
so a line whose right edge reaches the cell boundary was a hard wrap and joins
with no separator, and any shorter line ended at a real space. See `_dewrap`.

XML shape and its hazards are documented in docs/tally/TALLY_PROTOCOL_REFERENCE.md
section 9. The ones this module is built around:

  9.1b  every text node must be XML-escaped; a single unescaped '&' makes the
        request malformed and Tally rejects the whole file with no field hint.
  9.8   under automatic numbering Tally silently discards a supplied
        VOUCHERNUMBER, so this module does not send one and puts the bank's
        reference in the narration, where it survives.

  Idempotency: IMPLEMENTATION_GUIDE.md 3.3a — a re-import carrying the same
  client-supplied REMOTEID *upserts* (CREATED=0, ALTERED=1); it does not
  duplicate. That is a correction path and a hazard at once, which is why
  `_remote_id` keys on the transaction's own content and not on its position
  in the file. Two different transactions must never collide on a REMOTEID:
  the second would silently overwrite the first.
  (9.3 in the protocol reference still says "no idempotency". It is stale —
  it was measured without a client-supplied REMOTEID. 3.3a supersedes it.)

  **UNVERIFIED for the voucher types this tool emits.** 9.8 bounds that
  evidence itself: it qualifies an exact-file repeat on the licensed *Journal*
  path and "does not establish ... other request shapes or voucher types,
  restart behavior, or universal REMOTEID semantics". This tool emits Payment,
  Receipt and Contra. The REMOTEID is carried because it is the best available
  key and because Delete-by-REMOTEID (9.7) is the nearest thing to a documented
  correction path — NOT because upsert-on-repeat has been shown for these types.
  A corrected re-import is a *different* payload, which is a further step
  again: 3.3a's own untested list includes "when the payload differs from the
  original (partial update semantics)". So re-importing a corrected file may
  overwrite, may partially update, or may duplicate.

  Delete-by-REMOTEID is not qualified here either. 9.7's Delete row was
  measured on an Education/Edit Log instance, not on a licensed Gold book, and
  not with the client key on these voucher types. The manifest carries the
  REMOTEIDs so that path is *available* to try on one voucher — not so it can
  be run over a batch. Neither correction path is routine; `main` says so.

  Qualifying a voucher type takes the import summary, not a voucher count: a
  repeat that Tally rejected also leaves the count unchanged. Read the second
  import's counters — ALTERED, with CREATED and the failure counters zero —
  and then read the voucher back.

  Attribution: on the one path where it was checked, a Voucher read returned
  Tally's own <company GUID>-<master id> in the REMOTEID attribute rather than
  the value sent, so the key may not be readable back. Do not substitute a
  date/ledger/amount fingerprint for it — a book with a recurring same-day
  payment already contains a voucher with that tuple, and a pre-existing one
  would stand in for a write that never happened. The narration this tool
  writes carries the bank's own transaction reference, which is the
  independent marker to match on.

  Sign convention: a debit is ISDEEMEDPOSITIVE Yes with a NEGATIVE amount.

WHAT THIS TOOL CANNOT CHECK, AND WHAT IT DOES INSTEAD

It is offline, so it cannot ask Tally which company is open — and per 9.11d
`SVCURRENTCOMPANY` is *not* a write guard. What is measured there: a name that
did NOT match the loaded company still posted into it, silently. Which kinds of
mismatch behave that way is *not* settled — 9.11d marks that reading a
hypothesis, because the box's company list was never enumerated — and a
separate measurement had an existing-but-unloaded name fail closed.

The uncertainty does not soften the conclusion, it hardens it: there is a
verified silent-misdirection case and no rule saying when it applies, so the
target cannot be verified from here at all. `--confirm-open-company` makes
confirming it a deliberate operator act instead of a silent one. See `main`.

Everything else fails closed. The design rule throughout: a malformed input, an
ambiguous mapping or an unproven extent must stop the run, because every output
of this tool is posted to a real book.
"""

import argparse
import contextlib
import csv
import datetime
import decimal
import getpass
import hashlib
import io
import os
import pathlib
import re
import shutil
import stat
import subprocess
import sys
import tempfile
import unicodedata
import xml.etree.ElementTree as ET

D = decimal.Decimal
WORD = re.compile(
    r'<word xMin="([\d.]+)" yMin="([\d.]+)" xMax="([\d.]+)" yMax="([\d.]+)">(.*?)</word>',
    re.S,
)
PASSWORD_ENV = "BANK_STATEMENT_PASSWORD"


class Refusal(SystemExit):
    """A fail-closed stop, carrying a stable category.

    The category is the contract: tests assert on it, so a refusal cannot be
    satisfied by some *other* refusal firing first. The message is for the
    operator and may change freely.
    """

    def __init__(self, category, message):
        self.category = category
        super().__init__(f"{category}: {message}")


class OutputCleanupFailure(OSError):
    """Committed output is present, but an old sensitive copy remains.

    `retained_paths` gives the operator the exact private backup location to
    protect or remove.  A normal return would conceal that copy.
    """

    def __init__(self, retained_paths):
        self.retained_paths = tuple(retained_paths)
        super().__init__(
            "output cleanup failed; prior output retained at "
            + ", ".join(self.retained_paths)
        )


# --------------------------------------------------------------------------- #
# PDF -> rows                                                                  #
# --------------------------------------------------------------------------- #

def _unescape(text):
    for entity, char in (("&lt;", "<"), ("&gt;", ">"),
                         ("&quot;", '"'), ("&apos;", "'"), ("&amp;", "&")):
        text = text.replace(entity, char)
    return text


def _pdf_pages(pdf, password):
    """Word boxes per page. Tries the user password then the owner password:
    banks commonly protect statements with the owner password only, which
    `pdftotext -upw` rejects."""
    if shutil.which("pdftotext") is None:
        raise Refusal(
            "missing_pdftotext",
            "pdftotext is not on PATH. It ships with Poppler: "
            "'brew install poppler' (macOS), 'apt install poppler-utils' (Debian/Ubuntu), "
            "or the poppler-utils package for your distribution.",
        )
    with tempfile.TemporaryDirectory() as tmp:
        out = pathlib.Path(tmp) / "stmt.xml"
        for flag in ("-upw", "-opw"):
            try:
                done = subprocess.run(
                    ["pdftotext", "-bbox-layout", flag, password, str(pdf), str(out)],
                    capture_output=True,
                )
            except FileNotFoundError:  # raced with the which() above
                raise Refusal("missing_pdftotext", "pdftotext disappeared from PATH mid-run")
            if done.returncode == 0:
                xml = out.read_text(encoding="utf-8")
                break
        else:
            raise Refusal("unreadable_pdf", f"cannot open {pdf}: wrong password or not a PDF")
    return xml.split("<page ")[1:]


def _lines(page):
    """Words grouped into visual lines, each sorted left to right."""
    words = []
    for x0, y0, x1, y1, text in WORD.findall(page):
        text = _unescape(text).strip()
        if text:
            words.append((float(x0), float(y0), float(x1), float(y1), text))
    words.sort(key=lambda w: (round(w[1], 1), w[0]))
    grouped = []
    for word in words:
        for line in grouped:
            if abs(line[0] - word[1]) < 3:
                line[1].append(word)
                break
        else:
            grouped.append((word[1], [word]))
    for _, group in grouped:
        group.sort(key=lambda w: w[0])
    return sorted(grouped, key=lambda line: line[0])


def _dewrap(fragments, right_edge, tol=4.0):
    """Rejoin cell-wrapped text. `fragments` is [(text, x_max)] in visual order.

    A fragment whose right edge reaches the cell boundary was broken mid-token,
    so the next fragment joins with no separator; anything shorter ended at a
    real space. This is a heuristic and its one failure mode is a line that
    happens to end at a space exactly at the boundary, which costs a space in
    display text but never corrupts a reference number.
    """
    out = ""
    for i, (text, _) in enumerate(fragments):
        if i == 0:
            out = text
        else:
            hard = fragments[i - 1][1] >= right_edge - tol
            out += ("" if hard else " ") + text
    return re.sub(r"\s+", " ", out).strip()


# --------------------------------------------------------------------------- #
# Bank profiles                                                                #
# --------------------------------------------------------------------------- #

class Bank:
    """Column geometry and row rules for one statement layout.

    `columns` are (x_min, x_max, name) in PDF points. `text_columns` are joined
    with the wrap heuristic; every other column is concatenated with no
    separator, because an amount split across lines ("1,00,000.0" / "0") must
    rejoin exactly.
    """

    name = ""
    columns = ()
    text_columns = {}
    date_column = "date"
    date_pattern = re.compile(r"")
    date_format = ""
    debit_column = "dr"
    credit_column = "cr"
    balance_column = "bal"
    #: the column carrying the bank's own description of the transaction
    narration_column = "narr"
    #: columns only ever populated on the row carrying the date
    row_scoped = ()
    #: a line carrying every token of any group ends the current page's table
    #: (the page footer). Matching whole groups rather than single words keeps a
    #: counterparty called "... LIMITED" from being mistaken for the footer.
    bottom_anchors = ()
    #: ... and these end the statement entirely; later pages are not read
    end_anchors = ()
    #: a page's table starts below the first line containing all tokens of any group
    top_anchors = ()
    #: the label the statement prints beside its account number. The binding
    #: reads digits only from the line carrying every one of these words —
    #: a header block also prints a phone number, a customer id, an IFSC, a
    #: MICR code and a postcode, and a four-digit tail matches one of those
    #: far more often than it matches the account.
    account_anchors = ()
    #: a line whose every word is one of these is column furniture, not data.
    #: SBI's header wraps onto three lines and repeats on every page, so it sits
    #: *below* the anchor and would otherwise be appended to the row in progress.
    header_words = frozenset()

    def column_of(self, x0, x1):
        centre = (x0 + x1) / 2
        for lo, hi, name in self.columns:
            if lo <= centre < hi:
                return name
        return self.columns[-1][2]

    def is_row_start(self, cells):
        cell = cells.get(self.date_column, [])
        return len(cell) == 1 and bool(self.date_pattern.match(cell[0][2]))

    def parse_date(self, text):
        return datetime.datetime.strptime(text, self.date_format).date()

    def party(self, row):
        raise NotImplementedError

    def reference(self, row):
        raise NotImplementedError


class SBI(Bank):
    """State Bank of India current-account statement."""

    name = "sbi"
    columns = ((0, 85, "date"), (85, 140, "vdt"), (140, 220, "narr"),
               (220, 299, "ref"), (299, 356, "branch"), (356, 441, "dr"),
               (441, 506, "cr"), (506, 9999, "bal"))
    text_columns = {"narr": 220.0, "ref": 299.0}
    date_pattern = re.compile(r"^\d{1,2} (?:Jan|Feb|Mar|Apr|May|Jun|Jul|Aug|Sep|Oct|Nov|Dec)$")
    date_format = "%d%b%Y"
    top_anchors = (("Txn",),)
    account_anchors = (("Account", "Number"),)
    header_words = frozenset({"Txn", "Date", "Value", "Description", "Ref",
                              "No./Cheque", "No.", "Branch", "Code", "Debit",
                              "Credit", "Balance"})

    def parse_date(self, text):
        # SBI stacks "31 Jul" over "2026" in one cell; spacing after the join is
        # not stable, so normalise it away before parsing
        return datetime.datetime.strptime(_strip(text), self.date_format).date()

    def is_row_start(self, cells):
        # SBI prints "1 Aug" then "2026" beneath it, so the date cell is two words.
        cell = " ".join(t for _, _, t in cells.get("date", []))
        return bool(self.date_pattern.match(cell.strip()))

    def party(self, row):
        narr, ref = row["narr_spaced"], row["ref_spaced"]
        if "ATM WDL" in narr:
            return "ATM CASH WITHDRAWAL"
        for pattern in (r"^TRANSFER TO \d+\s+(.+?)\s*/\s*\d+$",
                        r"^CT0\S*\s*\S*\s+TRANSFER FROM \d+\s+(.+?)\s*/$"):
            found = re.match(pattern, ref)
            if found:
                return _squash(found.group(1))
        found = re.search(r"/\s*(\S.*)$", ref)
        if found and not found.group(1).strip().isdigit():
            return _squash(found.group(1))
        found = re.match(r"^TRANSFER TO (\d+)\s+(.+?)\s*/$", ref)
        if found and not re.fullmatch(r"[\d\s]+", found.group(2)):
            return _squash(found.group(2))
        found = re.search(r"NEFT\*(.+)$", narr)
        if found:
            parts = [p.strip() for p in found.group(1).split("*")]
            if len(parts) >= 3:
                return _squash(parts[2].rstrip("-"))
        found = re.search(r"RTGS UTR NO:\s*.*?\d+-(.+)$", narr)
        if found:
            return _squash(found.group(1))
        found = re.search(r"UPI/(?:DR|CR)/(.+)$", narr)
        if found:
            parts = found.group(1).split("/")
            if len(parts) >= 2:
                return _squash(parts[1].rstrip("-"))
        found = re.search(r"IMPS/(.+)$", narr)
        if found:
            parts = found.group(1).split("/")
            if len(parts) >= 2:
                inner = re.match(r"^[A-Za-z]+-\s*[Xx]+\d+-\s*(.*)$", parts[1])
                return _squash((inner.group(1) if inner else parts[1]).rstrip("-")) or "UNNAMED"
        return "UNRESOLVED"

    def reference(self, row):
        narr, ref = row["narr"], row["ref"]
        found = re.search(r"UTR NO:\s*([A-Z0-9\s]+?)-", narr)
        if found:
            return ("RTGS" if "RTGS" in narr else "NEFT"), _strip(found.group(1))
        found = re.search(r"NEFT\*[^*]*\*([^*]+)\*", narr)
        if found:
            return "NEFT", _strip(found.group(1))
        found = re.search(r"UPI/(?:DR|CR)/([\d\s]+?)/", narr)
        if found:
            vpa = re.search(r"UPI/(?:DR|CR)/[^/]*/[^/]*/[^/]*/([^/]+)/", narr)
            tail = f" ({_strip(vpa.group(1))})" if vpa else ""
            return "UPI", _strip(found.group(1)) + tail
        found = re.search(r"IMPS/([\d\s]+?)/", narr)
        if found:
            return "IMPS", _strip(found.group(1))
        found = re.search(r"ATM CASH\s*([\d\s]+?)\s*[A-Z]", narr)
        if found:
            return "ATM", _strip(found.group(1))
        found = re.search(r"(CT0[\w\s]{6,10}?)\s*TRANSFER", ref)
        if found:
            return "INB", _strip(found.group(1))
        found = re.search(r"/\s*(\d+)\s*$", ref)
        if found:
            return "CHQ", found.group(1)
        if "INT TRF" in narr:
            found = re.search(r"TO\s*(\d+)", narr)
            return "INT-TRF", _strip(found.group(1)) if found else ""
        return "TXN", ""


# The digit count of an ACH bank reference — **exactly** the observed length,
# not a minimum, because this is the only thing separating a reference from a
# number inside the counterparty's own name.
#
# An earlier version guessed a lower bound of six, reasoning about a "gap"
# between a name's number (a unit or a year) and a reference. That was wrong in
# the most ordinary way available: an Indian PIN code is six digits and is
# routinely printed with a space, so `ACH D- TP ACH ACME-400 001` resolved to
# `ACME` and an `ACME` mapping would post that transaction to the wrong ledger.
# Reasoning about where a threshold "ought" to sit invented a gap that the
# address line walks straight through.
#
# So it is not reasoned, it is observed: every ACH reference seen in a real
# narration is ten digits. Anything else — longer, shorter, wrapped into a
# different length — is UNRESOLVED and reaches suspense, where an operator sees
# it. An unrecognised narration costs a look; a misattributed one announces
# nothing. When a reference of another length is genuinely observed, widen this
# and record the observation.
ACH_REFERENCE_DIGITS = 10
# `-\s*` before the run, and `\s*` between its digits: the cell wraps wherever
# the column edge falls, which includes immediately after the delimiter. See
# finding S1.
ACH_PARTY = re.compile(
    r"^ACH D-\s*TP ACH (.+)-\s*((?:\d\s*){%d})$" % ACH_REFERENCE_DIGITS)


class HDFC(Bank):
    """HDFC Bank current-account statement."""

    name = "hdfc"
    columns = ((0, 70, "date"), (70, 280, "narr"), (280, 358, "ref"),
               (358, 400, "vdt"), (400, 480, "dr"), (480, 560, "cr"),
               (560, 9999, "bal"))
    text_columns = {"narr": 240.0}
    date_pattern = re.compile(r"^\d{2}/\d{2}/\d{2}$")
    date_format = "%d/%m/%y"
    row_scoped = ("ref", "vdt", "dr", "cr", "bal")
    bottom_anchors = (("HDFC", "BANK", "LIMITED"), ("STATEMENT", "SUMMARY"))
    end_anchors = (("STATEMENT", "SUMMARY"),)
    # page 1 repeats the column header; later pages only repeat the period line
    top_anchors = (("Narration",), ("Statement", "account"))
    # "Account No :50200000000000"; "Account Status" and "Account Type" print
    # the same first word, which is why the anchor is both words
    account_anchors = (("Account", "No"),)

    @staticmethod
    def _bounded_party(narr, prefix, is_boundary, skip=0, back=0):
        """The name field of a hyphen-delimited narration, which may itself
        contain hyphens.

        HDFC delimits every field with `-`, so a counterparty called
        `ACME-INDUSTRIES` occupies two fields and a non-greedy `(.+?)-` stops
        at the first of them. That either misses a correct mapping or — worse —
        hits an unrelated ledger that happens to be named `ACME`.

        So the name runs from field `skip` up to the next *structured* field
        found by `is_boundary`, less `back` fields for anything that sits
        between the name and that marker.

        `skip` matters as much as the boundary: an IFSC ahead of the name looks
        exactly like the UTR that terminates it, so scanning from field 0 finds
        the wrong marker and returns nothing. `is_boundary` must be a shape the
        *name* cannot take — a masked account, a VPA, a 12-digit reference —
        never a bare 4-letter code, which is also a plausible company name.

        An empty result means the narration is not the shape this rule was
        written for, and the caller must send it to suspense rather than guess.
        """
        parts = narr[len(prefix):].split("-")
        for index, part in enumerate(parts):
            if index < skip:
                continue
            # the boundary test runs on the field with whitespace removed. The
            # cell wrap that this module exists to undo also lands *inside* a
            # reference — a UTR arrives as "HDF CH12345678901" when the
            # fragment before it stopped short of the cell edge — and a marker
            # that is only recognisable when unbroken is not a marker. Names
            # keep their spaces; only the test strips.
            if is_boundary(_strip(part)):
                return "-".join(parts[skip:max(index - back, skip)]).strip()
        return ""

    @staticmethod
    def _upi_party(narr):
        """Name field of a UPI narration, which may itself contain hyphens.

        HDFC prints `UPI-<name>-<vpa>-<bank code>-<12-digit ref>-<remark>`, and
        a counterparty called `ACME-INDUSTRIES` occupies two hyphen fields. The
        name is therefore bounded by the *next structured field*, never by the
        first hyphen: taking the first hyphen turns `ACME-INDUSTRIES` into
        `ACME`, which either misses a correct mapping or — worse — hits an
        unrelated ledger that happens to be called ACME.

        Two boundaries are recognised, in order of reliability. Neither found
        means the shape is not the one this rule was written for, and the
        caller must send it to suspense rather than guess.
        """
        # the VPA is the strongest boundary; the reference is the fallback,
        # with the bank code sitting between it and the name
        by_vpa = HDFC._bounded_party(narr, "UPI-", lambda p: "@" in p)
        if by_vpa:
            return by_vpa
        return HDFC._bounded_party(
            narr, "UPI-", lambda p: bool(re.fullmatch(r"\d{12,}", p)), back=1)

    def party(self, row):
        # the space-preserving reading, not the de-wrapped one. `parse_pages`
        # keeps both precisely so names and references can disagree about a
        # wrap: de-wrapping is what keeps a split reference intact, and it is
        # also what welds `ACME INDUSTRIES` into `ACMEINDUSTRIES`. SBI's
        # `party` has always read this form; HDFC's read the other, against the
        # module's own stated rule. On a real statement that cost the correct
        # spelling of a counterparty appearing twice.
        narr = row.get("narr_spaced") or row["narr"]
        if re.match(r"^IMPS-\d+-", narr):
            # IMPS-<ref>-<name…>-<bank code>-…; the name ends at the bank code
            # the masked account is the marker: a name can be four capitals
            # (ACME) and so can the bank code, but it cannot be XXXXXXXX1234
            prefix = narr[:narr.index("-", 5) + 1]
            name = self._bounded_party(
                narr, prefix,
                lambda p: bool(re.fullmatch(r"[Xx]{4,}\d*", p)), back=1)
            return _squash(name) if name else "UNRESOLVED"
        if re.match(r"^NEFT (?:CR|DR)-", narr):
            # NEFT DR-<IFSC>-<name…>-<branch>-<UTR>-…; the UTR is the marker and
            # the branch sits between it and the name
            prefix = narr[:narr.index("-", 5) + 1]
            name = self._bounded_party(
                narr, prefix,
                _looks_like_utr, skip=1, back=1)
            if name:
                return _squash(name)
            return "UNRESOLVED"
        found = re.match(r"^\d{10,}-TPT-[^-]*-(.+)$", narr)
        if found:
            return _squash(found.group(1))
        if narr.startswith("UPI-"):
            candidate = _squash(self._upi_party(narr))
            if not candidate:
                return "UNRESOLVED"
            # UPI-XXXXXX4230-... is a masked account, not a payee name
            return "UNNAMED" if re.fullmatch(r"[X]+\d*", candidate) else candidate
        # Shape and delimiter: finding S2 in `docs/tally/README.md`
        # ("Statement-layout findings"). Internal spaces in the reference, and
        # why this branch reads `narr_spaced` rather than `narr`: finding S1
        # there, which also records the residual neither reading resolves.
        #
        # Greedy binds the name to the *last* qualifying reference: `(.+)`
        # prefers the longest match, and `ACME TRADERS-12345` fails because the
        # character after it is a space rather than a hyphen.
        #
        # The **digit count is part of the shape**, and it has to be, because
        # "hyphen then digits" alone does not distinguish a bank reference from
        # an ordinary name. `ACH D- TP ACH STUDIO-54` resolved to `STUDIO` and
        # `ACH D- TP ACH STUDIO-5 4` — a name whose number wrapped at the column
        # edge — did too, so a mapping for `STUDIO` silently posted a `STUDIO-54`
        # transaction to the wrong ledger. Requiring a reference-length run of
        # digits leaves both as UNRESOLVED, which routes them to suspense where
        # an operator sees them. That is the direction to fail in: an
        # unrecognised narration costs a look, a misattributed one does not
        # announce itself at all.
        found = ACH_PARTY.match(narr)
        if found:
            return _squash(found.group(1))
        for prefix, label in (("EMI ", "EMI"), ("DEBIT CARD", "DEBIT CARD FEE")):
            if narr.startswith(prefix):
                return label
        if "INSTAALERTCHG" in narr:
            return "BANK CHARGES"
        return "UNRESOLVED"

    def reference(self, row):
        narr = row["narr"]
        ref = row["ref"].lstrip("0") or row["ref"]
        for mode, pattern in (("IMPS", r"^IMPS-(\d+)-"),
                              ("UPI", r"^UPI-.*?-(\d{12,})-"),
                              ("NEFT", r"^NEFT (?:CR|DR)-.*?-([A-Z]{2,}\w+)$")):
            found = re.search(pattern, narr)
            if found:
                return mode, found.group(1)
        if narr.startswith("NEFT"):
            return "NEFT", ref
        if narr.startswith("ACH"):
            return "ACH", ref
        return "TXN", ref


BANKS = {cls.name: cls for cls in (SBI, HDFC)}


def _squash(text):
    return re.sub(r"\s+", " ", text).strip()


def _strip(text):
    return re.sub(r"\s+", "", text)


def _looks_like_utr(text):
    """A bank reference: long, upper-case alphanumeric, and mostly digits.

    The digit requirement is the whole point. `[A-Z]{2,}[A-Z0-9]{8,}` also
    matches `INTERNATIONAL`, so a counterparty with one long all-capitals word
    terminates its own name and the parser returns nothing. A reference always
    carries digits; a company name may carry none.
    """
    return (len(text) >= 10 and text.isalnum() and text.isupper()
            and sum(character.isdigit() for character in text) >= 4)


def _key(text):
    """Mapping key: **whitespace-insensitive, everything else significant.**

    Whitespace is folded because the PDF cell-wrap genuinely splits one
    counterparty's name two ways in one statement — measured on the delivered
    HDFC book, where `MERCURYM ANUFACTURERS` and `MERCURYMANUFACTURERS` are the
    same payee and must reach the same mapping row. That is a property of
    `pdftotext` geometry, established here, and it is the whole reason this key
    is looser than an exact compare.

    **Punctuation used to be dropped too, and that part was removed.** Keeping
    only letters, digits and marks also collapsed `A & B` with `AB`,
    `S.K. Minerals` with `SK Minerals`, `M/s Mercury` with `Ms Mercury` and
    `Shree-Ram Traders` with `Shree Ram Traders` — different names, silently
    posted to one ledger. `load_mapping` refuses two *mapping rows* that
    collide, but nothing refuses a **statement** party colliding with a mapping
    row written for somebody else: there is exactly one candidate, no ambiguity
    to reject, and the write goes to a ledger the operator never chose for it.
    §9.4b calls this out — a sole candidate under a loose fold is not a
    resolution — and named `ledger_lookup_key`'s alphanumeric-only fold as the
    example. This was the same fold.

    **Removing it cost nothing measurable.** Across the 23 distinct parties in
    the delivered manifests, **none** carried punctuation that the old key
    dropped — bank narration party fields are upper-case alphanumeric — and the
    single real merge, the wrap case above, survives unchanged. The risky half
    was doing no work.

    Folding is done on the *characters*, not by a category filter, so a name
    written in Devanagari, Tamil or Bengali keeps every character rather than
    reducing toward the empty string, and combining marks (category `Mn`, for
    which `str.isalnum` is false) are preserved with the letters they modify.

    That also retires a refusal. `load_mapping` used to reject a party whose key
    came out empty — reachable when the key kept only letters and digits, since
    `---` reduced to nothing and would have bucketed with every other such name.
    Removing only whitespace makes it **unreachable**: `_squash` has already
    dropped any name that is entirely whitespace, and every other name keeps at
    least one character. The check was deleted rather than left in place,
    because an unreachable guard reads as protection and is not.
    """
    return "".join(character for character in text.upper()
                   if not character.isspace())


def _ledger_key(name):
    r"""A fold **looser than** Tally's measured master-name identity (§9.4b).

    It was documented here as *being* Tally's identity. It is not. Every row
    below marked **folds** against anything other than VERIFIED is a
    transformation Tally has never been shown to make. The first draft of this
    table said there were three; there are six, and the three it missed are the
    ones that do not look like decisions:

    | transformation                   | §9.4b      | this fold   |
    | -------------------------------- | ---------- | ----------- |
    | ASCII case fold                  | VERIFIED   | folds       |
    | space supplied for stored hyphen | VERIFIED   | folds       |
    | one trailing space               | VERIFIED   | folds       |
    | **hyphen supplied for stored space** | UNVERIFIED | **folds**   |
    | **internal whitespace run collapsed** | UNVERIFIED | **folds**   |
    | **leading whitespace ignored**   | UNVERIFIED | **folds**   |
    | **two or more trailing spaces**  | UNVERIFIED | **folds**   |
    | **non-ASCII case fold**          | UNVERIFIED | **folds**   |
    | **tab / NBSP / other Unicode space as a space** | UNVERIFIED | **folds** |
    | `&` vs `AND`                     | rejects    | keeps apart |
    | `&` deleted                      | UNVERIFIED | keeps apart |
    | en dash, underscore, `/`         | UNVERIFIED | keeps apart |

    Three of those need spelling out, because they are properties of `.upper()`
    and `_squash` rather than anything written here, and that is exactly why the
    first version of this table missed them:

      * `str.upper()` is **not** an ASCII case fold. It applies Unicode case
        mapping, so `straße` and `STRASSE` collide — and note the length
        changes, which no rule in §9.4b contemplates at all.
      * `_squash` is `re.sub(r"\s+", " ", text).strip()`. `\s` matches tab,
        newline, NBSP and the rest of Unicode whitespace, so all of them fold to
        an ASCII space; §9.4b measured a single ASCII space.
      * `.strip()` removes **arbitrary** leading and trailing whitespace. §9.4b
        measured *one* trailing space, and the reverse direction not at all.

    §9.4b's measurement is **directional** — a space was supplied where the
    master carried a hyphen, and the reverse was never sent — and a fold is
    symmetric by construction, so it cannot express that. §9.4b gives the
    asymmetric predicate to use when the question is "will Tally match these?".

    **Why a loose fold is nonetheless safe here, and this is the whole
    argument:** nothing in this tool resolves a name against Tally's master
    list. It is offline; it never sees the masters. The ledger name comes from
    the operator's own mapping CSV and is written into the XML verbatim, and
    Tally does its own matching at import. This fold is only ever used for
    three internal comparisons, and being loose in each of them fails safe:

      * `voucher_xml` refuses a voucher whose legs collapse together — looser
        refuses more, which is the direction a refusal should err;
      * `build` decides whether a row counts as unidentified and warrants an
        operator warning — looser warns more often;
      * `selfcheck` matches the bank leg, where both sides came from the same
        `--bank-ledger` argument, so the fold changes nothing.

    **Do not copy this function into anything that binds a name to a master.**
    There, every unverified row above silently posts to a ledger Tally would not
    have matched, and a sole candidate under a loose fold is not a resolution.
    Use §9.4b's `accepts(candidate, tally_name)` predicate instead — it is
    written out in the reference, as alternatives rather than as a fold,
    precisely because a fold is symmetric and the separator result is not.
    """
    return _squash(name.replace("-", " ")).upper()


# --------------------------------------------------------------------------- #
# Parsing                                                                      #
# --------------------------------------------------------------------------- #

def _matches(group, anchors):
    """Does this visual line carry every word of any one anchor group?

    Whole groups rather than single words, so a counterparty called
    "... LIMITED" is not mistaken for the "HDFC BANK LIMITED" footer.
    """
    words = {t for *_, t in group}
    return any(all(token in words for token in anchor) for anchor in anchors)


def _table_top(lines, bank):
    """y of the line that starts this page's transaction table, or None.

    Anchors are tried in order, so a profile can put its strongest marker
    first: HDFC's page 1 carries the column header, later pages only the
    period line.
    """
    for anchor in bank.top_anchors:
        found = next((y for y, group in lines if _matches(group, (anchor,))), None)
        if found is not None:
            return found
    return None


def parse_pages(pages, bank):
    """Statement rows, in printed order, from `pdftotext -bbox-layout` pages.

    Split out from `parse` so the whole geometry pipeline — page anchors,
    column bounds, row-start detection, multi-line row assembly and the wrap
    heuristic — is reachable from a test without a PDF or a password.
    """
    rows, current, stop = [], None, False
    for page in pages:
        if stop:
            break
        lines = _lines(page)
        top = _table_top(lines, bank)
        if top is None:
            continue
        if bank.end_anchors and any(_matches(g, bank.end_anchors) for _, g in lines):
            stop = True
        for y, group in lines:
            if y <= top:
                continue
            if bank.header_words and all(t in bank.header_words for *_, t in group):
                continue
            cells = {}
            for x0, _, x1, _, text in group:
                cells.setdefault(bank.column_of(x0, x1), []).append((x0, x1, text))
            # A footer anchor is a *subset* test, so any line carrying its words
            # ends the page — including a transaction whose counterparty is the
            # bank itself, which would drop that row and every row after it.
            # A line that opens a transaction is a transaction whatever else it
            # says, so the date decides and the anchor only breaks the ties.
            #
            # Residual, and why it is left: a wrapped *continuation* line has no
            # date, so this guard does not cover one that carried the anchor
            # words. Reaching that needs all of them as separate tokens, and
            # these narrations join fields with hyphens — a payee "HDFC BANK
            # LIMITED" tokenises as "…-HDFC", "BANK", "LIMITED-…", so only the
            # middle word is ever bare. Geometry does not separate the two
            # either: the real "STATEMENT SUMMARY" footer centres inside the
            # narration column, exactly where a continuation sits.
            started = bank.is_row_start(cells)
            if not started and bank.bottom_anchors and _matches(group, bank.bottom_anchors):
                break
            if started:
                current = {name: [] for _, _, name in bank.columns}
                rows.append(current)
            if current is None:
                continue
            wanted = cells if (started or not bank.row_scoped) else {
                k: v for k, v in cells.items() if k not in bank.row_scoped
            }
            for name, cell in wanted.items():
                current[name].append((" ".join(t for _, _, t in cell),
                                      max(x1 for _, x1, _ in cell)))

    out = []
    for row in rows:
        flat = {}
        for _, _, name in bank.columns:
            if name in bank.text_columns:
                flat[name] = _dewrap(row[name], bank.text_columns[name])
                # Both readings of a wrapped cell are kept. The de-wrapped form is
                # the only one where a reference number is intact; the space-joined
                # form is the only one that never welds two words together. Name
                # extraction wants the second, reference extraction the first.
                flat[name + "_spaced"] = _squash(" ".join(t for t, _ in row[name]))
            else:
                flat[name] = "".join(text for text, _ in row[name]).replace(",", "")
        out.append(flat)
    return out


def parse(pdf, password, bank):
    """Statement rows plus the pages they came from."""
    pages = _pdf_pages(pdf, password)
    return parse_pages(pages, bank), pages


def account_number_runs(pages, bank):
    """Digit runs printed on the statement's own account-number line.

    Narrowed twice, and the second narrowing was the one that mattered. Reading
    the whole document lets any transaction reference stand in for the account.
    Reading the whole header block is barely better: a header prints a phone
    number, a customer id, an IFSC, a MICR code and a postcode, and on a real
    statement four different wrong tails all matched. Only the line the bank
    labels as the account number identifies the account.
    """
    if not bank.account_anchors:
        return set()
    runs = set()
    for page in pages:
        for _, group in _lines(page):
            if not _matches(group, bank.account_anchors):
                continue
            for *_, text in group:
                runs.update(re.findall(r"\d+", _unescape(text)))
        if runs:
            break
    return runs


def account_digits(account_tail):
    """The digits of the operator's free-form label, for matching only."""
    return re.sub(r"\D", "", account_tail)


def require_account_match(pages, bank, account_tail):
    """Refuse a statement that does not print the account being posted to.

    The running-balance proof validates the PDF's own arithmetic and nothing
    else, so it is equally happy to certify the *wrong account's* statement. It
    cannot notice that the operator opened last month's file for the other
    bank, or mistyped `--bank-ledger`. This is the one check that ties the
    document in hand to the ledger it will be posted against, so it runs before
    any XML exists.

    Matching is on a trailing digit run, because banks mask the leading digits
    (`XXXXXX4230`) and space them unpredictably, and only against the line the
    statement labels as its account number (see `account_number_runs`).

    Residual, stated so it is known rather than assumed: the label line can
    carry a second number — HDFC prints a product code beside it — so a tail
    that happens to end that number would also pass. Four digits against one
    line is a far weaker coincidence than four digits against a page.
    """
    digits = account_digits(account_tail)
    if len(digits) < 4:
        raise Refusal(
            "unbindable_account",
            f"--account-tail {account_tail!r} carries {len(digits)} digits; "
            "at least 4 are needed to bind the statement to the ledger",
        )
    runs = account_number_runs(pages, bank)
    if not runs:
        raise Refusal(
            "no_account_number_line",
            "this statement prints no line matching "
            f"{' / '.join(' '.join(a) for a in bank.account_anchors)!r}, so the "
            "account number could not be located. The layout has changed, or this "
            f"is not a {bank.name.upper()} statement.",
        )
    matched = [run for run in runs if run.endswith(digits)]
    if not matched:
        raise Refusal(
            "account_not_in_statement",
            f"no number ending {digits} appears on this statement's account-number "
            "line. Either the PDF is not the account named by --bank-ledger/"
            "--account-tail, or the account digits were mistyped. Refusing to "
            "post it anywhere.",
        )
    if len(set(matched)) > 1:
        raise Refusal(
            "ambiguous_account_match",
            f"{digits} matches more than one number on the account-number line "
            f"({', '.join(sorted(set(matched)))}); lengthen --account-tail.",
        )
    # the statement's own account number, not the operator's label — see
    # `_remote_id`
    return matched[0]


_AMOUNT = re.compile(r"\d+(\.\d{1,2})?")
_BALANCE = re.compile(r"-?\d+(\.\d{1,2})?")


def _decimal(text, pattern, field, row_number, kind):
    """Parse one numeric cell, or refuse.

    Blank is a legitimate value in an amount column and returns None. Anything
    else that does not parse is a *parse failure*, not a zero: reading a
    malformed debit cell as zero lets the running-balance replay succeed on a
    row whose amount was never actually read, which is exactly the proof this
    tool sells. Similarly a third decimal place is rejected here rather than
    rounded at XML-writing time, where the rounded leg would no longer be the
    reconciled one.
    """
    text = (text or "").strip()
    if not text:
        return None
    if not pattern.fullmatch(text):
        raise Refusal(
            kind,
            f"row {row_number}: {field} cell {text!r} is not an amount with at most "
            "two decimal places",
        )
    return D(text)


def control_value(text, flag, signed=False):
    """An operator-supplied control total, read off the printed statement.

    Parsed through the same boundary as a statement cell, because it is
    compared against one. Thousands separators are stripped first: the operator
    is copying `1,00,000.00` off a page, and a bare `Decimal()` on that raises
    `InvalidOperation` — an uncaught traceback where every other bad input to
    this tool produces a category.
    """
    cleaned = (text or "").replace(",", "").strip()
    pattern = _BALANCE if signed else _AMOUNT
    if not pattern.fullmatch(cleaned):
        raise Refusal(
            "malformed_control_value",
            f"{flag} {text!r} is not an amount with at most two decimal places"
            + ("" if signed else " (a total of withdrawals or deposits is never negative)"),
        )
    return D(cleaned)


def _money(text, field="amount", row_number=0):
    """A transaction amount: unsigned, at most two decimal places."""
    return _decimal(text, _AMOUNT, field, row_number, "malformed_amount")


def _balance(text, field="balance", row_number=0):
    """A running balance, which may be negative on an overdrawn account."""
    return _decimal(text, _BALANCE, field, row_number, "malformed_balance")


def reconcile(rows, bank, opening, expect_closing):
    """Replay the statement's own running balance. Returns the closing balance.

    This is the parser's correctness proof: an amount misread by one digit, a
    dropped row or a row counted twice all break the chain. Raises on the first
    row that does not reproduce its printed balance.

    The chain alone proves the rows that *were* read are internally consistent
    — it says nothing about whether they are all of them. A layout change, a
    missed page anchor or a false footer match truncates the parse after a
    perfectly self-consistent prefix, and an empty parse satisfies it
    vacuously. `expect_closing` is the first piece of independent evidence: the
    operator reads the statement's own printed closing balance and the replayed
    chain has to land on it.

    **It is not sufficient on its own and must not be read as such.** The
    closing balance is the *net* of the transactions, so a dropped suffix whose
    debits and credits cancel — an omitted 50 debit followed by an omitted 50
    credit — still closes on the printed figure. Two vouchers vanish and every
    check here passes. That is why `main` also requires the printed debit and
    credit totals: they are independent of the net and of each other.
    """
    if not rows:
        raise Refusal(
            "empty_statement",
            "no transaction rows were parsed. Either the statement is empty, or the "
            "layout has changed and the row anchors no longer match.",
        )
    running = D(opening)
    for index, row in enumerate(rows, 1):
        # Keep the parsed optionals: `is not None` asks whether the **cell was
        # filled**, which is what a column-geometry failure looks like. Testing
        # the Decimals for truthiness asked whether they were non-zero, so a row
        # carrying `0.00` in one column and a real amount in the other walked
        # past this check — the balance replay and the printed totals still
        # matched, and `build()` went on to emit a voucher from a structurally
        # invalid row.
        debit_cell = _money(row[bank.debit_column], bank.debit_column, index)
        credit_cell = _money(row[bank.credit_column], bank.credit_column, index)
        debit = debit_cell or D(0)
        credit = credit_cell or D(0)
        if debit_cell is not None and credit_cell is not None:
            raise Refusal(
                "two_sided_row",
                f"row {index} ({row[bank.date_column]}) fills both amount columns "
                f"(debit {debit}, credit {credit}). A statement row has one side; "
                "this is a column-geometry failure, not a transaction.",
            )
        printed = _balance(row[bank.balance_column], bank.balance_column, index)
        running = running - debit + credit
        if printed is None or running != printed:
            raise Refusal(
                "balance_chain_broken",
                f"row {index} ({row[bank.date_column]}) breaks the running balance: "
                f"expected {running}, statement prints {printed}",
            )
    if running != D(expect_closing):
        raise Refusal(
            "extent_unproven",
            f"the replayed chain closes at {running}, but the statement's printed "
            f"closing balance is {expect_closing}. Every row read reconciles, so this "
            "means rows are missing — the parse stopped early or started late.",
        )
    return running


# --------------------------------------------------------------------------- #
# Tally XML                                                                    #
# --------------------------------------------------------------------------- #

def escape(text):
    """Mandatory — see 9.1b. 'Duties & Taxes' is a stock group in every company."""
    for char, entity in (("&", "&amp;"), ("<", "&lt;"), (">", "&gt;"),
                         ('"', "&quot;"), ("'", "&apos;")):
        text = text.replace(char, entity)
    return text


def voucher_xml(kind, date, remote_id, narration, debit_ledger, credit_ledger,
                amount, party_ledger=None):
    if _ledger_key(debit_ledger) == _ledger_key(credit_ledger):
        raise Refusal(
            "self_cancelling_voucher",
            f"both legs of the {date.isoformat()} voucher resolve to the same ledger "
            f"({debit_ledger!r} / {credit_ledger!r}). It would balance, import cleanly "
            "and move nothing — the transaction would vanish from the book.",
        )
    entries = ""
    for ledger, is_debit in ((debit_ledger, True), (credit_ledger, False)):
        signed = -amount if is_debit else amount
        entries += (
            "\n   <ALLLEDGERENTRIES.LIST>"
            f"\n    <LEDGERNAME>{escape(ledger)}</LEDGERNAME>"
            f"\n    <ISDEEMEDPOSITIVE>{'Yes' if is_debit else 'No'}</ISDEEMEDPOSITIVE>"
            f"\n    <AMOUNT>{signed:.2f}</AMOUNT>"
            "\n   </ALLLEDGERENTRIES.LIST>"
        )
    stamp = date.strftime("%Y%m%d")
    party = (f"\n   <PARTYLEDGERNAME>{escape(party_ledger)}</PARTYLEDGERNAME>"
             if party_ledger else "")
    return (
        f'\n  <VOUCHER VCHTYPE="{kind}" ACTION="Create"'
        f' OBJVIEW="Accounting Voucher View" REMOTEID="{escape(remote_id)}">'
        f"\n   <DATE>{stamp}</DATE>"
        f"\n   <EFFECTIVEDATE>{stamp}</EFFECTIVEDATE>"
        f"\n   <VOUCHERTYPENAME>{kind}</VOUCHERTYPENAME>"
        f"{party}"
        f"\n   <NARRATION>{escape(narration)}</NARRATION>"
        f"{entries}"
        "\n  </VOUCHER>"
    )


def envelope(company, vouchers):
    body = "".join(vouchers).replace("\n", "\n    ")
    return (
        '<?xml version="1.0" encoding="UTF-8"?>\n<ENVELOPE>\n <HEADER>\n'
        "  <TALLYREQUEST>Import Data</TALLYREQUEST>\n </HEADER>\n <BODY>\n"
        "  <IMPORTDATA>\n   <REQUESTDESC>\n    <REPORTNAME>Vouchers</REPORTNAME>\n"
        "    <STATICVARIABLES>\n"
        f"     <SVCURRENTCOMPANY>{escape(company)}</SVCURRENTCOMPANY>\n"
        "    </STATICVARIABLES>\n   </REQUESTDESC>\n   <REQUESTDATA>\n"
        '    <TALLYMESSAGE xmlns:UDF="TallyUDF">'
        f"{body}"
        "\n    </TALLYMESSAGE>\n   </REQUESTDATA>\n  </IMPORTDATA>\n"
        " </BODY>\n</ENVELOPE>\n"
    )


# --------------------------------------------------------------------------- #
# Mapping and voucher construction                                             #
# --------------------------------------------------------------------------- #

MAPPING_COLUMNS = ("party", "ledger", "treatment")


# The words `party()` returns when it could not identify a counterparty. They
# are output, not input: every unrecognised narration shape reports the same
# one, so a mapping row for one would gather unrelated transactions under a
# single ledger — or, with `skip`, silently drop all of them — instead of
# letting them fall to suspense where an operator can see them. Reserved in
# `load_mapping`, and bypassed at lookup so a future sentinel cannot re-open
# the hole by being forgotten here.
PARSER_SENTINELS = frozenset({"UNRESOLVED", "UNNAMED"})


def load_mapping(path):
    """CSV: party,ledger,treatment

    `party` is the counterparty name exactly as this tool prints it for the
    statement (run with --dry-run to list them). `treatment` is one of:

      auto    (default) Payment when money leaves the bank, Receipt when it arrives
      contra  both legs are bank/cash ledgers — an own-account or ATM transfer
      skip    do not emit a voucher. Use for a transfer between two accounts that
              are BOTH being imported: the movement is already carried by the
              other statement's contra voucher, and emitting it twice doubles it.

    Anything absent from the file falls to the suspense ledger.
    """
    mapping = {}
    if not path:
        return mapping
    with open(path, newline="", encoding="utf-8") as handle:
        reader = csv.DictReader(handle)
        normalised = [(name or "").strip().lower() for name in (reader.fieldnames or ())]
        repeated = sorted({name for name in normalised if normalised.count(name) > 1})
        if repeated:
            raise Refusal(
                "mapping_headers_duplicated",
                f"{path}: column(s) {', '.join(repeated)} appear more than once once case "
                "and surrounding space are ignored. One silently overwrites the other, and "
                "if the survivor is blank the row is skipped and its transactions fall to "
                "suspense while the run reports success.",
            )
        headers = set(normalised)
        missing = [name for name in MAPPING_COLUMNS if name not in headers]
        if missing:
            raise Refusal(
                "mapping_headers_missing",
                f"{path}: missing column(s) {', '.join(missing)}. A mapping without "
                "'party' silently maps nothing and one without 'ledger' silently sends "
                f"everything to suspense; required header row is "
                f"{','.join(MAPPING_COLUMNS)}",
            )
        origin = {}
        for line, raw in enumerate(reader, 2):
            # the header check normalises; the records must be normalised too, or
            # `Party,Ledger,Treatment` passes validation and then reads as empty
            # on every row — the mapping is silently discarded and everything
            # lands in suspense, which is the failure the check exists to stop
            record = {(name or "").strip().lower(): value for name, value in raw.items()}
            party = _squash(record.get("party", ""))
            if not party:
                continue
            treatment = (record.get("treatment") or "auto").strip().lower()
            if treatment not in ("auto", "contra", "skip"):
                raise Refusal("unknown_treatment",
                              f"unknown treatment {treatment!r} for {party!r}")
            ledger = _squash(record.get("ledger", ""))
            if treatment == "contra" and not ledger:
                raise Refusal(
                    "contra_without_ledger",
                    f"{path} line {line}: {party!r} is marked contra with no ledger. "
                    "A Contra's other leg must be a bank or cash ledger; defaulting it "
                    "to suspense would post a Contra against a non-bank account and "
                    "misclassify the transaction in Tally.",
                )
            # key on letters and digits only: the wrap heuristic can place a space
            # differently in two printings of the same name ("ZEPHYRM ANUFACTURING"
            # vs "ZEPHYRMANUFACTURING"), and both must reach the same ledger
            key = _key(party)
            if key in PARSER_SENTINELS:
                raise Refusal(
                    "mapping_claims_a_sentinel",
                    f"{path} line {line}: {party!r} is a value this tool prints when it "
                    f"could NOT identify a counterparty, not a counterparty name. "
                    "Every unrecognised narration shape reports the same word, so a row "
                    "for it would collect transactions that have nothing to do with each "
                    "other and send them to one ledger — or, with treatment 'skip', drop "
                    "them all. Unidentified rows must reach suspense, where they are "
                    "visible. Map the individual narrations the dry run prints beside "
                    f"{party!r} instead.",
                )
            if key in mapping and mapping[key] != (ledger, treatment):
                first_party, first_line = origin[key]
                raise Refusal(
                    "mapping_key_collision",
                    f"{path} line {line}: {party!r} and {first_party!r} (line "
                    f"{first_line}) reduce to the same mapping key {key!r} but give "
                    "different instructions. Every transaction for both would be "
                    "posted under whichever row came last. Rename one, or give them "
                    "the same ledger and treatment.",
                )
            mapping[key] = (ledger, treatment)
            origin.setdefault(key, (party, line))
    return mapping


def _remote_id(account, date, row, bank):
    """Idempotency key derived from the transaction, not from its position.

    3.3a makes a repeated REMOTEID an *upsert*, so this key decides two
    opposite failures. Keyed on the row ordinal (the earlier design), a second
    download of the same account collides two different transactions onto one
    key and the later silently overwrites the earlier, while an overlapping
    export that shifts a transaction's ordinal gives it a new key and posts it
    twice. Both are silent and both corrupt a real book.

    So the key is a digest of what the bank itself printed on that line: the
    date, both amount columns, the running balance and the narration. The
    balance in particular makes two genuinely identical same-day payments
    distinguishable, because the second lands on a different balance.

    Nothing operator-controlled may enter this. The account identity comes from
    the **statement**, not from `--account-tail`: that flag is a free-form label,
    and `HDFC CA xx1234`, `HDFC xx1234` and `xx001234` all name one account while
    reducing to three different strings. Any of them would give every
    transaction a new key and duplicate the whole statement on re-import — which
    is the failure the digest exists to prevent, reintroduced through the label.
    `require_account_match` returns the number it matched, and that is what is
    used here.
    """
    material = "\0".join((
        account,
        date.isoformat(),
        (row.get(bank.debit_column) or "").strip(),
        (row.get(bank.credit_column) or "").strip(),
        (row.get(bank.balance_column) or "").strip(),
        _squash(row.get(bank.narration_column, "")),
    ))
    digest = hashlib.sha256(material.encode("utf-8")).hexdigest()[:12].upper()
    return f"AC{account}-{date.strftime('%Y%m%d')}-{digest}"


MANIFEST_COLUMNS = ("row", "date", "voucher_type", "amount", "dr_ledger", "cr_ledger",
                    "suspense", "remoteid", "party", "narration")


def _manifest_row(**values):
    """One manifest record, with every column present.

    Written from `MANIFEST_COLUMNS` rather than from a literal so a column can
    never exist in one kind of row and not the other — `csv.DictWriter` would
    raise on the mismatch, after the XML had already been written.
    """
    unknown = set(values) - set(MANIFEST_COLUMNS)
    assert not unknown, unknown
    return {column: values.get(column, "") for column in MANIFEST_COLUMNS}


def build(rows, bank, company, bank_ledger, suspense, mapping, account_tail,
          account=None, date_from=None, date_to=None):
    """Vouchers and a manifest row per statement row, in printed order.

    The manifest is not a log. It is the operator's record of what each voucher
    was made from and, through `remoteid`, the only way to remove one
    afterwards — including for rows this run deliberately did not emit.

    `account_tail` is the operator's label and appears only in narrations, where
    it is for a human to read. `account` is the number `require_account_match`
    read off the statement, and it is what the REMOTEID is keyed on — a label
    can be respelled, an account number cannot.
    """
    account = account or account_digits(account_tail)
    vouchers, manifest, seen = [], [], {}
    for index, row in enumerate(rows, 1):
        raw_date = f"{row[bank.date_column]}".strip()
        try:
            date = bank.parse_date(raw_date)
        except ValueError:
            # matched the row-start shape but is not a real calendar date, e.g.
            # 31/02/26 — a layout or extraction fault, and it must arrive as a
            # category rather than a traceback
            raise Refusal(
                "unparseable_date",
                f"row {index}: {raw_date!r} is not a date this statement layout "
                f"can produce ({bank.date_format}). The columns have shifted or "
                "the extraction is corrupt.",
            )
        if (date_from and date < date_from) or (date_to and date > date_to):
            continue
        debit = _money(row[bank.debit_column], bank.debit_column, index)
        credit = _money(row[bank.credit_column], bank.credit_column, index)
        amount = debit or credit
        if amount is None:
            raise Refusal("row_without_amount", f"row {index} has no amount")
        outward = bool(debit)
        party = bank.party(row)
        # A sentinel means "not identified", so it must go to suspense whatever
        # the mapping says. `load_mapping` already refuses such a row, and this
        # is the second lock: the refusal is a rule about a file, while this is
        # a property of the run, and only one of the two survives someone adding
        # a sentinel without remembering the loader.
        if _key(party) in PARSER_SENTINELS:
            ledger, treatment = suspense, "auto"
        else:
            ledger, treatment = mapping.get(_key(party), (suspense, "auto"))
        ledger = ledger or suspense
        if treatment == "skip":
            # the REMOTEID is recorded even though no voucher is emitted: if this
            # statement was already imported before the mapping said `skip`, this
            # is the key the operator needs to delete the voucher that is still
            # sitting in the book. Omitting a voucher from a re-import does not
            # remove it — a re-import upserts what is present and ignores what
            # is not.
            manifest.append(_manifest_row(
                row=index, date=date.isoformat(), voucher_type="SKIPPED",
                amount=f"{amount:.2f}", party=party,
                remoteid=_remote_id(account, date, row, bank),
                narration="excluded: carried by the other account's contra"))
            continue
        kind = "Contra" if treatment == "contra" else ("Payment" if outward else "Receipt")
        mode, reference = bank.reference(row)
        # Whether this row needs an operator's eye. Deliberately the **loose**
        # fold: over-flagging costs a look, under-flagging posts an
        # unidentified row and says nothing.
        #
        # It is not a claim that Tally would treat the two names as one master.
        # This comment used to say Tally resolves `suspense-acc` and
        # `SUSPENSE ACC` to the same master and cite 3.3b; §9.4b measures that
        # direction as UNVERIFIED, and `_ledger_key` is looser than §9.4b in six
        # ways besides. Nothing here can know what Tally would match.
        #
        # Which is why the message below names `ledger` — the name actually
        # written into the voucher — rather than "Suspense". When the fold
        # over-flags (a mapping to `A-B` against a suspense master named `A B`),
        # the old wording told the operator to reallocate from a ledger the
        # voucher was never posted to, which is an instruction that cannot be
        # followed. Naming the real destination makes a false positive cost a
        # look instead of a wrong search.
        unidentified = _ledger_key(ledger) == _ledger_key(suspense)
        shown = party if unidentified else ledger
        narration = _squash(
            f"{mode} {reference} {'to' if outward else 'from'} {shown}"
            f" | {account_tail} | {date.strftime('%d-%b-%Y')}"
        )
        if unidentified:
            narration += f" | UNIDENTIFIED - reallocate from {ledger}"
        remote_id = _remote_id(account, date, row, bank)
        if remote_id in seen:
            raise Refusal(
                "duplicate_remoteid",
                f"rows {seen[remote_id]} and {index} produce the same REMOTEID "
                f"{remote_id}: same date, same amounts, same balance and same "
                "narration. Importing both would upsert one over the other (3.3a). "
                "Check whether the statement really contains the row twice.",
            )
        seen[remote_id] = index
        debit_ledger, credit_ledger = ((ledger, bank_ledger) if outward
                                       else (bank_ledger, ledger))
        vouchers.append(voucher_xml(
            kind, date, remote_id, narration, debit_ledger, credit_ledger, amount,
            party_ledger=None if kind == "Contra" else ledger,
        ))
        manifest.append(_manifest_row(
            row=index, date=date.isoformat(), voucher_type=kind,
            amount=f"{amount:.2f}", dr_ledger=debit_ledger, cr_ledger=credit_ledger,
            suspense="YES" if unidentified else "", remoteid=remote_id,
            party=party, narration=narration))
    if not manifest:
        raise Refusal(
            "empty_selection",
            f"no statement row falls between {date_from or 'the first row'} and "
            f"{date_to or 'the last'}. The window is ordered but does not overlap the "
            "statement — the run would otherwise write an empty file and report a "
            "successful zero-voucher import.",
        )
    return vouchers, manifest


def selfcheck(xml_text, bank_ledger, manifest):
    """Re-read what we just wrote. Cheap, and it has caught real defects.

    Note what this cannot do: it validates the XML against the manifest, and
    both come from the same parse. It is a transcription check, not a
    correctness proof — `reconcile` is the correctness proof.
    """
    root = ET.fromstring(xml_text)
    count, outward, inward = 0, D(0), D(0)
    for voucher in root.iter("VOUCHER"):
        count += 1
        total = D(0)
        for entry in voucher.iter("ALLLEDGERENTRIES.LIST"):
            value = D(entry.findtext("AMOUNT"))
            total += value
            positive = entry.findtext("ISDEEMEDPOSITIVE") == "Yes"
            if positive != (value < 0):
                raise Refusal("sign_convention",
                              f"sign convention broken in {voucher.get('REMOTEID')}")
            if _ledger_key(entry.findtext("LEDGERNAME") or "") == _ledger_key(bank_ledger):
                if value < 0:
                    inward += -value
                else:
                    outward += value
        if total != 0:
            raise Refusal("unbalanced_voucher",
                          f"unbalanced voucher {voucher.get('REMOTEID')}")
        if not (voucher.findtext("NARRATION") or "").strip():
            raise Refusal("empty_narration",
                          f"empty narration in {voucher.get('REMOTEID')}")
        if voucher.findtext("DATE") != voucher.findtext("EFFECTIVEDATE"):
            raise Refusal("date_disagreement",
                          f"DATE and EFFECTIVEDATE disagree in {voucher.get('REMOTEID')}")
    posted = sum(1 for record in manifest if record["voucher_type"] != "SKIPPED")
    if count != posted:
        raise Refusal("manifest_count_mismatch",
                      f"manifest says {posted} vouchers, XML has {count}")
    return count, outward, inward


# --------------------------------------------------------------------------- #
# Command line                                                                 #
# --------------------------------------------------------------------------- #

def read_password(env=None, interactive=None):
    """The PDF password, from the environment or an interactive prompt.

    Never an argument: a value on the command line lands in shell history and
    in the process list, and these passwords are commonly derived from personal
    identifiers — a date of birth, part of a phone number, a customer ID.

    Residual exposure, stated rather than papered over: `pdftotext` takes the
    password as an argument, so for the duration of the extraction it is
    visible in that child's argv to anyone who can read the process table on
    this host. Closing that would mean a PDF-decryption dependency, which this
    tool deliberately does not have. Run it on a host you control.
    """
    env = os.environ if env is None else env
    password = env.get(PASSWORD_ENV)
    if password:
        return password
    if interactive is None:
        interactive = sys.stdin.isatty()
    if not interactive:
        raise Refusal(
            "no_password",
            f"set {PASSWORD_ENV} or run interactively so the password can be prompted "
            "for. It is not accepted as a command-line argument.",
        )
    return getpass.getpass("statement PDF password: ")


def _existing_target_on_windows(path):
    # The acknowledgement covers the *directory* the operator checked with
    # `icacls`. It does not cover a file that is already there: an overwrite
    # keeps that file's existing DACL rather than inheriting the directory's,
    # and `os.chmod` cannot restrict it. So an operator can follow the
    # instruction exactly and still overwrite a file readable by other
    # principals — with the tool reporting success.
    return Refusal(
        "existing_target_on_windows",
        f"{path} already exists. On Windows an overwrite keeps the file's "
        "own ACL, not the directory's, so checking the directory says "
        "nothing about this file. Delete it (or choose a new name) and "
        "re-run, so the new file inherits the ACL you checked.",
    )


def windows_destination_refusal(path, accept_inherited):
    """Why this destination cannot be written on Windows, or None.

    One rule, applied twice on purpose. `preflight` applies it to **every**
    destination before the run writes anything, because refusing the manifest
    after the XML has been written leaves a partial result whose remedy —
    remove the manifest and re-run — then fails on the XML the failed run
    created. `_open_private` applies it again at the moment of the create,
    because a preflight result is a fact about the past: between the check and
    the create, another process can put a file there.
    """
    if os.name != "nt":
        return None
    if not accept_inherited:
        # On Windows `os.chmod` toggles the read-only attribute and the mode
        # argument to `os.open` is ignored; the file inherits the directory's
        # ACL. So the owner-only guarantee is simply false there, and a file
        # carrying account numbers, counterparties, amounts and every narration
        # would be readable by anyone the directory allows — while the tool
        # reported it as owner-only. Refuse rather than reassure.
        return Refusal(
            "cannot_restrict_on_windows",
            f"{path}: this tool writes owner-only files, and POSIX modes do not "
            "do that on Windows — the output would inherit the directory's ACL "
            "while claiming to be private. Write to a directory only you can "
            "read (check with `icacls <dir>`) and re-run with "
            "--accept-inherited-permissions, which records that you have "
            "checked and makes the claim your own rather than this tool's.",
        )
    if os.path.exists(path):
        return _existing_target_on_windows(path)
    return None


def _open_private(path, accept_inherited=False):
    """Create one output file readable only by its owner, and return the handle.

    The XML and the manifest carry counterparty names, amounts, an account
    label and every narration in the statement. On a shared host the default
    022 umask would publish all of it as mode 0644.

    Always `O_EXCL`: this only ever creates a file that did not exist. On
    Windows that is the rule itself — an overwrite would keep the existing
    file's ACL — and deciding it at create time rather than after an
    `os.path.exists` is what makes it race-free. On POSIX an overwrite is
    permitted, but it is `write_outputs` that performs it, by renaming a
    staged file over the destination once every payload is safely on disk.
    """
    refusal = windows_destination_refusal(path, accept_inherited)
    if refusal:
        raise refusal
    try:
        return os.open(path, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600)
    except FileExistsError:
        raise _existing_target_on_windows(path) from None


def _file_identity(path):
    stat_result = os.stat(path)
    return stat_result.st_dev, stat_result.st_ino


def _fd_identity(handle):
    stat_result = os.fstat(handle)
    return stat_result.st_dev, stat_result.st_ino


def _entry_identity(path):
    """The inode `unlink` would remove, without following a replacement link."""
    stat_result = os.lstat(path)
    return stat_result.st_dev, stat_result.st_ino


def _unlink_for_cleanup(path, owned_identity, failures):
    """Remove a path only while it still names the inode this run created."""
    try:
        if _entry_identity(path) != owned_identity:
            # The pathname has been reclaimed. It is not ours to delete, and
            # reporting it gives the operator a chance to find the private copy
            # if it still exists without making a claim about foreign bytes.
            if os.path.lexists(path):
                failures.append(str(path))
            return
        os.unlink(path)
    except FileNotFoundError:
        pass
    except OSError:
        # A filesystem call can report an error after taking effect. Only retain
        # the path when reconciliation shows bytes may still be present.
        if os.path.lexists(path):
            failures.append(str(path))


def _open_regular_output(path, expected_identity):
    """Open and pin one existing regular output without waiting on a FIFO."""
    handle = os.open(path, os.O_RDONLY | getattr(os, "O_NONBLOCK", 0))
    try:
        stat_result = os.fstat(handle)
        if not stat.S_ISREG(stat_result.st_mode):
            raise Refusal(
                "output_not_regular",
                f"{path}: an existing output must be a regular file",
            )
        if (stat_result.st_dev, stat_result.st_ino) != expected_identity:
            raise Refusal(
                "output_path_changed",
                f"{path} changed while its rollback copy was prepared",
            )
        return handle
    except BaseException:
        os.close(handle)
        raise


def _owned_path(path, handle):
    """Record a pathname and retain the descriptor that pins its inode."""
    try:
        identity = _fd_identity(handle)
    except BaseException as error:
        # The creator has already made `path`, but until its descriptor and
        # pathname agree on one identity it has not entered any ownership list.
        # Retry once for a transient fstat failure; if that still cannot prove
        # ownership, preserve a possibly reclaimed pathname and say so rather
        # than deleting a foreign file during failure cleanup.
        failures = []
        try:
            try:
                identity = _fd_identity(handle)
            except BaseException:
                identity = None
            if identity is None:
                if os.path.lexists(path):
                    failures.append(str(path))
            else:
                _unlink_for_cleanup(path, identity, failures)
        finally:
            try:
                os.close(handle)
            except OSError:
                if os.path.lexists(path):
                    failures.append(str(path))
        if failures:
            _append_cleanup_detail(
                error,
                "output ownership registration failed; retained path(s): "
                + ", ".join(sorted(set(failures))),
            )
        raise
    return {"path": path, "identity": identity, "pin": handle}


def _close_owned_path(record, failures):
    """Release an ownership pin only after its cleanup decision is complete."""
    handle = record.get("pin")
    if handle is None:
        return
    record["pin"] = None
    try:
        os.close(handle)
    except OSError:
        failures.append(record["path"])


def _cleanup_owned_path(record, failures):
    """Remove one owned pathname, releasing its pin first on Windows.

    POSIX keeps the descriptor open through the identity decision so an inode
    cannot be recycled before cleanup. Windows does not permit unlinking an
    open file, so fresh exclusive outputs close first and retain the existing
    Windows cleanup behavior; actual Windows filesystem evidence remains
    required for that platform-specific branch.
    """
    if os.name == "nt":
        _close_owned_path(record, failures)
        _unlink_for_cleanup(record["path"], record["identity"], failures)
    else:
        _unlink_for_cleanup(record["path"], record["identity"], failures)
        _close_owned_path(record, failures)


def _metadata_from_handle(path, handle):
    """Capture regular-output metadata while its original inode is pinned.

    The private backup remains mode 0600. These values are applied only after
    that backup has been atomically restored to its original pathname.
    """
    stat_result = os.fstat(handle)
    metadata = {
        "mode": stat.S_IMODE(stat_result.st_mode),
        "uid": stat_result.st_uid,
        "gid": stat_result.st_gid,
        "atime_ns": stat_result.st_atime_ns,
        "mtime_ns": stat_result.st_mtime_ns,
        "xattrs": None,
    }
    if hasattr(os, "listxattr"):
        try:
            metadata["xattrs"] = {
                name: os.getxattr(handle, name)
                for name in os.listxattr(handle)
            }
        except OSError as error:
            raise Refusal(
                "output_metadata_unavailable",
                f"{path}: could not record extended attributes for rollback: {error}",
            ) from None
    return metadata


def _restore_metadata(handle, metadata):
    """Restore captured metadata to an already-restored regular output."""
    current = os.fstat(handle)
    if (current.st_uid, current.st_gid) != (metadata["uid"], metadata["gid"]):
        os.fchown(handle, metadata["uid"], metadata["gid"])
    os.fchmod(handle, metadata["mode"])
    os.utime(handle, ns=(metadata["atime_ns"], metadata["mtime_ns"]))
    original_xattrs = metadata["xattrs"]
    if original_xattrs is not None:
        for name in os.listxattr(handle):
            if name not in original_xattrs:
                os.removexattr(handle, name)
        for name, value in original_xattrs.items():
            os.setxattr(handle, name, value)


def _copy_private_backup(source_path, original_identity, backup_handle):
    """Copy the original inode into an owner-only backup already opened O_EXCL.

    The caller already pins the original inode through the transaction; this
    read descriptor pins it while bytes are copied. The source path is checked
    both before and after the copy; that catches an atomic path replacement
    during preparation. Without filesystem locking, an adversary that modifies
    the same inode while it is being read remains outside this command's
    authority, so this does not promise a crash transaction.
    """
    source_handle = _open_regular_output(source_path, original_identity)
    try:
        if _fd_identity(source_handle) != original_identity:
            raise Refusal(
                "output_path_changed",
                f"{source_path} changed while its rollback copy was prepared",
            )
        copied = hashlib.sha256()
        while True:
            chunk = os.read(source_handle, 1024 * 1024)
            if not chunk:
                break
            copied.update(chunk)
            view = memoryview(chunk)
            while view:
                written = os.write(backup_handle, view)
                if written == 0:
                    raise OSError("private backup write made no progress")
                view = view[written:]
        os.fsync(backup_handle)
        os.lseek(backup_handle, 0, os.SEEK_SET)
        verified = hashlib.sha256()
        while True:
            chunk = os.read(backup_handle, 1024 * 1024)
            if not chunk:
                break
            verified.update(chunk)
        if copied.digest() != verified.digest():
            raise OSError("private backup did not retain the copied bytes")
        if (_fd_identity(source_handle) != original_identity
                or _file_identity(source_path) != original_identity):
            raise Refusal(
                "output_path_changed",
                f"{source_path} changed while its rollback copy was prepared",
            )
    finally:
        os.close(source_handle)


def _restore_backup(backup, backup_identity, destination, original_identity,
                    staged_identity, metadata, swap_started, failures,
                    metadata_scope_warnings):
    """Restore an owned private backup after a caught swap failure.

    `os.replace` can report an exception after the filesystem call took effect.
    Roll back only when the destination still names this run's staged inode.
    A different inode may be a foreign writer's success, so keep the private
    backup and report the conflict rather than overwriting it.
    """
    if not swap_started:
        _unlink_for_cleanup(backup, backup_identity, failures)
        return
    try:
        current_identity = _file_identity(destination)
    except OSError:
        current_identity = None
    if current_identity == original_identity:
        # The replace did not take effect, or a previous reconciliation already
        # restored it. The extra private copy is ours to remove.
        _unlink_for_cleanup(backup, backup_identity, failures)
        return
    if current_identity != staged_identity:
        failures.append(backup)
        return
    restored = False
    try:
        if _entry_identity(backup) != backup_identity:
            failures.append(backup)
            return
        # This observes ownership immediately before the replace. POSIX has no
        # compare-and-swap rename, so a hostile concurrent rename after this
        # check is still outside the CLI's locking authority.
        os.replace(backup, destination)
        restored = True
    except OSError:
        try:
            restored = _file_identity(destination) == backup_identity
        except OSError:
            restored = False
        if not restored:
            try:
                backup_retained = _entry_identity(backup) == backup_identity
            except OSError:
                backup_retained = False
            failures.append(backup if backup_retained else destination)
    if restored:
        try:
            restore_handle = _open_regular_output(destination, backup_identity)
            try:
                _restore_metadata(restore_handle, metadata)
            finally:
                os.close(restore_handle)
            metadata_scope_warnings.append(destination)
        except (OSError, Refusal):
            failures.append(destination)


def _append_cleanup_detail(error, message):
    """Keep recovery details visible on Python versions without add_note."""
    # Python prints `SystemExit.code`, not exception notes. A Refusal is a
    # SystemExit so that command-line validation exits without a traceback;
    # put the retained location in its visible code rather than hiding it in
    # an unrendered note.
    if isinstance(error, Refusal):
        error.code = f"{error.code}\n{message}"
        return
    add_note = getattr(error, "add_note", None)
    if callable(add_note):
        add_note(message)
        return
    # BaseException.add_note arrived in Python 3.11. OSError can render cached
    # errno, strerror, and filename fields instead of its mutable `args`, so
    # retain the original exception intact and write the recovery detail where
    # an unhandled CLI failure will still show it on Python 3.10.
    print(message, file=sys.stderr)


def _note_cleanup_failures(error, failures):
    if failures:
        retained = ", ".join(sorted(set(failures)))
        message = "output cleanup or rollback failed; retained path(s): " + retained
        _append_cleanup_detail(error, message)


def _note_rollback_metadata_scope(error, restored_paths):
    """Disclose metadata classes not established by this caught rollback."""
    if not restored_paths:
        return
    restored = ", ".join(sorted(set(restored_paths)))
    message = (
        "rollback restored bytes and captured portable metadata for: "
        f"{restored}; extended ACLs and file flags were not verified"
    )
    _append_cleanup_detail(error, message)


def _cleanup_committed_outputs(replaced, claimed, failures):
    """Remove old private copies after every replacement has committed."""
    for swap in replaced:
        backup = swap["backup"]
        _unlink_for_cleanup(backup["path"], backup["identity"], failures)
        _close_owned_path(backup, failures)
        _close_owned_path(swap["original"], failures)
    for record in claimed:
        _close_owned_path(record, failures)


def _reconcile_interrupted_committed_cleanup(replaced, claimed, failures):
    """Close pins and disclose owned old copies without undoing a commit."""
    for swap in replaced:
        backup = swap["backup"]
        try:
            if _entry_identity(backup["path"]) == backup["identity"]:
                failures.append(str(backup["path"]))
            elif os.path.lexists(backup["path"]):
                failures.append(str(backup["path"]))
        except FileNotFoundError:
            pass
        except OSError:
            if os.path.lexists(backup["path"]):
                failures.append(str(backup["path"]))
        finally:
            _close_owned_path(backup, failures)
            _close_owned_path(swap["original"], failures)
    for record in claimed:
        _close_owned_path(record, failures)


def write_outputs(targets, accept_inherited=False, after_claim=None):
    """Claim **every** destination, then write them. All of them or none.

    `targets` is [(path, text), ...] in the order they should be reported.
    `after_claim` runs once every destination is claimed and still empty;
    raising from it rolls the whole set back.

    Creating each file at its own write site left a partial result that the
    refusal's own remedy could not clear: with `--out` new and `--manifest`
    taken, the XML was written, the manifest was refused, and "remove it and
    re-run" then failed on the XML the failed run had just created. Preflight
    narrowed that to a race but did not close it, because the two creates were
    still independent events with the first payload written in between.

    **Nothing that already exists is touched until every payload is written.**
    An earlier version claimed each destination with `O_TRUNC` on POSIX, which
    emptied an existing output at claim time — so a later failure rolled back by
    unlinking a file whose previous contents the run had already destroyed. A
    command that refused could therefore leave the operator with neither the old
    output nor a new one. That is worse than the partial result it was fixing.

    So a destination that exists is **staged**: the payload is written to a
    sibling temporary file and renamed over the destination at the end, which is
    atomic and happens only once every target has succeeded. A destination that
    does not exist is created exclusively under its own name, which is both the
    reservation against a concurrent run and, on Windows, the rule itself —
    there an existing destination is refused outright rather than staged.

    A destination that is a **symlink** is written through to its target: the
    sibling temporary file is created next to (and the final rename targets)
    `os.path.realpath(path)`, not `path` itself. `os.replace` does not follow a
    symlink at the destination — it replaces the link, the same as `unlink`
    would — so renaming onto the link's own name would silently turn it into a
    plain file and leave whatever else reads through that link looking at
    stale content.

    Before replacing an existing POSIX destination, its old inode is copied from
    an opened descriptor to a private sibling backup. The destination therefore
    remains present until one `os.replace` atomically changes it from old bytes
    to new bytes. This is rollback for exceptions caught in this process, not a
    multi-file crash transaction: a process or host crash can retain private
    `.bak` files and leave different destinations at different committed
    versions.

    The supplied destination's resolved path and inode are revalidated right
    before that replacement. A symlink (or symlinked parent) retargeted after
    claiming is refused instead of silently writing the stale target. This
    detects changes observed at the commit boundary; a hostile filesystem that
    changes the path again after that check remains outside this CLI's locking
    authority.
    """
    claimed, staged, replaced = [], [], []
    # A record is the one ownership authority for a pathname: cleanup may
    # unlink it only while its identity still equals record["identity"].
    pending_backup = None
    pending_swap = None
    cleanup_failures = []
    metadata_scope_warnings = []
    committed = False
    try:
        for path, _ in targets:
            if os.path.exists(path):
                # Refuses here on Windows; on POSIX, stage beside it.
                refusal = windows_destination_refusal(path, accept_inherited)
                if refusal:
                    raise refusal
                real_path = os.path.realpath(path)
                handle, temporary = tempfile.mkstemp(
                    dir=os.path.dirname(real_path),
                    prefix=os.path.basename(real_path) + ".", suffix=".part")
                record = _owned_path(temporary, handle)
                claimed.append(record)
                staged.append({"temporary": record, "supplied_path": path,
                               "real_path": real_path,
                               "original_identity": _file_identity(real_path)})
            else:
                handle = _open_private(path, accept_inherited)
                record = _owned_path(path, handle)
                claimed.append(record)
        if after_claim is not None:
            after_claim()
        for (_, text), record in zip(targets, claimed):
            where = record["path"]
            # The record already owns the opened descriptor, so a duplicate
            # failure here can still close it and unlink the created path.
            handle = os.dup(record["pin"])
            with os.fdopen(handle, "w", encoding="utf-8", newline="") as stream:
                stream.write(text)
        # Every payload is on disk. A private copy preserves the old bytes while
        # the requested destination stays present until the atomic replacement.
        for state in staged:
            temporary = state["temporary"]
            supplied_path, real_path = state["supplied_path"], state["real_path"]
            original_identity = state["original_identity"]
            backup_handle, backup = tempfile.mkstemp(
                dir=os.path.dirname(real_path),
                prefix=os.path.basename(real_path) + ".", suffix=".bak")
            pending_backup = _owned_path(backup, backup_handle)
            pending_swap = {"backup": pending_backup, "destination": real_path,
                            "original_identity": original_identity,
                            "staged_identity": temporary["identity"],
                            "original": None, "metadata": None,
                            "swap_started": False}
            pending_backup = None
            # This ownership pin both prevents original-inode ABA reuse and
            # captures metadata before the backup read can update atime.
            original_handle = _open_regular_output(real_path, original_identity)
            pending_swap["original"] = _owned_path(real_path, original_handle)
            pending_swap["metadata"] = _metadata_from_handle(
                real_path, original_handle)
            _copy_private_backup(real_path, original_identity, backup_handle)
            # The destination stays present until this one atomic replacement.
            # `pending_swap` is set first because an interrupt may arrive after
            # the filesystem call has taken effect but before it returns.
            # Revalidate *after* the backup operation: it is a filesystem call
            # an attacker can use to retarget the supplied symlink before this
            # commit. The pending backup lets the refusal cleanly undo itself.
            if (os.path.realpath(supplied_path) != real_path
                    or _file_identity(real_path) != original_identity):
                raise Refusal(
                    "output_path_changed",
                    f"{supplied_path} changed after it was claimed; no output was replaced",
                )
            if _entry_identity(temporary["path"]) != temporary["identity"]:
                raise Refusal(
                    "output_path_changed",
                    f"{supplied_path} staged output changed before replacement",
                )
            if _entry_identity(pending_swap["backup"]["path"]) != \
                    pending_swap["backup"]["identity"]:
                raise Refusal(
                    "output_path_changed",
                    f"{supplied_path} rollback copy changed before replacement",
                )
            pending_swap["swap_started"] = True
            os.replace(temporary["path"], real_path)
            replaced.append(pending_swap)
            pending_swap = None
        # The final replacement is the boundary between rollback and committed
        # cleanup. Keep it in this same handler so an interrupt before cleanup
        # starts cannot skip both recovery paths.
        committed = True
        _cleanup_committed_outputs(replaced, claimed, cleanup_failures)
        if cleanup_failures:
            raise OutputCleanupFailure(cleanup_failures)
    except BaseException as error:
        if committed:
            # This is after the transaction committed. Never call
            # `_restore_backup` here: an interrupt during old-copy cleanup must
            # preserve the new output, close every ownership pin, and identify
            # any old private copy that still needs operator cleanup.
            _reconcile_interrupted_committed_cleanup(
                replaced, claimed, cleanup_failures)
            _note_cleanup_failures(error, cleanup_failures)
            raise
        # Reconcile a swap which may have completed before raising, then undo
        # earlier committed swaps. Cleanup failures remain attached to the
        # original exception with their recoverable locations.
        if pending_swap is not None:
            backup = pending_swap["backup"]
            _restore_backup(backup["path"], backup["identity"],
                            pending_swap["destination"],
                            pending_swap["original_identity"],
                            pending_swap["staged_identity"],
                            pending_swap["metadata"],
                            pending_swap["swap_started"], cleanup_failures,
                            metadata_scope_warnings)
            _close_owned_path(backup, cleanup_failures)
            if pending_swap["original"] is not None:
                _close_owned_path(pending_swap["original"], cleanup_failures)
        if pending_backup is not None and (
                pending_swap is None
                or pending_backup["path"] != pending_swap["backup"]["path"]
                or pending_backup["identity"] != pending_swap["backup"]["identity"]):
            _cleanup_owned_path(pending_backup, cleanup_failures)
        for swap in reversed(replaced):
            # An interrupt can arrive after `_record_replaced_swap` appends but
            # before its caller clears `pending_swap`. That one backup has
            # already been reconciled above; restoring it twice risks treating
            # the now-restored destination as a second transaction outcome.
            if swap is pending_swap:
                continue
            backup = swap["backup"]
            _restore_backup(backup["path"], backup["identity"], swap["destination"],
                            swap["original_identity"], swap["staged_identity"],
                            swap["metadata"], swap["swap_started"], cleanup_failures,
                            metadata_scope_warnings)
            _close_owned_path(backup, cleanup_failures)
            _close_owned_path(swap["original"], cleanup_failures)
        for record in claimed:
            _cleanup_owned_path(record, cleanup_failures)
        _note_cleanup_failures(error, cleanup_failures)
        _note_rollback_metadata_scope(error, metadata_scope_warnings)
        raise


def _check_paths(args):
    """Every path this run touches must be a distinct file.

    `--out` pointing at the PDF destroys the source statement after parsing;
    `--out` and `--manifest` sharing a path leaves whichever was written second,
    with both success lines printed.
    """
    named = [(flag, pathlib.Path(value).expanduser().resolve())
             for flag, value in (("--pdf", args.pdf), ("--mapping", args.mapping),
                                 ("--out", args.out), ("--manifest", args.manifest))
             if value]
    for index, (flag, path) in enumerate(named):
        for other_flag, other in named[:index]:
            # `resolve()` follows symlinks but two hard links to one inode keep
            # different names, and `--out` naming a second link to the input PDF
            # destroys the statement on the O_TRUNC. `samefile` asks the
            # filesystem; it needs both paths to exist, so the lexical compare
            # stays for the output that does not yet.
            same = path == other
            if not same and path.exists() and other.exists():
                same = path.samefile(other)
            if same:
                raise Refusal(
                    "path_collision",
                    f"{flag} ({path}) and {other_flag} ({other}) are the same file",
                )


def _cli_date(text, flag):
    """An ISO date from the command line, or a typed refusal.

    `2026-02-30` has the right shape and is not a date, so `fromisoformat`
    raises `ValueError` — a traceback where every other malformed input to this
    tool produces a category.
    """
    if not text:
        return None
    try:
        return datetime.date.fromisoformat(text)
    except ValueError:
        raise Refusal("malformed_cli_date",
                      f"{flag} {text!r} is not a calendar date in YYYY-MM-DD form")


def preflight(args):
    """Everything that can be refused before the PDF is opened.

    Kept together and run first on purpose: a mistyped flag should cost
    nothing, and none of these checks needs the statement. Returns the parsed
    control values so `main` never re-parses a string it has already validated.
    """
    if not args.company.strip():
        raise Refusal(
            "company_blank",
            "--company is empty. An empty SVCURRENTCOMPANY matches no company, and "
            "9.11d means Tally imports into whichever company is open rather than "
            "refusing — so a blank name is the most dangerous value this flag can "
            "take, not the most harmless.",
        )
    if args.confirm_open_company != args.company:
        raise Refusal(
            "company_unconfirmed",
            f"--company {args.company!r} and --confirm-open-company "
            f"{args.confirm_open_company!r} differ. They must match exactly; read the "
            "name off the open Tally window rather than copying it from this command.",
        )
    if not (args.out or args.manifest or args.dry_run):
        raise Refusal(
            "no_output_requested",
            "nothing would be written. Pass --out and/or --manifest to keep the "
            "generated document, or --dry-run to review the mapping without writing.",
        )
    _check_paths(args)
    # Both destinations, before either is written. Judging them one at a time at
    # write time meant a run with a new `--out` and an existing `--manifest`
    # wrote the XML, then refused the manifest: a partial result whose stated
    # remedy — remove the manifest and re-run — immediately failed on the XML
    # the failed run had just created. `_claim_destinations` re-checks at the
    # create, where it can be atomic.
    #
    # Not under `--dry-run`, which returns before opening either destination.
    # These checks are about writing a file; refusing a preview that would write
    # nothing made `--dry-run` unusable on Windows for exactly the command an
    # operator wants to preview — their real one, flags and all.
    if not args.dry_run:
        for path in (args.out, args.manifest):
            if path:
                refusal = windows_destination_refusal(path, args.accept_inherited_permissions)
                if refusal:
                    raise refusal

    window = tuple(_cli_date(value, flag) for value, flag in
                   ((args.date_from, "--from"), (args.date_to, "--to")))
    if all(window) and window[0] > window[1]:
        raise Refusal(
            "reversed_date_window",
            f"--from {window[0]} is after --to {window[1]}; every transaction would be "
            "excluded and the run would report a successful zero-voucher import",
        )
    # a balance may be negative on an overdrawn account; a total of withdrawals
    # or deposits may not
    return window, {
        "opening": control_value(args.opening, "--opening", signed=True),
        "closing": control_value(args.expect_closing, "--expect-closing", signed=True),
        "debits": control_value(args.expect_debits, "--expect-debits"),
        "credits": control_value(args.expect_credits, "--expect-credits"),
    }


def verify_against_statement(rows, bank, expected):
    """Prove the parse reproduces every control total the statement prints.

    Three comparisons, and all three are needed. The running balance (already
    replayed in `reconcile`) proves each row's arithmetic; the closing balance
    proves the chain ends where the statement says; and the debit and credit
    totals are the only ones that see a dropped tail whose two sides cancel,
    because the closing balance is their net.
    """
    totals = {
        "debits": sum(_money(row[bank.debit_column], bank.debit_column, index) or D(0)
                      for index, row in enumerate(rows, 1)),
        "credits": sum(_money(row[bank.credit_column], bank.credit_column, index) or D(0)
                       for index, row in enumerate(rows, 1)),
    }
    for label, actual in totals.items():
        if actual != expected[label]:
            raise Refusal(
                "control_total_mismatch",
                f"{label} total {actual} does not match the statement's printed "
                f"{expected[label]}. Rows are missing or misread — the closing balance "
                "alone cannot see this, because it is the net of the two.")
    return totals


def _manifest_csv(records):
    """The manifest as text. Separated from writing it so both payloads exist
    before either destination is claimed."""
    writer_target = io.StringIO()
    writer = csv.DictWriter(writer_target, fieldnames=list(MANIFEST_COLUMNS))
    writer.writeheader()
    writer.writerows(records)
    return writer_target.getvalue()


def _print_dry_run(manifest):
    """The list the operator writes the mapping from, grouped the way the mapping
    is read.

    Grouped on `_key`, not on the printed name. One counterparty can reach this
    list under two spellings — the wrap heuristic decides per line whether a
    space survives, so a real statement showed one payee as both
    `MERCURY MANUFACTURERS` and `MERCURY M ANUFACTURERS`. Those are one mapping
    row, because `_key` collapses them, and listing them as two invites the
    operator to write two rows and then wonder why the second never fires.

    The spelling shown is the one with the **fewest words**, then the most
    value. Fewest words is not a guess: a cell wrap can only ever insert a space
    that the printed name did not have, never remove one, so between
    `MERCURY MANUFACTURERS` and `MERCURY M ANUFACTURERS` the shorter word count
    is the one the bank printed. The others are named beneath it, so an operator
    searching the statement for a variant still finds it here.
    """
    grouped = {}
    for record in manifest:
        key = (_key(record["party"]), record["voucher_type"], record["suspense"])
        bucket = grouped.setdefault(key, {"total": D(0), "spellings": {}})
        amount = D(record["amount"])
        bucket["total"] += amount
        bucket["spellings"][record["party"]] = (
            bucket["spellings"].get(record["party"], D(0)) + amount)

    print(f"\n{'COUNTERPARTY':<34}{'TYPE':<10}{'TOTAL':>14}  SUSPENSE")
    for (_, kind, suspense), bucket in sorted(
            grouped.items(), key=lambda item: -item[1]["total"]):
        spellings = sorted(bucket["spellings"].items(), key=lambda kv: -kv[1])
        shown = min(spellings, key=lambda kv: (len(kv[0].split()), -kv[1]))[0]
        spellings = [entry for entry in spellings if entry[0] != shown]
        print(f"{shown[:33]:<34}{kind:<10}{bucket['total']:>14,}  {suspense}")
        for variant, _amount in spellings:
            print(f"  also printed as {variant[:60]}")
    print("\nOne line per mapping row: spellings that differ only in SPACING are "
          "one counterparty,\nand one row in the CSV covers them. Punctuation is "
          "significant — `A & B` and `AB`\nare two rows, and a variant left "
          "unmapped falls to suspense.")


def _print_operator_notes(company, skipped):
    """The three things that decide whether this file lands correctly.

    Printed after the write rather than before it, because that is when the
    operator turns to Tally, and none of them is enforceable from here.
    """
    print("\nBEFORE IMPORTING: confirm the open company in Tally is "
          f"{company!r}. Tally will not check it for you (9.11d).")
    if skipped:
        print("\nIF THIS STATEMENT WAS ALREADY IMPORTED: the "
              f"{skipped} skipped row(s) are omitted from this file, and omission "
              "is not deletion — a re-import upserts what is present and leaves the "
              "rest standing. Delete those vouchers by the REMOTEIDs in the manifest, "
              "or the transfer they represent is counted twice.")
    print("\nTO CORRECT A BATCH ALREADY IMPORTED: neither available path is qualified for "
          "these voucher types, so do not treat either as routine.\n"
          "  * Re-importing over the top relies on upsert-by-REMOTEID, verified for Journal "
          "only (9.8), and a corrected file is a DIFFERENT payload — 3.3a lists that as "
          "untested, and it may overwrite, partially update, or duplicate.\n"
          "  * Deleting by the manifest's REMOTEIDs relies on 9.7's Delete row, measured on "
          "an Education/Edit Log instance, not on a licensed Gold book.\n"
          "Try one on a SINGLE voucher first and read it back. Judge by the import summary "
          "and the voucher, never by a count: a rejected repeat leaves the count unchanged "
          "too. Until then, correct by hand or with a journal.")


def _nonblank(flag):
    """An argparse type that rejects an empty or whitespace-only value.

    `required=True` only asserts the flag was **given**. A shell that expands
    `--bank-ledger "$LEDGER"` with `LEDGER` unset supplies an empty string,
    which satisfied argparse and then reached every voucher as an empty
    `<LEDGERNAME>`. `selfcheck` compared that empty value back against the same
    empty argument and agreed with itself, so the run printed correct voucher
    and bank totals for a file Tally cannot use.
    """
    def parse(text):
        if not text.strip():
            raise argparse.ArgumentTypeError(
                f"{flag} is empty. An empty ledger name reaches every voucher as an "
                "empty <LEDGERNAME>, and the self-check cannot see it because it "
                "compares the file against this same argument.")
        return text
    return parse


def build_parser():
    """The command line, kept apart from the run so it can be read as a whole.

    Every `required=True` here is a review finding: each was optional once, and
    each optional one was a way to produce a confident-looking file that was
    wrong. See `preflight` for the checks argparse cannot express.
    """
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("--pdf", required=True)
    parser.add_argument("--accept-inherited-permissions", action="store_true",
                        help="Windows only: proceed although the output will inherit the "
                             "directory's ACL rather than being owner-only. Use after "
                             "checking the directory with `icacls`.")
    parser.add_argument("--bank", required=True, choices=sorted(BANKS))
    parser.add_argument("--company", required=True,
                        help="EXACT company name as Tally shows it")
    parser.add_argument("--confirm-open-company", required=True, metavar="NAME",
                        help="repeat the company name as it appears in the title bar of "
                             "the Tally window you are about to import into. A "
                             "mismatched company name has been seen to import into "
                             "whichever company was open, silently (9.11d); when that "
                             "happens is not settled. So this tool cannot verify the "
                             "target and neither can the XML. This flag makes "
                             "confirming it a deliberate act.")
    parser.add_argument("--bank-ledger", required=True, type=_nonblank("--bank-ledger"),
                        help="EXACT name of the bank ledger in Tally")
    parser.add_argument("--account-tail", required=True,
                        help="short account label carrying at least the last 4 digits of "
                             "the account, e.g. 'SBI CA xx2129'. Those digits must appear "
                             "in the statement or the run is refused.")
    parser.add_argument("--opening", required=True, help="opening balance, to replay the statement")
    parser.add_argument("--expect-closing", required=True,
                        help="the closing balance the statement itself prints. The replayed "
                             "chain must land on it.")
    parser.add_argument("--expect-debits", required=True,
                        help="the statement's own printed total of withdrawals")
    parser.add_argument("--expect-credits", required=True,
                        help="the statement's own printed total of deposits. Required with "
                             "--expect-debits because the closing balance alone is the NET of "
                             "the transactions: a dropped tail whose debits and credits cancel "
                             "still lands on the printed closing figure. These two totals are "
                             "independent of the net and of each other.")
    parser.add_argument("--mapping", help="CSV: " + ",".join(MAPPING_COLUMNS))
    parser.add_argument("--suspense", default="SUSPENSE ACC")
    parser.add_argument("--from", dest="date_from", help="YYYY-MM-DD")
    parser.add_argument("--to", dest="date_to", help="YYYY-MM-DD")
    parser.add_argument("--out")
    parser.add_argument("--manifest")
    parser.add_argument("--dry-run", action="store_true",
                        help="list counterparties and totals; write nothing")
    return parser


def main(argv=None):
    parser = build_parser()
    args = parser.parse_args(argv)
    window, expected = preflight(args)

    bank = BANKS[args.bank]()
    rows, pages = parse(pathlib.Path(args.pdf), read_password(), bank)
    account = require_account_match(pages, bank, args.account_tail)
    closing = reconcile(rows, bank, expected["opening"], expected["closing"])
    totals = verify_against_statement(rows, bank, expected)
    print(f"parsed {len(rows)} rows; running balance reproduced on every row")
    print(f"  debits {totals['debits']:,}  credits {totals['credits']:,}  closing {closing:,}")
    print("  debits, credits and closing all match the statement's printed totals")

    vouchers, manifest = build(
        rows, bank, args.company, args.bank_ledger, args.suspense,
        load_mapping(args.mapping), args.account_tail, account, *window,
    )
    if args.dry_run:
        _print_dry_run(manifest)
        return 0

    xml_text = envelope(args.company, vouchers)
    count, outward, inward = selfcheck(xml_text, args.bank_ledger, manifest)
    skipped = sum(1 for record in manifest if record["voucher_type"] == "SKIPPED")
    print(f"\n{count} vouchers; {skipped} skipped as already carried elsewhere")
    print(f"  bank ledger out {outward:,}  in {inward:,}")
    print(f"  suspense vouchers {sum(1 for record in manifest if record['suspense'])}")
    targets = []
    if args.out:
        targets.append((args.out, xml_text))
    if args.manifest:
        targets.append((args.manifest, _manifest_csv(manifest)))
    if targets:
        # `after_claim` re-runs the collision check once both destinations
        # exist. The check before the run can only compare the two paths
        # lexically, because `samefile` needs both to exist — and a lexical
        # compare is case-sensitive while the volume may not be, so
        # `--out Result.xml --manifest result.XML` passed it. Asking the
        # filesystem here costs one call, and now happens while both files are
        # still **empty**: previously the XML had already been written and the
        # manifest truncated it, with both success lines printed.
        write_outputs(targets, args.accept_inherited_permissions,
                      after_claim=lambda: _check_paths(args))
        for path, _ in targets:
            print(f"  wrote {path} (mode 0600)")
    _print_operator_notes(args.company, skipped)
    return 0


if __name__ == "__main__":
    sys.exit(main())
