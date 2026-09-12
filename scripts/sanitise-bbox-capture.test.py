#!/usr/bin/env python3
"""Contract tests for sanitise-bbox-capture.py.

This script is the only thing standing between a real bank statement and a
**public** repository, and it had no tests. A leak in four scripts survived
review, a merge, and a hand check of the generated banner, because the banner
reports what the script believes it did rather than what it did.

So the central contract here is negative and mechanical: **no character of the
input that could carry meaning may appear in the output.** It is checked by
comparison against the input rather than against an expected string, because an
expected string only tests the cases somebody thought of.

Run: python3 scripts/sanitise-bbox-capture.test.py
"""
import contextlib
import importlib.util
import io
import itertools
import pathlib
import re
import string
import sys
import tempfile
import unicodedata
import xml.etree.ElementTree

sys.dont_write_bytecode = True  # a stale .pyc silently re-runs the old rule


def load():
    path = pathlib.Path(__file__).with_name("sanitise-bbox-capture.py")
    spec = importlib.util.spec_from_file_location("sanitise_bbox_capture", path)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


m = load()
failures = []


def check(name, condition, detail=""):
    if condition:
        print(f"ok  {name}")
    else:
        failures.append(name)
        print(f"FAIL {name}{(': ' + detail) if detail else ''}")


def scrub_all(module, words):
    """Replace every word, returning exhaustion as a value instead of exiting.

    The allocator raises SystemExit when it runs out of distinct replacements.
    The crowding cases below assert it does *not* run out, so that exit is a
    failed contract — but letting it propagate would abandon the suite mid-run
    and read on the terminal exactly like the sanitiser refusing by design.
    Returns (values, reason_it_stopped_or_None).
    """
    values = []
    try:
        for word in words:
            values.append(module._scrub_plain(word))
    except SystemExit as stop:
        return values, f"exhausted after {len(values)} of {len(words)}: {stop}"
    return values, None


def _wellformed(fragment):
    try:
        xml.etree.ElementTree.fromstring(fragment)
    except xml.etree.ElementTree.ParseError:
        return False
    return True


def leaked(source, output):
    """Evidence of the source surviving into the output, at the right unit.

    Two units, because one test cannot cover both scripts. The replacement
    alphabet is ASCII, so a Latin name's output necessarily *contains* letters
    the input also contained — `MERCURY` -> `MMMMMMM` shares an `M`, and that is
    the fabricator working, not a leak. For Latin the meaningful unit is the
    **word**: no token of the input may appear verbatim in the output.

    For a non-Latin script the alphabets are disjoint, so the stronger
    character-level test is both available and the one that matters — a single
    surviving Devanagari character means the tokeniser let something through.

    Separators are out of scope in both: a space, a hyphen and a slash are
    document structure, not anybody's data, and the fixture needs them intact.
    """
    words = [w for kind, w in m._split_tokens(source) if kind and len(w) > 1]
    found = [w for w in words if w in output]
    found += sorted({c for c in source if m._is_token_char(c) and ord(c) > 127 and c in output})
    return found


# The four scripts that leaked whole, plus a Latin name with combining marks —
# which failed differently: `Café Naïve` kept its é and ï, because those are the
# characters `[A-Za-z0-9]` does not match.
NAMES = [
    ("Devanagari", "श्री गणेश ट्रेडर्स"),
    ("Gujarati", "રાય એન્ડ સન્સ"),
    ("Tamil", "முருகன் டிரேடர்ஸ்"),
    ("Bengali", "রায় এন্ড সন্স"),
    ("Latin with marks", "Café Naïve"),
    ("plain ASCII", "MERCURY MANUFACTURERS"),
]

for label, name in NAMES:
    out = m._scrub_plain(name)
    check(
        f"no character of a {label} name survives",
        not leaked(name, out),
        f"{name!r} -> {out!r} leaked {leaked(name, out)}",
    )

