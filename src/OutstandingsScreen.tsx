import React from "react";
import { Building2, ChevronRight, Download, RefreshCw } from "lucide-react";
import { invoke } from "@tauri-apps/api/core";
import { isNonRetryableOutstandingsBoundary, outstandingsAgeingAnchorLabel, outstandingsAgeingDisclosure, outstandingsPartialState, type OutstandingsAgeingAnchor } from "./outstandings-copy";
import { csvNumericCell, csvRow, csvTextCell, type CsvCell } from "./outstandings-csv";
import { canStartOutstandingsRead, outstandingsCurrencySymbol } from "./outstandings-currency";
import { groupOpenBillsByParty, type OpenBill, type PartyBillsState } from "./outstandings-bills";
import {
  asOfBoundValueForAsOf,
  asOfYyyymmdd,
  bulkPartyStatementsInvokeArgument,
  partyStatementInvokeArgument,
  settleAsOfBoundValue,
  singleCompanyOutstandingsInvokeArgument,
  workingPaperInvokeArgument,
  type AsOfBoundValue,
} from "./outstandings-as-of";

type Props = {
  config: { host: string; port: number };
  company?: { name: string; guid: string };
  onChangeSetup: () => void;
  /// Switches to the cross-client view. Present only when more than one book
  /// is open, because a scope switch with one option is noise.
  onViewAllClients?: () => void;
  openBookCount?: number;
  asOf: string;
  onAsOfChange: (value: string) => void;
};

