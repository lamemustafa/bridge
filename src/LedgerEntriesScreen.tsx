import React from "react";
import { Search } from "lucide-react";
import { invoke } from "@tauri-apps/api/core";

type Company = { name: string; guid: string; company_number: string; books_from_yyyymmdd: string; canonical_origin: string };
type Entry = { ledger: string; amount: string; is_deemed_positive: "Yes" | "No" };
type Voucher = { date: string; voucher_number?: string | null; voucher_type: string; party?: string | null; narration?: string | null; guid?: string | null; alter_id?: number | null; master_id?: string | null; amounts: Entry[]; cancelled: boolean; optional: boolean };
type ReadEvidence = { state: "complete" | "partial"; reason_code?: string | null; read_at?: string | null; duration_ms?: number | null; bytes: number };
type Result = { state: "complete" | "partial"; reason?: string | null; items: Voucher[]; offset: number; total: number; profile: string };
type Response = { company: unknown; read_at: string; evidence: ReadEvidence; truncated: boolean; result: Result };
type ObservationScope = { companyName: string; ledger: string; from: string; to: string };

export function LedgerEntriesScreen({ config, company, locked, onReadActivity }: {
  config: { host: string; port: number }; company?: Company; locked: boolean; onReadActivity: (delta: 1 | -1) => void;
}) {
  const [ledger, setLedger] = React.useState("");
  const [from, setFrom] = React.useState("");
  const [to, setTo] = React.useState("");
  const [response, setResponse] = React.useState<Response | null>(null);
  const [observationScope, setObservationScope] = React.useState<ObservationScope | null>(null);
  const [error, setError] = React.useState<string | null>(null);
  const [loading, setLoading] = React.useState(false);
  const requestVersion = React.useRef(0);
  const scopeKey = company ? [config.host, config.port, company.name, company.guid, company.company_number, company.books_from_yyyymmdd, company.canonical_origin, ledger, from, to].join("\u0000") : "unselected";
  const latestScope = React.useRef(scopeKey);
  latestScope.current = scopeKey;

  React.useEffect(() => {
    requestVersion.current += 1;
    setResponse(null);
    setObservationScope(null);
    setError(null);
    setLoading(false);
    return () => { requestVersion.current += 1; };
  }, [scopeKey]);

  async function investigate(offset = 0) {
    if (!company || !ledger.trim() || !from || !to) return;
    const version = ++requestVersion.current;
    const submittedScope = scopeKey;
    setLoading(true); setError(null); onReadActivity(1);
    try {
      const next = await invoke<Response>("fetch_selected_ledger_entries", { request: {
        config, selected_company: { display_name: company.name, company_guid: company.guid, company_number: company.company_number, books_from_yyyymmdd: company.books_from_yyyymmdd }, ledger: ledger.trim(), from: from.replace(/-/g, ""), to: to.replace(/-/g, ""), offset, limit: 100,
      }});
      if (version === requestVersion.current && submittedScope === latestScope.current) {
        setResponse(next);
        setObservationScope({ companyName: company.name, ledger: ledger.trim(), from, to });
      }
    } catch (cause) {
      if (version === requestVersion.current && submittedScope === latestScope.current) setError(operatorMessage(cause));
    } finally {
      onReadActivity(-1);
      if (version === requestVersion.current && submittedScope === latestScope.current) setLoading(false);
    }
  }
  if (!company) return <section className="panel wide"><h2>Select a verified Tally company</h2><p>Ledger investigation is available after Bridge has observed the company name, number, GUID, and books-from date.</p></section>;
  const result = response?.result;
  const sourceComplete = response?.evidence.state === "complete" && result?.state === "complete";
  return <section className="panel wide ledger-investigation">
    <div className="panel-heading"><div><h2>Investigate ledger entries</h2><p>Choose one ledger and a date range. Bridge reads the complete requested window before filtering; page controls only limit what is displayed.</p></div></div>
    <form className="ledger-investigation-form" onSubmit={(event) => { event.preventDefault(); void investigate(); }}>
      <label>Ledger<input required value={ledger} onChange={(event) => setLedger(event.target.value)} placeholder="Exact ledger name" disabled={loading || locked} /></label>
      <label>From<input required type="date" value={from} onChange={(event) => setFrom(event.target.value)} disabled={loading || locked} /></label>
      <label>To<input required type="date" value={to} onChange={(event) => setTo(event.target.value)} disabled={loading || locked} /></label>
      <button className="primary" type="submit" disabled={loading || locked || !ledger.trim() || !from || !to}><Search size={18} />{loading ? "Reading entries…" : "Show entries"}</button>
    </form>
    {error && <p className="ledger-investigation-error" role="alert">{error}</p>}
    {result && <>
      <p className="ledger-investigation-provenance">Observed {formatObservedAt(response.read_at)} for {observationScope?.companyName}; requested ledger {observationScope?.ledger}, {formatDateInput(observationScope?.from)} to {formatDateInput(observationScope?.to)}.</p>
      <p className="ledger-investigation-scope" role="status">{sourceComplete ? `${result.total} matching voucher${result.total === 1 ? "" : "s"} in the complete selected date window.` : `Bridge could not establish a complete source for this selected date window${response?.evidence.reason_code ? ` (${response.evidence.reason_code})` : ""}.`}</p>
      {sourceComplete && result.items.length === 0 && <p>No vouchers in the complete source matched this ledger.</p>}
      {result.items.length > 0 && <div className="ledger-entry-list">{result.items.map((voucher, index) => <details key={voucher.guid ?? `${voucher.date}-${voucher.voucher_number ?? index}`}><summary><span>{formatDate(voucher.date)}</span><strong>{voucher.voucher_type}{voucher.voucher_number ? ` · ${voucher.voucher_number}` : ""}</strong><span>{voucher.party ?? "No party"}</span></summary><div className="ledger-entry-detail">{voucher.narration && <p>{voucher.narration}</p>}<p className="ledger-entry-lines-heading">Voucher entries and counterpart lines</p><dl>{voucher.amounts.map((entry, entryIndex) => <div key={`${entry.ledger}-${entryIndex}`}><dt>{entry.ledger}</dt><dd>{entry.amount} · {entry.is_deemed_positive === "Yes" ? "debit-side" : "credit-side"}</dd></div>)}</dl>{(voucher.cancelled || voucher.optional) && <p>Accounting state: {voucher.cancelled ? "cancelled" : "optional"}.</p>}</div></details>)}</div>}
      {response?.truncated && <p className="ledger-investigation-scope">This observation is display-bounded. “Show next entries” makes a new read of the same selected window; Bridge does not combine pages from different observations.</p>}
      {result.offset + result.items.length < result.total && <button type="button" className="secondary-action" disabled={loading || locked} onClick={() => void investigate(result.offset + result.items.length)}>Show next entries</button>}
    </>}
  </section>;
}
function formatDate(value: string) { return value.length === 8 ? `${value.slice(6, 8)}-${value.slice(4, 6)}-${value.slice(0, 4)}` : value; }
function formatDateInput(value?: string) { return value || "unavailable"; }
function formatObservedAt(value: string) { const date = new Date(value); return Number.isNaN(date.getTime()) ? "an unavailable time" : date.toLocaleString(); }
function operatorMessage(cause: unknown) {
  if (typeof cause === "string") return cause;
  if (cause && typeof cause === "object" && "message" in cause && typeof cause.message === "string") {
    const remediation = "remediation" in cause && typeof cause.remediation === "string" ? cause.remediation : "";
    return [cause.message, remediation].filter(Boolean).join(" ");
  }
  return "Bridge could not complete this ledger investigation.";
}
