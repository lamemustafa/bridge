import React from "react";
import { RefreshCw } from "lucide-react";
import { invoke } from "@tauri-apps/api/core";
import { outstandingsPartialReason } from "./outstandings-copy";
import { canStartOutstandingsRead } from "./outstandings-currency";

type Props = {
  config: { host: string; port: number };
  company?: { name: string; guid: string };
  onChangeSetup: () => void;
};

type Report = {
  company_name: string;
  as_of_yyyymmdd: string;
  receivable_total: string;
  payable_total: string;
  ageing: {
    days_0_30: string;
    days_31_60: string;
    days_61_90: string;
    days_90_plus: string;
  };
  open_receivable_bill_count: number;
  ageing_bill_counts: {
    days_0_30: number;
    days_31_60: number;
    days_61_90: number;
    days_90_plus: number;
  };
  top_parties: Array<{
    party: string;
    receivable: string;
    payable: string;
    outstanding_total: string;
    oldest_bill_age_days: number | null;
  }>;
  source_voucher_count: number;
  source_bytes: number;
};

type LoadResult =
  | { state: "complete"; report: Report; currency_assertion: string; synced_at_unix_ms: number }
  | { state: "partial"; reason_code: string; synced_at_unix_ms: number };

type InrCompleteResult = Extract<LoadResult, { state: "complete" }> & { currency_assertion: "INR" };

export function OutstandingsScreen({ config, company, onChangeSetup }: Props) {
  const [result, setResult] = React.useState<LoadResult | null>(null);
  const [error, setError] = React.useState<string | null>(null);
  const [loading, setLoading] = React.useState(false);
  const [inrAssertedCompanyGuid, setInrAssertedCompanyGuid] = React.useState<string | null>(null);
  const [, refreshClock] = React.useReducer((value) => value + 1, 0);
  const requestVersion = React.useRef(0);

  React.useEffect(() => {
    const timer = window.setInterval(refreshClock, 30_000);
    return () => window.clearInterval(timer);
  }, []);

  React.useEffect(() => {
    requestVersion.current += 1;
    setResult(null);
    setError(null);
    setLoading(false);
    setInrAssertedCompanyGuid(null);
  }, [config.host, config.port, company?.guid, company?.name]);

  const readPermitted = canStartOutstandingsRead(company, inrAssertedCompanyGuid);

  const load = React.useCallback(async () => {
    if (!readPermitted || !company) return;
    const version = requestVersion.current + 1;
    requestVersion.current = version;
    setLoading(true);
    setError(null);
    try {
      const next = await invoke<LoadResult>("fetch_tally_outstandings", {
        request: {
          config,
          company: company.name,
          expected_company_guid: company.guid,
          currency_assertion: "INR",
        },
      });
      if (requestVersion.current !== version) return;
      setResult(next);
    } catch (cause) {
      if (requestVersion.current !== version) return;
      setResult(null);
      setError(operatorMessage(cause));
    } finally {
      if (requestVersion.current === version) setLoading(false);
    }
  }, [config.host, config.port, company?.guid, company?.name, readPermitted]);

  React.useEffect(() => {
    if (readPermitted) void load();
    // The screen is mounted only when the operator opens Outstandings.
  }, [load, readPermitted]);

  if (!company) {
    return (
      <section className="panel wide outstandings-empty">
        <h2>Select a verified Tally company</h2>
        <p>Outstandings require a persisted company name and observed GUID before any voucher read can start.</p>
        <button type="button" onClick={onChangeSetup}>Open Tally setup</button>
      </section>
    );
  }

  if (!readPermitted) {
    return (
      <section className="panel wide outstandings-empty">
        <h2>Confirm the company base currency</h2>
        <p>Bridge cannot read a company’s base currency from the sealed Unit A export. Outstandings are available only after you explicitly confirm that the selected company uses INR.</p>
        <button type="button" onClick={() => setInrAssertedCompanyGuid(company.guid)}>This company uses INR</button>
      </section>
    );
  }

  const completeResult = isInrCompleteResult(result) ? result : null;
  const report = completeResult?.report ?? null;
  const unsupportedCurrencyAssertion = result?.state === "complete" && !completeResult;
  return (
    <section className="outstandings-screen" aria-busy={loading}>
      <div className="outstandings-heading">
        <div>
          <h2>{company.name}</h2>
          <p>
            {result
              ? `${result.state === "complete" ? "Synced" : "Checked"} ${relativeTime(result.synced_at_unix_ms)}`
              : "Not read in this session"}
            {report ? ` · ${report.source_voucher_count.toLocaleString("en-IN")} vouchers verified` : ""}
          </p>
        </div>
        <button type="button" onClick={load} disabled={loading}>
          <RefreshCw size={18} className={loading ? "spin" : undefined} />
          {loading ? "Reading verified segments…" : result ? "Refresh" : "Load outstandings"}
        </button>
      </div>

      {error && <div className="outstandings-state error" role="alert"><strong>Read failed</strong><span>{error}</span></div>}
      {loading && !report && (
        <div className="outstandings-state" role="status" aria-live="polite">
          <strong>Reading verified segments</strong>
          <span>Bridge is checking each voucher segment twice. Totals stay withheld until complete coverage is proven.</span>
        </div>
      )}
      {!loading && result?.state === "partial" && (
        <div className="outstandings-state" role="status">
          <strong>Partial result withheld</strong>
          <span>Bridge could not prove every requested segment complete ({outstandingsPartialReason(result.reason_code)}). No totals were calculated.</span>
        </div>
      )}
      {unsupportedCurrencyAssertion && (
        <div className="outstandings-state error" role="alert">
          <strong>Totals withheld</strong>
          <span>Bridge received an unsupported currency assertion. Unit A displays totals only for an explicit INR assertion.</span>
        </div>
      )}

      {completeResult ? (
        <>
          <div className="outstandings-totals" role="group" aria-label="Outstanding totals">
            <div><span>Receivable</span><strong>{formatMoney(completeResult.report.receivable_total, completeResult.currency_assertion)}</strong></div>
            <div><span>Payable</span><strong>{formatMoney(completeResult.report.payable_total, completeResult.currency_assertion)}</strong></div>
          </div>

          <div className="outstandings-ageing" role="group" aria-label="Receivable ageing buckets">
            <div className="ageing-label"><span>Receivable ageing</span><small>as of {formatDate(completeResult.report.as_of_yyyymmdd)}</small></div>
            {[
              ["0–30", completeResult.report.ageing.days_0_30],
              ["31–60", completeResult.report.ageing.days_31_60],
              ["61–90", completeResult.report.ageing.days_61_90],
              ["90+", completeResult.report.ageing.days_90_plus],
            ].map(([label, amount]) => (
              <div key={label}><span>{label} days</span><strong>{formatMoney(amount, completeResult.currency_assertion)}</strong></div>
            ))}
          </div>

          <div className="outstandings-parties">
            <div className="outstandings-table-heading">
              <h3>Top exposure</h3>
              <span>Outstanding · oldest bill reference</span>
            </div>
            {completeResult.report.top_parties.length ? (
              <div role="table" aria-label="Top party exposure" aria-rowcount={completeResult.report.top_parties.length + 1}>
                <div className="visually-hidden" role="row">
                  <span role="columnheader">Party</span>
                  <span role="columnheader">Outstanding</span>
                  <span role="columnheader">Oldest bill reference</span>
                </div>
                {completeResult.report.top_parties.map((party) => {
                  const hasReceivable = party.receivable !== "0";
                  const hasPayable = party.payable !== "0";
                  const kind = hasReceivable && hasPayable
                    ? "Receivable + payable"
                    : hasReceivable ? "Receivable" : "Payable";
                  return (
                    <div className="outstandings-party" role="row" key={party.party}>
                      <div role="cell"><strong>{party.party}</strong><span>{kind}</span></div>
                      <strong role="cell">{formatMoney(party.outstanding_total, completeResult.currency_assertion)}</strong>
                      <span role="cell">{party.oldest_bill_age_days === null ? "No bill reference" : `${party.oldest_bill_age_days} days`}</span>
                    </div>
                  );
                })}
              </div>
            ) : <p className="outstandings-none">No open bill allocations remained after exact reconciliation.</p>}
          </div>
        </>
      ) : !loading && !result && !error ? (
        <div className="outstandings-state">
          <strong>Ready for a read-only scan</strong>
          <span>Bridge will pin the company, read bounded voucher segments twice, and show numbers only when both copies agree.</span>
        </div>
      ) : null}
    </section>
  );
}

