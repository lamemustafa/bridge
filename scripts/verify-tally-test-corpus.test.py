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
    assert module.sanitize_invalid_numeric_references("<A>&#4;</A>") == "<A>\ufffd#4;</A>"
    assert module.sanitize_invalid_numeric_references("<A>&#4294967296;</A>") == "<A>&#4294967296;</A>"
    with tempfile.TemporaryDirectory() as directory:
        capture = pathlib.Path(directory) / "local.xml"
        capture.write_text(
            "<ENVELOPE><BODY><DATA><COLLECTION>"
            "<VOUCHER><ALTERID>1</ALTERID><DATE>20240401</DATE></VOUCHER>"
            "<VOUCHER><ALTERID>3</ALTERID><DATE>20240501</DATE></VOUCHER>"
            "<VOUCHER><ALTERID>5</ALTERID><DATE>20240601</DATE></VOUCHER>"
            "</COLLECTION></DATA></BODY></ENVELOPE>"
        )
        assert module.main(["--locality-xml", str(capture)]) == 0
    with tempfile.TemporaryDirectory() as directory:
        capture = pathlib.Path(directory) / "scattered.xml"
        capture.write_text(
            "<ENVELOPE><BODY><DATA><COLLECTION>"
            "<VOUCHER><ALTERID>1</ALTERID><DATE>20240401</DATE></VOUCHER>"
            "<VOUCHER><ALTERID>100</ALTERID><DATE>20240402</DATE></VOUCHER>"
            "<VOUCHER><ALTERID>2</ALTERID><DATE>20240501</DATE></VOUCHER>"
            "<VOUCHER><ALTERID>3</ALTERID><DATE>20240601</DATE></VOUCHER>"
            "</COLLECTION></DATA></BODY></ENVELOPE>"
        )
        assert module.main(["--locality-xml", str(capture)]) == 1
    with tempfile.TemporaryDirectory() as directory:
        capture = pathlib.Path(directory) / "one-month.xml"
        capture.write_text(
            "<ENVELOPE><BODY><DATA><COLLECTION>"
            "<VOUCHER><ALTERID>1</ALTERID><DATE>20240401</DATE></VOUCHER>"
            "<VOUCHER><ALTERID>2</ALTERID><DATE>20240402</DATE></VOUCHER>"
            "</COLLECTION></DATA></BODY></ENVELOPE>"
        )
        assert module.main(["--locality-xml", str(capture)]) == 2
    with tempfile.TemporaryDirectory() as directory:
        capture = pathlib.Path(directory) / "two-dense-months.xml"
        april = "".join(
            f"<VOUCHER><ALTERID>{alter_id}</ALTERID><DATE>20240401</DATE></VOUCHER>"
            for alter_id in range(1, 26)
        )
        may = "".join(
            f"<VOUCHER><ALTERID>{alter_id}</ALTERID><DATE>20240501</DATE></VOUCHER>"
            for alter_id in range(26, 51)
        )
        capture.write_text(
            "<ENVELOPE><BODY><DATA><COLLECTION>"
            f"{april}{may}"
            "</COLLECTION></DATA></BODY></ENVELOPE>"
        )
        assert module.main(["--locality-xml", str(capture)]) == 2
    with tempfile.TemporaryDirectory() as directory:
        capture = pathlib.Path(directory) / "bad-date.xml"
        capture.write_text(
            "<ENVELOPE><BODY><DATA><COLLECTION>"
            "<VOUCHER><ALTERID>1</ALTERID><DATE>202641</DATE></VOUCHER>"
            "<VOUCHER><ALTERID>2</ALTERID><DATE>20240501</DATE></VOUCHER>"
            "</COLLECTION></DATA></BODY></ENVELOPE>"
        )
        assert module.main(["--locality-xml", str(capture)]) == 2
    for name, alter_id in (
        ("unicode-alter-id", "١"),
        ("underscore-alter-id", "1_0"),
        ("overflow-alter-id", "18446744073709551616"),
    ):
        with tempfile.TemporaryDirectory() as directory:
            capture = pathlib.Path(directory) / f"{name}.xml"
            capture.write_text(
                "<ENVELOPE><BODY><DATA><COLLECTION>"
                f"<VOUCHER><ALTERID>{alter_id}</ALTERID><DATE>20240401</DATE></VOUCHER>"
                "</COLLECTION></DATA></BODY></ENVELOPE>"
            )
            assert module.main(["--locality-xml", str(capture)]) == 2
    with tempfile.TemporaryDirectory() as directory:
        capture = pathlib.Path(directory) / "invalid-reference.xml"
        capture.write_text(
            "<ENVELOPE><BODY><DATA><COLLECTION><LEDGER>&#4;</LEDGER>"
            "<VOUCHER><ALTERID>1</ALTERID><DATE>20240401</DATE></VOUCHER>"
            "<VOUCHER><ALTERID>3</ALTERID><DATE>20240501</DATE></VOUCHER>"
            "<VOUCHER><ALTERID>5</ALTERID><DATE>20240601</DATE></VOUCHER>"
            "</COLLECTION></DATA></BODY></ENVELOPE>"
        )
        assert module.main(["--locality-xml", str(capture)]) == 0


if __name__ == "__main__":
    main()