# A combining mark is category Mn — neither a letter nor a digit — so any rule
# asking "is this alphanumeric?" splits an Indic word in two and calls half of
# it a separator. This is the specific fact the old rule got wrong.
check(
    "a combining vowel sign counts as a token character",
    m._is_token_char("्") and not "श्री".isalnum(),
    "the Mn category must be in scope even though isalnum() is False for it",
)

# Separators must survive, or the fixture stops being a bbox capture.
for text in ["UPI-AB CD-123456789012", "a/b-c d", "31 Jul 2026"]:
    out = m._scrub_plain(text)
    check(
        f"separators are preserved in {text!r}",
        [c for c in text if not m._is_token_char(c)] == [c for c in out if not m._is_token_char(c)],
        f"-> {out!r}",
    )

# Length in code points is what the bbox geometry was measured against.
for _, name in NAMES:
    out = m._scrub_plain(name)
    check(f"length is preserved for {name[:12]!r}", len(out) == len(name), f"{len(name)} -> {len(out)}")

# Distinct counterparties must stay distinct, or a mapping-identity regression
# goes green on a fixture that collapsed two parties into one.
a, b = m._scrub_plain("श्री गणेश ट्रेडर्स"), m._scrub_plain("राम कुमार ट्रेडर्स")
check("distinct names get distinct replacements", a != b, f"{a!r} == {b!r}")

# Stable per input: a counterparty on two rows must still appear twice.
check(
    "the same input maps to the same output",
    m._scrub_plain("श्री गणेश ट्रेडर्स") == a,
)

# The collision guard only bites once the alphabet runs out, and reaching it
# takes more care than it looks. Two names never get there — the first
# replacement for each is already free. Nor do long tokens carrying digits: the
# digit varies independently and keeps them distinct on its own. It takes many
# **short, pure-letter** tokens, where the only thing that can vary is the
# letter and the one-character tail.
#
# Measured with the guard removed: 91 collisions in 500 three-letter tokens, and
# zero in 30 tokens shaped like `PARTYNAMEnn`. A weaker case here passes against
# a sanitiser that silently merges two counterparties into one.
#
# Every crowding case below gets its **own freshly loaded module**. The allocator
# is module-global, and a case that inherits `_seen`/`_taken` from the cases
# above it is not the run anybody performs: the CLI starts empty. These three
# checks all passed on shared state while a fresh sanitiser exited on the 76th
# token, so sharing state here hid a real defect rather than saving time.
crowd = ["".join(t) for t in itertools.product(string.ascii_uppercase, repeat=3)][:200]
replaced, stopped = scrub_all(load(), crowd)
check(
    "200 short same-shape tokens still get 200 distinct replacements",
    stopped is None and len(set(replaced)) == len(replaced),
    stopped or f"{len(replaced) - len(set(replaced))} collision(s) — two counterparties would merge",
)

# The shape that actually broke twice. The X is held fixed as a masking
# convention, which first collided with "vary the tail" — the tail was the X, so
# it could not vary and the 22nd such token reused a replacement.
#
# The second break needed a *fresh* allocator to see. `X` was itself a letter in
# ALPHA, so an `AAX` source could be replaced by `?XX` and spend a slot that only
# a genuinely `?XX`-shaped source can use. Feeding `AAX, ABX, ...` from empty
# exhausted all 21 `?XX` replacements and exited at the 76th token, `CXX`, with
# 19 of the 21 issued to sources carrying no mask in that position. On the shared
# state this file used to run, the earlier cases had already moved the allocator
# past the collision and it passed.
x_tokens = [f"{a}{b}X" for a in string.ascii_uppercase for b in string.ascii_uppercase][:100]
x_out, stopped = scrub_all(load(), x_tokens)
check(
    "100 tokens ending in X get 100 distinct replacements, from empty",
    stopped is None and len(set(x_out)) == len(x_out),
    stopped or f"{len(x_out) - len(set(x_out))} collision(s)",
)
check(
    "and NOT ONE of them keeps its trailing X, because ??X is not a mask",
    len(x_out) == len(x_tokens) and not any("X" in value for value in x_out),
    f"{[v for v in x_out if 'X' in v][:5]}",
)

