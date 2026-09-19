#!/usr/bin/env python3
"""Pure privacy classification for ``merge-gate.sh``.

The program accepts raw, already-complete gate input on stdin and writes only a
small JSON decision record.  It deliberately reports counts and categories,
never matched values.  UUIDs are recognized before generic hexadecimal digest
masking: a nil UUID is an explicit sentinel, while every other UUID needs
exact, current-head fixture provenance.
"""
from __future__ import print_function

import argparse
import json
import re
import sys
import unicodedata


NIL_UUID = "00000000-0000-0000-0000-000000000000"
UUID_RE = re.compile(
    r"(?<![0-9A-Fa-f])[0-9A-Fa-f]{8}-[0-9A-Fa-f]{4}-"
    r"[0-9A-Fa-f]{4}-[0-9A-Fa-f]{4}-[0-9A-Fa-f]{12}(?![0-9A-Fa-f])"
)
UUID_OR_DIGEST_RE = re.compile(
    r"[0-9A-Fa-f]{8}-[0-9A-Fa-f]{4}-[0-9A-Fa-f]{4}-[0-9A-Fa-f]{4}-"
    r"[0-9A-Fa-f]{12}|[0-9A-Fa-f]{32,}"
)
PLACEHOLDER_RE = re.compile(r"^(X+|Z+)\d+(X|Z)?$|^\d{2}(X+|Z+)\d+[0-9A-Z]*$", re.I)
# Boundaries include ordinary words plus snake/kebab field names and the first
# capital of camelCase.  A UUID cannot evade the credential rule by changing
# only its presentation (for example, ``credential_session_id`` or
# ``bearerToken``).
CREDENTIAL_CONTEXT_RE = re.compile(
    r"(?:\b(?:credential|session|token|bearer)\b|"
    r"(?:credential|session|token|bearer)(?:[_-]|(?=[A-Z])))",
    re.I,
)
EMAIL_RE = re.compile(r"(?<![A-Za-z0-9._%+-])[A-Za-z0-9.!#$%&'*+/=?^_`{|}~-]+@([A-Za-z0-9](?:[A-Za-z0-9-]{0,61}[A-Za-z0-9])?(?:\.[A-Za-z0-9](?:[A-Za-z0-9-]{0,61}[A-Za-z0-9])?)+)(?![A-Za-z0-9._%+-])")
EXAMPLE_EMAIL_DOMAINS = {"example.com", "example.org", "example.net", "example.invalid"}
# This is a single known public automated-contributor identity, not an
# email-domain allowlist. It is eligible only in the exact Git attribution
# trailer parsed below; PR bodies, source files, names, and all other
# commit-message text are still privacy-scanned normally. Keep the components
# separate so this privacy-gate change has no raw address shape in its own diff.
PUBLIC_AGENT_ATTRIBUTION_LOCAL_PART = "noreply"
PUBLIC_AGENT_ATTRIBUTION_DOMAIN = "anthropic.com"
PUBLIC_AGENT_ATTRIBUTION_ADDRESS = (
    PUBLIC_AGENT_ATTRIBUTION_LOCAL_PART + "@" + PUBLIC_AGENT_ATTRIBUTION_DOMAIN
)
GIT_TRAILER_RE = re.compile(r"^[A-Za-z][A-Za-z0-9-]*:[ \t]+[^\r\n]+$")
PUBLIC_AGENT_COAUTHOR_TRAILER_RE = re.compile(
    r"^(?i:Co-Authored-By):[ \t]+(?P<name>\S(?:.*\S)?)[ \t]+<"
    + re.escape(PUBLIC_AGENT_ATTRIBUTION_ADDRESS) + r">$"
)
CREDENTIAL_KEY_RE = r"(?:api[_-]?key|access[_-]?token|refresh[_-]?token|client[_-]?secret|credential|session[_-]?token|password|passphrase|private[_-]?key)"
CREDENTIAL_ASSIGNMENT_RE = re.compile(
    r"(?i)(?P<prefix>(?<![A-Za-z0-9_-])(?P<key_quote>['\"])?" + CREDENTIAL_KEY_RE +
    r"(?(key_quote)(?P=key_quote))(?![A-Za-z0-9_-])\s*(?:=|:)\s*)"
)
AUTHORIZATION_BEARER_RE = re.compile(
    r"(?i)(?P<prefix>\bauthorization\s*:\s*bearer(?:\s+)?)(?P<value>.*)$"
)
QUOTED_VALUE_RE = re.compile(r"^(['\"])((?:\\.|(?!\1).)*)\1(?:\s*[,;].*|\s*[}\]])?$")
PLACEHOLDER_VALUE_RE = re.compile(
    r"^(?:\*{3,}|(?:redacted|masked|placeholder|example|sample|null|none|n/?a)|"
    r"(?:your|replace(?:_me)?|example|sample)[_-](?:api[_-]?key|access[_-]?token|refresh[_-]?token|client[_-]?secret|credential|session[_-]?token))$",
    re.I,
)
SUBSTITUTION_VALUE_RE = re.compile(
    r"^(?:\$[A-Za-z_][A-Za-z0-9_]*|\$\{[^{}\s]+\}|\{\{[^{}\n]+\}\}|"
    r"<(?:redacted|masked|token|secret|credential)>|\[(?:redacted|masked)\]|"
    r"(?:process\.env\.[A-Za-z_][A-Za-z0-9_]*|os\.environ(?:\.get)?\([^\n]+\)|env\([^\n]+\)))$",
    re.I,
)
# Only established type spellings may be lower-case.  Keeping the custom-type
# branch case-sensitive prevents ordinary values such as ``productionToken``
# and ``supersecret`` from being mistaken for annotations.
TYPE_REFERENCE_RE = re.compile(r"^(?:str|string|bytes|secret(?:str)?|token|Optional\[[A-Za-z]+\]|[A-Z][A-Za-z0-9_]*(?:Token|Secret))$")
HOME_RE = re.compile(
    r"(^|[^\w])(?:/Users/[^/\s]+|/home/[^/\s]+|/root|[A-Za-z]:[\\/]{1,2}Users[\\/]{1,2}[^\\/\s]+)"
    r"($|/|\\|[^\w.-])",
    re.I,
)
PEM_RE = re.compile(r"-----BEGIN\s+(?:(?:X509|TRUSTED)\s+)?CERTIFICATE-----", re.I)
IDENTIFIER_RE = re.compile(
    r"\d{2}[A-Z]{5}\d{4}[A-Z][0-9A-Z]{3}|[A-Z]{5}[ -]\d{4}[ -][A-Z]|"
    r"[A-Z]{5}\d{4}[A-Z]|[6-9]\d{9}",
    re.I,
)
LONG_RUN_RE = re.compile(r"\d{11,18}")
PHONE_RE = re.compile(r"(^|[^\w])[6-9](?:[ ()+._-]{0,3}\d){9}($|[^\w])")
LANDLINE_RE = re.compile(r"(^|[^\w])0[1-9]\d[ ._-]\d{4}[ ._-]\d{4}($|[^\w])")
STANDARD_LANDLINE_RE = re.compile(
    r"(^|[^\w])0(?:[1-9]\d[ -]\d{8}|[1-9]\d{2}[ -]\d{7})($|[^\w])"
)
GROUPED_NUMBER_RE = re.compile(r"(^|[^\w])\d{4}[ ._-]\d{4}[ ._-]\d{4}(?:[ ._-]\d{4})?($|[^\w])")
DATE_RANGE_RE = re.compile(r"^(?:0[1-9]|1[0-2])(?:0[1-9]|[12]\d|3[01])-20\d{2}\s+(?:0[1-9]|1[0-2])(?:0[1-9]|[12]\d|3[01])-20\d{2}$")


