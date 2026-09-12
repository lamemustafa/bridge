"""Turn a real pdftotext -bbox-layout capture into a fixture that keeps the
bank's template and geometry and none of the customer.

Allowlist-only: a token survives verbatim only if it is vocabulary the bank
prints on every statement regardless of who the customer is. Everything else is
substituted, so a value that was never anticipated is fabricated by default
rather than kept by default.
"""
import decimal
import importlib.util
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

def _is_token_char(character):
    """Only ASCII punctuation and ASCII whitespace separate. Everything else is
    customer text until the TEMPLATE allowlist says otherwise.

    Stated as "what may pass through", not "what is a letter", because this is
    the one predicate standing between a real statement and a **public**
    repository and the rest of the file is allowlist-only. A rule shaped as
    "these categories are data" leaks by default: whatever it forgets is copied
    verbatim. This shape refuses by default, and what it forgets is fabricated.

    It has been wrong twice, in the same direction both times:

      * `[^A-Za-z0-9]+` put every character of a Devanagari, Gujarati, Tamil or
        Bengali name in the separator class, so four scripts reached
        `_scrub_plain`'s pass-through branch whole, and `Café Naïve` kept its
        `é` and `ï`.
      * `unicodedata.category(c)[0] in "LNM"` fixed those and still leaked
        anything that is not a letter, digit or mark. Measured: the merchant
        name `👩🏽‍💻 Consulting` kept its emoji (`So`), its skin-tone modifier
        (`Sk`) and its zero-width joiner (`Cf`); `Café ☕ Ltd` kept the `☕`; and
        a soft hyphen survived inside a word.

    So: an ASCII character that is not alphanumeric is structure — the spaces,
    hyphens and slashes the fixture needs intact. Everything else, including
    every non-ASCII symbol, format character and space, is fabricated. A rupee
    sign or an em dash is fabricated too, which is the intended trade: this file
    keeps what it recognises and invents the rest.
    """
    return not (ord(character) < 128 and not character.isalnum())


def _split_tokens(text):
    """Alternating runs of token characters and separators, token runs first-ish.

    Returns runs rather than a regex split so the rule above is the definition
    rather than a character class that has to be kept in step with it.
    """
    runs = []
    for character in text:
        kind = _is_token_char(character)
        if runs and runs[-1][0] == kind:
            runs[-1][1].append(character)
        else:
            runs.append((kind, [character]))
    return [(kind, "".join(run)) for kind, run in runs]


# No X. X is the masking convention — it is held fixed wherever the *source*
# had one — so it must never also be a letter the fabricator can emit. While it
# was one, a token shaped `AAX` could be replaced by `?XX` and eat a slot the
# genuinely `?XX`-shaped tokens need, and the output space of one mask shape was
# no longer reserved for it. Measured on a fresh sanitiser: feeding `AAX, ABX,
# ...` exhausted all 21 `?XX` replacements at the 76th token and exited, because
# 19 of them had been issued to sources with no mask in that position at all.
# Excluding X makes an X in a replacement mean exactly one thing — the source
# was masked there — so the shapes no longer compete.
# The shortest run of `X` that `bank_statement_import` will treat as a masked
# account (`[Xx]{4,}\d*`). Below this a run of `X` is data, not a convention.
MASK_MIN_XS = 4