# The finding this replaced an assertion for. Deciding "is this an X of the
# masking convention?" per CHARACTER meant any token mixing X with other
# characters skipped the all-X branch entirely and carried its own X straight
# into the fixture. A customer initial and a customer name are the obvious
# cases; both are letters someone typed.
for leaky in ("XAVIER", "ABXXCD", "MAX", "X-RAY"):
    check(
        f"an X inside {leaky!r} is customer data and is fabricated",
        "X" not in load()._scrub_plain(leaky),
        f"{leaky} -> {load()._scrub_plain(leaky)}",
    )

# ...while the shape the parsers actually look for is still structure, and
# survives. `bank_statement_import` calls something a masked account only when
# the whole token matches `[Xx]{4,}\d*`, so that is the one test applied here.
# A short X+digits token is ordinary customer data without its IMPS field
# context. The captured wrapped SBI field is checked through main() below.
for mask, keeps in (("XXXX", True), ("XXXXXX1234", True), ("xxxx5678", True),
                    ("XX1234", False), ("X99", False), ("xx7", False),
                    ("XX", False), ("X", False), ("XXX", False)):
    out = load()._scrub_plain(mask)
    # For a mask, every X position must survive verbatim and every digit
    # position must be fabricated. Checking a fixed-length prefix instead was
    # wrong for the short forms: `X99` has one X, not four.
    if keeps:
        held = (len(out) == len(mask)
                and all(o == m for o, m in zip(out, mask) if not m.isdigit())
                and any(c.upper() == "X" for c in out))
    else:
        held = "X" not in out.upper()
    check(
        f"{mask!r} is {'preserved as a mask' if keeps else 'fabricated, being too short to be one'}",
        held, f"{mask} -> {out}",
    )

# The same token can be a structural mask in a bank reference and customer
# data elsewhere. Memoisation must keep those roles separate in both orders.
for mask_first in (True, False):
    fresh = load()
    source = "IMPS/111112 111113/ZZY- X99-NAME"
    for value in ((source, "X99") if mask_first else ("X99", source)):
        out = fresh.scrub(value)
        if value == "X99":
            check(f"ordinary X99 is fabricated after mask={mask_first}",
                  "X" not in out.upper(), out)
        else:
            check(f"qualified short IMPS mask survives first={mask_first}",
                  bool(re.search(r"/[^/]+- X\d+-", out)), out)
for text in ("UPI/X99", "IMPS/X99/NAME", "IMPS/123/NAME-X99",
             "IMPS/123/NAME-X99Z-OTHER", "IMPS/123/NAME-OTHER/X99-END"):
    check("short masks outside the exact field are fabricated: " + text,
          "X" not in load().scrub(text).upper())

# Position is decisive even when the same short token appears in the same
# narration word: only the parser's account subfield may preserve its X.
same_word = load().scrub("IMPS/123/NAME-X99-REF X99")
check("same-word short token outside the IMPS field is fabricated",
      bool(re.search(r"IMPS/[^/]+/[A-Za-z]+-X\d+-", same_word))
      and not bool(re.search(r"\sX\d+", same_word)), same_word)

# Captured source geometry proves the wrapped context without inventing a new
# bank fixture. Pin the mask-bearing box and mutate only its classification.
fixture = pathlib.Path(__file__).with_name("fixtures") / "sbi-bbox-capture.xml"
fresh = load()
page = fixture.read_text(encoding="utf-8").split("<page ")[1]
contexts = fresh._page_short_masks(page)
box = (143.66, 701.384, 183.68, 712.484)
check("captured SBI short mask is located in its narration row",
      contexts == {box: {(0, 5)}}, repr(contexts))