type Report = {
  company_name: string;
  as_of_yyyymmdd: string;
  receivable_total: string;
  payable_total: string;
  has_unaged_receivable: boolean;
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

type TopParty = Report["top_parties"][number];
type PartySortKey = "party" | "outstanding" | "age";
type PartySort = { key: PartySortKey; direction: "asc" | "desc" };

// These limits bound only what the dashboard renders. The Tauri result keeps
// complete statement-source rows so an export is never silently narrowed by a
// presentation cap. DISPLAY_OPEN_BILL_ROW_LIMIT applies per party (see
// outstandings-bills.ts) -- never to the flattened cross-party list -- so one
// party's bill count can never push another party's rows out of view.
const DISPLAY_OPEN_BILL_ROW_LIMIT = 2_000;
const DISPLAY_UNALLOCATED_PARTY_LIMIT = 10;

type LoadResult =
  | {
      state: "complete";
      report: Report;
      currency_assertion: string;
      ageing_anchor: OutstandingsAgeingAnchor;
      synced_at_unix_ms: number;
      working_paper_export_id?: string;
      working_paper_unavailable_reason_code?: string;
      // Absent when the read path cannot establish it. Absent is not zero and
      // must never render as zero.
      unallocated_total?: string;
      // Complete, uncapped rows. The dashboard applies its own display caps;
      // statement exports always use these complete sources.
      statement_open_bills?: Array<OpenBill>;
      statement_unallocated_by_party?: Array<{
        party: string;
        amount: string;
        direction: "receivable" | "payable";
      }>;
    }
  | {
      state: "partial";
      reason_code: string;
      requested_as_of_yyyymmdd?: string;
      tally_as_of_yyyymmdd?: string;
      foreign_currency_ledger_name?: string;
      synced_at_unix_ms: number;
    };

type InrCompleteResult = Extract<LoadResult, { state: "complete" }> & { currency_assertion: "INR" };
type BulkPartyStatementDestinationSelection = {
  destination: string;
  approval_id: string;
};

export function OutstandingsScreen({
  config,
  company,
  onChangeSetup,
  onViewAllClients,
  openBookCount = 1,
  asOf,
  onAsOfChange,
}: Props) {
  const [loadedResult, setLoadedResult] = React.useState<AsOfBoundValue<LoadResult> | null>(null);
  const [error, setError] = React.useState<string | null>(null);
  const [loading, setLoading] = React.useState(false);
  const [inrAssertedCompanyGuid, setInrAssertedCompanyGuid] = React.useState<string | null>(null);
  const [ageingAnchor, setAgeingAnchor] = React.useState<OutstandingsAgeingAnchor>("due_date");
  const [view, setView] = React.useState<"ageing" | "unallocated">("ageing");
  const [exportNotice, setExportNotice] = React.useState<{
    message: string;
    path?: string;
    location?: string;
    failures?: Array<{ party: string; code: string; reason: string }>;
  } | null>(null);
  const [expandedParty, setExpandedParty] = React.useState<string | null>(null);
  const [exporting, setExporting] = React.useState<"csv" | "working-paper" | "batch" | "party" | null>(null);
  const [consumedWorkingPaperId, setConsumedWorkingPaperId] = React.useState<string | null>(null);
  const exportLock = React.useRef(false);
  const [partySort, setPartySort] = React.useState<PartySort | null>(null);
  const [currencyCheck, setCurrencyCheck] = React.useState<"idle" | "checking" | "inr" | "undetermined">("idle");
  const [, refreshClock] = React.useReducer((value) => value + 1, 0);
  const requestVersion = React.useRef(0);
  const initialReadKey = React.useRef<string | null>(null);
  const requestedAsOf = asOfYyyymmdd(asOf);
  const currentRequestedAsOf = React.useRef(requestedAsOf);
  currentRequestedAsOf.current = requestedAsOf;
  const result = asOfBoundValueForAsOf(loadedResult, requestedAsOf);
  // Settles the initial tab once per loaded report, keyed on the report's own
  // sync timestamp -- not on every render, and never after the operator has
  // clicked a tab, since this effect only fires again when a NEW report
  // arrives.
  const defaultedViewForSyncedAt = React.useRef<number | null>(null);

  React.useEffect(() => {
    const timer = window.setInterval(refreshClock, 30_000);
    return () => window.clearInterval(timer);
  }, []);

  React.useEffect(() => {
    requestVersion.current += 1;
    setLoadedResult(null);
    setError(null);
    setLoading(false);
    setInrAssertedCompanyGuid(null);
    setExpandedParty(null);
  }, [config.host, config.port, company?.guid, company?.name]);

  React.useEffect(() => {
    // The heading/control can change at local midnight while a read is still
    // pending. Date-bind results and invalidate the issued request so neither
    // completed nor late-settling money can be displayed under a new date.
    requestVersion.current += 1;
    setLoadedResult(null);
    setError(null);
    setLoading(false);
    setExpandedParty(null);
  }, [requestedAsOf]);

  React.useEffect(() => {
    // Switching Tally's F6-equivalent basis changes every bucket and every
    // statement row. Hide the previous basis immediately and issue a fresh,
    // version-bound read through the normal selected-company path.
    requestVersion.current += 1;
    setLoadedResult(null);
    setError(null);
    setLoading(false);
    setExpandedParty(null);
  }, [ageingAnchor]);

  const currencyReadPermitted = canStartOutstandingsRead(company, inrAssertedCompanyGuid);
  const readPermitted = currencyReadPermitted && requestedAsOf !== null;
  const partialState = result?.state === "partial"
    ? outstandingsPartialState(
      result.reason_code,
      result.requested_as_of_yyyymmdd,
      result.tally_as_of_yyyymmdd,
      result.foreign_currency_ledger_name,
    )
    : null;
  const outstandingsUnavailable = result?.state === "partial" && isNonRetryableOutstandingsBoundary(result.reason_code);
  const tallyReadAttempted = result?.state === "partial" && partialState?.tallyReadAttempted;

  const load = React.useCallback(async () => {
    if (!readPermitted || !company || !requestedAsOf) return;
    const argument = singleCompanyOutstandingsInvokeArgument(config, company, asOf, ageingAnchor);
    if (!argument) return;
    const requestedAsOfYyyymmdd = argument.request.as_of_yyyymmdd;
    const version = requestVersion.current + 1;
    requestVersion.current = version;
    setLoading(true);
    setError(null);
    try {
      const next = await invoke<LoadResult>("fetch_tally_outstandings", argument);
      if (requestVersion.current !== version) return;
      const settled = settleAsOfBoundValue(
        currentRequestedAsOf.current,
        requestedAsOfYyyymmdd,
        next,
      );
      if (!settled) return;
      setLoadedResult(settled);
    } catch (cause) {
      if (requestVersion.current !== version) return;
      setLoadedResult(null);
      setError(operatorMessage(cause));
    } finally {
      if (requestVersion.current === version) setLoading(false);
    }
  }, [ageingAnchor, asOf, config.host, config.port, company?.guid, company?.name, readPermitted, requestedAsOf]);

  React.useEffect(() => {
    const key = company ? `${config.host}:${config.port}:${company.guid}:${ageingAnchor}` : null;
    if (!readPermitted || !key || initialReadKey.current === key) return;
    initialReadKey.current = key;
    void load();
    // A newly opened company keeps the existing default-today behaviour. A
    // later date choice is deliberate and waits for the operator to refresh.
  }, [ageingAnchor, company?.guid, config.host, config.port, load, readPermitted]);

  // Establish the base currency from Tally rather than making the operator
  // assert it. The INR requirement stays -- putting a rupee symbol in front of
  // a foreign balance misstates money -- but it is a fact Tally holds, and
  // asking for it on every company was a step the product can answer itself.
  // Where Tally cannot settle it (several currencies defined, or a non-Indian
  // one) the manual confirmation below is still shown.
  React.useEffect(() => {
    if (!company || inrAssertedCompanyGuid === company.guid) return;
    let cancelled = false;
    setCurrencyCheck("checking");
    void invoke<{ is_inr: boolean; mailing_name: string; currency_count: number }>(
      "detect_tally_base_currency",
      { request: { config, company: company.name, expected_company_guid: company.guid } },
    )
      .then((currency) => {
        if (cancelled) return;
        if (currency.is_inr) setInrAssertedCompanyGuid(company.guid);
        setCurrencyCheck(currency.is_inr ? "inr" : "undetermined");
      })
      .catch(() => {
        if (!cancelled) setCurrencyCheck("undetermined");
      });
    return () => {
      cancelled = true;
    };
  }, [config.host, config.port, company?.guid, company?.name]);

  // Must sit above the early returns below: a hook placed after them runs on
  // some renders and not others, which React rejects outright with "Rendered
  // more hooks than during the previous render" -- and the whole screen blanks.
  const inrCompleteResult = isInrCompleteResult(result) ? result : null;
  // Grouped from the COMPLETE source Bridge received -- the display cap is
  // applied per party inside groupOpenBillsByParty, never to the flattened
  // cross-party list, so one party's bill count can never push another
  // party's real bills out of the map entirely. null when statement rows are
  // absent altogether (voucher-scan path) -- distinct from a present map that
  // simply has no entry for a given party (that party has zero bill rows).
  const openBillsByParty = React.useMemo(
    () => groupOpenBillsByParty(inrCompleteResult?.statement_open_bills, DISPLAY_OPEN_BILL_ROW_LIMIT),
    [inrCompleteResult],
  );
  // Parties for which the operator has asked to see every bill row beyond the
  // default per-party cap. The complete rows are already in memory -- no
  // further Tally read is needed to satisfy this.
  const [fullyLoadedParties, setFullyLoadedParties] = React.useState<Set<string>>(new Set());
  React.useEffect(() => {
    setFullyLoadedParties(new Set());
  }, [inrCompleteResult]);

  // On a book where most balances carry no bill reference, the ageing panel
  // can describe a rounding error against total exposure. Default to
  // whichever breakdown describes more money, but only once per report -- a
  // later click on the other tab must never be overridden by this effect
  // re-running on an unrelated render.
  React.useEffect(() => {
    if (!inrCompleteResult) return;
    if (defaultedViewForSyncedAt.current === inrCompleteResult.synced_at_unix_ms) return;
    defaultedViewForSyncedAt.current = inrCompleteResult.synced_at_unix_ms;
    const unallocatedTotal = inrCompleteResult.unallocated_total;
    if (unallocatedTotal === undefined) return;
    const exposure = amountOf(inrCompleteResult.report.receivable_total) + amountOf(inrCompleteResult.report.payable_total);
    setView(amountOf(unallocatedTotal) > exposure ? "unallocated" : "ageing");
  }, [inrCompleteResult]);

  if (!company) {
    return (
      <section className="panel wide outstandings-empty">
        <h2>Select a verified Tally company</h2>
        <p>Outstandings require a persisted company name and observed GUID before any voucher read can start.</p>
        <button type="button" onClick={onChangeSetup}>Manage Tally</button>
      </section>
    );
  }

  if (!currencyReadPermitted) {
    if (currencyCheck === "checking" || currencyCheck === "idle") {
      return (
        <section className="panel wide outstandings-empty">
          <h2>Opening {company.name}</h2>
          <p>Reading the company&rsquo;s base currency from Tally.</p>
        </section>
      );
    }
    return (
      <section className="panel wide outstandings-empty">
        <h2>Confirm the base currency</h2>
        <p>Tally did not settle this company&rsquo;s base currency — it defines more than one currency, or one that is not the Indian rupee. Bridge shows totals in rupees, so confirm before continuing.</p>
        <button type="button" onClick={() => setInrAssertedCompanyGuid(company.guid)}>This company uses INR</button>
      </section>
    );
  }

  const completeResult = inrCompleteResult;
  const report = completeResult?.report ?? null;
  const composition = report
    ? exposureComposition(report, completeResult?.unallocated_total)
    : null;
  const unallocatedParties = completeResult?.statement_unallocated_by_party
    ?.slice(0, DISPLAY_UNALLOCATED_PARTY_LIMIT) ?? [];
  const largestUnallocated = Math.max(...unallocatedParties.map((entry) => amountOf(entry.amount)), 0);
  const largestExposure = report
    ? Math.max(...report.top_parties.map((party) => amountOf(party.outstanding_total)), 0)
    : 0;
  // Default order (no click yet) is exactly what Tally/Bridge already
  // returned -- largest exposure first -- so an unsorted column never
  // reshuffles rows the operator hasn't asked to sort.
  const sortedTopParties = report && partySort
    ? [...report.top_parties].sort(comparePartiesBy(partySort))
    : report?.top_parties ?? [];
  function togglePartySort(key: PartySortKey) {
    setPartySort((current) => (current?.key === key
      ? { key, direction: current.direction === "asc" ? "desc" : "asc" }
      // Amount and age read naturally biggest/oldest first; a name reads
      // naturally A-first. Each column's first click picks that direction.
      : { key, direction: key === "party" ? "asc" : "desc" }));
  }
  function partyAriaSort(key: PartySortKey): React.AriaAttributes["aria-sort"] {
    if (partySort?.key !== key) return "none";
    return partySort.direction === "asc" ? "ascending" : "descending";
  }
  function beginExport(kind: NonNullable<typeof exporting>) {
    if (exportLock.current) return false;
    exportLock.current = true;
    setExporting(kind);
    return true;
  }
  function endExport() {
    exportLock.current = false;
    setExporting(null);
  }
  // Redundant once the unallocated total is known: that case now has its own
  // "Unallocated" figure in the totals row and its own "Unallocated by
  // party" tab, so this paragraph only earns its place when the total is
  // NOT known and those two are absent.
  const ageingDisclosure = report && completeResult?.unallocated_total === undefined
    && outstandingsAgeingDisclosure(report.has_unaged_receivable);
  const unsupportedCurrencyAssertion = result?.state === "complete" && !completeResult;
  const batchStatementRowsAvailable = completeResult !== null
    && (completeResult.statement_open_bills !== undefined
      || completeResult.statement_unallocated_by_party !== undefined);
  // A present unallocated control identifies the native path that computed
  // the complete bill and residual sources. Empty vectors are omitted from
  // serialization, so vector absence can also truthfully mean zero rows.
  const workingPaperExportId = completeResult?.working_paper_export_id;
  const workingPaperAvailable = workingPaperExportId !== undefined
    && (exporting === "working-paper" || workingPaperExportId !== consumedWorkingPaperId);
  const exportAllPartyStatements = async (format: "xlsx" | "pdf") => {
    if (!completeResult || !beginExport("batch")) return;
    let approvalIdToRevoke: string | null = null;
    // Disable both batch-export controls before opening the native picker so
    // two click handlers cannot race each other in the renderer.
    try {
      const selection = await invoke<BulkPartyStatementDestinationSelection | null>("select_party_statement_destination");
      if (!selection) return;
      approvalIdToRevoke = selection.approval_id;
      const preview = await previewBulkPartyStatements(completeResult);
      if (preview.party_count === 0) {
        setExportNotice({ message: "No parties with outstanding balances are available for statements." });
        return;
      }
      const label = format === "xlsx" ? "Excel" : "PDF";
      const confirmed = window.confirm(
        `Create ${preview.party_count} ${label} statement${preview.party_count === 1 ? "" : "s"} in:\n${selection.destination}\n\nThe dashboard shows only its largest parties. This batch includes every party with a non-zero outstanding balance.`,
      );
      if (!confirmed) return;
      const batch = await exportBulkPartyStatements(completeResult, selection, format);
      setExportNotice({
        message: `${batch.written.length} ${label} statement${batch.written.length === 1 ? "" : "s"} written`,
        path: batch.manifest_path,
        location: batch.destination,
        failures: batch.failures,
      });
    } catch (cause) {
      setExportNotice({ message: operatorMessage(cause) });
    } finally {
      if (approvalIdToRevoke) {
        try {
          await revokePartyStatementDestination(approvalIdToRevoke);
        } catch (cause) {
          setExportNotice({ message: operatorMessage(cause) });
        }
      }
      endExport();
    }
  };
  return (
    <section className="outstandings-screen" aria-busy={loading}>
      <div className="outstandings-heading">
        <div>
          <h2>{company.name}</h2>
          <p>
            {result
              ? result.state === "complete"
                ? `Synced ${relativeTime(result.synced_at_unix_ms)}`
                : tallyReadAttempted
                  ? `Checked ${relativeTime(result.synced_at_unix_ms)}`
                  : "No Tally data was read"
              : "Not read in this session"}
            {report ? ` · ${readProvenance(report)}` : ""}
          </p>
        </div>
        <div className="outstandings-heading-actions">
          <label className="outstandings-as-of">
            <span>As of</span>
            <input
              type="date"
              value={asOf}
              onChange={(event) => {
                onAsOfChange(event.target.value);
                setLoadedResult(null);
                setError(null);
              }}
              disabled={loading}
              aria-describedby="outstandings-as-of-help"
            />
            <small id="outstandings-as-of-help">Choose the exact date, then refresh.</small>
          </label>
          <label className="outstandings-as-of">
            <span>Age from</span>
            <select
              value={ageingAnchor}
              onChange={(event) => setAgeingAnchor(event.target.value as OutstandingsAgeingAnchor)}
              disabled={loading}
              aria-describedby="outstandings-ageing-anchor-help"
            >
              <option value="due_date">Due date</option>
              <option value="bill_date">Bill date</option>
            </select>
            <small id="outstandings-ageing-anchor-help">Refreshes the report and every statement export.</small>
          </label>
          {onViewAllClients && openBookCount > 1 && (
            <button className="secondary-action" type="button" onClick={onViewAllClients}>
              <Building2 size={16} />
              Compare clients
            </button>
          )}
          {completeResult && (
            <button
              className="secondary-action"
              type="button"
              disabled={exporting !== null}
              onClick={async () => {
                if (!beginExport("csv")) return;
                try {
                  const path = await exportCsv(completeResult);
                  setExportNotice({ message: fileNameOf(path), path });
                } catch (cause) {
                  setExportNotice({ message: operatorMessage(cause) });
                } finally {
                  endExport();
                }
              }}
            >
              <Download size={16} />
              Export
            </button>
          )}
          {workingPaperAvailable && (
            <button
              className="secondary-action"
              type="button"
              disabled={exporting !== null}
              onClick={async () => {
                if (!completeResult || !beginExport("working-paper")) return;
                setConsumedWorkingPaperId(completeResult.working_paper_export_id ?? null);
                try {
                  const path = await exportWorkingPaper(completeResult);
                  setExportNotice({ message: fileNameOf(path), path });
                } catch (cause) {
                  setExportNotice({
                    message: `${operatorMessage(cause)} Refresh outstandings before trying another working-paper export.`,
                  });
                } finally {
                  endExport();
                }
              }}
            >
              <Download size={16} />
              {exporting === "working-paper" ? "Building working paper…" : "Excel working paper"}
            </button>
          )}
          {batchStatementRowsAvailable && (
            <>
              <button
                className="secondary-action"
                type="button"
                disabled={exporting !== null}
                onClick={() => void exportAllPartyStatements("xlsx")}
              >
                <Download size={16} />
                {exporting === "batch" ? "Writing statements…" : "All Excel statements"}
              </button>
              <button
                className="secondary-action"
                type="button"
                disabled={exporting !== null}
                onClick={() => void exportAllPartyStatements("pdf")}
              >
                <Download size={16} />
                All PDF statements
              </button>
            </>
          )}
          <button className="secondary-action" type="button" onClick={onChangeSetup}>Manage Tally</button>
          {!outstandingsUnavailable && (
            <button type="button" onClick={load} disabled={loading || !requestedAsOf}>
              <RefreshCw size={18} className={loading ? "spin" : undefined} />
              {loading ? "Reading…" : result ? "Refresh" : "Load outstandings"}
            </button>
          )}
        </div>
      </div>

      {exportNotice && (
        <div className="outstandings-export-notice" role="status">
          <span>
            {exportNotice.path
              ? <>Saved <strong>{exportNotice.message}</strong> to {exportNotice.location ?? "Downloads"}</>
              : exportNotice.message}
          </span>
          <span className="export-notice-actions">
            {exportNotice.path && (
              <button type="button" onClick={() => void invoke("reveal_exported_file", { path: exportNotice.path })}>
                {revealLabel()}
              </button>
            )}
            <button type="button" onClick={() => setExportNotice(null)} aria-label="Dismiss">Dismiss</button>
          </span>
          {exportNotice.failures && exportNotice.failures.length > 0 && (
            <ul className="outstandings-export-failures">
              {exportNotice.failures.map((failure) => (
                <li key={failure.party}><strong>{failure.party}</strong>: {failure.reason}</li>
              ))}
            </ul>
          )}
        </div>
      )}
      {error && <div className="outstandings-state error" role="alert"><strong>Read failed</strong><span>{error}</span></div>}
      {loading && !report && (
        <div className="outstandings-state" role="status" aria-live="polite">
          <strong>Reading from Tally</strong>
          <span>Every read is taken twice and compared. Totals stay withheld unless both copies agree.</span>
        </div>
      )}
      {!loading && result?.state === "partial" && (
        <div className="outstandings-state" role="status">
          <strong>{partialState?.title}</strong>
          <span>{partialState?.message}</span>
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
            {completeResult.unallocated_total !== undefined && (
              <div>
                <span>Unallocated<em title="Money on a party's ledger with no bill reference. Tally gives it no bill and no age, so it is excluded from the ageing below.">no bill reference</em></span>
                <strong>{formatMoney(completeResult.unallocated_total, completeResult.currency_assertion)}</strong>
              </div>
            )}
          </div>

          {/* Part-to-whole. The three figures above are routinely orders of
              magnitude apart -- on a bulk book the unallocated share is >99% --
              and equal-sized tiles hide exactly that. */}
          {composition && composition.length > 1 && (
            <div className="exposure-share" aria-hidden="true">
              <div className="exposure-share-track">
                {composition.map((slice) => (
                  <span
                    key={slice.label}
                    className={`exposure-share-slice is-${slice.tone}`}
                    style={{ flexGrow: slice.value }}
                  />
                ))}
              </div>
              <ul className="exposure-share-key">
                {composition.map((slice) => (
                  <li key={slice.label}>
                    <i className={`is-${slice.tone}`} />
                    {slice.label} <b>{slice.share}</b>
                  </li>
                ))}
              </ul>
            </div>
          )}

          <div className="outstandings-ageing" role="group" aria-label="Exposure breakdown">
            <div className="ageing-heading">
              {unallocatedParties.length > 0 ? (
                <div className="view-switch" role="tablist" aria-label="Breakdown">
                  <button role="tab" type="button" aria-selected={view === "ageing"}
                    className={view === "ageing" ? "is-on" : undefined}
                    onClick={() => setView("ageing")}>Ageing</button>
                  <button role="tab" type="button" aria-selected={view === "unallocated"}
                    className={view === "unallocated" ? "is-on" : undefined}
                    onClick={() => setView("unallocated")}>Unallocated by party</button>
                </div>
              ) : <h3>Receivable ageing</h3>}
              <span>
                {view === "ageing"
                  ? `bill references only · ${outstandingsAgeingAnchorLabel(completeResult.ageing_anchor)} · as of ${formatDate(completeResult.report.as_of_yyyymmdd)}`
                  : `no bill reference · ${unallocatedParties.length} ${unallocatedParties.length === 1 ? "party" : "parties"}`}
              </span>
            </div>
            {view === "ageing" && ageingRows(completeResult.report).map((row) => (
              <div className="ageing-row" key={row.label}>
                <span className="ageing-bucket">{row.label}</span>
                <span className="ageing-track">
                  <span
                    className={`ageing-fill tier-${row.tier}`}
                    style={{ width: `${row.percent}%` }}
                  />
                </span>
                <strong>{formatMoney(row.amount, completeResult.currency_assertion)}</strong>
                <span className="ageing-count">{row.count === 0 ? "—" : `${row.count} ${row.count === 1 ? "bill" : "bills"}`}</span>
              </div>
            ))}
            {view === "unallocated" && unallocatedParties.map((entry) => {
              const percent = largestUnallocated > 0
                ? Math.max(1, (amountOf(entry.amount) / largestUnallocated) * 100)
                : 0;
              return (
                <div className="ageing-row is-party" key={entry.party}>
                  <span className="ageing-party" title={entry.party}>{entry.party}</span>
                  <span className="ageing-track">
                    <span className="ageing-fill is-unallocated" style={{ width: `${percent}%` }} />
                  </span>
                  <strong>{formatMoney(entry.amount, completeResult.currency_assertion)}</strong>
                </div>
              );
            })}
          </div>
          {ageingDisclosure && <p className="outstandings-ageing-note" role="note">{ageingDisclosure}</p>}

          <div className="outstandings-parties">
            <div className="outstandings-table-heading">
              <h3>Top exposure</h3>
              <span>Outstanding · oldest bill reference</span>
            </div>
            {completeResult.report.top_parties.length ? (
              <div role="table" aria-label="Top party exposure" aria-rowcount={completeResult.report.top_parties.length + 1}>
                <div className="outstandings-column-headers" role="row">
                  <button type="button" role="columnheader" aria-sort={partyAriaSort("party")} onClick={() => togglePartySort("party")}>Party</button>
                  <button type="button" role="columnheader" aria-sort={partyAriaSort("outstanding")} onClick={() => togglePartySort("outstanding")}>Outstanding</button>
                  <button type="button" role="columnheader" aria-sort={partyAriaSort("age")} onClick={() => togglePartySort("age")}>Oldest bill reference</button>
                </div>
                {sortedTopParties.map((party) => {
                  const hasReceivable = party.receivable !== "0";
                  const hasPayable = party.payable !== "0";
                  const kind = hasReceivable && hasPayable
                    ? "Receivable + payable"
                    : hasReceivable ? "Receivable" : "Payable";
                  const share = largestExposure > 0
                    ? Math.max(1, (amountOf(party.outstanding_total) / largestExposure) * 100)
                    : 0;
                  const expandable = openBillsByParty !== null;
                  const isExpanded = expandable && expandedParty === party.party;
                  const panelId = `party-bills-${slugify(party.party)}`;
                  const rowCells = (
                    <>
                      {/* Rank is readable from the row itself rather than from
                          comparing ten right-aligned numbers. */}
                      <span className="party-magnitude" style={{ width: `${share}%` }} aria-hidden="true" />
                      <div role="cell" className="party-name-cell">
                        {expandable && (
                          <ChevronRight
                            size={14}
                            className={`party-caret${isExpanded ? " is-open" : ""}`}
                            aria-hidden="true"
                          />
                        )}
                        <div><strong>{party.party}</strong><span>{kind}</span></div>
                      </div>
                      <strong role="cell">{formatMoney(party.outstanding_total, completeResult.currency_assertion)}</strong>
                      <span role="cell" className="party-age">
                        {party.oldest_bill_age_days === null
                          ? <em className="age-chip is-none">no bill reference</em>
                          : <em className={`age-chip tier-${ageTier(party.oldest_bill_age_days)}`}>{party.oldest_bill_age_days}d</em>}
                      </span>
                    </>
                  );
                  return (
                    <div className="outstandings-party-group" key={party.party}>
                      {expandable ? (
                        <button
                          type="button"
                          role="row"
                          className="outstandings-party is-expandable"
                          aria-expanded={isExpanded}
                          aria-controls={panelId}
                          onClick={() => setExpandedParty(isExpanded ? null : party.party)}
                        >
                          {rowCells}
                        </button>
                      ) : (
                        <div className="outstandings-party" role="row">
                          {rowCells}
                        </div>
                      )}
                      {isExpanded && (
                        <div className="outstandings-party-bills" id={panelId} role="row">
                          <div className="party-bills-actions">
                            <button
                              type="button"
                              className="party-statement-action"
                              disabled={exporting !== null}
                              onClick={async () => {
                                if (!beginExport("party")) return;
                                try {
                                  const path = await exportPartyStatement(completeResult, party.party, "xlsx");
                                  setExportNotice({ message: fileNameOf(path), path });
                                } catch (cause) {
                                  setExportNotice({ message: operatorMessage(cause) });
                                } finally {
                                  endExport();
                                }
                              }}
                            >
                              <Download size={14} />
                              Excel statement
                            </button>
                            <button
                              type="button"
                              className="party-statement-action"
                              disabled={exporting !== null}
                              onClick={async () => {
                                if (!beginExport("party")) return;
                                try {
                                  const path = await exportPartyStatement(completeResult, party.party, "pdf");
                                  setExportNotice({ message: fileNameOf(path), path });
                                } catch (cause) {
                                  setExportNotice({ message: operatorMessage(cause) });
                                } finally {
                                  endExport();
                                }
                              }}
                            >
                              <Download size={14} />
                              PDF statement
                            </button>
                          </div>
                          {renderPartyBills(
                            openBillsByParty?.get(party.party),
                            completeResult.currency_assertion,
                            fullyLoadedParties.has(party.party),
                            () => setFullyLoadedParties((current) => new Set(current).add(party.party)),
                          )}
                        </div>
                      )}
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
          <span>Bridge pins the company by GUID, reads each report twice, and shows numbers only when both copies agree.</span>
        </div>
      ) : null}
    </section>
  );
}

/// Builds one party's statement in the selected format via the Rust command
/// and writes it to Downloads. Sends the complete statement source rows this
/// screen already holds from `fetch_tally_outstandings` -- Bridge never reads
/// Tally a second time to produce a statement.
async function exportPartyStatement(
  result: InrCompleteResult,
  party: string,
  format: "xlsx" | "pdf",
) {
  return invoke<string>("export_party_statement", partyStatementInvokeArgument(result, party, format));
}

type BulkPartyStatementResult = {
  destination: string;
  manifest_path: string;
  written: Array<{ party: string; file_name: string; amount: string }>;
  failures: Array<{ party: string; code: string; reason: string }>;
};

type BulkPartyStatementsPreview = { party_count: number };

/// Uses the complete statement-source rows returned by the finished read. The
/// dashboard's top-ten and drill-down projections are intentionally not used:
/// a batch must not silently omit a party beyond a display cap.
async function exportBulkPartyStatements(
  result: InrCompleteResult,
  selection: BulkPartyStatementDestinationSelection,
  format: "xlsx" | "pdf",
) {
  return invoke<BulkPartyStatementResult>(
    "export_bulk_party_statements",
    bulkPartyStatementsInvokeArgument(result, selection.destination, selection.approval_id, format),
  );
}

async function revokePartyStatementDestination(approvalId: string) {
  return invoke<void>("revoke_party_statement_destination", { approvalId });
}

/// Uses the same complete source rows and backend counting rule as the writer,
/// so the confirmation names the exact scope before any files are created.
async function previewBulkPartyStatements(result: InrCompleteResult) {
  return invoke<BulkPartyStatementsPreview>("preview_bulk_party_statements", {
    request: {
      open_bills: result.statement_open_bills ?? [],
      unallocated_by_party: result.statement_unallocated_by_party ?? [],
    },
  });
}

/// Builds the complete dual-ageing workbook from the finished native read.
/// The Rust command re-parses dates, recomputes both ages, and reconciles all
/// exact controls before it writes a file; it performs no Tally request.
async function exportWorkingPaper(result: InrCompleteResult) {
  const argument = workingPaperInvokeArgument(result);
  if (!argument) {
    throw new Error("This read cannot substantiate a complete working paper.");
  }
  return invoke<string>("export_outstandings_working_paper", argument);
}

/// Builds the report as CSV.
///
/// Amounts are written as raw decimal strings, never the rupee-formatted
/// display value: `₹4,69,474.80` lands in a spreadsheet as text and silently
/// breaks every downstream SUM. The disclosure rows are part of the export
/// because a figure that needs a caveat on screen needs it in the file too --
/// the file is what gets forwarded.
async function exportCsv(result: InrCompleteResult) {
  const csv = reportToCsv(
    result.report,
    result.ageing_anchor,
    result.unallocated_total,
    result.statement_unallocated_by_party,
  );
  const slug = result.report.company_name.replace(/[^a-z0-9]+/gi, "-").toLowerCase();
  // A BOM so Excel reads UTF-8 party names instead of mojibake -- Indian
  // ledger names routinely carry non-ASCII characters.
  return invoke<string>("save_report_download", {
    fileName: `outstandings-${slug}-${result.report.as_of_yyyymmdd}.csv`,
    contents: `\ufeff${csv}`,
  });
}

function reportToCsv(
  report: Report,
  ageingAnchor: OutstandingsAgeingAnchor,
  unallocatedTotal: string | undefined,
  unallocatedByParty: Array<{ party: string; amount: string }> | undefined,
) {
  const text = csvTextCell;
  const number = csvNumericCell;
  const row = (...values: Array<CsvCell>) => csvRow(...values);

  const lines = [
    row(text("Bridge — aged outstandings")),
    row(text("Company"), text(report.company_name)),
    row(text("As of"), text(formatDate(report.as_of_yyyymmdd))),
    row(text("Currency"), text("INR")),
    row(text("Ageing basis"), text(outstandingsAgeingAnchorLabel(ageingAnchor))),
    "",
    row(text("Measure"), text("Amount")),
    row(text("Receivable"), number(report.receivable_total)),
    row(text("Payable"), number(report.payable_total)),
    ...(unallocatedTotal === undefined ? [] : [row(text("Unallocated (no bill reference)"), number(unallocatedTotal))]),
    "",
    row(text("Receivable ageing (bill references only)"), text("Amount"), text("Bills")),
    row(text("0-30 days"), number(report.ageing.days_0_30), number(report.ageing_bill_counts.days_0_30)),
    row(text("31-60 days"), number(report.ageing.days_31_60), number(report.ageing_bill_counts.days_31_60)),
    row(text("61-90 days"), number(report.ageing.days_61_90), number(report.ageing_bill_counts.days_61_90)),
    row(text("90+ days"), number(report.ageing.days_90_plus), number(report.ageing_bill_counts.days_90_plus)),
    "",
    row(text("Party"), text("Receivable"), text("Payable"), text("Outstanding"), text("Oldest bill (days)")),
    ...report.top_parties.map((party) => row(
      text(party.party),
      number(party.receivable),
      number(party.payable),
      number(party.outstanding_total),
      party.oldest_bill_age_days === null ? text("no bill reference") : number(party.oldest_bill_age_days),
    )),
  ];

  if (unallocatedByParty && unallocatedByParty.length > 0) {
    lines.push("", row(text("Unallocated by party"), text("Amount")));
    for (const entry of unallocatedByParty) lines.push(row(text(entry.party), number(entry.amount)));
  }

  if (report.has_unaged_receivable) {
    lines.push("", row(text("Note"), text("Receivable includes entries with no bill reference. Tally gives them no bill and no age, so they are excluded from the ageing buckets.")));
  }
  return lines.join("\n");
}

function fileNameOf(path: string) {
  const parts = path.split(/[\\/]/);
  return parts[parts.length - 1] || path;
}

/// "Show in Finder" on macOS, "Show in Explorer" on Windows -- naming the
/// user's own file manager rather than a generic "Open folder".
function revealLabel() {
  const platform = navigator.userAgent;
  if (platform.includes("Mac")) return "Show in Finder";
  if (platform.includes("Win")) return "Show in Explorer";
  return "Open folder";
}

/// Bills with no reference carry `oldest_bill_age_days: null`. They sort
/// after every aged party regardless of direction -- "no bill reference" is
/// not younger or older than an actual age, so a direction flip must not
/// move it to the top.
function comparePartiesBy(sort: PartySort) {
  return (a: TopParty, b: TopParty) => {
    let cmp: number;
    if (sort.key === "party") {
      cmp = a.party.localeCompare(b.party);
    } else if (sort.key === "outstanding") {
      cmp = amountOf(a.outstanding_total) - amountOf(b.outstanding_total);
    } else {
      if (a.oldest_bill_age_days === null && b.oldest_bill_age_days === null) return 0;
      if (a.oldest_bill_age_days === null) return 1;
      if (b.oldest_bill_age_days === null) return -1;
      cmp = a.oldest_bill_age_days - b.oldest_bill_age_days;
    }
    return sort.direction === "asc" ? cmp : -cmp;
  };
}

function amountOf(value: string) {
  if (!/^-?\d+(?:\.\d+)?$/.test(value)) {
    throw new Error("Bridge could not read an outstandings amount.");
  }
  const parsed = Number(value);
  if (!Number.isFinite(parsed)) {
    throw new Error("Bridge could not read an outstandings amount.");
  }
  return parsed;
}

/// Ageing rows carry their own bar width. Widths are relative to the LARGEST
/// bucket, not to the total: on a book where one bucket holds everything, a
/// total-relative scale renders the other three as invisible slivers and the
/// distribution reads as a single block.
function ageingRows(report: Report) {
  const rows = [
    { label: "0–30", amount: report.ageing.days_0_30, count: report.ageing_bill_counts.days_0_30, tier: 1 },
    { label: "31–60", amount: report.ageing.days_31_60, count: report.ageing_bill_counts.days_31_60, tier: 2 },
    { label: "61–90", amount: report.ageing.days_61_90, count: report.ageing_bill_counts.days_61_90, tier: 3 },
    { label: "90+", amount: report.ageing.days_90_plus, count: report.ageing_bill_counts.days_90_plus, tier: 4 },
  ];
  const largest = Math.max(...rows.map((row) => amountOf(row.amount)), 0);
  return rows.map((row) => ({
    ...row,
    percent: largest > 0 ? (amountOf(row.amount) / largest) * 100 : 0,
  }));
}

function slugify(value: string) {
  return value.replace(/[^a-zA-Z0-9]+/g, "-").toLowerCase();
}

/// Bill rows for one party's drill-down.
///
/// `state` is undefined only when this party genuinely has zero bill rows in
/// the complete source Bridge received -- a real fact, rendered as one: "no
/// bill references" (common, and on some books most parties, since exposure
/// can sit entirely in unallocated entries). A `"not_loaded"` state must
/// never reach this same rendering: it means Bridge already holds this
/// party's remaining bills, just did not render all of them by default, and
/// that is said plainly rather than shown as an empty, unallocated party.
function renderPartyBills(
  state: PartyBillsState | undefined,
  currencyAssertion: "INR",
  fullyLoaded: boolean,
  onLoadAll: () => void,
) {
  if (!state) {
    return <p className="party-bills-empty">No bill references — this party's balance is unallocated.</p>;
  }
  const bills = state.status === "not_loaded" && !fullyLoaded ? state.shown : state.bills;
  return (
    <>
      {state.status === "not_loaded" && (
        <p className="party-bills-not-loaded" role="status">
          {fullyLoaded
            ? `All ${state.bills.length.toLocaleString("en-IN")} bills loaded.`
            : (
              <>
                Showing {state.shown.length.toLocaleString("en-IN")} of {state.bills.length.toLocaleString("en-IN")} bills for this party — the rest are not loaded below.
                {" "}
                <button type="button" className="party-statement-action" onClick={onLoadAll}>
                  Load all {state.bills.length.toLocaleString("en-IN")} bills
                </button>
              </>
            )}
        </p>
      )}
      <div className="party-bills-table" role="table" aria-label="Open bills">
        <div className="party-bills-heading" role="row">
          <span role="columnheader">Reference</span>
          <span role="columnheader">Bill date</span>
          <span role="columnheader">Amount</span>
          <span role="columnheader">Age</span>
        </div>
        {bills.map((bill, index) => {
          // A bill and due date that differ mean this party has a credit
          // period -- the reason Tally's ageing can outrun a naive
          // bill-date calculation, and worth surfacing rather than hiding.
          const hasCreditPeriod = bill.due_date !== bill.bill_date;
          return (
            <div className="party-bill-row" role="row" key={`${bill.reference}-${bill.bill_date}-${index}`}>
              <span role="cell" className="party-bill-reference">{bill.reference || "—"}</span>
              <span role="cell" className="party-bill-dates">
                {formatDate(bill.bill_date)}
                {hasCreditPeriod && <em className="party-bill-due">due {formatDate(bill.due_date)}</em>}
              </span>
              <strong role="cell">{formatMoney(bill.amount, currencyAssertion)}</strong>
              {bill.age_days === null
                ? <em role="cell" className="age-chip is-none">not due</em>
                : <em role="cell" className={`age-chip tier-${ageTier(bill.age_days)}`}>{bill.age_days}d</em>}
            </div>
          );
        })}
      </div>
    </>
  );
}

function ageTier(days: number) {
  if (days <= 30) return 1;
  if (days <= 60) return 2;
  if (days <= 90) return 3;
  return 4;
}

function exposureComposition(report: Report, unallocatedTotal: string | undefined) {
  const slices = [
    { label: "Receivable", value: amountOf(report.receivable_total), tone: "receivable" },
    { label: "Payable", value: amountOf(report.payable_total), tone: "payable" },
    ...(unallocatedTotal === undefined
      ? []
      : [{ label: "Unallocated", value: amountOf(unallocatedTotal), tone: "unallocated" }]),
  ].filter((slice) => slice.value > 0);
  const total = slices.reduce((sum, slice) => sum + slice.value, 0);
  if (total <= 0) return null;
  return slices.map((slice) => {
    const percent = (slice.value / total) * 100;
    // Sub-1% slices would otherwise round to "0%" while still being drawn.
    const share = percent < 1 ? "<1%" : `${Math.round(percent)}%`;
    return { ...slice, share };
  });
}

// Describes what was actually read. The native bills path reads no vouchers at
// all, so reporting a voucher count there would be a false provenance claim —
// and "0 vouchers verified" reads as a failure rather than as a different,
// cheaper read.
function readProvenance(report: Report) {
  if (report.source_voucher_count > 0) {
    return `${report.source_voucher_count.toLocaleString("en-IN")} vouchers verified`;
  }
  const bills = report.open_receivable_bill_count.toLocaleString("en-IN");
  return `${bills} open ${report.open_receivable_bill_count === 1 ? "bill" : "bills"} read from Tally`;
}

function formatMoney(value: string, currencyAssertion: "INR") {
  const negative = value.startsWith("-");
  const unsigned = negative ? value.slice(1) : value;
  const [whole, fraction] = unsigned.split(".");
  const tail = whole.slice(-3);
  const head = whole.slice(0, -3).replace(/\B(?=(\d{2})+(?!\d))/g, ",");
  const grouped = head ? `${head},${tail}` : tail;
  return `${negative ? "−" : ""}${outstandingsCurrencySymbol(currencyAssertion)}${grouped}${fraction ? `.${fraction.padEnd(2, "0")}` : ""}`;
}

function isInrCompleteResult(result: LoadResult | null): result is InrCompleteResult {
  return result?.state === "complete" && result.currency_assertion === "INR";
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
