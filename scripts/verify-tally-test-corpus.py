"""Acceptance check for the bill-bearing test corpus.

Run AFTER Codex finishes creating the company and has closed all Tally data-entry
screens (the GUI blocks the XML gateway).

Usage:
    python3 scripts/verify-tally-test-corpus.py "Bridge Billwise Lab"

Ageing is measured against a FIXED date, not today - see RECONCILIATION_AS_OF.

Checks, in order of how expensive the defect is to discover late:
  1. AlterID <-> date ordering   (UNFIXABLE afterwards - determines corpus validity)
  2. Bill references present     (New Ref / Agst Ref, named)
  3. Ageing spread              (all four buckets occupied)
  4. Education date legality    (day 1/2/31)
  5. Size in the 200-500 range
"""
import sys, re, datetime, os
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import tally_probe as tally

# The accepted corpus's documented reconciliation as-of date. Fixed so the
# acceptance result depends on the corpus, not on when the check is run.

def xml_escape(value):
    """Company names are free text and legitimately contain `&` (e.g. "A & B
    Traders"). Interpolating one raw produces malformed XML and the verifier
    cannot run against an otherwise valid company."""
    return (value.replace('&', '&amp;').replace('<', '&lt;')
                 .replace('>', '&gt;').replace('"', '&quot;'))

RECONCILIATION_AS_OF = datetime.date(2026, 7, 31)


CO = sys.argv[1] if len(sys.argv) > 1 else 'Bridge Billwise Lab'
FETCH = ('GUID, MASTERID, ALTERID, DATE, VOUCHERTYPENAME, VOUCHERNUMBER, '
         'ISOPTIONAL, ISCANCELLED, ISDELETED, '
         'PARTYLEDGERNAME, ISCANCELLED, ISDELETED, ALLLEDGERENTRIES.*')


def req(frm, to):
    return ('<ENVELOPE><HEADER><VERSION>1</VERSION><TALLYREQUEST>Export</TALLYREQUEST>'
            '<TYPE>Collection</TYPE><ID>VC</ID></HEADER><BODY><DESC><STATICVARIABLES>'
            '<SVEXPORTFORMAT>$$SysName:XML</SVEXPORTFORMAT>'
            f'<SVCURRENTCOMPANY>{xml_escape(CO)}</SVCURRENTCOMPANY>'
            f'<SVFROMDATE TYPE="Date">{frm}</SVFROMDATE><SVTODATE TYPE="Date">{to}</SVTODATE>'
            '</STATICVARIABLES><TDL><TDLMESSAGE>'
            '<SYSTEM TYPE="Formulae" NAME="FW">$Date &gt;= ##SVFromDate AND $Date &lt;= ##SVToDate</SYSTEM>'
            '<COLLECTION NAME="VC" ISMODIFY="No"><TYPE>Voucher</TYPE>'
            f'<FETCH>{FETCH}</FETCH><FILTERS>FW</FILTERS></COLLECTION>'
            '</TDLMESSAGE></TDL></DESC></BODY></ENVELOPE>')