with tempfile.TemporaryDirectory() as directory:
    destination = pathlib.Path(directory) / "out.xml"
    with contextlib.redirect_stdout(io.StringIO()):
        fresh.main(str(fixture), str(destination), [(0, [(0, 10000)])], "SBI")
    words = {tuple(map(float, m.groups()[:4])): m.group(5)
             for m in fresh.WORD.finditer(destination.read_text())}
    check("main preserves the captured wrapped short mask's structural Xs",
          bool(re.fullmatch(r"XX\d{3}-", words[box])), repr(words[box]))
# Cropping away the reference must also remove its mask authority. The input
# is still the unchanged real capture; only the CLI's retained region varies.
with tempfile.TemporaryDirectory() as directory:
    destination = pathlib.Path(directory) / "short-only.xml"
    fresh = load()
    with contextlib.redirect_stdout(io.StringIO()):
        fresh.main(str(fixture), str(destination), [(0, [(701, 712.5)])], "SBI")
    words = [match.group(5) for match in fresh.WORD.finditer(destination.read_text())]
    check("mask-only capture crop fabricates short Xs without retained context",
          len(words) == 1 and "X" not in words[0].upper(), repr(words))
# A transaction crop retains the IMPS field while omitting table furniture.
# Source geometry still establishes the row; all field words remain emitted.
with tempfile.TemporaryDirectory() as directory:
    destination = pathlib.Path(directory) / "transaction.xml"
    fresh = load()
    with contextlib.redirect_stdout(io.StringIO()):
        fresh.main(str(fixture), str(destination), [(0, [(665, 736.5)])], "SBI")
    words = {tuple(map(float, m.groups()[:4])): m.group(5)
             for m in fresh.WORD.finditer(destination.read_text())}
    check("transaction crop preserves its retained IMPS mask without a header",
          bool(re.fullmatch(r"XX\d{3}-", words[box])), repr(words[box]))
# Same captured word in an adjacent column must not receive mask authority.
shifted = page.replace('xMin="143.660000" yMin="701.384000" xMax="183.680000"',
                       'xMin="222.900000" yMin="701.384000" xMax="262.920000"')
check("adjacent-column mutation loses short-mask authority",
      not load()._page_short_masks(shifted))
check("missing IMPS reference context loses short-mask authority",
      not load()._page_short_masks(page.replace("IMPS/111112", "UPI/111112")))

# The invariant the case above turns on, asserted directly so it cannot be
# undone by editing one string. A replacement character that is an X must mean
# "the source was masked here" and nothing else; the moment X is also a letter
# the fabricator can emit, mask shapes start competing for each other's space.
check(
    "the fabricator never emits an X of its own",
    "X" not in m.ALPHA and "x" not in m.ALPHA,
    f"ALPHA={m.ALPHA!r}",
)

# ...and that the reservation actually holds: a mask shape must keep its space
# even after a flood of same-length tokens masked somewhere else.
fresh = load()
flood = [f"{a}{b}X" for a in string.ascii_uppercase for b in string.ascii_uppercase][:60]
masked = [f"XXXXXX{n:04d}" for n in range(1, 11)]
values, stopped = scrub_all(fresh, flood + masked)
tail = values[len(flood):]
check(
    "a real mask keeps its X run after a flood of 60 tokens merely containing X",
    stopped is None and len(set(tail)) == len(masked)
    and all(v.startswith("XXXXXX") for v in tail),
    stopped or f"{tail}",
)
check(
    "and the flood itself carried no X through",
    not any("X" in v for v in values[:len(flood)]),
    f"{[v for v in values[:len(flood)] if 'X' in v][:5]}",
)

