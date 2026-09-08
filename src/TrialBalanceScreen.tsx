import React from "react";
import { Download, RefreshCw } from "lucide-react";
import { invoke } from "@tauri-apps/api/core";

type Company = {
  name: string;
  guid: string;
  company_number: string;
  books_from_yyyymmdd: string;
  canonical_origin: string;
};

type Amount = { state: "present"; value: string } | { state: "present_empty" };

type TrialBalanceResult = {
  read: {
    company_guid: string;
    company_name: string;
    from: string;
    to: string;
    currency: { symbol: string; mailing_name: string; currency_count: number; decimal_places: number; is_inr: boolean };
    report: { rows: Array<{ name: string; guid: string; opening: Amount; debit: Amount; credit: Amount; closing: Amount }> };
    totals: { opening: { sum: string; empty_count: number }; debit: { sum: string; empty_count: number }; credit: { sum: string; empty_count: number }; closing: { sum: string; empty_count: number } };
    read_at: number;
    evidence: { request_sha256: string; response_sha256: string; bytes: number };
  };
  export_id: string;
};

type Props = {
  config: { host: string; port: number };
  company?: Company;
  liveReadNavigationLocked: boolean;
  liveReadSuppressed: boolean;
  onChangeSetup: () => void;
  onTallyReadActivityChange: (delta: 1 | -1) => void;
};

function toInputDate(value: string) {
  return value.length === 8 ? `${value.slice(0, 4)}-${value.slice(4, 6)}-${value.slice(6)}` : value;
}

function toYyyymmdd(value: string) {
  return value.replace(/-/g, "");
}

function todayInputDate() {
  const now = new Date();
  return `${now.getFullYear()}-${String(now.getMonth() + 1).padStart(2, "0")}-${String(now.getDate()).padStart(2, "0")}`;
}

function formatAmount(amount: Amount, symbol: string, decimals: number) {
  if (amount.state === "present_empty") return "—";
  const negative = amount.value.startsWith("-");
  const unsigned = negative ? amount.value.slice(1) : amount.value;
  const [whole, fraction = ""] = unsigned.split(".");
  const grouped = whole.replace(/\B(?=(\d{3})+(?!\d))/g, ",");
  const exactFraction = decimals > 0 ? `.${fraction.padEnd(decimals, "0")}` : "";
  return `${negative ? "−" : ""}${symbol}${grouped}${exactFraction}`;
}

function formatSum(sum: string, symbol: string, decimals: number) {
  return formatAmount({ state: "present", value: sum }, symbol, decimals);
}

function readScope(company: Company | undefined, config: Props["config"], from: string, to: string) {
  return `${config.host}:${config.port}|${company?.name ?? ""}|${company?.guid ?? ""}|${company?.company_number ?? ""}|${company?.books_from_yyyymmdd ?? ""}|${company?.canonical_origin ?? ""}|${from}|${to}`;
}