# `bank_statement_import` recognises the short mask `[Xx]+\d+` ONLY inside an
# `IMPS/` component, behind an alphabetic prefix and hyphens
# (`^[A-Za-z]+-\s*[Xx]+\d+-`). Outside that, a short run of X with digits is not
# a masking convention to any parser here — it is a customer token that happens
# to start with the letter X.
def _is_mask(token):
    """True when `token` is a masked account: `[Xx]{4,}` optionally then digits.

    **Only the unambiguous form.** `bank_statement_import` also reads a short
    `[Xx]+\\d+` inside an `IMPS/` component, and mirroring that here cost four
    revisions — per character, per token, per word containing `IMPS/`, per
    position within the word — each one leaking a customer `X` into a public
    fixture in a narrower place than the last, because a context-free tokeniser
    cannot reliably mirror a context-sensitive rule.

    Measured before dropping it: the short form preserves **one** token across
    both committed fixtures, and the importer's own IMPS tests use constructed
    eight-X masks rather than that token. So the whole feature bought one
    masked-account shape in one fixture and produced four rounds of findings.

    A sanitiser may be narrower than the parser — the cost is a fabricated mask
    shape — but never wider, because the cost there is a customer character
    preserved verbatim. Given a doubt about scope, this is the narrow answer and
    it needs no context at all to be checked.
    """
    return bool(re.fullmatch(rf"[Xx]{{{MASK_MIN_XS},}}\d*", token))

ALPHA = "ZQVWKJYBGFHLMNPRSTDC"
# Markup escapes: syntax, held out and restored untouched.
STRUCTURAL_ENTITY = re.compile(r"&(?:amp|lt|gt|quot|apos);")
# Character references: content, decoded and then fabricated like any other text.
NUMERIC_ENTITY = re.compile(r"&#(?:(\d+)|[xX]([0-9A-Fa-f]+));")
# Decoding a reference can put a raw `&`, `<` or `>` back into the text — `&#38;`
# is an ampersand — and writing one out unescaped produces a file that is no
# longer XML. Separators are emitted through this.
XML_ESCAPES = {"&": "&amp;", "<": "&lt;", ">": "&gt;"}
_seen = {}
_taken = set()
# Every token the capture itself contains, collected before any allocation.
_source = set()
# Next index to try for each shape, so allocation is O(1) per token rather than
# a rescan of everything already issued.
_next = {}
# Digits 1-9, not 0-9: a leading zero changes what a reference looks like, and
# several parsers strip them.
DIGITS = "123456789"


# Below this length a *numeric* token carries no identity, and reserving one is
# actively harmful: a one-digit token has nine possible replacements in total, a
# statement contains most of the ten digits somewhere, and reserving all of them
# left the shape with nothing to allocate. Measured — reserving every token made
# the sanitiser abort on its own committed HDFC fixture with "no distinct
# replacement left for a token shaped like 9".
#
# Three is also the threshold the documented leak scan in USAGE uses.
IDENTIFYING_LENGTH = 3


def _identifying(token):
    """Whether this token is worth reserving against fabrication.

    Length alone was the wrong rule, and the exemption it granted was a leak.
    Excusing everything under three characters excused **short letter tokens**,
    which are somebody's initials: with sources `AA` and `ZZ`, `AA` was replaced
    by `ZZ` and the customer's own `ZZ` then stood in the fixture verbatim. The
    starvation the cutoff existed to prevent was never about letters — a
    two-letter shape has 400 replacements — it was about digits, where the
    alphabet is nine wide and the source contains most of it.

    So the exemption is narrowed to what actually starves: a short run of
    digits, which is a lone `7` in the middle of a reference and nobody's data.
    Anything containing a letter is reserved at any length.
    """
    return not (token.isdigit() and len(token) < IDENTIFYING_LENGTH)


def reserve_source_tokens(text):
    """Record the tokens the source contains, before anything is fabricated.

    A fabricated token that happens to equal a *different* customer token puts
    that customer's text in the fixture verbatim. Nothing downstream can tell
    that apart from a leak — including this file's own contract test, whose rule
    is "no token of the input may appear in the output" — so the allocator has
    to know the whole input before it issues the first replacement. That is why
    `main` makes a pass over the kept words before it scrubs any of them.
    """
    for is_token, piece in _split_tokens(_decode_numeric_entities(text)):
        if is_token and _identifying(piece):
            _source.add(piece.upper())


