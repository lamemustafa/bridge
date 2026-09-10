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
`SVCURRENTCOMPANY` is *not* a write guard: a name matching no loaded company is
ignored and the vouchers land in whichever company happens to be open. There is
no offline fix for that, so `--confirm-open-company` makes it a deliberate
operator act instead of a silent one. See `main`.

Everything else fails closed. The design rule throughout: a malformed input, an
ambiguous mapping or an unproven extent must stop the run, because every output
of this tool is posted to a real book.
"""

import argparse
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
import subprocess
import sys
import tempfile
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
            # reference — a real UTR arrives as "HDF CH01206262147" when the
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
        narr = row["narr"]
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
        found = re.match(r"^ACH D-\s*TP ACH (.+?)-\d+", narr)
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
    """Mapping key: letters and digits only, case-folded.

    Deliberately more aggressive than Tally's own matching (`_ledger_key`),
    because the wrap heuristic can space one counterparty's name two ways and
    both spellings must reach the same mapping row. The cost is that it can
    also collapse two *different* names — `load_mapping` refuses a file where
    that happens rather than letting one silently win.
    """
    return re.sub(r"[^A-Z0-9]", "", text.upper())


def _ledger_key(name):
    """Tally's own master-name identity — IMPLEMENTATION_GUIDE.md 3.3b.

    VERIFIED there: matching is case-insensitive and treats a hyphen as a
    space, and is otherwise exact ('&' is NOT equivalent to 'AND', and a
    missing word does not match). So this is the comparison to use whenever
    the question is "will Tally consider these the same ledger?" — an exact
    string compare answers a different, wrong question.
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
            started = bank.is_row_start(cells)
            # A footer anchor is a *subset* test, so a transaction whose
            # counterparty is the bank itself carries "HDFC BANK LIMITED" and
            # would end the page — dropping that row and every row after it. A
            # line that opens a transaction is a transaction, whatever else it
            # says, so the date decides and the anchor only breaks the ties.
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
    """The canonical account identity: the digits of the operator's label.

    `HDFC CA xx1234` and `HDFC xx1234` name one account and must reduce to one
    value, because this feeds the REMOTEID — two spellings of a free-form label
    must not turn one transaction into two vouchers on re-import.
    """
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
    if not any(run.endswith(digits) for run in runs):
        raise Refusal(
            "account_not_in_statement",
            f"no number ending {digits} appears on this statement's account-number "
            "line. Either the PDF is not the account named by --bank-ledger/"
            "--account-tail, or the account digits were mistyped. Refusing to "
            "post it anywhere.",
        )


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
        debit = _money(row[bank.debit_column], bank.debit_column, index) or D(0)
        credit = _money(row[bank.credit_column], bank.credit_column, index) or D(0)
        if debit and credit:
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
        headers = {(name or "").strip().lower() for name in (reader.fieldnames or ())}
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