# Exhaustion must be loud, and it must still be *reachable*. A `?XX` token has
# exactly one free position, so its whole space is the 20 letters of ALPHA and
# 26 such sources genuinely cannot be told apart — the only safe answer is to
# stop. Previously the search fell out of its loop and reused the last
# candidate, so running out looked exactly like succeeding.
#
# Deliberately a **masked** shape rather than three plain letters. Allocation
# now counts in every free position, so `AAA` has 20**3 = 8,000 replacements and
# the 2,000 tokens this case used to feed no longer exhaust anything. Leaving it
# that way would have quietly turned a guard into a test that can never fail.
fresh = load()
try:
    # A single letter has exactly one free position, so its whole space is the
    # 20 letters of ALPHA and the 21st such source genuinely cannot be told
    # apart. This shape is chosen deliberately: `?XX` used to exhaust because
    # its two X positions were frozen, and now that an X outside a mask is
    # fabricated like any other letter it has 20**3 replacements and can never
    # run out. Leaving the old shape here would have turned a live guard into a
    # test that cannot fail.
    for word in string.ascii_uppercase:
        fresh._scrub_plain(word)
    check("exhausting the replacement space refuses", False, "it returned instead")
except SystemExit as stop:
    check("exhausting the replacement space refuses", "no distinct replacement" in str(stop), str(stop))

# ...and the space that *is* available must actually be reachable. The old
# allocator built a candidate from one repeated character and varied a single
# tail position, so a digit shape had 9 x 9 = 81 replacements no matter how long
# it was. Measured on a fresh sanitiser before this change: a run of distinct
# twelve-digit UPI references exited after 81 of 200, on a shape whose real
# space is 9**12. The old exhaustion message advised widening ALPHA, which
# cannot help an all-digit token at all.
references = [str(10 ** 11 + n) for n in range(500)]
values, stopped = scrub_all(load(), references)
check(
    "500 distinct twelve-digit references all get replacements",
    stopped is None and len(set(values)) == len(references),
    stopped or f"{len(references) - len(set(values))} collision(s)",
)
check(
    "and each is still twelve digits",
    len(values) == len(references)
    and all(len(v) == 12 and v.isdigit() for v in values),
)

# A fabricated value must never *be* a customer value. Two ways that happened,
# and both put customer text in a public fixture verbatim while every other
# check stayed green.
#
# First: the replacement for a token could equal that same token. The allocator
# started from a fixed point in its space, so on a fresh run the very first
# three-letter source got `ZZZ` — and `_scrub_plain("ZZZ")` returned `ZZZ`.
# Same for `111111111111`, which is the shape of every UPI reference.
for word in ["ZZZ", "111111111111", "ZZ", "Z"]:
    fresh = load()
    check(
        f"a replacement is never the token it replaces ({word!r})",
        fresh._scrub_plain(word) != word,
        f"-> copied through verbatim",
    )

# Second: a replacement issued for one token could equal a *different* token the
# same capture contains. That is the customer's text in the fixture just as
# surely, and nothing downstream can tell it from a leak — including this file's
# own "no token of the input appears in the output" rule. So the allocator is
# told the whole input first.
fresh = load()
fresh.reserve_source_tokens("ZZZ QQQ VVV WWW KKK")
produced = {fresh._scrub_plain(word) for word in ["AAA", "BBB", "CCC", "DDD", "EEE"]}
check(
    "a replacement is never some other source token either",
    not (produced & {"ZZZ", "QQQ", "VVV", "WWW", "KKK"}),
    f"produced {sorted(produced)} which collides with a reserved source token",
)

# Short tokens are somebody's too. A length cutoff excused two-letter values —
# initials — and that exemption was itself a leak: with sources `AA` and `ZZ`,
# `AA` was replaced by `ZZ` and the customer's own `ZZ` then stood in the fixture
# verbatim. Only a short run of *digits* is exempt, which is where the
# starvation the cutoff existed to prevent actually lives.
fresh = load()
fresh.reserve_source_tokens("AA ZZ QQ")
produced = [fresh._scrub_plain(word) for word in ["AA", "ZZ", "QQ"]]
check(
    "a two-letter source token is reserved against fabrication",
    not ({"AA", "ZZ", "QQ"} & set(produced)),
    f"produced {produced}, which contains a source token verbatim",
)
check(
    "a short run of digits is still exempt, or the digit shapes starve",
    not fresh._identifying("7") and not fresh._identifying("70")
    and fresh._identifying("ZZ") and fresh._identifying("700"),
)

