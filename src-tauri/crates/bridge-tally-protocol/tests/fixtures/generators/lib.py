import sys, os, re, datetime, urllib.request
# `post` came from a session-local probe helper that is not part of this repository.
# This file is committed as the RECORD of what was sent, not as a runnable script.
# It performs one POST of `xml` to the gateway and returns (text, bytes, seconds, _).
from tq import post  # noqa: F401  -- see note above

def status():
    try:
        with urllib.request.urlopen("http://localhost:9001/status", timeout=12) as r:
            return r.read().decode('utf-8','replace').strip()
    except Exception as e:
        return f"UNHEALTHY: {e}"

def send(xml, label=""):
    """One import, health-checked both sides. Never silent."""
    pre = status()
    if "Running" not in pre:
        raise SystemExit(f"ABORT before {label}: {pre}")
    text, n, dt, _ = post(xml, timeout=900)
    post_ = status()
    c = {t: (re.search(rf"<{t}>(.*?)</{t}>", text).group(1)
             if re.search(rf"<{t}>(.*?)</{t}>", text) else "?")
         for t in ("CREATED","ALTERED","ERRORS","EXCEPTIONS")}
    print(f"  {label:<28} created={c['CREATED']:>5} errors={c['ERRORS']} exceptions={c['EXCEPTIONS']}"
          f"  {n}B {dt:.1f}s")
    if "Running" not in post_:
        raise SystemExit(f"ABORT after {label}: {post_}")
    for tag in ("LINEERROR","DESC"):
        for m in re.finditer(rf"<{tag}>(.*?)</{tag}>", text, re.S):
            print(f"     {tag}: {m.group(1)[:200]}")
    return c

def envelope(company, report, body):
    return ('<ENVELOPE><HEADER><TALLYREQUEST>Import Data</TALLYREQUEST></HEADER><BODY><IMPORTDATA>'
            f'<REQUESTDESC><REPORTNAME>{report}</REPORTNAME><STATICVARIABLES>'
            f'<SVCURRENTCOMPANY>{company}</SVCURRENTCOMPANY></STATICVARIABLES></REQUESTDESC>'
            f'<REQUESTDATA><TALLYMESSAGE xmlns:UDF="TallyUDF">{body}</TALLYMESSAGE></REQUESTDATA>'
            '</IMPORTDATA></BODY></ENVELOPE>')

def ledger(name, parent, billwise=True, opening=None):
    ob = f'<OPENINGBALANCE>{opening}</OPENINGBALANCE>' if opening is not None else ''
    return (f'<LEDGER NAME="{name}" ACTION="Create"><NAME.LIST><NAME>{name}</NAME></NAME.LIST>'
            f'<PARENT>{parent}</PARENT><ISBILLWISEON>{"Yes" if billwise else "No"}</ISBILLWISEON>{ob}</LEDGER>')

def voucher(kind, date, entries, narration):
    """date: datetime.date. EFFECTIVEDATE is mandatory -- without it Tally
    silently discards BILLCREDITPERIOD and every bill ages from its bill date."""
    ds = date.strftime("%Y%m%d")
    return (f'<VOUCHER VCHTYPE="{kind}" ACTION="Create" OBJVIEW="Accounting Voucher View">'
            f'<DATE>{ds}</DATE><EFFECTIVEDATE>{ds}</EFFECTIVEDATE>'
            f'<NARRATION>{narration}</NARRATION><VOUCHERTYPENAME>{kind}</VOUCHERTYPENAME>'
            + "".join(entries) + '</VOUCHER>')

def entry(led, debit, amount, bill=None, billtype=None, billdate=None, credit_period=None):
    """debit=True -> ISDEEMEDPOSITIVE Yes and a negative AMOUNT, Tally's convention."""
    amt = -abs(amount) if debit else abs(amount)
    b = ""
    if billtype:
        parts = []
        if bill: parts.append(f'<NAME>{bill}</NAME>')
        parts.append(f'<BILLTYPE>{billtype}</BILLTYPE>')
        if billdate: parts.append(f'<BILLDATE>{billdate.strftime("%Y%m%d")}</BILLDATE>')
        if credit_period: parts.append(f'<BILLCREDITPERIOD>{credit_period}</BILLCREDITPERIOD>')
        parts.append(f'<AMOUNT>{amt:.2f}</AMOUNT>')
        b = '<BILLALLOCATIONS.LIST>' + "".join(parts) + '</BILLALLOCATIONS.LIST>'
    return (f'<ALLLEDGERENTRIES.LIST><LEDGERNAME>{led}</LEDGERNAME>'
            f'<ISDEEMEDPOSITIVE>{"Yes" if debit else "No"}</ISDEEMEDPOSITIVE>'
            f'<AMOUNT>{amt:.2f}</AMOUNT>{b}</ALLLEDGERENTRIES.LIST>')

def batches(items, size=400):
    for i in range(0, len(items), size):
        yield items[i:i+size]
