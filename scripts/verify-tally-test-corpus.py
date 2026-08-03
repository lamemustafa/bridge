"""Fail closed on corpus acceptance and provide an offline locality diagnostic."""

import argparse
import datetime
import sys
import xml.etree.ElementTree as ET


def locality_diagnostic(path):
    try:
        root = ET.parse(path).getroot()
    except (OSError, ET.ParseError) as error:
        print(f"LOCALITY DIAGNOSTIC FAILED: capture unreadable ({error})")
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
            alter_id = int(alter_id)
            datetime.datetime.strptime(date, "%Y%m%d")
        except (TypeError, ValueError):
            print("LOCALITY DIAGNOSTIC FAILED: invalid voucher alter ID or date")
            return 2
        if alter_id <= 0 or alter_id in alter_ids:
            print("LOCALITY DIAGNOSTIC FAILED: non-positive or duplicate voucher alter ID")
            return 2
        alter_ids.add(alter_id)
        months.setdefault(date[:6], []).append(alter_id)

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
    child = element.find(name)
    return child.text.strip() if child is not None and child.text else None


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