def result_record():
    return {"blockers": [], "indeterminate": [], "notes": []}


def add(record, bucket, message):
    record[bucket].append(message)


def load_fixture_provenance(path, head, record):
    """Return exact UUIDs authorized by a current-head fixture manifest.

    The manifest is deliberately narrow: a list of objects with only an exact
    UUID, the current full head, and ``source: fixture`` qualifies.  The gate
    currently supplies an empty manifest; this interface exists so a later,
    server-bound fixture inventory can authorize values without any text or
    path-name heuristic.
    """
    if not path:
        return set()
    try:
        with open(path, "r") as source:
            values = json.load(source)
    except (IOError, ValueError, TypeError):
        add(record, "indeterminate", "could not read exact current-head fixture UUID provenance")
        return set()
    if not isinstance(values, list):
        add(record, "indeterminate", "fixture UUID provenance was malformed")
        return set()
    allowed = set()
    for value in values:
        if not isinstance(value, dict):
            add(record, "indeterminate", "fixture UUID provenance was malformed")
            return set()
        uuid = value.get("uuid")
        if not isinstance(uuid, str) or not UUID_RE.fullmatch(uuid):
            add(record, "indeterminate", "fixture UUID provenance was malformed")
            return set()
        if value.get("head") == head and value.get("source") == "fixture":
            allowed.add(uuid.lower())
    return allowed