def _shape_of(token):
    """The classes this token's replacement must reproduce, as a key."""
    return "".join(
        "X" if character == "X"
        else "9" if character.isdigit()
        else "a" if character.islower()
        else "A"
        for character in token)


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

    **Every non-ASCII character is replaced with an ASCII letter**, one per code
    point, so length and "this is a word" survive but the script does not. That
    is a deliberate trade: no parser in this repository branches on script, and
    fabricating in-script would mean generating valid Devanagari or Tamil, which
    is a lot of machinery to preserve a property nothing reads. Worth knowing if
    you capture a statement whose names are not in Latin script — the fixture
    will exercise your boundary logic but will not look like the original.

    Stable per distinct input, so a counterparty appearing on two rows still
    appears twice — the repeat structure is what mapping and suspense logic
    reads. Keyed on first-appearance order rather than on the characters, so
    this is not a cipher over the original text.

    **Allocation counts in every free position**, not in one.

    The previous version built a candidate out of one repeated character and
    then varied a single tail position, which made the space `len(ALPHA)` wide
    by the tail and nothing else. For letters that was 400 three-letter
    replacements; for **digits it was 81, whatever the length** — so 82 distinct
    twelve-digit UPI references exhausted it and the run aborted, on a shape
    whose real space is 9**12. Measured before this change: exit after 81 of 200.
    Widening ALPHA, which the old exhaustion message suggested, could not have
    helped an all-digit token at all.

    Counting across the free positions makes the space the product of the
    per-position alphabets, which is the whole of what shape-preservation
    allows.
    """
    if token in _seen:
        return _seen[token]
    # An `X` is only a masking convention when the WHOLE token is the shape the
    # parsers actually look for. `bank_statement_import` requires
    # `[Xx]{4,}\d*` to call something a masked account, so a bare `X` or `XX`
    # is not a mask — it is a customer value that happens to be the letter X,
    # an initial for instance. Returning those verbatim copied source text into
    # the fixture and bypassed `reserve_source_tokens` entirely, which is the
    # one check that exists to stop exactly that.
    #
    # Deciding that per CHARACTER rather than per token leaked the same way by
    # a narrower door: in `XAVIER` or `ABXXCD` the non-X characters make the
    # free-position list nonempty, so the all-X branch never runs, and every
    # `X` survives into the fixture as `XZZZZZ` or `ZZXXZZ`. Those `X`s are
    # customer letters. Classify the token against the parser's own pattern
    # first, and only then treat `X` as structure; everywhere else an `X` is
    # data like any other letter.
    if _is_mask(token):
        positions = [index for index, character in enumerate(token) if character.isdigit()]
        if not positions:
            return token
    else:
        positions = list(range(len(token)))

    alphabets = [
        DIGITS if token[index].isdigit()
        else ALPHA.lower() if token[index].islower()
        else ALPHA
        for index in positions
    ]
    total = 1
    for alphabet in alphabets:
        total *= len(alphabet)

    shape = _shape_of(token)
    # Fixed mask positions do not share the ordinary token allocation space.
    allocation_key = (shape, tuple(positions))
    index = _next.get(allocation_key, 0)
    candidate = None
    while index < total:
        digits, built = index, list(token)
        for position, alphabet in zip(reversed(positions), reversed(alphabets)):
            built[position] = alphabet[digits % len(alphabet)]
            digits //= len(alphabet)
        index += 1
        built = "".join(built)
        # Distinctness is judged **case-folded**, because that is how the reader
        # of these fixtures judges it. `bank_statement_import._key` upper-cases a
        # counterparty name before matching, so `ZZZZZ` and `zzzzz` are one
        # mapping row — and with a counter per exact shape, the first all-upper
        # and the first all-lower token of a length both landed on the alphabet's
        # first letter. `ALPHA` and `bravo` became `ZZZZZ` and `zzzzz`, two
        # counterparties that collapse downstream while `_taken` called them
        # distinct. Folding here makes this file agree with the only definition
        # of "the same party" that matters.
        # Three ways a candidate is unusable, and they are different failures:
        # equal to this token, or to any other token the capture contains, means
        # customer text would sit in the fixture verbatim; already taken means
        # two counterparties would merge.
        folded = built.upper()
        if built != token and folded not in _source and folded not in _taken:
            candidate = built
            break
    if candidate is None:
        # Previously the search fell out of its loop and used the last candidate
        # anyway, so exhaustion looked exactly like success and two
        # counterparties quietly became one. A sanitiser that cannot keep them
        # apart must say so: the alternative is a fixture that is wrong in a way
        # no later check can detect.
        raise SystemExit(
            f"sanitise: no distinct replacement left for a token shaped like "
            f"{shape} — all {total} of them are already issued, equal to a token "
            f"the capture contains, or equal to the token being replaced. The "
            f"fixture would merge two distinct values into one, or copy one "
            f"through. Shorten the capture, or widen this shape's alphabet "
            f"(ALPHA for letters, DIGITS for digits)."
        )
    _next[allocation_key] = index
    _seen[token] = candidate
    _taken.add(candidate.upper())
    return candidate


def _decode_numeric_entities(text):
    """`&#2358;` is a customer's letter wearing an ASCII costume.

    A numeric character reference **is content** — it is how `pdftotext` writes
    a character it will not emit as a raw byte, which is precisely the exotic
    ones. Holding it out as though it were syntax copied it through: measured,
    `scrub('&#2358;&#2381;&#2352;&#2368; TRADERS')` returned the whole
    Devanagari name unchanged. Worse, that leak is **invisible to a scan for
    non-ASCII characters**, because every byte in the output really is ASCII,
    so the artefact check over the committed fixtures reported them clean.

    Decoding first puts the real character into the token stream, where the
    ordinary rules fabricate it. One reference becomes one replacement
    character, which is also what the bbox geometry was measured against.
    """
    def one(match):
        digits, hexits = match.groups()
        try:
            return chr(int(digits or hexits, 10 if digits else 16))
        except (ValueError, OverflowError):
            return match.group(0)  # not a character; leave it for the eye
    return NUMERIC_ENTITY.sub(one, text)


def scrub(text):
    """Sanitise one word's text, preserving XML *syntax*.

    `&amp;` is one character in the document and four in the file. Splitting on
    non-alphanumerics treats `amp` as customer text and rewrites the word to
    something like `Z&qqq;X` — no longer valid bbox XML, and no longer the
    parsing behaviour the real bytes exercise. The five named entities are held
    out, the text around them is scrubbed, and they go back exactly as they
    were.

    Only those five. They are markup escapes and carry no information about the
    customer. A numeric reference is the opposite — see above.
    """
    text = _decode_numeric_entities(text)
    parts = STRUCTURAL_ENTITY.split(text)
    if len(parts) > 1:
        entities = STRUCTURAL_ENTITY.findall(text)
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
    # The short mask shape is only a convention inside an IMPS component, so the
    # decision needs the surrounding field, which the token alone cannot carry.
    out = []
    for is_token, piece in _split_tokens(text):
        if piece in TEMPLATE:
            out.append(piece)
        elif is_token:
            out.append(_fake_token(piece))
        else:
            # ASCII punctuation and whitespace only. Nothing reaches this branch
            # that could be a name, which is the whole change — previously an
            # Indic name arrived here and was passed through.
            #
            # Re-escaped, because decoding a numeric reference can have put a
            # raw `&`, `<` or `>` here and the output has to stay XML.
            out.append("".join(XML_ESCAPES.get(character, character)
                               for character in piece))
    return "".join(out)


BANNER_TEMPLATE = """<!--
  pdftotext -bbox-layout output from a real {bank} statement, with every
  customer value replaced and the geometry untouched.

  What is real: the page size, every xMin/yMin/xMax/yMax, the word and line
  breaks, the five named XML entities, and the bank's own template vocabulary
  (column headers, field labels, transaction-mode tags, the page footer). That
  is the part a template change or a pdftotext change would break, and the part
  a hand-written fixture cannot honestly reproduce.

  Numeric character references are NOT preserved. `&#2358;` is a customer's
  letter written in ASCII, not markup — holding it out as syntax copied whole
  names through, invisibly to any check that scans for non-ASCII bytes. Each
  reference is decoded and then fabricated like the character it is, so one
  reference becomes one replacement character and the geometry still holds.

  What is fabricated: every other token. Sanitisation is allowlist-only, so a
  token survives verbatim only if it is vocabulary the bank prints for every
  customer; anything unanticipated is fabricated by default rather than kept by
  default. Distinct source tokens map to distinct fabricated ones in
  first-appearance order, so repeats and name/reference structure survive while
  the substitution is not a cipher over the original text. Character shape is
  preserved — digits stay digits and whole-token masks with at least four Xs
  retain those Xs — because the
  parsers find the end of a counterparty name by recognising the shape of the
  field after it. Shorter X-plus-digit forms are fabricated even inside IMPS.
  A regenerated capture therefore does not preserve that contextual mask shape;
  the existing captured short-mask parser evidence must be retained separately.

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