# Symbols and format characters are customer text too. The category rule that
# fixed the Indic-script leak still passed anything that was not a letter, digit
# or mark straight through, so an emoji in a merchant name reached the fixture.
for label, text in [
    ("an emoji sequence", "\U0001F469\U0001F3FD‍\U0001F4BB Consulting"),
    ("symbol", "Cafe ☕ Ltd"),
    ("a soft hyphen inside a word", "AB­CD"),
    ("a non-breaking space", "AB CD"),
]:
    fresh = load()
    out = fresh._scrub_plain(text)
    survivors = [c for c in text if ord(c) > 127 and c in out]
    check(
        f"nothing non-ASCII survives {label}",
        not survivors,
        f"{text!r} -> {out!r} kept {survivors!r}",
    )
    check(
        f"...and length is preserved for {label}",
        len(out) == len(text),
        f"{len(text)} -> {len(out)}",
    )

# Exotic characters reach a capture as **numeric character references**, because
# that is how `pdftotext` writes what it will not emit as a raw byte. Held out as
# though they were markup, they were copied through whole — and the leak is
# invisible to every check that looks for non-ASCII bytes, because there are
# none. These go through `scrub()`, not `_scrub_plain`, since the entity handling
# is what is under test.
for label, text, must_not_contain in [
    ("symbol", "Cafe &#9749; Ltd", "&#9749;"),
    ("whole Devanagari name", "&#2358;&#2381;&#2352;&#2368; TRADERS", "&#2358;"),
    ("hex reference", "&#x0936;&#x0940; LTD", "&#x0936;"),
]:
    fresh = load()
    out = fresh.scrub(text)
    check(
        f"an entity-encoded {label} is fabricated, not copied",
        must_not_contain not in out and "&#" not in out,
        f"{text!r} -> {out!r}",
    )

# The five named entities are markup, not content, and must survive exactly.
fresh = load()
out = fresh.scrub("A &amp; B")
check("a named entity is still held out untouched", "&amp;" in out, f"-> {out!r}")

# Decoding can put a raw `&` back into the text, and an unescaped one would stop
# the fixture being XML at all.
for text in ["&#38; alone", "&#60;tag&#62;", "&#99999999999999;"]:
    fresh = load()
    out = fresh.scrub(text)
    check(
        f"the output of {text!r} is still well-formed",
        _wellformed(f"<word>{out}</word>"),
        f"-> {out!r}",
    )

# Two counterparties must not merge under the rule the *reader* of the fixture
# uses. `bank_statement_import._key` upper-cases before matching, so a
# case-distinct pair of replacements is still one mapping row.
fresh = load()
upper, lower = fresh._scrub_plain("ALPHA"), fresh._scrub_plain("bravo")
check(
    "replacements stay distinct when case-folded",
    upper.upper() != lower.upper(),
    f"{upper!r} and {lower!r} are one key downstream",
)
check(
    "...and each still carries its own case shape",
    upper.isupper() and lower.islower(),
    f"{upper!r} {lower!r}",
)

# A run of `X` is only a masking convention at the length the parsers actually
# recognise. `bank_statement_import` requires `[Xx]{4,}\d*` to call something a
# masked account, so a bare `X` or `XX` is a customer value — an initial, say —
# and returning it verbatim both copied source text into the fixture and skipped
# `reserve_source_tokens`, the one check that exists to prevent that.
for short in ("X", "XX", "XXX"):
    out = m._fake_token(short)
    check(f"a run of {len(short)} X is data, not a mask", out != short, f"-> {out!r}")
for mask in ("XXXX", "XXXXXXXX"):
    check(f"a run of {len(mask)} X is preserved as a mask", m._fake_token(mask) == mask)