def tokenize_uuids(text, allowed, record):
    """Classify UUIDs before masking generic hexadecimal shapes."""
    retained = []
    unknown = 0
    credential = 0
    for line in text.splitlines(True):
        cursor = 0
        pieces = []
        for match in UUID_RE.finditer(line):
            pieces.append(line[cursor:match.start()])
            value = match.group(0).lower()
            if value == NIL_UUID:
                pieces.append("<uuid>")
            elif CREDENTIAL_CONTEXT_RE.search(line):
                credential += 1
                pieces.append("<blocked-uuid>")
            elif value in allowed:
                pieces.append("<uuid>")
            else:
                unknown += 1
                pieces.append("<unknown-uuid>")
            cursor = match.end()
        pieces.append(line[cursor:])
        retained.append("".join(pieces))
    if credential:
        add(record, "blockers", "privacy scan found %d non-nil UUID shape(s) in credential, session, token, or bearer context" % credential)
    if unknown:
        add(record, "indeterminate", "privacy scan found %d non-nil UUID shape(s) without exact current-head fixture provenance" % unknown)
    return "".join(retained)


def credential_value_status(value):
    """Classify an explicitly assigned credential value without returning it."""
    value = value.strip()
    if not value:
        return "indeterminate"
    quoted = QUOTED_VALUE_RE.match(value)
    if value[0:1] in ("'", '"') and not quoted:
        return "indeterminate"
    if quoted:
        value = quoted.group(2)
    else:
        value = re.split(r"[\s,;#]", value, 1)[0]
    if not value:
        return "indeterminate"
    if PLACEHOLDER_VALUE_RE.fullmatch(value) or SUBSTITUTION_VALUE_RE.fullmatch(value) or TYPE_REFERENCE_RE.fullmatch(value):
        return "placeholder"
    return "blocker"


def credential_value_span(text, start):
    """Return one assignment value's bounds without consuming later fields."""
    if start >= len(text):
        return start, start
    if text[start] in ("'", '"'):
        quote = text[start]
        cursor = start + 1
        while cursor < len(text):
            if text[cursor] == "\\":
                cursor += 2
            elif text[cursor] == quote:
                return start, cursor + 1
            else:
                cursor += 1
        return start, len(text)
    cursor = start
    while cursor < len(text) and text[cursor] not in "\t\r\n ,;#":
        cursor += 1
    return start, cursor