WORD = re.compile(
    r'<word xMin="([\d.]+)" yMin="([\d.]+)" xMax="([\d.]+)" yMax="([\d.]+)">(.*?)</word>',
    re.S)


def _kept_words(pages, keep):
    """Every word inside a retained region, page by page."""
    for page_index, spans in keep:
        page = pages[page_index]
        head = page[:page.index(">") + 1]
        words = [match.groups() for match in WORD.finditer(page)
                 if any(low <= float(match.group(2)) <= high for low, high in spans)]
        yield head, words


def _load_parser(bank_name):
    """Load one of the parsers used to qualify a generated fixture."""
    if bank_name not in ("hdfc", "sbi"):
        raise SystemExit("sanitise: BANK must be one of: hdfc, sbi")
    path = pathlib.Path(__file__).with_name("bank_statement_import.py")
    spec = importlib.util.spec_from_file_location("sanitise_bank_parser", path)
    if spec is None or spec.loader is None:
        raise SystemExit("sanitise: cannot load the selected bank parser")
    parser = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(parser)
    return parser, parser.BANKS[bank_name]()


def _assert_party_partition(source_keys, output_keys, bank_name):
    """Require a two-way, one-to-one mapping of party equivalence classes."""
    if not source_keys or len(source_keys) != len(output_keys):
        raise SystemExit(f"sanitise: {bank_name} party evidence is empty or misaligned")
    source_to_output, output_to_source = {}, {}
    for index, (source, output) in enumerate(zip(source_keys, output_keys)):
        if not source or not output or source in ("UNRESOLVED", "UNNAMED") or output in ("UNRESOLVED", "UNNAMED"):
            raise SystemExit(f"sanitise: {bank_name} party evidence is underdetermined at row {index}")
        old = source_to_output.setdefault(source, output)
        reverse = output_to_source.setdefault(output, source)
        if old != output:
            raise SystemExit(f"sanitise: {bank_name} party partition split at row {index}")
        if reverse != source:
            raise SystemExit(f"sanitise: {bank_name} party partition merged at row {index}")


