# SPDX-License-Identifier: Apache-2.0
"""Regenerate `src/support.rs`'s case-mapping exception lists: the code points where the pinned
Rust toolchain's `char::to_uppercase` / `to_lowercase` differ from the reference's Python 3.13
`str.upper()` / `str.lower()`.

    cargo run --example case_mapping_dump > /tmp/rust_case.tsv        # from src-tauri, pinned toolchain
    uv run -q --python 3.13 python parity/case_exceptions.py /tmp/rust_case.tsv

prints the Unicode versions on both sides and the two Rust arrays to paste into `support.rs`
(`UPPER_UNCHANGED`, `LOWER_UNCHANGED`), and refuses if Python maps any listed code point to
anything but itself (then `py_upper`/`py_lower` would need a mapping table, not a list).
"""
from __future__ import annotations

import sys
import unicodedata


def main() -> int:
    rows = {}
    lines = open(sys.argv[1], encoding="utf-8").read().splitlines()
    rust_version = lines[0]
    for line in lines[1:]:
        cp, up, lo = line.split("\t")
        rows[int(cp, 16)] = (up, lo)
    hx = lambda s: " ".join(f"{ord(c):X}" for c in s)  # noqa: E731
    upper = [cp for cp, (up, _) in sorted(rows.items()) if hx(chr(cp).upper()) != up]
    lower = [cp for cp, (_, lo) in sorted(rows.items()) if hx(chr(cp).lower()) != lo]
    moved = [cp for cp in upper if chr(cp).upper() != chr(cp)] + [cp for cp in lower if chr(cp).lower() != chr(cp)]
    if moved:
        print(f"refusing: Python maps these listed code points to something else: {moved}", file=sys.stderr)
        return 1
    print(f"// {rust_version}; Python {sys.version.split()[0]}, Unicode {unicodedata.unidata_version}")
    for name, cps in (("UPPER_UNCHANGED", upper), ("LOWER_UNCHANGED", lower)):
        print(f"const {name}: [u32; {len(cps)}] = [")
        for i in range(0, len(cps), 8):
            print("    " + ", ".join(f"0x{c:04X}" for c in cps[i:i + 8]) + ",")
        print("];")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
