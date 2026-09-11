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
import importlib.util
import itertools
import pathlib
import string
import sys
import unicodedata

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
crowd = ["".join(t) for t in itertools.product(string.ascii_uppercase, repeat=3)][:200]
replaced = [m._scrub_plain(word) for word in crowd]
check(
    "200 short same-shape tokens still get 200 distinct replacements",
    len(set(replaced)) == len(replaced),
    f"{len(replaced) - len(set(replaced))} collision(s) — two counterparties would merge",
)

# The shape that actually broke: a token ending in X. The X is held fixed as a
# masking convention, which used to collide with "vary the tail" — the tail was
# the X, so it could not vary, and the only freedom left was the 21 letters of
# ALPHA. The 22nd such token silently reused a replacement.
x_tokens = [f"{a}{b}X" for a in string.ascii_uppercase for b in string.ascii_uppercase][:100]
x_out = [m._scrub_plain(word) for word in x_tokens]
check(
    "100 tokens ending in X get 100 distinct replacements",
    len(set(x_out)) == len(x_out),
    f"{len(x_out) - len(set(x_out))} collision(s)",
)
check(
    "and the trailing X is still preserved in every one",
    all(value.endswith("X") for value in x_out),
)

# Exhaustion must be loud. A three-character token has 21 letters x 21 tails of
# room, so 2,000 of them genuinely cannot be told apart — and the only safe
# answer is to stop. Previously the search fell out of its loop and reused the
# last candidate, so running out looked exactly like succeeding.
many = ["".join(t) for t in itertools.product(string.ascii_uppercase, repeat=3)][:2000]
try:
    for word in many:
        m._scrub_plain(word)
    check("exhausting the replacement space refuses", False, "it returned instead")
except SystemExit as stop:
    check("exhausting the replacement space refuses", "no distinct replacement" in str(stop), str(stop))

# A masked account is a convention, not data, and the parsers read the X run.
out = m._scrub_plain("XXXXXXXX1234")
check("an X run is left alone", out.startswith("XXXXXXXX"), f"-> {out!r}")
check("the digits behind an X run are replaced", out != "XXXXXXXX1234", f"-> {out!r}")

# The committed fixtures are the artefact this script produced. If any real
# non-ASCII word is sitting in one, it got there through the hole above.
for fixture in sorted(pathlib.Path(__file__).with_name("fixtures").glob("*-bbox-capture.xml")):
    text = fixture.read_text(encoding="utf-8")
    exotic = sorted({c for c in text if ord(c) > 127 and unicodedata.category(c)[0] in "LM"})
    check(
        f"{fixture.name} carries no non-ASCII letters or marks",
        not exotic,
        f"found {exotic}",
    )

if failures:
    print(f"\n{len(failures)} failing contract(s)")
    sys.exit(1)
print("\nall sanitiser contracts hold")
