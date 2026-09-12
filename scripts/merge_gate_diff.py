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


def diff_destination(line: str) -> str:
    value = remove_prefix(line, "diff --git ")
    if value.startswith('"'):
        _source, index = quoted_token(value, 0)
        if index >= len(value) or value[index] != " ":
            raise ValueError("missing destination")
        destination, end = quoted_token(value, index + 1)
        if end != len(value):
            raise ValueError("trailing header text")
        path = decode_quoted_path(destination)
        if not path.startswith("b/"):
            raise ValueError("destination does not start b/")
        return path[2:]
    source, separator, destination = value.partition(" b/")
    if not separator or not source.startswith("a/") or not destination:
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

    for line in lines:
        if line.startswith("diff --git "):
            emit()
            record = {
                "destination": diff_destination(line),
                "textual_destination": None,
                "added": 0,
                "deleted": 0,
                "binary": False,
                "in_hunk": False,
            }
            continue
        if record is None:
            continue
        if not record["in_hunk"] and line.startswith("+++ "):
            record["textual_destination"] = textual_destination(line)
            continue
        if not record["in_hunk"] and line in {"GIT binary patch"} or (
            not record["in_hunk"] and line.startswith("Binary files ") and line.endswith(" differ")
        ):
            record["binary"] = True
            continue
        if line.startswith("@@ "):
            record["in_hunk"] = True
            continue
        if record["in_hunk"] and line.startswith("+"):
            record["added"] = int(record["added"]) + 1
            added_payload.append(line[1:])
        elif record["in_hunk"] and line.startswith("-"):
            record["deleted"] = int(record["deleted"]) + 1
    emit()
    for record in records:
        record.pop("in_hunk")
    return {"records": records, "added_payload": added_payload}


if __name__ == "__main__":
    try:
        print(json.dumps(parse(sys.stdin.read().splitlines())))
    except (UnicodeError, ValueError) as error:
        print(f"merge_gate_diff_error:{error}", file=sys.stderr)
        raise SystemExit(2)
