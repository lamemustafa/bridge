"""Offline regressions for the operator-only corpus verifier.

The verifier normally imports ``tally_probe``, which is intentionally live-only.
These tests replace it before loading the verifier, so no test opens a socket.
"""
import contextlib
import importlib.util
import io
import pathlib
import sys
import types
import unittest


ROOT = pathlib.Path(__file__).resolve().parent
VERIFIER = ROOT / "verify-tally-test-corpus.py"


class Response:
    def __init__(self, status, data):
        self.status = status
        self.data = data
        self.elapsed = 0.0
        self.nbytes = len(data.encode("utf-8"))


class FakeTally:
    def __init__(self, response):
        self.response = response
        self.post_calls = 0

    def alive(self):
        return True

    def post(self, *_args, **_kwargs):
        self.post_calls += 1
        return self.response


def load_verifier(fake_tally):
    previous = sys.modules.get("tally_probe")
    sys.modules["tally_probe"] = fake_tally
    try:
        spec = importlib.util.spec_from_file_location("verify_tally_test_corpus", VERIFIER)
        module = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(module)
        return module
    finally:
        if previous is None:
            del sys.modules["tally_probe"]
        else:
            sys.modules["tally_probe"] = previous


def voucher(alterid, voucher_date, party, allocation=""):
    return (
        f"<VOUCHER><ALTERID>{alterid}</ALTERID><DATE>{voucher_date}</DATE>"
        "<VOUCHERTYPENAME>Sales</VOUCHERTYPENAME>"
        f"<PARTYLEDGERNAME>{party}</PARTYLEDGERNAME>"
        "<ISOPTIONAL>No</ISOPTIONAL><ISCANCELLED>No</ISCANCELLED>"
        f"<ISDELETED>No</ISDELETED>{allocation}</VOUCHER>"
    )


def allocation(reference, bill_type, amount, bill_date=""):
    bill_date_xml = f"<BILLDATE>{bill_date}</BILLDATE>" if bill_date else ""
    return (
        "<BILLALLOCATIONS.LIST>"
        f"<NAME>{reference}</NAME><BILLTYPE>{bill_type}</BILLTYPE>"
        f"<AMOUNT>{amount}</AMOUNT>{bill_date_xml}"
        "</BILLALLOCATIONS.LIST>"
    )


def corpus_with_cross_party_reference():
    rows = []
    dates = ["20240401", "20250501", "20260401", "20260702", "20260731"]
    open_bills = [
        ("Recent", "20260702"),
        ("Middle", "20260601"),
        ("Older", "20260502"),
        ("Shared", "20260401"),
    ]
    alterid = 1
    for index, voucher_date in enumerate(dates):
        for _ in range(44):
            allocation_xml = ""
            party = "Party A"
            if index == 0 and alterid <= 4:
                reference, bill_date = open_bills[alterid - 1]
                allocation_xml = allocation(reference, "New Ref", "100", bill_date)
            elif index == 0 and alterid == 5:
                # This must not settle Party A's bill with the same reference.
                party = "Party B"
                allocation_xml = allocation("Shared", "Agst Ref", "-100")
            rows.append(voucher(alterid, voucher_date, party, allocation_xml))
            alterid += 1
    return "".join(rows)


class CorpusVerifierTests(unittest.TestCase):
    def run_verifier(self, response):
        fake_tally = FakeTally(response)
        verifier = load_verifier(fake_tally)
        output = io.StringIO()
        with contextlib.redirect_stdout(output):
            result = verifier.main()
        return result, output.getvalue(), fake_tally

    def test_non_success_status_is_rejected_before_rows_are_parsed(self):
        result, output, fake_tally = self.run_verifier(
            Response("0", "<VOUCHER><ALTERID>not-parsed</ALTERID></VOUCHER>")
        )

        self.assertEqual(result, 1)
        self.assertEqual(fake_tally.post_calls, 1)
        self.assertIn("FAIL: export STATUS='0'; corpus cannot be accepted.", output)
        self.assertNotIn("vouchers parsed:", output)
        self.assertNotIn("CORPUS ACCEPTED", output)

    def test_reused_reference_is_scoped_to_party(self):
        result, output, _fake_tally = self.run_verifier(
            Response("1", corpus_with_cross_party_reference())
        )

        self.assertEqual(result, 0, output)
        self.assertIn("'90+': 1", output)
        self.assertIn("CORPUS ACCEPTED", output)


if __name__ == "__main__":
    unittest.main()
