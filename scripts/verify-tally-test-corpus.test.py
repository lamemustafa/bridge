"""Offline contract tests for the corpus-verifier wrapper."""

import pathlib
import subprocess
import sys


ROOT = pathlib.Path(__file__).resolve().parent.parent
SCRIPT = ROOT / "scripts" / "verify-tally-test-corpus.py"


def run(*args):
    return subprocess.run([sys.executable, str(SCRIPT), *args], capture_output=True, text=True)


def main():
    help_result = run("--help")
    assert help_result.returncode == 0, help_result.stderr
    assert "--extent-xml" in help_result.stdout
    assert "--voucher-xml" in help_result.stdout
    missing_capture = run(
        "--company", "Synthetic Company", "--guid", "synthetic-guid",
        "--from", "20260401", "--to", "20260401", "--as-of", "20260401",
        "--extent-xml", "missing-extent.xml", "--voucher-xml", "missing-vouchers.xml",
    )
    assert missing_capture.returncode != 0
    assert "capture is not a regular file" in missing_capture.stderr


if __name__ == "__main__":
    main()