def main():
    if not tally.alive():
        print('FAIL: gateway not responding. Close all Tally data-entry screens first.')
        return 1

    r = tally.post(req('20240401', '20260731'), timeout=240, tag='vc')
    d = r.data
    print(f'read: {r.elapsed:.1f}s  {r.nbytes/1048576:.2f} MB  status={r.status}')

    # Structural, not a single textual spelling. Tally may emit `<VOUCHER>`
    # with no attributes, or a newline before the first attribute; a
    # `'<VOUCHER '` scan drops those rows, and a partially omitted corpus
    # would then be accepted as calibration evidence.
    vs = re.findall(r'<VOUCHER(?:\s[^>]*)?>.*?</VOUCHER>', d, re.S)
    rows = []
    for v in vs:
        g = lambda t: (re.search(rf'<{t}[^>]*>([^<]*)</{t}>', v) or type('', (), {'group': lambda s, i: ''})()).group(1)
        rows.append({'alterid': g('ALTERID').strip(), 'date': g('DATE').strip(),
                     'vtype': g('VOUCHERTYPENAME'), 'block': v,
                     'optional': g('ISOPTIONAL').strip(),
                     'cancelled': g('ISCANCELLED').strip(),
                     'deleted': g('ISDELETED').strip()})
    # Dropping malformed rows silently lets the corpus be ACCEPTED while the
    # production parser would reject the very same response: it fails closed on
    # a missing/non-numeric ALTERID or a bad date. The verifier must agree with
    # the parser about what is parseable, so any invalid row fails the corpus.
    def real_date(text):
        # An 8-character string is not a date: `20261301` passes a length check
        # and even the Education day rule, while production's TallyDate::parse
        # rejects it. The verifier must agree with the parser.
        try:
            datetime.date(int(text[:4]), int(text[4:6]), int(text[6:8]))
            return True
        except (ValueError, TypeError):
            return False

    valid = [x for x in rows
             if x['alterid'].isdigit() and len(x['date']) == 8 and real_date(x['date'])]
    if len(valid) != len(rows):
        print(f'FAIL: {len(rows) - len(valid)} of {len(rows)} vouchers have a '
              'missing/non-numeric ALTERID or a malformed date. The production '
              'parser rejects these; the corpus cannot be accepted.')
        return 1
    seen = {}
    dupes = sorted({x['alterid'] for x in valid if x['alterid'] in seen or seen.setdefault(x['alterid'], 1) is None})
    if dupes:
        # Production segment parsing rejects duplicate voucher AlterIDs outright
        # (duplicate_voucher_alter_id_within_segment). A corpus containing them
        # cannot calibrate anything, so it must not be ACCEPTED here either.
        print(f'FAIL: {len(dupes)} duplicate ALTERID value(s), e.g. {dupes[:5]}. '
              'The production parser rejects duplicates within a segment.')
        return 1

    rows = valid

    # Production `compute_outstandings` excludes optional, cancelled and
    # deleted vouchers. If the required New Ref / Agst Ref examples or the
    # ageing spread came only from those, the corpus would be accepted while
    # exercising nothing the product actually computes.
    def posting(x):
        # Production parses these with a REQUIRED boolean parser, so a missing or
        # unrecognised value is a hard error there. Treating anything that is not
        # literally "yes" as posting would let the verifier accept a response the
        # product rejects.
        for k in ('optional', 'cancelled', 'deleted'):
            if x[k].lower() not in ('yes', 'no'):
                raise ValueError(
                    f"voucher has a missing/unrecognised {k} state "
                    f"({x[k]!r}); production's boolean parser rejects this")
        return not any(x[k].lower() == 'yes' for k in ('optional', 'cancelled', 'deleted'))

    try:
        [posting(x) for x in rows]
    except ValueError as exc:
        print(f'FAIL: {exc}')
        return 1
    non_posting = [x for x in rows if not posting(x)]
    rows = [x for x in rows if posting(x)]
    print(f'vouchers parsed: {len(rows)} posting'
          + (f' ({len(non_posting)} non-posting excluded)' if non_posting else ''))
    if not rows:
        print('FAIL: no vouchers. Wrong company name, or nothing created yet.')
        return 1

    ok = True

    # 1. AlterID <-> date ordering  (THE critical one)
    seq = sorted(rows, key=lambda x: int(x['alterid']))
    inv = sum(1 for a, b in zip(seq, seq[1:]) if b['date'] < a['date'])
    # What actually matters is not zero inversions but LOCALITY: does a date window
    # map to a compact AlterID band? A stray adjacent swap is harmless; scattering is fatal.
    from collections import defaultdict
    per_month = defaultdict(list)
    for x in seq:
        per_month[x['date'][:6]].append(int(x['alterid']))
    total_span = int(seq[-1]['alterid']) - int(seq[0]['alterid']) + 1
    worst = max((max(v) - min(v) + 1) / total_span for v in per_month.values()) * 100
    pct = 100.0 * inv / max(1, len(seq) - 1)
    print(f'\n[1] AlterID/date locality: {inv} inversions ({pct:.1f}%), '
          f'worst month spans {worst:.1f}% of the AlterID range')
    if worst <= 40:
        print('    PASS - date windows map to compact AlterID bands; valid for calibration.')
    else:
        print('    FAIL - dates are scattered across the AlterID range, like Aarav.')
        print('    UNFIXABLE without re-entering in ascending date order (guide 2.4b).')
        ok = False

    # 2. Bill references
    # Build from the FILTERED posting vouchers, not the raw response. Earlier
    # this rebuilt from `d`, so the posting-state filter never reached the
    # allocation, ageing or naming checks and an optional/cancelled/deleted
    # voucher could still satisfy them.
    posting_xml = ''.join(x['block'] for x in rows)
    allocs = re.findall(r'<BILLALLOCATIONS\.LIST>(.*?)</BILLALLOCATIONS\.LIST>', posting_xml, re.S)
    from collections import Counter

    bt = Counter()
    named_by_kind = Counter()
    named = 0
    for a in allocs:
        m = re.search(r'<BILLTYPE>([^<]*)', a)
        kind = (m.group(1) if m else 'MISSING') or '(empty)'
        bt[kind] += 1
        n = re.search(r'<NAME>([^<]*)', a)
        if n and n.group(1).strip():
            named += 1
            named_by_kind[kind] += 1
    print(f'\n[2] allocations: {len(allocs)}  named: {named}')
    for k, v in bt.most_common():
        print(f'    {k:<12} {v}')
    if named_by_kind.get('New Ref', 0) and named_by_kind.get('Agst Ref', 0):
        print('    PASS - both New Ref and Agst Ref present WITH names.')
    else:
        # An unnamed allocation carries no bill identity, so a corpus whose only
        # New Ref / Agst Ref examples lack <NAME> cannot exercise the reference
        # lifecycle the outstandings computation is built on. Counting them and
        # only printing the total let such a corpus pass.
        print('    FAIL - need BOTH New Ref and Agst Ref as NAMED references '
              f'(named New Ref: {named_by_kind.get("New Ref", 0)}, '
              f'named Agst Ref: {named_by_kind.get("Agst Ref", 0)}).')
        ok = False

    # 3. Education date legality
    baddays = sorted({x['date'] for x in rows if x['date'][6:8] not in ('01', '02', '31')})
    print(f'\n[3] illegal entry dates (day not 1/2/31): {len(baddays)}')
    if baddays:
        print('    ', baddays[:10])
        ok = False
    else:
        print('    PASS')

    # 4. Ageing spread of OPEN bills
    opened = {}
    for v in (x['block'] for x in rows):
        dm = re.search(r'<DATE[^>]*>(\d{8})<', v)
        for a in re.findall(r'<BILLALLOCATIONS\.LIST>(.*?)</BILLALLOCATIONS\.LIST>', v, re.S):
            nm = re.search(r'<NAME>([^<]*)', a)
            ty = re.search(r'<BILLTYPE>([^<]*)', a)
            am = re.search(r'<AMOUNT>([^<]*)', a)
            if not (nm and ty and nm.group(1).strip()):
                continue
            # Production parses amounts with ExactDecimal: an EMPTY amount is not
            # zero, a missing one is an error, and forms like `1e3` are rejected.
            # `float()` accepts all three, so the verifier could measure buckets
            # from values the product refuses.
            raw_amount = (am.group(1) if am else '').strip()
            if not re.fullmatch(r'-?\d+(\.\d+)?', raw_amount):
                print(f'FAIL: allocation amount {raw_amount!r} is not an exact decimal; '
                      "production's ExactDecimal rejects it.")
                return 1
            key = nm.group(1).strip()
            val = float(raw_amount)
            # Production ages a bill from Tally's own BILLDATE when supplied,
            # falling back to the voucher date. The verifier must use the same
            # date or it measures a different ageing spread than the product.
            bd = re.search(r'<BILLDATE[^>]*>(\d{8})<', a)
            opened_on = bd.group(1) if bd else (dm.group(1) if dm else '')
            if ty.group(1) == 'New Ref':
                prior = opened.get(key)
                # A reference that was fully settled and is then reused starts a
                # NEW bill. Keeping the first cycle's date would age a freshly
                # reopened bill into an older bucket.
                if prior is None or abs(prior['amt']) < 0.01:
                    opened[key] = {'date': opened_on, 'amt': 0.0}
                opened[key]['amt'] += val
            elif ty.group(1) == 'Agst Ref' and key in opened:
                opened[key]['amt'] += val
    # The accepted corpus ends 2026-07-02 and its documented reconciliation
    # target is aged as of 2026-07-31. Ageing against the workstation clock
    # makes acceptance drift with the calendar: from 2026-08-02 the newest open
    # bills leave the 0-30 bucket and the "all four buckets occupied" check
    # fails with no corpus change at all.
    today = RECONCILIATION_AS_OF
    buckets = {'0-30': 0, '31-60': 0, '61-90': 0, '90+': 0}
    for k, v in opened.items():
        if abs(v['amt']) < 0.01 or not v['date']:
            continue
        try:
            dt = datetime.date(int(v['date'][:4]), int(v['date'][4:6]), int(v['date'][6:8]))
        except ValueError:
            continue
        age = (today - dt).days
        if age < 0:
            # production's days_between rejects a bill dated after the report's
            # as-of date; a negative age silently landed such a bill in 0-30 and
            # could supply the final bucket needed for CORPUS ACCEPTED.
            print(f'FAIL: open bill {k!r} is dated {v["date"]}, after the '
                  f'reconciliation as-of {today:%Y%m%d}.')
            return 1
        b = '0-30' if age <= 30 else '31-60' if age <= 60 else '61-90' if age <= 90 else '90+'
        buckets[b] += 1
    print(f'\n[4] open bills by age: {buckets}')
    if all(buckets.values()):
        print('    PASS - all four buckets occupied.')
    else:
        print('    FAIL - every bucket needs at least one open bill.')
        ok = False

    # 5. Size
    print(f'\n[5] voucher count: {len(rows)} (target 200-500)')
    if not 200 <= len(rows) <= 500:
        # A corpus outside the calibration range must not be ACCEPTED: the range
        # is what the documented acceptance depends on, and a warning that
        # leaves `ok` untouched lets a too-small corpus pass on bill types and
        # ageing spread alone.
        print('    FAIL - outside the 200-500 calibration range.')
        ok = False

    print('\n==== ' + ('CORPUS ACCEPTED' if ok else 'CORPUS REJECTED - fix before use') + ' ====')
    return 0 if ok else 1


if __name__ == '__main__':
    sys.exit(main())
