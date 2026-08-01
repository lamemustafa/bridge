"""Run corpus acceptance through Bridge's production Rust parser.

This wrapper deliberately does no XML parsing. Capture the sealed, read-only
CompanyBookExtentV1 and VoucherOutstandingsV1 responses on the Tally host, then
pass their file paths here. The Rust verifier is the sole implementation of
voucher parsing, exact-decimal validation, posting filtering, bill identity,
ageing, and acceptance criteria.
"""

import argparse
import pathlib
import subprocess
import sys


def main(argv):
    parser = argparse.ArgumentParser()
    parser.add_argument("--company", required=True)
    parser.add_argument("--guid", required=True)
    parser.add_argument("--from", dest="from_date", required=True)
    parser.add_argument("--to", dest="to_date", required=True)
    parser.add_argument("--as-of", required=True)
    parser.add_argument("--extent-xml", type=pathlib.Path, required=True)
    parser.add_argument("--voucher-xml", type=pathlib.Path, required=True)
    arguments = parser.parse_args(argv)
    for capture in (arguments.extent_xml, arguments.voucher_xml):
        if not capture.is_file():
            parser.error(f"capture is not a regular file: {capture}")

    root = pathlib.Path(__file__).resolve().parent.parent
    command = [
        "rustup", "run", "1.96.0", "cargo", "run", "--quiet", "--locked",
        "--manifest-path", str(root / "src-tauri" / "Cargo.toml"),
        "-p", "bridge-tally-protocol", "--bin", "bridge-tally-test-corpus-verifier", "--",
        "--company", arguments.company,
        "--guid", arguments.guid,
        "--from", arguments.from_date,
        "--to", arguments.to_date,
        "--as-of", arguments.as_of,
        "--extent-xml", str(arguments.extent_xml),
        "--voucher-xml", str(arguments.voucher_xml),
    ]
    result = subprocess.run(command, cwd=root, check=False)
    return result.returncode


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