def _validate_parser_evidence(parser, bank, source_pages, output_pages, bank_name):
    """Parse complete page sets and compare only structure preserved by scrubbing."""
    try:
        source_rows = parser.parse_pages(source_pages, bank)
        output_rows = parser.parse_pages(output_pages, bank)
    except (KeyError, IndexError, TypeError, ValueError, decimal.InvalidOperation) as error:
        raise SystemExit(f"sanitise: {bank_name} parser evidence is invalid: {type(error).__name__}") from error
    if not source_rows or not output_rows or len(source_rows) != len(output_rows):
        raise SystemExit(f"sanitise: {bank_name} parser evidence is empty or misaligned")

    source_dates, output_dates = [], []
    source_keys, output_keys = [], []
    for index, (source, output) in enumerate(zip(source_rows, output_rows)):
        try:
            source_date = str(source[bank.date_column]).strip()
            output_date = str(output[bank.date_column]).strip()
            bank.parse_date(source_date)
            bank.parse_date(output_date)
            source_dates.append(source_date)
            output_dates.append(output_date)
            for row in (source, output):
                for column in (bank.debit_column, bank.credit_column, bank.balance_column):
                    value = str(row.get(column) or "").strip()
                    if value:
                        parser.D(value)
            source_ref = bank.reference(source)
            output_ref = bank.reference(output)
            source_shape = (bool(source.get(bank.debit_column)), bool(source.get(bank.credit_column)),
                            bool(source.get(bank.balance_column)), source_ref[0], len(str(source_ref[1])))
            output_shape = (bool(output.get(bank.debit_column)), bool(output.get(bank.credit_column)),
                            bool(output.get(bank.balance_column)), output_ref[0], len(str(output_ref[1])))
            if source_shape != output_shape:
                raise SystemExit(f"sanitise: {bank_name} amount/reference alignment failed at row {index}")
            source_keys.append(parser._key(bank.party(source)))
            output_keys.append(parser._key(bank.party(output)))
        except SystemExit:
            raise
        except (KeyError, IndexError, TypeError, ValueError, decimal.InvalidOperation) as error:
            raise SystemExit(f"sanitise: {bank_name} row alignment failed at row {index}: {type(error).__name__}") from error

    _assert_party_partition(source_dates, output_dates, bank_name)
    _assert_party_partition(source_keys, output_keys, bank_name)


