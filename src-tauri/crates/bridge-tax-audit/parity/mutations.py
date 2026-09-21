# SPDX-License-Identifier: Apache-2.0
"""Run the crate's recorded mutations: each one edits one source file, runs the whole crate test
suite, records which tests fail, and puts the file back. A mutation the suite does not fail on
"survives"; one whose `from` text is no longer in its file "did not apply" and is stale (update or
retire it). The list, `parity/mutations.json`, keeps every mutation a reviewer or an author has
written for this crate, with who wrote it.

    python3 parity/mutations.py              # all of them
    python3 parity/mutations.py M08 B12      # by id

Run from the crate root with the pinned toolchain exported (RUSTC, RUSTDOC and PATH; see
AGENTS.md). It refuses to start while any source file it could edit has uncommitted changes, and
it restores the edited file on exit, on an exception and on SIGINT/SIGTERM, then touches it so
cargo does not reuse a mutated build.
"""
from __future__ import annotations

import json
import os
import re
import signal
import subprocess
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
LIST = ROOT / "parity" / "mutations.json"


def main() -> int:
    mutations = json.loads(LIST.read_text(encoding="utf-8"))
    wanted = set(sys.argv[1:])
    if wanted:
        mutations = [m for m in mutations if m["id"] in wanted]
    files = sorted({m["file"] for m in mutations})
    dirty = subprocess.run(["git", "status", "--porcelain", "--", *files], cwd=ROOT, capture_output=True,
                           text=True, check=True).stdout.strip()
    if dirty:
        print(f"refusing: uncommitted changes in files a mutation edits:\n{dirty}", file=sys.stderr)
        return 2

    current: dict[str, str] = {}

    def restore(*_):
        for path, text in current.items():
            (ROOT / path).write_text(text, encoding="utf-8")
            os.utime(ROOT / path, None)
        current.clear()

    def interrupted(signum, _frame):
        restore()
        raise SystemExit(128 + signum)

    signal.signal(signal.SIGINT, interrupted)
    signal.signal(signal.SIGTERM, interrupted)
    results = []
    try:
        for m in mutations:
            path = ROOT / m["file"]
            text = path.read_text(encoding="utf-8")
            if text.count(m["from"]) != 1:
                results.append((m, f"DID NOT APPLY ({text.count(m['from'])} matches)"))
                print(m["id"], results[-1][1], flush=True)
                continue
            current[m["file"]] = text
            path.write_text(text.replace(m["from"], m["to"]), encoding="utf-8")
            r = subprocess.run(["cargo", "test", "--locked", "-q", "--no-fail-fast", "-p", "bridge-tax-audit"],
                               cwd=ROOT, capture_output=True, text=True)
            restore()
            out = r.stdout + r.stderr
            failed = sorted(set(re.findall(r"^---- (\S+) stdout ----", out, re.M)))
            if "error[" in out:
                verdict = "COMPILE ERROR"
            elif failed:
                verdict = f"killed by {len(failed)}: {', '.join(failed)}"
            else:
                verdict = "SURVIVED" if r.returncode == 0 else f"failed, rc={r.returncode}, no test named"
            results.append((m, verdict))
            print(m["id"], verdict[:200], flush=True)
    finally:
        restore()
    killed = sum(v.startswith("killed") for _, v in results)
    print(f"\n{killed} of {len(results)} killed")
    for m, v in results:
        if not v.startswith("killed"):
            print(f"  {m['id']} ({m['by']}: {m['what']}): {v}")
    return 0 if killed == len(results) else 1


if __name__ == "__main__":
    raise SystemExit(main())
