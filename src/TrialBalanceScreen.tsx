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
    read_at: string;
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

const TABLE_PAGE_SIZE = 100;

function toInputDate(value: string) {
  return value.length === 8 ? `${value.slice(0, 4)}-${value.slice(4, 6)}-${value.slice(6)}` : value;
}

function toYyyymmdd(value: string) {
  return value.replace(/-/g, "");
}

export function formatAmount(amount: Amount, symbol: string, decimals: number, magnitude = false) {
  if (amount.state === "present_empty") return "—";
  const sourceNegative = amount.value.startsWith("-");
  const negative = sourceNegative && !magnitude;
  const unsigned = sourceNegative ? amount.value.slice(1) : amount.value;
  const [whole, fraction = ""] = unsigned.split(".");
  const tail = whole.slice(-3);
  const head = whole.slice(0, -3).replace(/\B(?=(\d{2})+(?!\d))/g, ",");
  const grouped = head ? `${head},${tail}` : tail;
  const exactFraction = fraction ? `.${fraction.padEnd(decimals, "0")}` : decimals > 0 ? `.${"0".repeat(decimals)}` : "";
  return `${negative ? "−" : ""}${symbol}${grouped}${exactFraction}`;
}

function formatSum(sum: string, symbol: string, decimals: number) {
  return formatAmount({ state: "present", value: sum }, symbol, decimals);
}

function formatBalance(amount: Amount, symbol: string, decimals: number) {
  if (amount.state === "present_empty") return "—";
  const debit = amount.value.startsWith("-");
  const value = debit ? amount.value.slice(1) : amount.value;
  return `${formatAmount({ state: "present", value }, symbol, decimals, true)}${value === "0" || /^0\.0*$/.test(value) ? "" : debit ? " Dr" : " Cr"}`;
}

function formatInvokeError(cause: unknown) {
  if (cause && typeof cause === "object") {
    const value = cause as { message?: unknown; code?: unknown; remediation?: unknown };
    if (typeof value.message === "string") {
      return [value.message, typeof value.code === "string" ? `[${value.code}]` : "", typeof value.remediation === "string" ? value.remediation : ""].filter(Boolean).join(" ");
    }
  }
  return cause instanceof Error ? cause.message : String(cause);
}

function readScope(company: Company | undefined, config: Props["config"], from: string, to: string) {
  return JSON.stringify([config.host, config.port, company?.name, company?.guid, company?.company_number, company?.books_from_yyyymmdd, company?.canonical_origin, from, to]);
}