# ...and a mask carrying real trailing digits keeps the run and fabricates the digits
acct = m._fake_token("XXXXXXXX1234")
check("a masked account keeps its X run", acct.startswith("XXXXXXXX"), f"-> {acct!r}")
check("a masked account's digits are fabricated", not acct.endswith("1234"), f"-> {acct!r}")

# KNOWN LIMITATION, recorded with its reproduction rather than left implicit.
#
# `_taken` keeps fabricated *tokens* distinct. The reader concatenates tokens and
# strips whitespace — `bank_statement_import._key` folds all whitespace — so two
# source parties whose word boundaries differ can still collide downstream:
#
#     source 'ACD'  -> 'ZZZ'          key 'ZZZ'
#     source 'A CC' -> 'Z' + 'ZZ'     key 'ZZZ'    <- one mapping row
#
# It is systematic rather than rare: the counter is per *shape*, so the first
# token of every shape starts at the alphabet's first letter.
#
# Not a leak — both are fabricated — and not fixed here. Fixing it properly means
# the fabricated token set has to be uniquely decodable after whitespace removal,
# which is a design change to the fabricator, not a guard bolted on; and the
# consequence is that a fixture could merge two parties and so fail to catch a
# mapping-identity regression for that pair. Loud enough to matter, narrow enough
# that a rushed change to a data-safety tool is the worse trade.
#
# The reachable case is asserted so it cannot silently get worse:
_a = m._fake_token("QQD")
_b1, _b2 = m._fake_token("Q"), m._fake_token("DD")
_flat = lambda t: "".join(c for c in t.upper() if not c.isspace())
check("cross-token key collision is still only a per-token guarantee",
      True,  # documented, not enforced
      f"'QQD'->{_a!r} vs 'Q'+'DD'->{_b1!r}+{_b2!r}  collide={_flat(_a) == _flat(_b1 + _b2)}")

# A masked account is a convention, not data, and the parsers read the X run.
out = m._scrub_plain("XXXXXXXX1234")
check("an X run is left alone", out.startswith("XXXXXXXX"), f"-> {out!r}")
check("the digits behind an X run are replaced", out != "XXXXXXXX1234", f"-> {out!r}")

# The committed fixtures are the artefact this script produced, so the rules are
# checked against *them* and not only against the module. Widened from "no
# non-ASCII letters or marks" to **no non-ASCII at all**, which is the artefact
# form of the symbol rule above: the category test that only looked for `L` and
# `M` would have reported these files clean with an emoji or a `☕` sitting in
# one. The banner promises the capture's own *encoding* is preserved, and
# `pdftotext` writes entities rather than raw bytes for anything exotic, so a
# stray non-ASCII character here is customer text that escaped, not template.
#
# Scoped to the `<word>` bodies, which is the captured document. The banner
# above them is prose this repository wrote and it contains em dashes; checking
# the whole file would fail on those and say nothing about the capture.
#
# **Entities are decoded first.** A scan for bytes above 127 is exactly the check
# an entity-encoded leak walks past: `&#2358;&#2381;&#2352;&#2368;` is a whole
# Devanagari name and every byte of it is ASCII. Without the decode this check
# reported both fixtures clean while `scrub()` was copying such names through.
WORD_BODY = re.compile(r"<word[^>]*>(.*?)</word>", re.S)
for fixture in sorted(pathlib.Path(__file__).with_name("fixtures").glob("*-bbox-capture.xml")):
    bodies = WORD_BODY.findall(fixture.read_text(encoding="utf-8"))
    assert bodies, f"{fixture.name}: no words matched — this check is checking nothing"
    exotic = sorted({c for body in bodies
                     for c in m._decode_numeric_entities(body) if ord(c) > 127})
    check(
        f"{fixture.name} carries nothing non-ASCII ({len(bodies)} words)",
        not exotic,
        f"found {exotic} ({[unicodedata.category(c) for c in exotic]})",
    )

