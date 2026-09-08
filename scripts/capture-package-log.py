# SPDX-License-Identifier: Apache-2.0
"""Drain package output with bounded retention; the caller must use pipefail."""
import json
from pathlib import Path
import sys

PREFIX_BYTES = 4 * 1024 * 1024
TAIL_BYTES = 64 * 1024


def capture(stream, directory):
    directory.mkdir(parents=True, exist_ok=True)
    total = kept = 0
    tail = b""
    with (directory / "build.log").open("wb") as log:
        while chunk := stream.read(8192):
            total += len(chunk)
            prefix = chunk[:max(0, PREFIX_BYTES - kept)]
            log.write(prefix)
            kept += len(prefix)
            tail = (tail + chunk)[-TAIL_BYTES:]
    (directory / "build-tail.log").write_bytes(tail)
    (directory / "capture.json").write_text(json.dumps({
        "total_bytes": total, "prefix_bytes": kept, "tail_bytes": len(tail),
        "prefix_truncated": total > kept,
    }, indent=2) + "\n")
    return tail


if __name__ == "__main__":
    tail = capture(sys.stdin.buffer, Path("package-diagnostics"))
    print("Package output tail (at most 64 KiB); retained prefix and byte counts are in the diagnostic artifact.", flush=True)
    sys.stdout.buffer.write(tail)