def main(source, destination, keep, bank):
    """keep: [(page_index, [(y_min, y_max), ...]), ...] regions to retain."""
    # `pdftotext` emits UTF-8. `read_text()` without an encoding decodes with
    # the host's locale, so on a Windows Python whose locale is not UTF-8 a raw
    # `Café` becomes mojibake with extra code points and Indic bytes raise
    # `UnicodeDecodeError` before sanitisation runs at all. Neither CI nor the
    # unit cases reach this boundary: CI is ubuntu-only, and the Unicode tests
    # call `_scrub_plain` with strings that are already decoded.
    keep = list(keep)
    pages = pathlib.Path(source).read_text(encoding="utf-8").split("<page ")[1:]
    parser, bank_profile = _load_parser(bank)
    regions = list(_kept_words(pages, keep))
    # Two passes, and the first one has to be complete before the second starts.
    # A replacement is only safe once the allocator knows every token the
    # capture contains: otherwise a fabricated value can equal some *other*
    # customer token, which puts that customer's text in the fixture verbatim
    # and is indistinguishable from a leak.
    for _, words in regions:
        for *_, body in words:
            reserve_source_tokens(body)
    def render(transform):
        return [
        "<page " + head + "\n"
        + "\n".join(f'<word xMin="{x0}" yMin="{y0}" xMax="{x1}" yMax="{y1}">'
                    f'{transform(body)}</word>' for x0, y0, x1, y1, body in words)
        + "\n</page>"
        for head, words in regions
        ]
    source_chunks = render(lambda body: body)
    chunks = render(scrub)
    output = BANNER_TEMPLATE.format(bank=bank_profile.name) + "\n".join(chunks) + "\n"
    _validate_parser_evidence(parser, bank_profile, [chunk[len("<page "):] for chunk in source_chunks],
                              output.split("<page ")[1:], bank_profile.name)
    pathlib.Path(destination).write_text(output, encoding="utf-8")
    print(f"wrote {destination}: "
          f"{sum(chunk.count('<word') for chunk in chunks)} words, {len(chunks)} pages")


USAGE = """usage: sanitise-bbox-capture.py SOURCE DEST BANK PAGE:Y0-Y1[,Y0-Y1] ...

  SOURCE  pdftotext -bbox-layout output from a real statement
  DEST    fixture to write
  BANK    parser profile: hdfc or sbi (closed selection)
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
