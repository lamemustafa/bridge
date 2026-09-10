"""Offline contract tests for the bank-statement importer.

No PDF, no network, no client data: every fixture here is synthetic. The
statement-shaped strings are hand-written in the layouts the parsers target.
"""

import datetime
import decimal
import importlib.util
import pathlib
import tempfile

SCRIPT = pathlib.Path(__file__).resolve().parent / "bank_statement_import.py"
D = decimal.Decimal


def load():
    spec = importlib.util.spec_from_file_location("bank_statement_import", SCRIPT)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


def test_dewrap(m):
    """A fragment reaching the cell edge was broken mid-token and joins with
    nothing; a shorter one ended at a real space."""
    edge = 100.0
    assert m._dewrap([("UPI/DR/1234", 99.5), ("56789012/X", 60.0)], edge) == "UPI/DR/123456789012/X"
    assert m._dewrap([("NORTH", 40.0), ("WIND", 45.0)], edge) == "NORTH WIND"
    assert m._dewrap([], edge) == ""


def test_escaping(m):
    """9.1b: one unescaped '&' makes the whole request malformed, and
    'Duties & Taxes' is a stock group in every company."""
    assert m.escape("Ram & Sons <Ltd>") == "Ram &amp; Sons &lt;Ltd&gt;"
    xml = m.envelope("A & B", [m.voucher_xml(
        "Payment", datetime.date(2026, 8, 1), "R1", "narr & more",
        "Ram & Sons", "Bank", D("10.00"))])
    assert "&amp;" in xml and " & " not in xml
    import xml.etree.ElementTree as ET
    ET.fromstring(xml)


def test_sign_convention(m):
    """Debit is ISDEEMEDPOSITIVE Yes with a NEGATIVE amount."""
    v = m.voucher_xml("Payment", datetime.date(2026, 8, 1), "R1", "n",
                      "Party", "Bank", D("100.00"), party_ledger="Party")
    assert "<LEDGERNAME>Party</LEDGERNAME>\n    <ISDEEMEDPOSITIVE>Yes</ISDEEMEDPOSITIVE>" \
           "\n    <AMOUNT>-100.00</AMOUNT>" in v
    assert "<LEDGERNAME>Bank</LEDGERNAME>\n    <ISDEEMEDPOSITIVE>No</ISDEEMEDPOSITIVE>" \
           "\n    <AMOUNT>100.00</AMOUNT>" in v
    assert "<DATE>20260801</DATE>" in v and "<EFFECTIVEDATE>20260801</EFFECTIVEDATE>" in v
    # a Contra has no party ledger
    c = m.voucher_xml("Contra", datetime.date(2026, 8, 1), "R2", "n", "Cash", "Bank", D("5.00"))
    assert "PARTYLEDGERNAME" not in c


def test_selfcheck_rejects_bad_xml(m):
    good = m.envelope("Co", [m.voucher_xml("Payment", datetime.date(2026, 8, 1), "R1",
                                           "n", "P", "Bank", D("10.00"))])
    count, out, inward = m.selfcheck(good, "Bank", [{"voucher_type": "Payment"}])
    assert (count, out, inward) == (1, D("10.00"), D(0))
    for broken, why in (
        (good.replace("<AMOUNT>10.00</AMOUNT>", "<AMOUNT>11.00</AMOUNT>"), "unbalanced"),
        (good.replace("<ISDEEMEDPOSITIVE>Yes</ISDEEMEDPOSITIVE>",
                      "<ISDEEMEDPOSITIVE>No</ISDEEMEDPOSITIVE>"), "sign"),
        (good.replace("<EFFECTIVEDATE>20260801</EFFECTIVEDATE>",
                      "<EFFECTIVEDATE>20260802</EFFECTIVEDATE>"), "dates"),
    ):
        try:
            m.selfcheck(broken, "Bank", [{"voucher_type": "Payment"}])
        except SystemExit:
            continue
        raise AssertionError(f"selfcheck accepted {why}")
    # manifest and XML must agree on how many vouchers exist
    try:
        m.selfcheck(good, "Bank", [{"voucher_type": "Payment"}] * 2)
    except SystemExit:
        pass
    else:
        raise AssertionError("selfcheck accepted a manifest/XML count mismatch")


def test_reconcile(m):
    bank = m.HDFC()
    rows = [{"date": "01/08/26", "dr": "", "cr": "100.00", "bal": "1100.00"},
            {"date": "02/08/26", "dr": "50.00", "cr": "", "bal": "1050.00"}]
    assert m.reconcile(rows, bank, "1000.00") == D("1050.00")
    rows[1]["bal"] = "1049.00"
    try:
        m.reconcile(rows, bank, "1000.00")
    except SystemExit:
        return
    raise AssertionError("reconcile accepted a broken balance chain")


