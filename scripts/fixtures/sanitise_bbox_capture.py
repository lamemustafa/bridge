"""Turn a real pdftotext -bbox-layout capture into a fixture that keeps the
bank's template and geometry and none of the customer.

Allowlist-only: a token survives verbatim only if it is vocabulary the bank
prints on every statement regardless of who the customer is. Everything else is
substituted, so a value that was never anticipated is fabricated by default
rather than kept by default.
"""
import re, sys, pathlib

TEMPLATE = set("""
Page No .: Account Branch Address City State Phone no. OD Limit Currency Email
Cust ID A/C Open Date Status Regular RTGS/NEFT IFSC MICR Code Type Nomination
Registered Statement of account From To Narration Chq./Ref.No. Value Dt
Withdrawal Amt. Deposit Closing Balance HDFC BANK LIMITED Bank STATEMENT SUMMARY
INR : - , . JOINT HOLDERS Opening Dr Cr Count Generated On: By: Requesting
This is a computer generated statement and does not require signature.
Contents this will be considered correct if no error reported within days
balance includes funds earmarked for hold uncleared Office Address: House
GSTIN number details are available at
IMPS ACH UPI NEFT TPT CR DR D TP EMI ACC INT TRF ATM WDL
""".split())
# transaction-mode tags the parsers anchor on; they are template, not customer
TEMPLATE |= {"CAGEN", "NET", "BIZ", "LITE", "PLUS", "ACCOUNT", "WITH", "A/C", "RTGS",
             "Chq", "Ref", "Amt", "On", "By", "days", "signature", "require",
             "computer", "statement.", "30"}
# SBI's own vocabulary
TEMPLATE |= set("""
Txn Description No./Cheque Cheque Debit Credit Name Number Drawing Power Interest
Rate(% p.a.) MOD CIF No. IFS Balance as on from to Please do share your ATM,
Debit/Credit card number, PIN and OTP with anyone over mail SMS, phone call or
any other media. Bank never asks for such information. **This a signature.
TRANSFER TO FROM INB IMPS NEFT RTGS ATM WDL CASH INT TRF BY
Jan Feb Mar Apr May Jun Jul Aug Sep Oct Nov Dec India
""".split())

SMALL = re.compile(r"^\d{1,2}$|^(?:19|20)\d{2}$")

DATE = re.compile(r"^(\d{2})/(\d{2})/(\d{2}(?:\d{2})?)$")
_dates = {}


def _fake_date(token):
    """Dates are remapped, not digit-substituted.

    A digit substitution produces 11/22/33, which is not a calendar date, and
    the parser would reject the fixture for the wrong reason. Distinct source
    dates map to consecutive days of one synthetic month in first-appearance
    order, so ordering and equality survive and the real dates do not.
    """
    if token not in _dates:
        day = len(_dates) + 1
        tail = "2026" if len(token) == 10 else "26"
        _dates[token] = f"{day:02d}/08/{tail}"
    return _dates[token]

SEP = re.compile(r"([^A-Za-z0-9]+)")
ALPHA = "ZQXVWKJYBGFHLMNPRSTDC"
_seen = {}


def _fake_token(token):
    """A fabricated token of the same length AND the same character shape.

    Shape matters as much as length. The parsers decide where a counterparty
    name ends by recognising the *shape* of the field after it — a masked
    account is a run of X, a UTR is letters followed by digits, a UPI reference
    is twelve digits. Substituting a mixed token into pure letters destroys
    those markers and the fixture stops exercising the boundary logic, which is
    most of what there is to test.

    So: digits map to digits, letters to letters, and a run of X is left alone
    because it is a masking convention rather than anybody's data.

    Stable per distinct input, so a counterparty appearing on two rows still
    appears twice — the repeat structure is what mapping and suspense logic
    reads. Keyed on first-appearance order rather than on the characters, so
    this is not a cipher over the original text.
    """
    if token not in _seen:
        index = len(_seen)
        letter = ALPHA[index % len(ALPHA)]
        digit = str((index % 9) + 1)
        _seen[token] = "".join(
            character if character == "X"
            else digit if character.isdigit()
            else letter.lower() if character.islower()
            else letter
            for character in token)
    return _seen[token]


def scrub(text):
    if DATE.match(text):
        return _fake_date(text)
    if SMALL.match(text):
        # a day-of-month or a four-digit year: SBI stacks "31" over "Jul" over
        # "2026" in one cell, so digit-substituting these produces a date the
        # parser rightly rejects. Neither identifies anybody.
        return text
    out = []
    for piece in SEP.split(text):
        if not piece:
            continue
        if piece in TEMPLATE or not piece.strip():
            out.append(piece)
        elif piece.isalnum():
            out.append(_fake_token(piece))
        else:
            out.append(piece)
    return "".join(out)