def _remote_id(account_tail, date, row, bank):
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

    Nothing operator-controlled may enter this. `--account-tail` is a free-form
    label, so `HDFC CA xx1234` and `HDFC xx1234` are the same account spelled
    two ways; only its digits are used, in both the digest and the prefix.
    Otherwise regenerating a file with a tidied-up label would give every
    transaction a new key and duplicate the whole statement on re-import.
    """
    material = "\0".join((
        account_digits(account_tail),
        date.isoformat(),
        (row.get(bank.debit_column) or "").strip(),
        (row.get(bank.credit_column) or "").strip(),
        (row.get(bank.balance_column) or "").strip(),
        _squash(row.get(bank.narration_column, "")),
    ))
    digest = hashlib.sha256(material.encode("utf-8")).hexdigest()[:12].upper()
    return f"AC{account_digits(account_tail)}-{date.strftime('%Y%m%d')}-{digest}"


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
          date_from=None, date_to=None):
    """Vouchers and a manifest row per statement row, in printed order.

    The manifest is not a log. It is the operator's record of what each voucher
    was made from and, through `remoteid`, the only way to remove one
    afterwards — including for rows this run deliberately did not emit.
    """
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
                remoteid=_remote_id(account_tail, date, row, bank),
                narration="excluded: carried by the other account's contra"))
            continue
        kind = "Contra" if treatment == "contra" else ("Payment" if outward else "Receipt")
        mode, reference = bank.reference(row)
        # Tally resolves 'suspense-acc' and 'SUSPENSE ACC' to the same master
        # (3.3b), so an exact string compare would post to suspense while
        # reporting the row as resolved and omitting the operator's warning.
        unidentified = _ledger_key(ledger) == _ledger_key(suspense)
        shown = party if unidentified else ledger
        narration = _squash(
            f"{mode} {reference} {'to' if outward else 'from'} {shown}"
            f" | {account_tail} | {date.strftime('%d-%b-%Y')}"
        )
        if unidentified:
            narration += " | UNIDENTIFIED - reallocate from Suspense"
        remote_id = _remote_id(account_tail, date, row, bank)
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


def _write_private(path, text):
    """Create output files readable only by their owner.

    The XML and the manifest carry counterparty names, amounts, an account
    label and every narration in the statement. On a shared host the default
    022 umask would publish all of it as mode 0644.
    """
    handle = os.open(path, os.O_WRONLY | os.O_CREAT | os.O_TRUNC, 0o600)
    with os.fdopen(handle, "w", encoding="utf-8", newline="") as stream:
        stream.write(text)
    os.chmod(path, 0o600)  # an existing file keeps its old mode through O_CREAT


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


def _write_csv(path, records):
    writer_target = io.StringIO()
    writer = csv.DictWriter(writer_target, fieldnames=list(MANIFEST_COLUMNS))
    writer.writeheader()
    writer.writerows(records)
    _write_private(path, writer_target.getvalue())


def _print_dry_run(manifest):
    totals = {}
    for record in manifest:
        key = (record["party"], record["voucher_type"], record["suspense"])
        totals[key] = totals.get(key, D(0)) + D(record["amount"])
    print(f"\n{'COUNTERPARTY':<34}{'TYPE':<10}{'TOTAL':>14}  SUSPENSE")
    for (party, kind, suspense), total in sorted(totals.items(), key=lambda kv: -kv[1]):
        print(f"{party[:33]:<34}{kind:<10}{total:>14,}  {suspense}")


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


def build_parser():
    """The command line, kept apart from the run so it can be read as a whole.

    Every `required=True` here is a review finding: each was optional once, and
    each optional one was a way to produce a confident-looking file that was
    wrong. See `preflight` for the checks argparse cannot express.
    """
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("--pdf", required=True)
    parser.add_argument("--bank", required=True, choices=sorted(BANKS))
    parser.add_argument("--company", required=True,
                        help="EXACT company name as Tally shows it")
    parser.add_argument("--confirm-open-company", required=True, metavar="NAME",
                        help="repeat the company name as it appears in the title bar of "
                             "the Tally window you are about to import into. Tally "
                             "ignores a company name that matches nothing and imports "
                             "into whichever company is open (9.11d), so this tool "
                             "cannot verify the target and neither can the XML. This "
                             "flag makes confirming it a deliberate act.")
    parser.add_argument("--bank-ledger", required=True)
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
    require_account_match(pages, bank, args.account_tail)
    closing = reconcile(rows, bank, expected["opening"], expected["closing"])
    totals = verify_against_statement(rows, bank, expected)
    print(f"parsed {len(rows)} rows; running balance reproduced on every row")
    print(f"  debits {totals['debits']:,}  credits {totals['credits']:,}  closing {closing:,}")
    print("  debits, credits and closing all match the statement's printed totals")

    vouchers, manifest = build(
        rows, bank, args.company, args.bank_ledger, args.suspense,
        load_mapping(args.mapping), args.account_tail, *window,
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
    if args.out:
        _write_private(args.out, xml_text)
        print(f"  wrote {args.out} (mode 0600)")
    if args.manifest:
        _write_csv(args.manifest, manifest)
        print(f"  wrote {args.manifest} (mode 0600)")
    _print_operator_notes(args.company, skipped)
    return 0


if __name__ == "__main__":
    sys.exit(main())