def test_mapping_key_ignores_wrap_spacing(m):
    with tempfile.TemporaryDirectory() as directory:
        path = pathlib.Path(directory) / "map.csv"
        path.write_text("party,ledger,treatment\n"
                        "ZEPHYR MANUFACTURING,M/s Zephyr,auto\n"
                        "OWN ACCOUNT,,skip\n", encoding="utf-8")
        mapping = m.load_mapping(path)
    # the wrap heuristic can space the same name two ways; both must map
    for spelling in ("ZEPHYRM ANUFACTURING", "ZEPHYRMANUFACTURING", "Zephyr Manufacturing"):
        assert mapping[m._key(spelling)] == ("M/s Zephyr", "auto"), spelling
    assert mapping[m._key("OWN ACCOUNT")][1] == "skip"


def test_party_and_reference_extraction(m):
    sbi, hdfc = m.SBI(), m.HDFC()
    row = {"narr_spaced": "TO TRANSFER- INB NEFT UTR NO: ZZZZ1111111 11111- NORTH WIND TRADERS",
           "ref_spaced": "NEFT INB: ZZZZZZZZZ 9 TRANSFER TO 00000000000 00 / NORTH WIND TRADERS",
           "narr": "TO TRANSFER- INB NEFT UTR NO: ZZZZ111111111111- NORTH WIND TRADERS",
           "ref": "NEFT INB: ZZZZZZZZZ9 TRANSFER TO 0000000000000 / NORTH WIND TRADERS"}
    assert sbi.party(row) == "NORTH WIND TRADERS"
    assert sbi.reference(row) == ("NEFT", "ZZZZ111111111111")
    # a 12-digit UPI reference split by the cell wrap must rejoin intact
    upi = {"narr": "TO TRANSFER- UPI/DR/1234 56789012/ACME/ZZZZ/ acme@zzz/BILL20-",
           "ref": "TRANSFER TO 00000000000 00 /"}
    upi["narr_spaced"] = upi["narr"]
    upi["ref_spaced"] = upi["ref"]
    assert sbi.reference(upi) == ("UPI", "123456789012 (acme@zzz)")

    h = {"narr": "IMPS-999999999999-ACME INDUSTRIES-ZZZZ-XXXXXXXX0000-OURFIRM",
         "ref": "0000999999999999"}
    assert hdfc.party(h) == "ACME INDUSTRIES"
    assert hdfc.reference(h) == ("IMPS", "999999999999")
    n = {"narr": "NEFT DR-ZZZZ0000000-ACME INTERNATIONAL-NETBANK, MUM-ZZZZZ00000000000-BB",
         "ref": "ZZZZZ00000000000"}
    assert hdfc.party(n) == "ACME INTERNATIONAL"
    # a masked account is not a payee name
    masked = {"narr": "UPI-XXXXXX0000-ZZZZ0000001-888888888888-PAYMENT FROM PHONE",
              "ref": "0000888888888888"}
    assert hdfc.party(masked) == "UNNAMED"


def test_build_treatments(m):
    """skip emits nothing; contra emits a Contra; unmapped falls to suspense."""
    bank = m.HDFC()
    rows = [{"date": "01/08/26", "narr": "UPI-ALPHA-9@x-ABCD0001-111111111111-P", "ref": "1",
             "dr": "10.00", "cr": "", "bal": ""},
            {"date": "02/08/26", "narr": "UPI-OWN ACCT-9@x-ABCD0001-222222222222-P", "ref": "2",
             "dr": "20.00", "cr": "", "bal": ""},
            {"date": "03/08/26", "narr": "UPI-GHOST-9@x-ABCD0001-333333333333-P", "ref": "3",
             "dr": "", "cr": "30.00", "bal": ""}]
    mapping = {m._key("ALPHA"): ("Alpha Ledger", "contra"),
               m._key("OWN ACCT"): ("", "skip")}
    vouchers, manifest = m.build(rows, bank, "Co", "Bank", "SUSPENSE ACC", mapping, "ACC")
    assert len(vouchers) == 2 and len(manifest) == 3
    assert 'VCHTYPE="Contra"' in vouchers[0] and "PARTYLEDGERNAME" not in vouchers[0]
    assert [r["voucher_type"] for r in manifest] == ["Contra", "SKIPPED", "Receipt"]
    assert manifest[2]["suspense"] == "YES"
    assert "reallocate from Suspense" in manifest[2]["narration"]
    # an unmapped party keeps the statement's own spelling in the narration so it
    # can still be identified later
    assert "GHOST" in manifest[2]["narration"]


def main():
    module = load()
    for name, test in sorted(globals().items()):
        if name.startswith("test_"):
            test(module)
            print(f"ok  {name}")
    print("all offline contract tests passed")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