export function TrialBalanceScreen({ config, company, liveReadNavigationLocked, liveReadSuppressed, onChangeSetup, onTallyReadActivityChange }: Props) {
  const initialFrom = toInputDate(company?.books_from_yyyymmdd ?? "");
  const [from, setFrom] = React.useState(initialFrom);
  const [to, setTo] = React.useState(todayInputDate());
  const [result, setResult] = React.useState<TrialBalanceResult | null>(null);
  const [error, setError] = React.useState<string | null>(null);
  const [loading, setLoading] = React.useState(false);
  const [exporting, setExporting] = React.useState(false);
  const requestVersion = React.useRef(0);
  const scope = readScope(company, config, from, to);

  React.useEffect(() => {
    requestVersion.current += 1;
    setResult(null);
    setError(null);
    setLoading(false);
    setExporting(false);
    if (company) {
      setFrom(toInputDate(company.books_from_yyyymmdd));
      setTo(todayInputDate());
    }
  }, [company?.name, company?.guid, company?.company_number, company?.books_from_yyyymmdd, company?.canonical_origin, config.host, config.port]);

  async function refresh() {
    if (!company || loading || liveReadNavigationLocked || liveReadSuppressed) return;
    if (!from || !to || from > to) {
      setError("Choose a valid date range. The start date must be on or before the end date.");
      setResult(null);
      return;
    }
    const version = ++requestVersion.current;
    const requestedScope = readScope(company, config, from, to);
    setLoading(true);
    setError(null);
    setResult(null);
    onTallyReadActivityChange(1);
    try {
      const next = await invoke<TrialBalanceResult>("fetch_tally_trial_balance", {
        request: {
          config,
          selected_company: {
            display_name: company.name,
            company_guid: company.guid,
            company_number: company.company_number,
            books_from_yyyymmdd: company.books_from_yyyymmdd,
          },
          from: toYyyymmdd(from),
          to: toYyyymmdd(to),
        },
      });
      if (version === requestVersion.current && requestedScope === readScope(company, config, from, to)) setResult(next);
    } catch (cause) {
      if (version === requestVersion.current) setError(cause instanceof Error ? cause.message : String(cause));
    } finally {
      if (version === requestVersion.current) setLoading(false);
      onTallyReadActivityChange(-1);
    }
  }

  async function exportReport() {
    if (!result || exporting || scope !== readScope(company, config, from, to)) return;
    setExporting(true);
    setError(null);
    try {
      const path = await invoke<string>("export_tally_trial_balance", { exportId: result.export_id });
      setError(`Trial Balance export saved to ${path}`);
    } catch (cause) {
      setError(cause instanceof Error ? cause.message : String(cause));
    } finally {
      setExporting(false);
    }
  }

  const read = result?.read;
  const currency = read?.currency;
  const disabled = liveReadNavigationLocked || liveReadSuppressed || loading;

  if (!company) {
    return <section className="panel wide trial-balance-empty"><h2>Trial Balance</h2><p>Select and verify a Tally company before reading its report.</p><button className="secondary-action" type="button" onClick={onChangeSetup}>Choose company</button></section>;
  }

  return (
    <section className="trial-balance" aria-labelledby="trial-balance-heading">
      <div className="panel wide trial-balance-hero">
        <div>
          <h2 id="trial-balance-heading">Trial Balance</h2>
          <p className="panel-description">Native ledger totals for {company.name}. Empty source amounts remain empty; they are not treated as zero.</p>
        </div>
        <div className="trial-balance-actions">
          <button className="primary" type="button" onClick={() => void refresh()} disabled={disabled}><RefreshCw size={17} aria-hidden="true" />{loading ? "Reading…" : "Refresh report"}</button>
          <button className="secondary-action" type="button" onClick={() => void exportReport()} disabled={!result || exporting || scope !== readScope(company, config, from, to)}><Download size={17} aria-hidden="true" />{exporting ? "Exporting…" : "Excel"}</button>
        </div>
      </div>
      <div className="toolbar trial-balance-toolbar">
        <label>From<input type="date" value={from} min={toInputDate(company.books_from_yyyymmdd)} onChange={(event) => { setFrom(event.target.value); setResult(null); }} disabled={disabled} /></label>
        <label>To<input type="date" value={to} onChange={(event) => { setTo(event.target.value); setResult(null); }} disabled={disabled} /></label>
      </div>
      {error && <div className={`error-banner${result ? " trial-balance-export-status" : ""}`} role={result ? "status" : "alert"}><span>{error}</span></div>}
      {loading && <div className="panel wide trial-balance-loading" role="status">Reading the selected company for the exact date range…</div>}
      {!loading && !result && !error && <div className="panel wide trial-balance-empty"><p>Refresh to read the native report for this company and date range.</p></div>}
      {read && currency && (
        <div className="panel wide trial-balance-report">
          <div className="trial-balance-meta"><span>{read.company_name}</span><span>{read.from} → {read.to}</span><span>Fresh at {new Date(read.read_at).toLocaleString()}</span><span>Source scope: {read.report.rows.length} ledger rows</span></div>
          <dl className="trial-balance-totals">
            <div><dt>Difference in opening balances</dt><dd>{read.totals.opening.empty_count === 0 ? formatSum(read.totals.opening.sum, currency.symbol, currency.decimal_places) : `${formatSum(read.totals.opening.sum, currency.symbol, currency.decimal_places)} · ${read.totals.opening.empty_count} empty source values`}</dd></div>
            <div><dt>Debit total</dt><dd>{formatSum(read.totals.debit.sum, currency.symbol, currency.decimal_places)}{read.totals.debit.empty_count ? ` · ${read.totals.debit.empty_count} empty` : ""}</dd></div>
            <div><dt>Credit total</dt><dd>{formatSum(read.totals.credit.sum, currency.symbol, currency.decimal_places)}{read.totals.credit.empty_count ? ` · ${read.totals.credit.empty_count} empty` : ""}</dd></div>
          </dl>
          <div className="trial-balance-table-wrap">
            <table className="trial-balance-table"><caption className="visually-hidden">Trial Balance ledger totals</caption><thead><tr><th scope="col">Ledger</th><th scope="col">Opening</th><th scope="col">Debit (Dr)</th><th scope="col">Credit (Cr)</th><th scope="col">Closing</th></tr></thead><tbody>{read.report.rows.map((row) => <tr key={row.guid}><th scope="row">{row.name}</th><td>{formatAmount(row.opening, currency.symbol, currency.decimal_places)}</td><td>{formatAmount(row.debit, currency.symbol, currency.decimal_places)}</td><td>{formatAmount(row.credit, currency.symbol, currency.decimal_places)}</td><td>{formatAmount(row.closing, currency.symbol, currency.decimal_places)}</td></tr>)}</tbody></table>
          </div>
          <p className="section-note">Currency: {currency.mailing_name || currency.symbol}. This native read is tied to the selected company and date range; it may include dormant ledger masters and is not an atomic snapshot of concurrent Tally changes.</p>
        </div>
      )}
    </section>
  );
}
