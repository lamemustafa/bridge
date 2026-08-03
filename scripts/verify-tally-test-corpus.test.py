"""Offline contract tests for the corpus-qualification boundary."""

import importlib.util
import pathlib
import tempfile


SCRIPT = pathlib.Path(__file__).resolve().parent / "verify-tally-test-corpus.py"


def main():
    spec = importlib.util.spec_from_file_location("verify_tally_test_corpus", SCRIPT)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    assert module.main([]) == 2
    with tempfile.NamedTemporaryFile(mode="w", suffix=".xml") as capture:
        capture.write(
            "<ENVELOPE><BODY><DATA><COLLECTION>"
            "<VOUCHER><ALTERID>1</ALTERID><DATE>20240401</DATE></VOUCHER>"
            "<VOUCHER><ALTERID>3</ALTERID><DATE>20240501</DATE></VOUCHER>"
            "</COLLECTION></DATA></BODY></ENVELOPE>"
        )
        capture.flush()
        assert module.main(["--locality-xml", capture.name]) == 0
    with tempfile.NamedTemporaryFile(mode="w", suffix=".xml") as capture:
        capture.write(
            "<ENVELOPE><BODY><DATA><COLLECTION>"
            "<VOUCHER><ALTERID>1</ALTERID><DATE>20240401</DATE></VOUCHER>"
            "<VOUCHER><ALTERID>100</ALTERID><DATE>20240402</DATE></VOUCHER>"
            "<VOUCHER><ALTERID>2</ALTERID><DATE>20240501</DATE></VOUCHER>"
            "</COLLECTION></DATA></BODY></ENVELOPE>"
        )
        capture.flush()
        assert module.main(["--locality-xml", capture.name]) == 1


if __name__ == "__main__":
    main()
