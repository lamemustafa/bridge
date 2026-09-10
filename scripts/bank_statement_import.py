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
  9.3   there is no idempotency — importing the same file twice creates a second
        set of vouchers. Each voucher carries a REMOTEID so a bad batch can be
        deleted (9.7: Delete by REMOTEID is the only working correction path).
  9.8   under automatic numbering Tally silently discards a supplied
        VOUCHERNUMBER, so this module does not send one and puts the bank's
        reference in the narration, where it survives.

  Sign convention: a debit is ISDEEMEDPOSITIVE Yes with a NEGATIVE amount.

Verification the caller should always do, in order of strength:
  1. the parser refuses to emit rows whose running balance does not reproduce
     every printed closing balance (--opening);
  2. where the statement prints its own control totals, check them (--expect);
  3. after import, the bank ledger's closing balance in Tally.
"""

import argparse
import csv
import datetime
import decimal
import pathlib
import re
import subprocess
import sys
import tempfile
import xml.etree.ElementTree as ET

D = decimal.Decimal
WORD = re.compile(
    r'<word xMin="([\d.]+)" yMin="([\d.]+)" xMax="([\d.]+)" yMax="([\d.]+)">(.*?)</word>',
    re.S,
)


# --------------------------------------------------------------------------- #
# PDF -> rows                                                                  #
# --------------------------------------------------------------------------- #

def _unescape(text):
    for entity, char in (("&amp;", "&"), ("&lt;", "<"), ("&gt;", ">"),
                         ("&quot;", '"'), ("&apos;", "'")):
        text = text.replace(entity, char)
    return text


def _pdf_pages(pdf, password):
    """Word boxes per page. Tries the user password then the owner password:
    banks commonly protect statements with the owner password only, which
    `pdftotext -upw` rejects."""
    with tempfile.TemporaryDirectory() as tmp:
        out = pathlib.Path(tmp) / "stmt.xml"
        for flag in ("-upw", "-opw"):
            done = subprocess.run(
                ["pdftotext", "-bbox-layout", flag, password, str(pdf), str(out)],
                capture_output=True,
            )
            if done.returncode == 0:
                xml = out.read_text(encoding="utf-8")
                break
        else:
            raise SystemExit(f"cannot open {pdf}: wrong password or not a PDF")
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

    def party(self, row):
        narr = row["narr"]
        found = re.match(r"^IMPS-\d+-(.+?)-[A-Z]{4}\d?", narr)
        if found:
            return _squash(found.group(1))
        found = re.match(r"^NEFT (?:CR|DR)-[A-Z0-9]+-(.+?)-", narr)
        if found:
            return _squash(found.group(1))
        found = re.match(r"^\d{10,}-TPT-[^-]*-(.+)$", narr)
        if found:
            return _squash(found.group(1))
        found = re.match(r"^UPI-(.+?)-", narr)
        if found:
            candidate = _squash(found.group(1))
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


def _key(text):
    """Mapping key: letters and digits only, case-folded."""
    return re.sub(r"[^A-Z0-9]", "", text.upper())


# --------------------------------------------------------------------------- #
# Parsing                                                                      #
# --------------------------------------------------------------------------- #

def _matches(group, anchors):
    words = {t for *_, t in group}
    return any(all(token in words for token in anchor) for anchor in anchors)


def parse(pdf, password, bank):
    """Statement rows, in printed order."""
    rows, current, stop = [], None, False
    for page in _pdf_pages(pdf, password):
        if stop:
            break
        lines = _lines(page)
        top = None
        for anchor in bank.top_anchors:
            top = next((y for y, group in lines
                        if all(any(t == token for *_, t in group) for token in anchor)), None)
            if top is not None:
                break
        if top is None:
            continue
        if bank.end_anchors and any(_matches(g, bank.end_anchors) for _, g in lines):
            stop = True
        for y, group in lines:
            if y <= top:
                continue
            if bank.bottom_anchors and _matches(group, bank.bottom_anchors):
                break
            if bank.header_words and all(t in bank.header_words for *_, t in group):
                continue
            cells = {}
            for x0, _, x1, _, text in group:
                cells.setdefault(bank.column_of(x0, x1), []).append((x0, x1, text))
            started = bank.is_row_start(cells)
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


def _money(text):
    return D(text) if re.fullmatch(r"\d+(\.\d+)?", text or "") else None


def reconcile(rows, bank, opening):
    """Replay the statement's own running balance. Returns the closing balance.

    This is the parser's correctness proof: an amount misread by one digit, a
    dropped row or a row counted twice all break the chain. Raises on the first
    row that does not reproduce its printed balance.
    """
    running = D(opening)
    for index, row in enumerate(rows):
        debit = _money(row[bank.debit_column]) or D(0)
        credit = _money(row[bank.credit_column]) or D(0)
        printed = _money(row[bank.balance_column])
        running = running - debit + credit
        if printed is None or running != printed:
            raise SystemExit(
                f"row {index + 1} ({row[bank.date_column]}) breaks the running balance: "
                f"expected {running}, statement prints {printed}"
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
        for record in csv.DictReader(handle):
            party = _squash(record.get("party", ""))
            if not party:
                continue
            treatment = (record.get("treatment") or "auto").strip().lower()
            if treatment not in ("auto", "contra", "skip"):
                raise SystemExit(f"unknown treatment {treatment!r} for {party!r}")
            # key on letters and digits only: the wrap heuristic can place a space
            # differently in two printings of the same name ("ZEPHYRM ANUFACTURING"
            # vs "ZEPHYRMANUFACTURING"), and both must reach the same ledger
            mapping[_key(party)] = (_squash(record.get("ledger", "")), treatment)
    return mapping


def build(rows, bank, company, bank_ledger, suspense, mapping, account_tail,
          date_from=None, date_to=None):
    vouchers, manifest, seen = [], [], set()
    for index, row in enumerate(rows, 1):
        date = bank.parse_date(f"{row[bank.date_column]}".strip())
        if (date_from and date < date_from) or (date_to and date > date_to):
            continue
        debit = _money(row[bank.debit_column])
        credit = _money(row[bank.credit_column])
        amount = debit or credit
        if amount is None:
            raise SystemExit(f"row {index} has no amount")
        outward = bool(debit)
        party = bank.party(row)
        ledger, treatment = mapping.get(_key(party), (suspense, "auto"))
        ledger = ledger or suspense
        if treatment == "skip":
            manifest.append({"row": index, "date": date.isoformat(), "voucher_type": "SKIPPED",
                             "amount": f"{amount:.2f}", "dr_ledger": "", "cr_ledger": "",
                             "suspense": "", "remoteid": "", "party": party,
                             "narration": "excluded: carried by the other account's contra"})
            continue
        kind = "Contra" if treatment == "contra" else ("Payment" if outward else "Receipt")
        mode, reference = bank.reference(row)
        shown = party if ledger == suspense else ledger
        narration = _squash(
            f"{mode} {reference} {'to' if outward else 'from'} {shown}"
            f" | {account_tail} | {date.strftime('%d-%b-%Y')}"
        )
        if ledger == suspense:
            narration += " | UNIDENTIFIED - reallocate from Suspense"
        remote_id = f"{account_tail.replace(' ', '')}-{date.strftime('%Y%m%d')}-{index:03d}"
        if remote_id in seen:
            raise SystemExit(f"duplicate REMOTEID {remote_id}")
        seen.add(remote_id)
        debit_ledger, credit_ledger = ((ledger, bank_ledger) if outward
                                       else (bank_ledger, ledger))
        vouchers.append(voucher_xml(
            kind, date, remote_id, narration, debit_ledger, credit_ledger, amount,
            party_ledger=None if kind == "Contra" else ledger,
        ))
        manifest.append({"row": index, "date": date.isoformat(), "voucher_type": kind,
                         "amount": f"{amount:.2f}", "dr_ledger": debit_ledger,
                         "cr_ledger": credit_ledger,
                         "suspense": "YES" if ledger == suspense else "",
                         "remoteid": remote_id, "party": party, "narration": narration})
    return vouchers, manifest


def selfcheck(xml_text, bank_ledger, manifest):
    """Re-read what we just wrote. Cheap, and it has caught real defects."""
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
                raise SystemExit("sign convention broken in generated XML")
            if entry.findtext("LEDGERNAME") == bank_ledger:
                if value < 0:
                    inward += -value
                else:
                    outward += value
        if total != 0:
            raise SystemExit(f"unbalanced voucher {voucher.get('REMOTEID')}")
        if not (voucher.findtext("NARRATION") or "").strip():
            raise SystemExit("empty narration in generated XML")
        if voucher.findtext("DATE") != voucher.findtext("EFFECTIVEDATE"):
            raise SystemExit("DATE and EFFECTIVEDATE disagree")
    posted = sum(1 for record in manifest if record["voucher_type"] != "SKIPPED")
    if count != posted:
        raise SystemExit(f"manifest says {posted} vouchers, XML has {count}")
    return count, outward, inward


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("--pdf", required=True)
    parser.add_argument("--password", required=True)
    parser.add_argument("--bank", required=True, choices=sorted(BANKS))
    parser.add_argument("--company", required=True,
                        help="EXACT company name as Tally shows it. A name matching no "
                             "company does not fail — Tally binds to whichever company "
                             "is loaded and imports there anyway.")
    parser.add_argument("--bank-ledger", required=True)
    parser.add_argument("--account-tail", default="",
                        help="short account label for narrations, e.g. 'SBI CA xx2129'")
    parser.add_argument("--opening", required=True, help="opening balance, to replay the statement")
    parser.add_argument("--mapping", help="CSV: party,ledger,treatment")
    parser.add_argument("--suspense", default="SUSPENSE ACC")
    parser.add_argument("--from", dest="date_from", help="YYYY-MM-DD")
    parser.add_argument("--to", dest="date_to", help="YYYY-MM-DD")
    parser.add_argument("--expect-debits")
    parser.add_argument("--expect-credits")
    parser.add_argument("--expect-closing")
    parser.add_argument("--out")
    parser.add_argument("--manifest")
    parser.add_argument("--dry-run", action="store_true",
                        help="list counterparties and totals; write nothing")
    args = parser.parse_args(argv)

    bank = BANKS[args.bank]()
    rows = parse(pathlib.Path(args.pdf), args.password, bank)
    closing = reconcile(rows, bank, args.opening)
    debits = sum(_money(r[bank.debit_column]) or D(0) for r in rows)
    credits = sum(_money(r[bank.credit_column]) or D(0) for r in rows)
    print(f"parsed {len(rows)} rows; running balance reproduced on every row")
    print(f"  debits {debits:,}  credits {credits:,}  closing {closing:,}")
    for label, actual, expected in (("debits", debits, args.expect_debits),
                                    ("credits", credits, args.expect_credits),
                                    ("closing", closing, args.expect_closing)):
        if expected is not None:
            if actual != D(expected):
                raise SystemExit(f"{label} {actual} does not match the statement's {expected}")
            print(f"  {label} match the statement's printed total")

    fmt = lambda text: datetime.date.fromisoformat(text) if text else None
    vouchers, manifest = build(
        rows, bank, args.company, args.bank_ledger, args.suspense,
        load_mapping(args.mapping), args.account_tail or args.bank_ledger,
        fmt(args.date_from), fmt(args.date_to),
    )
    if args.dry_run:
        counts = {}
        for record in manifest:
            key = (record["party"], record["voucher_type"], record["suspense"])
            counts[key] = counts.get(key, D(0)) + D(record["amount"])
        print(f"\n{'COUNTERPARTY':<34}{'TYPE':<10}{'TOTAL':>14}  SUSPENSE")
        for (party, kind, susp), total in sorted(counts.items(), key=lambda kv: -kv[1]):
            print(f"{party[:33]:<34}{kind:<10}{total:>14,}  {susp}")
        return 0

    xml_text = envelope(args.company, vouchers)
    count, outward, inward = selfcheck(xml_text, args.bank_ledger, manifest)
    skipped = [r for r in manifest if r["voucher_type"] == "SKIPPED"]
    print(f"\n{count} vouchers; {len(skipped)} skipped as already carried elsewhere")
    print(f"  bank ledger out {outward:,}  in {inward:,}")
    print(f"  suspense vouchers {sum(1 for r in manifest if r['suspense'])}")
    if args.out:
        pathlib.Path(args.out).write_text(xml_text, encoding="utf-8")
        print(f"  wrote {args.out}")
    if args.manifest:
        with open(args.manifest, "w", newline="", encoding="utf-8") as handle:
            writer = csv.DictWriter(handle, fieldnames=list(manifest[0]))
            writer.writeheader()
            writer.writerows(manifest)
        print(f"  wrote {args.manifest}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
