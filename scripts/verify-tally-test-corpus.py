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
RECONCILIATION_AS_OF = datetime.date(2026, 7, 31)


CO = sys.argv[1] if len(sys.argv) > 1 else 'Bridge Billwise Lab'
FETCH = ('GUID, MASTERID, ALTERID, DATE, VOUCHERTYPENAME, VOUCHERNUMBER, '
         'PARTYLEDGERNAME, ISCANCELLED, ISDELETED, ALLLEDGERENTRIES.*')


def req(frm, to):
    return ('<ENVELOPE><HEADER><VERSION>1</VERSION><TALLYREQUEST>Export</TALLYREQUEST>'
            '<TYPE>Collection</TYPE><ID>VC</ID></HEADER><BODY><DESC><STATICVARIABLES>'
            '<SVEXPORTFORMAT>$$SysName:XML</SVEXPORTFORMAT>'
            f'<SVCURRENTCOMPANY>{CO}</SVCURRENTCOMPANY>'
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
                     'vtype': g('VOUCHERTYPENAME'), 'block': v})
    rows = [x for x in rows if x['alterid'].isdigit() and len(x['date']) == 8]
    print(f'vouchers parsed: {len(rows)}')
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
    allocs = re.findall(r'<BILLALLOCATIONS\.LIST>(.*?)</BILLALLOCATIONS\.LIST>', d, re.S)
    from collections import Counter

    bt = Counter()
    named = 0
    for a in allocs:
        m = re.search(r'<BILLTYPE>([^<]*)', a)
        bt[(m.group(1) if m else 'MISSING') or '(empty)'] += 1
        n = re.search(r'<NAME>([^<]*)', a)
        if n and n.group(1).strip():
            named += 1
    print(f'\n[2] allocations: {len(allocs)}  named: {named}')
    for k, v in bt.most_common():
        print(f'    {k:<12} {v}')
    if bt.get('New Ref', 0) and bt.get('Agst Ref', 0):
        print('    PASS - both New Ref and Agst Ref present.')
    else:
        print('    FAIL - need BOTH New Ref (opens a bill) and Agst Ref (settles one).')
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
    for v in vs:
        dm = re.search(r'<DATE[^>]*>(\d{8})<', v)
        for a in re.findall(r'<BILLALLOCATIONS\.LIST>(.*?)</BILLALLOCATIONS\.LIST>', v, re.S):
            nm = re.search(r'<NAME>([^<]*)', a)
            ty = re.search(r'<BILLTYPE>([^<]*)', a)
            am = re.search(r'<AMOUNT>([^<]*)', a)
            if not (nm and ty and am and nm.group(1).strip()):
                continue
            key = nm.group(1).strip()
            val = float(am.group(1) or 0)
            if ty.group(1) == 'New Ref':
                opened.setdefault(key, {'date': dm.group(1) if dm else '', 'amt': 0.0})
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
        print('    WARN - outside target range.')

    print('\n==== ' + ('CORPUS ACCEPTED' if ok else 'CORPUS REJECTED - fix before use') + ' ====')
    return 0 if ok else 1


if __name__ == '__main__':
    sys.exit(main())