def tokenize_credential_literals(text, record):
    """Block assigned secret material before UUID/digest masking can hide it."""
    blocked = 0
    malformed = 0
    retained = []
    cursor = 0
    while cursor < len(text):
        bearer = AUTHORIZATION_BEARER_RE.search(text, cursor)
        assignment = CREDENTIAL_ASSIGNMENT_RE.search(text, cursor)
        match = min((candidate for candidate in (bearer, assignment) if candidate), key=lambda candidate: candidate.start(), default=None)
        if not match:
            retained.append(text[cursor:])
            break
        value_start = match.end("prefix")
        value_start, value_end = credential_value_span(text, value_start)
        status = credential_value_status(text[value_start:value_end])
        retained.append(text[cursor:value_start])
        if status == "placeholder":
            retained.append(text[value_start:value_end])
        else:
            retained.append("<credential-value>")
            if status == "blocker":
                blocked += 1
            else:
                malformed += 1
        cursor = value_end
    if blocked:
        add(record, "blockers", "privacy scan found %d literal credential, bearer, or API token value(s)" % blocked)
    if malformed:
        add(record, "indeterminate", "privacy scan found %d malformed or truncated credential assignment(s)" % malformed)
    return "".join(retained)


def customer_email_count(text):
    count = 0
    for match in EMAIL_RE.finditer(text):
        if match.group(1).lower() not in EXAMPLE_EMAIL_DOMAINS:
            count += 1
    return count


def redact_public_agent_attribution_trailer(message):
    """Redact only the known public-agent address from a terminal Git trailer.

    A trailer-shaped line elsewhere in the commit message is ordinary scanned
    content.  The entire footer must be well-formed trailers and the exact
    address must occupy the angle-bracket email field with no trailing payload.
    The contributor name remains in the returned text for the normal scanner.
    """
    if not isinstance(message, str):
        return message
    without_trailing_newlines = message.rstrip("\n")
    trailing_newlines = message[len(without_trailing_newlines):]
    footer_boundary = without_trailing_newlines.rfind("\n\n")
    if footer_boundary < 0:
        return message
    prefix = without_trailing_newlines[:footer_boundary + 2]
    footer = without_trailing_newlines[footer_boundary + 2:]
    lines = footer.split("\n")
    if not lines or not all(GIT_TRAILER_RE.fullmatch(line) for line in lines):
        return message
    redacted = []
    for line in lines:
        match = PUBLIC_AGENT_COAUTHOR_TRAILER_RE.fullmatch(line)
        if match:
            start = line.rfind("<") + 1
            end = line.rfind(">")
            redacted.append(line[:start] + "public-agent-attribution" + line[end:])
        else:
            redacted.append(line)
    return prefix + "\n".join(redacted) + trailing_newlines


def redact_public_agent_attribution_messages(messages):
    """Apply the trailer-only treatment to the isolated commit-message class."""
    if not isinstance(messages, list) or not all(isinstance(message, str) for message in messages):
        raise ValueError("commit messages must be a JSON array of strings")
    return [redact_public_agent_attribution_trailer(message) for message in messages]


def nonplaceholder_count(pattern, text):
    count = 0
    for match in pattern.finditer(text):
        value = match.group(0).upper()
        compact = value.replace(" ", "").replace("-", "")
        if not PLACEHOLDER_RE.fullmatch(compact):
            count += 1
    return count


def normalize_whitespace(text):
    return "".join(" " if char == "\t" or unicodedata.category(char) == "Zs" else char for char in text)


def grouped_numbers(text):
    values = []
    for match in GROUPED_NUMBER_RE.finditer(text):
        # group(0) includes the single boundary character captured by each of
        # the regex's leading/trailing `(^|[^\w])` anchors (0 or 1 char each,
        # e.g. a wrapping paren or a trailing comma/period). Strip exactly
        # those captured characters -- not just whitespace -- before testing
        # the compact-date exclusion, or a parenthesized/punctuated date range
        # such as "(0101-2026 0201-2026)" never matches DATE_RANGE_RE and is
        # misreported as an unexplained long digit run.
        whole = match.group(0)
        lead = len(match.group(1))
        trail = len(match.group(2))
        value = whole[lead:len(whole) - trail]
        if not DATE_RANGE_RE.fullmatch(value):
            values.append(value)
    return "\n".join(values)


