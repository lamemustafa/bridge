"""Fail closed on corpus acceptance and provide an offline locality diagnostic."""

import argparse
import datetime
import pathlib
import re
import sys
import xml.etree.ElementTree as ET


NUMERIC_REFERENCE = re.compile(r"&#(?P<token>(?:[xX][0-9A-Fa-f]+)|(?:[0-9]+));")


def locality_diagnostic(path):
    try:
        xml = pathlib.Path(path).read_text(encoding="utf-8")
        root = ET.fromstring(sanitize_invalid_numeric_references(xml))
    except (OSError, UnicodeError, ET.ParseError) as error:
        print(f"LOCALITY DIAGNOSTIC FAILED: capture unreadable ({error})")
        return 2

    if child_text(root.find("./HEADER"), "STATUS") != "1":
        print("LOCALITY DIAGNOSTIC FAILED: capture status is not success")
        return 2

    data = root.find("./BODY/DATA")
    vouchers = data.findall(".//VOUCHER") if data is not None else []
    if not vouchers:
        print("LOCALITY DIAGNOSTIC FAILED: no voucher rows")
        return 2

    months = {}
    alter_ids = set()
    for voucher in vouchers:
        alter_id = child_text(voucher, "ALTERID")
        date = child_text(voucher, "DATE")
        try:
            if alter_id is None or re.fullmatch(r"[0-9]+", alter_id) is None:
                raise ValueError("alter ID is not canonical unsigned decimal")
            alter_id = int(alter_id)
            if date is None or re.fullmatch(r"[0-9]{8}", date) is None:
                raise ValueError("date is not canonical YYYYMMDD")
            datetime.datetime.strptime(date, "%Y%m%d")
        except (TypeError, ValueError):
            print("LOCALITY DIAGNOSTIC FAILED: invalid voucher alter ID or date")
            return 2
        if alter_id > 0xFFFFFFFFFFFFFFFF:
            print("LOCALITY DIAGNOSTIC FAILED: invalid voucher alter ID or date")
            return 2
        if alter_id <= 0 or alter_id in alter_ids:
            print("LOCALITY DIAGNOSTIC FAILED: non-positive or duplicate voucher alter ID")
            return 2
        alter_ids.add(alter_id)
        months.setdefault(date[:6], []).append(alter_id)

    if len(months) < 3:
        print(
            "LOCALITY DIAGNOSTIC INCONCLUSIVE: fewer than three month bands; "
            "not corpus acceptance"
        )
        return 2

    low, high = min(alter_ids), max(alter_ids)
    total_span = high - low + 1
    worst_month, worst_span = max(
        ((month, max(ids) - min(ids) + 1) for month, ids in months.items()),
        key=lambda item: item[1],
    )
    exceeds_limit = worst_span * 100 > total_span * 40
    print(
        "LOCALITY DIAGNOSTIC "
        f"{'FAILED' if exceeds_limit else 'PASSED'}: "
        f"worst_month={worst_month}:span={worst_span}/{total_span}; "
        "not corpus acceptance"
    )
    return 1 if exceeds_limit else 0


def child_text(element, name):
    if element is None:
        return None
    child = element.find(name)
    return child.text.strip() if child is not None and child.text else None


def sanitize_invalid_numeric_references(xml):
    """Make the observed XML 1.0-illegal numeric references parseable.

    This mirrors the production boundary's replacement-marker representation:
    malformed references remain strict XML errors, but a numeric reference to an
    XML-illegal character becomes a legal, identity-preserving text marker.
    """

    def replace(match):
        token = match.group("token")
        try:
            value = int(token[1:], 16) if token[:1].lower() == "x" else int(token)
        except ValueError:
            return match.group(0)
        if value > 0xFFFFFFFF:
            return match.group(0)
        if is_xml_10_char(value):
            return match.group(0)
        return f"\ufffd#{value};"

    return NUMERIC_REFERENCE.sub(replace, xml)


def is_xml_10_char(value):
    return value in (0x9, 0xA, 0xD) or (
        0x20 <= value <= 0xD7FF
        or 0xE000 <= value <= 0xFFFD
        or 0x10000 <= value <= 0x10FFFF
    )


def main(argv):
    parser = argparse.ArgumentParser()
    parser.add_argument("--locality-xml")
    args = parser.parse_args(argv)
    if args.locality_xml:
        return locality_diagnostic(args.locality_xml)
    print(
        "CORPUS UNQUALIFIED: paired, partitioned voucher and opening-coverage "
        "captures are required before Bridge can accept a corpus."
    )
    return 2


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