def main(src, dst, keep, banner):
    """keep: [(page_index, [(y_min, y_max), ...]), ...] regions to retain."""
    pages = pathlib.Path(src).read_text().split("<page ")[1:]
    chunks = []
    for page_index, spans in keep:
        page = pages[page_index]
        head = page[:page.index(">") + 1]
        kept = []
        for match in re.finditer(
                r'<word xMin="([\d.]+)" yMin="([\d.]+)" xMax="([\d.]+)" yMax="([\d.]+)">(.*?)</word>',
                page, re.S):
            x0, y0, x1, y1, body = match.groups()
            if not any(low <= float(y0) <= high for low, high in spans):
                continue
            kept.append(f'<word xMin="{x0}" yMin="{y0}" xMax="{x1}" yMax="{y1}">'
                        f'{scrub(body)}</word>')
        chunks.append("<page " + head + "\n" + "\n".join(kept) + "\n</page>")
    pathlib.Path(dst).write_text(banner + "\n".join(chunks) + "\n", encoding="utf-8")
    print(f"wrote {dst}: {sum(c.count('<word') for c in chunks)} words, {len(chunks)} pages")


BANNER_TEMPLATE = """<!--
  pdftotext -bbox-layout output from a real {bank} statement, with every
  customer value replaced and the geometry untouched.

  What is real: the page size, every xMin/yMin/xMax/yMax, the word and line
  breaks, the entity encoding, and the bank's own template vocabulary (column
  headers, field labels, transaction-mode tags, the page footer). That is the
  part a template change or a pdftotext change would break, and the part a
  hand-written fixture cannot honestly reproduce.

  What is fabricated: every other token. Sanitisation is allowlist-only, so a
  token survives verbatim only if it is vocabulary the bank prints for every
  customer; anything unanticipated is fabricated by default rather than kept by
  default. Distinct source tokens map to distinct fabricated ones in
  first-appearance order, so repeats and name/reference structure survive while
  the substitution is not a cipher over the original text. Character shape is
  preserved — digits stay digits, a run of X stays a run of X — because the
  parsers find the end of a counterparty name by recognising the shape of the
  field after it.

  Dates are remapped to consecutive days of one synthetic month rather than
  digit-substituted, since a digit substitution produces 11/22/33, which is not
  a calendar date. Days of the month and four-digit years survive as printed;
  neither identifies anybody.

  Amounts are fabricated and therefore do NOT form a balance chain. This
  fixture proves parsing, which is what a layout regression breaks; reconcile()
  is proved separately against row fixtures.
-->
"""


def main(source, destination, keep, bank):
    """keep: [(page_index, [(y_min, y_max), ...]), ...] regions to retain."""
    pages = pathlib.Path(source).read_text().split("<page ")[1:]
    chunks = []
    for page_index, spans in keep:
        page = pages[page_index]
        head = page[:page.index(">") + 1]
        kept = []
        for match in re.finditer(
                r'<word xMin="([\d.]+)" yMin="([\d.]+)" xMax="([\d.]+)" yMax="([\d.]+)">(.*?)</word>',
                page, re.S):
            x0, y0, x1, y1, body = match.groups()
            if not any(low <= float(y0) <= high for low, high in spans):
                continue
            kept.append(f'<word xMin="{x0}" yMin="{y0}" xMax="{x1}" yMax="{y1}">'
                        f'{scrub(body)}</word>')
        chunks.append("<page " + head + "\n" + "\n".join(kept) + "\n</page>")
    pathlib.Path(destination).write_text(
        BANNER_TEMPLATE.format(bank=bank) + "\n".join(chunks) + "\n", encoding="utf-8")
    print(f"wrote {destination}: "
          f"{sum(chunk.count('<word') for chunk in chunks)} words, {len(chunks)} pages")


USAGE = """usage: sanitise_bbox_capture.py SOURCE DEST BANK PAGE:Y0-Y1[,Y0-Y1] ...

  SOURCE  pdftotext -bbox-layout output from a real statement
  DEST    fixture to write
  BANK    a description for the banner, e.g. "HDFC current-account"
  PAGE:.. 0-based page index and the y ranges to keep from it

Always diff the result against the source before committing it, and scan the
output for surviving tokens:

  python3 -c "import re,sys;\
    t=lambda p:{x for m in re.finditer(r'<word[^>]*>(.*?)</word>',open(p).read(),re.S)\
                  for x in re.split(r'[^A-Za-z0-9]+',m.group(1)) if len(x)>=3};\
    print(sorted(t(sys.argv[1]) & t(sys.argv[2])))" SOURCE DEST

Everything it prints should be bank vocabulary. If a customer value is in that
list, add nothing to TEMPLATE — work out why it matched and fix the rule.
"""


if __name__ == "__main__":
    if len(sys.argv) < 5:
        raise SystemExit(USAGE)
    regions = []
    for argument in sys.argv[4:]:
        page, _, spans = argument.partition(":")
        regions.append((int(page), [tuple(float(v) for v in span.split("-"))
                                    for span in spans.split(",")]))
    main(sys.argv[1], sys.argv[2], regions, sys.argv[3])