def redact_shapes(text):
    """Mask the classifier's own identifier/digest/email shapes in-place.

    Used to sanitize a bounded diagnostic example (for example, a filename
    echoed into a merge-gate coverage message) before it leaves the gate, so
    the example itself cannot carry a home-directory path, identifier, long
    digit run, digest/UUID, or email shape into gate output.
    """
    text = HOME_RE.sub(lambda m: m.group(1) + "<home>" + m.group(2), text)
    text = UUID_OR_DIGEST_RE.sub("<digest>", text)
    text = IDENTIFIER_RE.sub("<identifier>", text)
    text = LONG_RUN_RE.sub("<digits>", text)
    text = EMAIL_RE.sub("<email>", text)
    return text


def scan(text, head, fixture_provenance=None):
    record = result_record()
    allowed = load_fixture_provenance(fixture_provenance, head, record)

    home_count = len(HOME_RE.findall(text))
    if home_count:
        add(record, "blockers", "privacy scan found %d developer-home path shape(s)" % home_count)
    pem_count = len(PEM_RE.findall(text))
    if pem_count:
        add(record, "blockers", "privacy scan found %d PEM certificate envelope(s)" % pem_count)

    credential_tokenized = tokenize_credential_literals(text, record)
    email_count = customer_email_count(credential_tokenized)
    if email_count:
        add(record, "blockers", "privacy scan found %d customer email shape(s)" % email_count)

    uuid_tokenized = tokenize_uuids(credential_tokenized, allowed, record)
    redacted = UUID_OR_DIGEST_RE.sub("<digest>", uuid_tokenized)
    exempt = len(UUID_OR_DIGEST_RE.findall(uuid_tokenized))
    if exempt:
        add(record, "notes", "%d added/path line(s) carried generated UUID/digest shapes; inspect those lines" % exempt)

    normalized = normalize_whitespace(redacted)
    phone = "\n".join(match.group(0) for match in PHONE_RE.finditer(normalized))
    landline = "\n".join(match.group(0) for match in LANDLINE_RE.finditer(normalized))
    standard_landline = "\n".join(match.group(0) for match in STANDARD_LANDLINE_RE.finditer(normalized))
    shapes = "\n".join((
        redacted,
        re.sub(r"\D", "", phone),
        re.sub(r"\D", "", landline),
        re.sub(r"\D", "", standard_landline),
        re.sub(r"\D", "", grouped_numbers(redacted)),
    ))
    hits = nonplaceholder_count(IDENTIFIER_RE, shapes)
    runs = nonplaceholder_count(LONG_RUN_RE, shapes)
    if hits or runs:
        add(record, "blockers", "privacy scan found %d identifier shape(s) and %d unexplained long digit run(s)" % (hits, runs))
    else:
        add(record, "notes", "PR metadata, destination paths, and payload lines carry no identifier shapes")
    return record


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--head")
    parser.add_argument("--fixture-provenance")
    parser.add_argument(
        "--redact-shapes", action="store_true",
        help="Redact identifier/digest/home-path/email shapes from stdin and "
        "print the result. No classification; no --head required.",
    )
    parser.add_argument(
        "--redact-public-agent-attribution-messages", action="store_true",
        help="Read a JSON array of commit messages and redact only the known public-agent address in exact Git trailers.",
    )
    args = parser.parse_args(argv)
    if args.redact_shapes:
        try:
            text = sys.stdin.read()
        except UnicodeError:
            return 0
        sys.stdout.write(redact_shapes(text))
        return 0
    if args.redact_public_agent_attribution_messages:
        try:
            messages = json.load(sys.stdin)
            json.dump(redact_public_agent_attribution_messages(messages), sys.stdout)
        except (TypeError, ValueError, json.JSONDecodeError) as error:
            parser.error(str(error))
        return 0
    if not args.head or not re.fullmatch(r"[0-9A-Fa-f]{40}", args.head):
        parser.error("--head must be a full 40-hex commit SHA")
    try:
        text = sys.stdin.read()
    except UnicodeError:
        print(json.dumps({"blockers": [], "indeterminate": ["could not read privacy scan input"], "notes": []}))
        return 0
    print(json.dumps(scan(text, args.head.lower(), args.fixture_provenance), sort_keys=True))
    return 0


if __name__ == "__main__":
    sys.exit(main())
