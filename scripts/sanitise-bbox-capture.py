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

DAY = re.compile(r"^\d{1,2}$")
YEAR = re.compile(r"^(?:19|20)\d{2}$")
SYNTHETIC_YEAR = "2026"
_days = {}

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
ENTITY = re.compile(r"&(?:amp|lt|gt|quot|apos|#\d+|#x[0-9A-Fa-f]+);")
_seen = {}
_taken = set()


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
    if token in _seen:
        return _seen[token]
    # Distinct inputs must get distinct outputs, or the fixture collapses two
    # counterparties into one and a parser regression involving name boundaries
    # or mapping identity stays green. A single repeated character runs out
    # after `len(ALPHA)` tokens of the same shape, so widen the replacement
    # until it is unused.
    for attempt in range(len(_seen), len(_seen) + 10_000):
        letter = ALPHA[attempt % len(ALPHA)]
        digit = str((attempt % 9) + 1)
        suffix = attempt // len(ALPHA)
        candidate = "".join(
            character if character == "X"
            else digit if character.isdigit()
            else letter.lower() if character.islower()
            else letter
            for character in token)
        if suffix and len(candidate) > 1:
            # Vary the tail so a second pass over the alphabet cannot repeat a
            # replacement already issued for a token of this shape — but vary it
            # *within its own character class*. Shape is the whole point: a
            # digit run that gains a trailing letter stops being a reference,
            # and the boundary parsers this fixture exists to exercise read
            # exactly that distinction.
            last = candidate[-1]
            if last == "X":
                replacement = last
            elif last.isdigit():
                replacement = str((suffix % 9) + 1)
            elif last.islower():
                replacement = ALPHA[suffix % len(ALPHA)].lower()
            else:
                replacement = ALPHA[suffix % len(ALPHA)]
            candidate = candidate[:-1] + replacement
        if candidate not in _taken:
            break
    _seen[token] = candidate
    _taken.add(candidate)
    return candidate


def scrub(text):
    """Sanitise one word's text, preserving XML entity syntax.

    `&amp;` is one character in the document and four in the file. Splitting on
    non-alphanumerics treats `amp` as customer text and rewrites the word to
    something like `Z&qqq;X` — no longer valid bbox XML, and no longer the
    parsing behaviour the real bytes exercise. Entities are held out, the text
    around them is scrubbed, and they go back exactly as they were.
    """
    parts = ENTITY.split(text)
    if len(parts) > 1:
        entities = ENTITY.findall(text)
        out = [_scrub_plain(parts[0])]
        for entity, rest in zip(entities, parts[1:]):
            out.append(entity)
            out.append(_scrub_plain(rest))
        return "".join(out)
    return _scrub_plain(text)


def _scrub_plain(text):
    if DATE.match(text):
        return _fake_date(text)
    if YEAR.match(text):
        # one fixed synthetic year. SBI stacks "31" over "Jul" over "2026" in
        # one cell, so a digit substitution here produces a date the parser
        # rightly rejects.
        return SYNTHETIC_YEAR
    if DAY.match(text):
        # A one- or two-digit number is *usually* a day-of-month in a split
        # date cell, and occasionally part of an address. Both are replaced;
        # only the shape is kept, remapped into 01..28 so it stays a valid day
        # in any month. Preserving these verbatim — the previous behaviour —
        # left the real transaction dates in the fixture and made the banner's
        # claim false.
        if text not in _days:
            _days[text] = f"{len(_days) % 28 + 1:02d}"
        return _days[text]
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

  Dates are remapped rather than digit-substituted, since a digit substitution
  produces 11/22/33, which is not a calendar date. A whole date becomes a
  consecutive day of one synthetic month. A date split across words — SBI
  stacks "31" over "Jul" over "2026" in one cell — is remapped piecewise: the
  month name is template vocabulary and stays, the day is remapped into 01..28
  so it remains valid in any month, and every four-digit year becomes the same
  fixed synthetic year — fixed, so it is the same for every capture and says
  nothing about which statement this one came from.

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


USAGE = """usage: sanitise-bbox-capture.py SOURCE DEST BANK PAGE:Y0-Y1[,Y0-Y1] ...

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