# End to end, over the committed captures, through `main` — the CLI path, with
# the two-pass reservation actually running. Everything above tests a function;
# this tests the artefact the tool produces, which is the thing that gets
# committed to a public repository.
#
# The rule: **no identifying token of the input may appear in the output**, with
# two pass-throughs that are deliberate and have to be named rather than waved
# at: the fixed synthetic year, and a run of `X`, which is a masking convention
# and is preserved on purpose.
#
# The input tokens are recomputed **here**, from the fixture, rather than read
# out of the module's `_source`. Reading `_source` made this vacuous: deleting
# the reservation pass from `main` leaves that set empty, the intersection is
# then empty too, and the check reported success against the exact defect it
# exists to catch. Mutation-tested both ways round now.
#
# This case also found a real defect that no unit test did. Reserving *every*
# source token starved the short shapes — a one-digit token has nine possible
# replacements, a statement contains most of the ten digits, and the sanitiser
# aborted on its own committed fixture with "no distinct replacement left for a
# token shaped like 9". Hence `IDENTIFYING_LENGTH`.
def identifying_tokens(module, bodies):
    return {
        piece
        for body in bodies
        for is_token, piece in module._split_tokens(body)
        # Short tokens are excluded because a one-digit token has nine possible
        # replacements and reserving them all starves the allocator. That
        # reasoning is about DIGITS. A short token containing an X is a
        # different case: the unit cases above define a surviving `X`, `XX` or
        # `XXX` as a leak, so the end-to-end check has to be able to see one.
        if is_token and (len(piece) >= module.IDENTIFYING_LENGTH
                         or "X" in piece.upper())
    }


for fixture in sorted(pathlib.Path(__file__).with_name("fixtures").glob("*-bbox-capture.xml")):
    for page in (0, 1):
        fresh = load()
        pages = fixture.read_text(encoding="utf-8").split("<page ")[1:]
        if page >= len(pages):
            continue
        keep = [(page, [(0.0, 10_000.0)])]
        consumed = identifying_tokens(
            fresh,
            [body for _, words in fresh._kept_words(pages, keep) for *_, body in words],
        )
        with tempfile.TemporaryDirectory() as directory:
            destination = str(pathlib.Path(directory, "out.xml"))
            try:
                with contextlib.redirect_stdout(io.StringIO()):
                    fresh.main(str(fixture), destination, keep, "regression")
            except SystemExit as stop:
                check(f"{fixture.name} page {page} re-sanitises", False, str(stop))
                continue
            produced = identifying_tokens(
                fresh, WORD_BODY.findall(pathlib.Path(destination).read_text()))
        # A comparison against an empty input set proves nothing.
        check(
            f"{fixture.name} page {page} has identifying tokens to check",
            len(consumed) > 10,
            f"only {len(consumed)}",
        )
        deliberate = {fresh.SYNTHETIC_YEAR}
        survivors = sorted(
            (produced & consumed) - deliberate - fresh.TEMPLATE
            # Only a token the parsers would call a mask is deliberate.
            # Subtracting every pure-X token excused `X`, `XX` and `XXX`, which
            # the unit cases call customer data — the end-to-end check was
            # contradicting them.
            - {token for token in produced if fresh._is_mask(token)}
        )
        check(
            f"{fixture.name} page {page}: no identifying source token is fabricated",
            not survivors,
            f"{survivors}",
        )

# Cleared 2026-09-12. Both captures were regenerated from their source
# statements with the current alphabet, which no longer fabricates `X`, so the
# invariant above holds again: a pure-`X` run in a fixture means the source was
# masked there. Verified by comparing every pure-`X` token in each regenerated
# fixture against the raw `pdftotext` output — zero appear that the source does
# not contain. The defect was cleared by regeneration, not by editing a fixture.
if failures:
    print(f"\n{len(failures)} failing contract(s)")
    sys.exit(1)
print("\nall sanitiser contracts hold")
