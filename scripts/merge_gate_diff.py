#!/usr/bin/env python3
"""Parse the textual portion of a GitHub pull-request diff for merge-gate."""
from __future__ import annotations

import json
import sys


def remove_prefix(value: str, prefix: str) -> str:
    """Python 3.8-compatible equivalent of str.removeprefix."""
    return value[len(prefix):] if value.startswith(prefix) else value


def decode_quoted_path(value: str) -> str:
    if not (value.startswith('"') and value.endswith('"')):
        raise ValueError("expected quoted path")
    body = value[1:-1]
    raw = bytearray()
    index = 0
    while index < len(body):
        char = body[index]
        if char != "\\":
            raw.extend(char.encode("utf-8"))
            index += 1
            continue
        index += 1
        if index == len(body):
            raise ValueError("unterminated escape")
        escaped = body[index]
        if escaped in '"\\':
            raw.append(ord(escaped))
            index += 1
        elif escaped in "abfnrtv":
            raw.append({"a": 7, "b": 8, "f": 12, "n": 10, "r": 13, "t": 9, "v": 11}[escaped])
            index += 1
        elif escaped in "01234567":
            octal = body[index:index + 3]
            if len(octal) != 3 or any(char not in "01234567" for char in octal):
                raise ValueError("invalid octal escape")
            raw.append(int(octal, 8))
            index += 3
        else:
            raise ValueError("unsupported escape")
    return raw.decode("utf-8")


def quoted_token(value: str, start: int) -> tuple[str, int]:
    if value[start] != '"':
        raise ValueError("expected quote")
    index = start + 1
    escaped = False
    while index < len(value):
        char = value[index]
        if escaped:
            escaped = False
        elif char == "\\":
            escaped = True
        elif char == '"':
            return value[start:index + 1], index + 1
        index += 1
    raise ValueError("unterminated quoted token")


def diff_destination(line: str) -> str | None:
    value = remove_prefix(line, "diff --git ")
    if value.startswith('"'):
        _source, index = quoted_token(value, 0)
        if index >= len(value) or value[index] != " ":
            raise ValueError("missing destination")
        destination = value[index + 1:]
        path = decode_quoted_path(destination) if destination.startswith('"') else destination
        if not path.startswith("b/"):
            raise ValueError("destination does not start b/")
        return path[2:]
    # Git quotes each path independently.  A plain source with a quoted
    # destination is therefore valid (and occurs for plain-to-Unicode
    # renames); the quote itself cannot occur in an unquoted token.
    quoted_destination = value.find(' "b/')
    if quoted_destination >= 0:
        source = value[:quoted_destination]
        destination = value[quoted_destination + 1:]
        if not source.startswith("a/"):
            raise ValueError("invalid unquoted source")
        path = decode_quoted_path(destination)
        if not path.startswith("b/"):
            raise ValueError("destination does not start b/")
        return path[2:]
    # An unquoted header has no escaping grammar.  Splitting on the first
    # `` b/`` silently misparses a legal-looking filename containing that
    # sequence.  Defer an ambiguous header to the independently parsed +++
    # destination; that destination is subsequently reconciled to the REST
    # changed-file inventory.  Metadata-only ambiguous records remain
    # indeterminate because they have no unambiguous textual identity.
    parts = value.split(" b/")
    if len(parts) != 2:
        # Pure mode/deletion records may have neither +++ nor rename-to.  A
        # same-path header can still be proven when exactly one candidate
        # delimiter leaves identical a/ and b/ paths.  Do not guess when the
        # filename makes that proof ambiguous.
        matches: list[str] = []
        start = 0
        while True:
            index = value.find(" b/", start)
            if index < 0:
                break
            source = value[:index]
            destination = value[index + 1:]
            if source.startswith("a/") and destination.startswith("b/") and source[2:] == destination[2:]:
                matches.append(destination[2:])
            start = index + 1
        return matches[0] if len(matches) == 1 else None
    source, destination = parts
    if not source.startswith("a/") or not destination:
        raise ValueError("invalid unquoted header")
    return destination


def textual_destination(line: str) -> str | None:
    value = remove_prefix(line, "+++ ")
    if value == "/dev/null":
        return None
    if value.startswith('"'):
        path = decode_quoted_path(value)
    else:
        path = value
    if not path.startswith("b/"):
        raise ValueError("textual destination does not start b/")
    return path[2:]


def parse(lines: list[str]) -> dict[str, object]:
    records: list[dict[str, object]] = []
    added_payload: list[str] = []
    record: dict[str, object] | None = None

    def emit() -> None:
        if record is not None:
            records.append(record.copy())

    for raw_line in lines:
        # A diff is delimited by LF.  CR from CRLF or in an added payload is
        # content for scanning/counting, but must not prevent recognising a
        # protocol header.
        line = raw_line[:-1] if raw_line.endswith("\r") else raw_line
        if line.startswith("diff --git "):
            emit()
            record = {
                "destination": diff_destination(line),
                "textual_destination": None,
                "rename_destination": None,
                "added": 0,
                "deleted": 0,
                "binary": False,
                "gitlink": False,
                "in_hunk": False,
            }
            continue
        if record is None:
            continue
        if not record["in_hunk"] and line.startswith("+++ "):
            record["textual_destination"] = textual_destination(line)
            continue
        if not record["in_hunk"] and line.startswith("rename to "):
            destination = remove_prefix(line, "rename to ")
            record["rename_destination"] = decode_quoted_path(destination) if destination.startswith('"') else destination
            continue
        if not record["in_hunk"] and line in {"GIT binary patch"} or (
            not record["in_hunk"] and line.startswith("Binary files ") and line.endswith(" differ")
        ):
            record["binary"] = True
            continue
        if not record["in_hunk"] and line in {
            "old mode 160000", "new mode 160000", "new file mode 160000",
            "deleted file mode 160000",
        }:
            record["gitlink"] = True
            continue
        if line.startswith("@@ "):
            record["in_hunk"] = True
            continue
        if record["in_hunk"] and raw_line.startswith("+"):
            record["added"] = int(record["added"]) + 1
            added_payload.append(raw_line[1:])
        elif record["in_hunk"] and raw_line.startswith("-"):
            record["deleted"] = int(record["deleted"]) + 1
    emit()
    for record in records:
        record.pop("in_hunk")
        if record["destination"] is None:
            if record["textual_destination"] is None and record["rename_destination"] is None:
                raise ValueError("ambiguous header without textual destination")
            record["destination"] = record["textual_destination"] or record["rename_destination"]
        record.pop("rename_destination")
    return {"records": records, "added_payload": added_payload}


if __name__ == "__main__":
    try:
        raw = sys.stdin.buffer.read().decode("utf-8")
        # Split only on the protocol's LF delimiter.  Do not let Python's
        # universal-newline mode erase CR or Unicode line-separator payload.
        print(json.dumps(parse(raw.split("\n"))))
    except (UnicodeError, ValueError) as error:
        print(f"merge_gate_diff_error:{error}", file=sys.stderr)
        raise SystemExit(2)
