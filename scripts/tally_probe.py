"""Hard-failing Tally probe harness.

OPERATOR TOOL — contacts a LIVE Tally instance on 127.0.0.1:9000.
It must never be imported or invoked from an automated test; the repository
rule is that no test contacts a live Tally, a government portal, or any
external provider. This exists for hand-run lab probing only.


Distinguishes, explicitly and loudly:
  NO_RESPONSE  - connection failed / timed out / no bytes  (harness or transport fault)
  BAD_SHAPE    - bytes arrived but not a recognisable Tally response
  OK           - a genuine Tally response, which may legitimately contain 0 rows
Never lets a failed connection masquerade as an empty result.
"""
import os,re,subprocess,tempfile,time
CO='Aarav Trading Company Demo'
OUTDIR=os.environ.get('TALLY_PROBE_OUTDIR') or os.path.join(tempfile.gettempdir(),'tally-probe')
os.makedirs(OUTDIR,exist_ok=True)
class NoResponse(Exception): pass
class BadShape(Exception): pass
class Resp:
    def __init__(self,body,elapsed,nbytes):
        self.body=body; self.elapsed=elapsed; self.nbytes=nbytes
    @property
    def status(self):
        m=re.search(r'<STATUS>\s*(\d+)',self.body); return m.group(1) if m else None
    @property
    def data(self):
        """Only the DATA section - CMPINFO contains bare <LEDGER>0</LEDGER> style
        counters that inflate naive tag counts."""
        m=re.search(r'<DATA>(.*)</DATA>',self.body,re.S)
        return m.group(1) if m else ''
    def count(self,tag):
        """Count real object rows: opening tag WITH attributes, inside DATA only."""
        return len(re.findall(r'<%s '%tag,self.data))
    def dates(self):
        return sorted(set(re.findall(r'<DATE[^>]*>(\d{8})<',self.data)))
    def counters(self):
        out={}
        for k in ('CREATED','ALTERED','DELETED','CANCELLED','IGNORED','ERRORS','EXCEPTIONS','LASTVCHID','LASTMID'):
            m=re.search(r'<%s>\s*(-?\d+)'%k,self.body); out[k]=int(m.group(1)) if m else None
        out['LINEERROR']=re.findall(r'<LINEERROR>([^<]*)',self.body)
        return out
def post(xml,timeout=300,tag='q'):
    p=os.path.join(OUTDIR,'%s.out'%tag)
    if os.path.exists(p): os.remove(p)
    t0=time.time()
    done=subprocess.run(['curl','-s','--max-time',str(timeout),'-o',p,'-X','POST',
        'http://127.0.0.1:9000','-H','Content-Type: text/xml; charset=utf-8',
        '--data-binary','@-'],input=xml.encode('utf-8'),capture_output=True)
    el=time.time()-t0
    # A timed-out or failed curl can still leave a PARTIAL file on disk, and a
    # truncated capture that happens to contain '<ENVELOPE' would otherwise be
    # read as a valid response - the corpus verifier would then reason from
    # missing rows instead of reporting transport failure. Transport failure is
    # NoResponse regardless of what bytes were written.
    if done.returncode!=0:
        raise NoResponse('curl exited %d after %.1fs (%s)'%(
            done.returncode,el,done.stderr.decode('utf-8',errors='replace').strip()[:200] or 'no stderr'))
    if not os.path.exists(p): raise NoResponse('no output file after %.1fs (connection failed or timed out)'%el)
    raw=open(p,'rb').read()
    if not raw: raise NoResponse('zero bytes after %.1fs'%el)
    body=raw.decode('utf-8',errors='replace')
    if 'Unknown Request' in body: raise BadShape('Unknown Request, cannot be processed')
    if not re.search(r'<(ENVELOPE|RESPONSE)',body): raise BadShape('unrecognised: %r'%body[:80])
    return Resp(body,el,len(raw))
def alive():
    try:
        r=subprocess.run(['curl','-s','--max-time','15','http://127.0.0.1:9000/status'],capture_output=True)
        return b'Running' in r.stdout
    except Exception: return False
def require_alive():
    if not alive(): raise NoResponse('gateway not alive at /status - aborting rather than recording false zeros')
def collection(typ,fetch,frm=None,to=None,filt=None,company=CO,idn='H'):
    sv='<SVCURRENTCOMPANY>%s</SVCURRENTCOMPANY>'%company if company else ''
    if frm: sv+='<SVFROMDATE TYPE="Date">%s</SVFROMDATE><SVTODATE TYPE="Date">%s</SVTODATE>'%(frm,to)
    sysd=fil=''
    if filt: sysd='<SYSTEM TYPE="Formulae" NAME="HF">%s</SYSTEM>'%filt; fil='<FILTERS>HF</FILTERS>'
    return ('<ENVELOPE><HEADER><VERSION>1</VERSION><TALLYREQUEST>Export</TALLYREQUEST><TYPE>Collection</TYPE><ID>%s</ID></HEADER>'
     '<BODY><DESC><STATICVARIABLES><SVEXPORTFORMAT>$$SysName:XML</SVEXPORTFORMAT>%s</STATICVARIABLES><TDL><TDLMESSAGE>%s'
     '<COLLECTION NAME="%s" ISMODIFY="No"><TYPE>%s</TYPE><FETCH>%s</FETCH>%s</COLLECTION>'
     '</TDLMESSAGE></TDL></DESC></BODY></ENVELOPE>')%(idn,sv,sysd,idn,typ,fetch,fil)