function formatMoney(value: string, currencyAssertion: "INR") {
  const negative = value.startsWith("-");
  const unsigned = negative ? value.slice(1) : value;
  const [whole, fraction] = unsigned.split(".");
  const tail = whole.slice(-3);
  const head = whole.slice(0, -3).replace(/\B(?=(\d{2})+(?!\d))/g, ",");
  const grouped = head ? `${head},${tail}` : tail;
  return `${negative ? "−" : ""}${currencySymbol(currencyAssertion)}${grouped}${fraction ? `.${fraction.padEnd(2, "0")}` : ""}`;
}

function isInrCompleteResult(result: LoadResult | null): result is InrCompleteResult {
  return result?.state === "complete" && result.currency_assertion === "INR";
}

function currencySymbol(currencyAssertion: "INR") {
  return currencyAssertion === "INR" ? "₹" : unreachableCurrencyAssertion(currencyAssertion);
}

function unreachableCurrencyAssertion(currencyAssertion: never): never {
  throw new Error(`Unsupported outstandings currency assertion: ${currencyAssertion}`);
}

function formatDate(value: string) {
  // `new Date("2026-07-01")` parses as UTC midnight, so west of UTC it renders
  // the PREVIOUS day -- and with a day-01 string, the previous MONTH. Report
  // dates are plain calendar dates with no time zone, so build them from local
  // components and never round-trip them through UTC.
  const year = Number(value.slice(0, 4));
  const month = Number(value.slice(4, 6));
  const day = value.slice(6, 8);
  const monthName = new Date(year, month - 1, 1).toLocaleString("en-IN", { month: "short" });
  return `${day} ${monthName} ${year}`;
}

function relativeTime(timestamp: number) {
  const minutes = Math.max(0, Math.floor((Date.now() - timestamp) / 60_000));
  if (minutes < 1) return "just now";
  if (minutes === 1) return "1 minute ago";
  return `${minutes} minutes ago`;
}

function operatorMessage(cause: unknown) {
  if (cause && typeof cause === "object" && "message" in cause && typeof cause.message === "string") {
    return cause.message;
  }
  return typeof cause === "string" ? cause : "The local Tally read did not complete.";
}
