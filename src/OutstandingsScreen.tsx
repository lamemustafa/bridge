import React from "react";
import { RefreshCw } from "lucide-react";
import { invoke } from "@tauri-apps/api/core";

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
  top_parties: Array<{
    party: string;
    receivable: string;
    payable: string;
    outstanding_total: string;
    oldest_bill_age_days: number;
  }>;
  source_voucher_count: number;
  source_bytes: number;
};

type LoadResult =
  | { state: "complete"; report: Report; synced_at_unix_ms: number }
  | { state: "partial"; reason_code: string; synced_at_unix_ms: number };

export function OutstandingsScreen({ config, company, onChangeSetup }: Props) {
  const [result, setResult] = React.useState<LoadResult | null>(null);
  const [error, setError] = React.useState<string | null>(null);
  const [loading, setLoading] = React.useState(false);
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
  }, [config.host, config.port, company?.guid, company?.name]);

  const load = React.useCallback(async () => {
    if (!company) return;
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
  }, [config.host, config.port, company?.guid, company?.name]);

  React.useEffect(() => {
    if (company) void load();
    // The screen is mounted only when the operator opens Outstandings.
  }, [load]);

  if (!company) {
    return (
      <section className="panel wide outstandings-empty">
        <h2>Select a verified Tally company</h2>
        <p>Outstandings require a persisted company name and observed GUID before any voucher read can start.</p>
        <button type="button" onClick={onChangeSetup}>Open Tally setup</button>
      </section>
    );
  }

  const report = result?.state === "complete" ? result.report : null;
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
          <span>Bridge could not prove every requested segment complete ({friendlyReason(result.reason_code)}). No totals were calculated.</span>
        </div>
      )}

      {report ? (
        <>
          <div className="outstandings-totals" role="group" aria-label="Outstanding totals">
            <div><span>Receivable</span><strong>{formatMoney(report.receivable_total)}</strong></div>
            <div><span>Payable</span><strong>{formatMoney(report.payable_total)}</strong></div>
          </div>

          <div className="outstandings-ageing" role="group" aria-label="Receivable ageing buckets">
            <div className="ageing-label"><span>Receivable ageing</span><small>as of {formatDate(report.as_of_yyyymmdd)}</small></div>
            {[
              ["0–30", report.ageing.days_0_30],
              ["31–60", report.ageing.days_31_60],
              ["61–90", report.ageing.days_61_90],
              ["90+", report.ageing.days_90_plus],
            ].map(([label, amount]) => (
              <div key={label}><span>{label} days</span><strong>{formatMoney(amount)}</strong></div>
            ))}
          </div>

          <div className="outstandings-parties">
            <div className="outstandings-table-heading">
              <h3>Top exposure</h3>
              <span>Outstanding · oldest bill</span>
            </div>
            {report.top_parties.length ? (
              <div role="table" aria-label="Top party exposure" aria-rowcount={report.top_parties.length + 1}>
                <div className="visually-hidden" role="row">
                  <span role="columnheader">Party</span>
                  <span role="columnheader">Outstanding</span>
                  <span role="columnheader">Oldest bill</span>
                </div>
                {report.top_parties.map((party) => {
                  const hasReceivable = party.receivable !== "0";
                  const hasPayable = party.payable !== "0";
                  const kind = hasReceivable && hasPayable
                    ? "Receivable + payable"
                    : hasReceivable ? "Receivable" : "Payable";
                  return (
                    <div className="outstandings-party" role="row" key={party.party}>
                      <div role="cell"><strong>{party.party}</strong><span>{kind}</span></div>
                      <strong role="cell">{formatMoney(party.outstanding_total)}</strong>
                      <span role="cell">{party.oldest_bill_age_days} days</span>
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

function formatMoney(value: string) {
  const negative = value.startsWith("-");
  const unsigned = negative ? value.slice(1) : value;
  const [whole, fraction] = unsigned.split(".");
  const tail = whole.slice(-3);
  const head = whole.slice(0, -3).replace(/\B(?=(\d{2})+(?!\d))/g, ",");
  const grouped = head ? `${head},${tail}` : tail;
  return `${negative ? "−" : ""}₹${grouped}${fraction ? `.${fraction.padEnd(2, "0")}` : ""}`;
}

function formatDate(value: string) {
  return `${value.slice(6, 8)} ${new Date(`${value.slice(0, 4)}-${value.slice(4, 6)}-01`).toLocaleString("en-IN", { month: "short" })} ${value.slice(0, 4)}`;
}

function relativeTime(timestamp: number) {
  const minutes = Math.max(0, Math.floor((Date.now() - timestamp) / 60_000));
  if (minutes < 1) return "just now";
  if (minutes === 1) return "1 minute ago";
  return `${minutes} minutes ago`;
}

function friendlyReason(value: string) {
  if (value === "tally_segment_latency_trending_restart_recommended") {
    return "comparable segments kept slowing toward the safety deadline; Tally may need a restart before another sync";
  }
  if (value === "tally_segment_deadline_restart_recommended") {
    return "a segment reached the safety deadline; Tally may need a restart before another sync";
  }
  if (value === "segment_response_size_limit_exceeded") {
    return "a wildcard segment exceeded Bridge's response safety bound";
  }
  if (value === "outstandings_segment_sizing_uncalibrated") {
    return "safe voucher-segment sizing is waiting for the ordered bill-bearing test company";
  }
  if (value === "company_voucher_alter_id_high_water_missing") {
    return "Tally did not return the voucher limit Bridge needs to prove complete coverage";
  }
  if (value === "empty_segment_has_no_adjacent_corroboration_window") {
    return "an empty voucher range reached the final known voucher without an adjacent range to confirm it";
  }
  if (value === "empty_corroboration_pair_mismatch") {
    return "the paired wider voucher checks did not return identical rows";
  }
  if (value === "empty_corroboration_wider_window_empty") {
    return "the adjacent voucher range was also empty, so absence could not be proven";
  }
  if (value === "empty_segment_contradicted_by_wider_read") {
    return "the wider check found vouchers inside the supposedly empty range";
  }
  if (value === "empty_corroboration_scope_ambiguous") {
    return "the wider check returned vouchers outside the expected adjacent range";
  }
  return value.replace(/_/g, " ");
}

function operatorMessage(cause: unknown) {
  if (cause && typeof cause === "object" && "message" in cause && typeof cause.message === "string") {
    return cause.message;
  }
  return typeof cause === "string" ? cause : "The local Tally read did not complete.";
}