export function TrialBalanceScreen({ config, company, liveReadNavigationLocked, liveReadSuppressed, onChangeSetup, onTallyReadActivityChange }: Props) {
  const [from, setFrom] = React.useState(toInputDate(company?.books_from_yyyymmdd ?? ""));
  const [to, setTo] = React.useState("");
  const [captured, setCaptured] = React.useState<{ scope: string; result: TrialBalanceResult } | null>(null);
  const [error, setError] = React.useState<string | null>(null);
  const [exportPath, setExportPath] = React.useState<string | null>(null);
  const [page, setPage] = React.useState(0);
  const [loading, setLoading] = React.useState(false);
  const [exporting, setExporting] = React.useState(false);
  const requestVersion = React.useRef(0);
  const scope = readScope(company, config, from, to);
  const latestScope = React.useRef(scope);
  latestScope.current = scope;

  React.useEffect(() => {
    requestVersion.current += 1;
    setCaptured(null);
    setError(null);
    setExportPath(null);
    setPage(0);
    setLoading(false);
    setExporting(false);
    setFrom(toInputDate(company?.books_from_yyyymmdd ?? ""));
    setTo("");
    return () => {
      requestVersion.current += 1;
    };
  }, [company?.name, company?.guid, company?.company_number, company?.books_from_yyyymmdd, company?.canonical_origin, config.host, config.port]);

  async function refresh() {
    if (!company || loading || exporting || liveReadNavigationLocked || liveReadSuppressed) return;
    if (!from || !to || from > to) {
      setError("Choose a valid date range. The start date must be on or before the end date.");
      setCaptured(null);
      return;
    }
    const version = ++requestVersion.current;
    const requestedScope = readScope(company, config, from, to);
    setLoading(true);
    setError(null);
    setCaptured(null);
    setExportPath(null);
    setPage(0);
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
      if (version === requestVersion.current && requestedScope === latestScope.current) {
        setCaptured({ scope: requestedScope, result: next });
        setPage(0);
      }
    } catch (cause) {
      if (version === requestVersion.current) setError(formatInvokeError(cause));
    } finally {
      if (version === requestVersion.current) setLoading(false);
      onTallyReadActivityChange(-1);
    }
  }

  async function exportReport() {
    if (!captured || exporting || captured.scope !== scope) return;
    const exportingScope = captured.scope;
    const version = requestVersion.current;
    setExporting(true);
    setError(null);
    setExportPath(null);
    onTallyReadActivityChange(1);
    try {
      const path = await invoke<string>("export_tally_trial_balance", { exportId: captured.result.export_id });
      if (version === requestVersion.current && exportingScope === latestScope.current) setExportPath(path);
    } catch (cause) {
      if (version === requestVersion.current && exportingScope === latestScope.current) setError(formatInvokeError(cause));
    } finally {
      if (version === requestVersion.current) setExporting(false);
      onTallyReadActivityChange(-1);
    }
  }

  const result = captured?.scope === scope ? captured.result : null;
  const read = result?.read;
  const currency = read?.currency;
  const totalRows = read?.report.rows.length ?? 0;
  const pageCount = Math.max(1, Math.ceil(totalRows / TABLE_PAGE_SIZE));
  const firstRow = totalRows === 0 ? 0 : page * TABLE_PAGE_SIZE + 1;
  const lastRow = Math.min((page + 1) * TABLE_PAGE_SIZE, totalRows);
  const visibleRows = read?.report.rows.slice(page * TABLE_PAGE_SIZE, (page + 1) * TABLE_PAGE_SIZE) ?? [];
  const disabled = liveReadNavigationLocked || liveReadSuppressed || loading || exporting;

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
          <button className="secondary-action" type="button" onClick={() => void exportReport()} disabled={!result || exporting || captured?.scope !== scope}><Download size={17} aria-hidden="true" />{exporting ? "Exporting…" : "Excel"}</button>
        </div>
      </div>
      <div className="toolbar trial-balance-toolbar">
        <label>From<input type="date" value={from} min={toInputDate(company.books_from_yyyymmdd)} onChange={(event) => { setFrom(event.target.value); setCaptured(null); setError(null); setExportPath(null); }} disabled={disabled} /></label>
        <label>To<input type="date" value={to} onChange={(event) => { setTo(event.target.value); setCaptured(null); setError(null); setExportPath(null); }} disabled={disabled} /></label>
      </div>
      <p className="section-note trial-balance-date-note">
        Choose the end date before reading. This report currently requires Licensed TallyPrime and one observed INR currency master.
      </p>
      <p className="section-note">Preview: validated with small synthetic companies. Compare this report with Tally before relying on it for production work.</p>
      {error && <div className="error-banner" role="alert"><span>{error}</span></div>}
      {exportPath && <div className="trial-balance-success" role="status">Trial Balance export saved to <code>{exportPath}</code></div>}
      {loading && <div className="panel wide trial-balance-loading" role="status">Reading the selected company for the exact date range…</div>}
      {!loading && !result && !error && <div className="panel wide trial-balance-empty"><p>Refresh to read the native report for this company and date range.</p></div>}
      {read && currency && (
        <div className="panel wide trial-balance-report">
          <div className="trial-balance-meta"><span>{read.company_name}</span><span>{toInputDate(read.from)} → {toInputDate(read.to)}</span><span>Fresh at {new Date(read.read_at).toLocaleString()}</span><span>Source scope: {read.report.rows.length} ledger rows</span></div>
          <dl className="trial-balance-totals">
            <div><dt>{read.totals.opening.empty_count === 0 ? "Difference in opening balances" : "Observed opening net"}</dt><dd>{formatSum(read.totals.opening.sum, currency.symbol, currency.decimal_places)}{read.totals.opening.empty_count ? ` · ${read.totals.opening.empty_count} empty source values` : ""}</dd></div>
            <div><dt>Debit total</dt><dd>{formatAmount({ state: "present", value: read.totals.debit.sum }, currency.symbol, currency.decimal_places, true)}{read.totals.debit.empty_count ? ` · ${read.totals.debit.empty_count} empty` : ""}</dd></div>
            <div><dt>Credit total</dt><dd>{formatAmount({ state: "present", value: read.totals.credit.sum }, currency.symbol, currency.decimal_places, true)}{read.totals.credit.empty_count ? ` · ${read.totals.credit.empty_count} empty` : ""}</dd></div>
            <div><dt>Closing total</dt><dd>{formatSum(read.totals.closing.sum, currency.symbol, currency.decimal_places)}{read.totals.closing.empty_count ? ` · ${read.totals.closing.empty_count} empty source values` : ""}</dd></div>
          </dl>
          <div className="trial-balance-table-wrap">
            <table className="trial-balance-table"><caption className="visually-hidden">Trial Balance ledger totals</caption><thead><tr><th scope="col">Ledger</th><th scope="col">Opening</th><th scope="col">Debit (Dr)</th><th scope="col">Credit (Cr)</th><th scope="col">Closing</th></tr></thead><tbody>{visibleRows.map((row) => <tr key={row.guid}><th scope="row">{row.name}</th><td>{formatBalance(row.opening, currency.symbol, currency.decimal_places)}</td><td>{formatAmount(row.debit, currency.symbol, currency.decimal_places, true)}</td><td>{formatAmount(row.credit, currency.symbol, currency.decimal_places, true)}</td><td>{formatBalance(row.closing, currency.symbol, currency.decimal_places)}</td></tr>)}</tbody></table>
          </div>
          <div className="trial-balance-pagination" aria-label="Trial Balance rows">
            <span>Rows {firstRow}–{lastRow} of {totalRows}</span>
            <div><button className="secondary-action" type="button" onClick={() => setPage((current) => Math.max(0, current - 1))} disabled={page === 0}>Previous</button><button className="secondary-action" type="button" onClick={() => setPage((current) => Math.min(pageCount - 1, current + 1))} disabled={page >= pageCount - 1}>Next</button></div>
          </div>
          <p className="section-note">Currency: {currency.mailing_name || currency.symbol}. This native read is tied to the selected company and date range; it may include dormant ledger masters and is not an atomic snapshot of concurrent Tally changes.</p>
        </div>
      )}
    </section>
  );
}
