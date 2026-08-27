// SPDX-License-Identifier: Apache-2.0

import React from "react";
import { ChevronRight, RefreshCw } from "lucide-react";
import { invoke } from "@tauri-apps/api/core";
import { applyClientGroupLabel, ClientGroupLabelSaveSequence, ClientGroupLabels, groupClientRows, isLatestClientGroupLabelSave, issueClientGroupLabelSave, reconcileLoadedSortPreference, rollbackFailedClientGroupLabel } from "./client-grouping";
import {
  outstandingsAgeingAnchorLabel,
  outstandingsPartialState,
  type OutstandingsAgeingAnchor,
} from "./outstandings-copy";
import {
  allCompaniesOutstandingsInvokeArgument,
  asOfBoundValueForAsOf,
  asOfYyyymmdd,
  settleAsOfBoundValue,
  type AsOfBoundValue,
} from "./outstandings-as-of";
import { outstandingsCurrencySymbol } from "./outstandings-currency";
import { companyIdentityKey } from "./company-identity";

type CompanyRef = {
  name: string;
  guid: string;
  company_number: string;
  books_from_yyyymmdd: string;
  canonical_origin: string;
};

type Props = {
  config: { host: string; port: number };
  companies: CompanyRef[];
  onOpenCompany: (company: CompanyRef) => void;
  /// Returns to the single-company view. The two screens are the same
  /// question at two altitudes, so the switch has to work both ways.
  onBack?: () => void;
  asOf: string;
};

type Report = {
  receivable_total: string;
  payable_total: string;
  ageing: { days_0_30: string; days_31_60: string; days_61_90: string; days_90_plus: string };
  open_receivable_bill_count: number;
  top_parties: Array<{ party: string; oldest_bill_age_days: number | null }>;
};

type LoadResult =
  | { state: "complete"; report: Report; unallocated_total?: string }
  | {
      state: "partial";
      reason_code: string;
      requested_as_of_yyyymmdd?: string;
      tally_as_of_yyyymmdd?: string;
      foreign_currency_ledger_name?: string;
    };

type Entry = {
  company: string;
  company_guid: string;
  company_number: string;
  books_from_yyyymmdd: string;
  canonical_origin: string;
  result: LoadResult;
};

function amountOf(value: string | undefined) {
  if (!value || !/^-?\d+(?:\.\d+)?$/.test(value)) return null;
  const parsed = Number(value);
  return Number.isFinite(parsed) ? parsed : null;
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

/// Compact form for a wide table: a crore figure at full precision makes every
/// column unreadable, and at this altitude the reader is comparing clients, not
/// reconciling paise. The exact figure is one click away on the client's own
/// screen, and in the export.
function formatCompact(value: string | undefined, currencyAssertion: "INR") {
  const amount = amountOf(value);
  if (amount === null) return "Amount unavailable";
  if (amount === 0) return "—";
  const symbol = outstandingsCurrencySymbol(currencyAssertion);
  if (amount >= 10_000_000) return `${symbol}${(amount / 10_000_000).toFixed(2)} cr`;
  if (amount >= 100_000) return `${symbol}${(amount / 100_000).toFixed(2)} L`;
  return `${symbol}${Math.round(amount).toLocaleString("en-IN")}`;
}

type SortKey = "client" | "receivable" | "overdue" | "unallocated" | "oldest";
type SortPreference = { key: SortKey; desc: boolean };

const defaultSort: SortPreference = { key: "overdue", desc: true };

function isSortPreference(value: unknown): value is SortPreference {
  return Boolean(
    value
      && typeof value === "object"
      && "key" in value
      && "desc" in value
      && ["client", "receivable", "overdue", "unallocated", "oldest"].includes(String(value.key))
      && typeof value.desc === "boolean",
  );
}

/// Severity tiers reuse the ageing ramp used on the single-client screen, so a
/// chip means the same thing in both places.
function ageTier(days: number | null) {
  if (days === null) return 0;
  if (days <= 30) return 1;
  if (days <= 60) return 2;
  if (days <= 90) return 3;
  return 4;
}

export function AllClientsScreen({ config, companies, onOpenCompany, onBack, asOf }: Props) {
  const [sort, setSort] = React.useState<SortPreference>(defaultSort);
  const [loadedEntries, setLoadedEntries] = React.useState<AsOfBoundValue<Entry[]> | null>(null);
  const [loading, setLoading] = React.useState(false);
  const [error, setError] = React.useState<string | null>(null);
  const [ageingAnchor, setAgeingAnchor] = React.useState<OutstandingsAgeingAnchor>("due_date");
  const [groupLabels, setGroupLabels] = React.useState<ClientGroupLabels>({});
  const [groupLabelError, setGroupLabelError] = React.useState<string | null>(null);
  const [groupLabelsReady, setGroupLabelsReady] = React.useState(false);
  const persistedGroupLabels = React.useRef<ClientGroupLabels>({});
  // Tracks, per company, the sequence number of the most recently ISSUED
  // group-label save. Saves are never serialized -- the user keeps typing
  // and each blur fires its own request -- so a response can arrive for a
  // save that a later one has already superseded. Comparing against this
  // table at settle time is what makes a superseded response inert instead
  // of clobbering a newer save's outcome.
  const groupLabelSaveSequence = React.useRef<ClientGroupLabelSaveSequence>({});
  const requestVersion = React.useRef(0);
  const sortChangedDuringLoad = React.useRef(false);
  const requestedAsOf = asOfYyyymmdd(asOf);
  const currentRequestedAsOf = React.useRef(requestedAsOf);
  currentRequestedAsOf.current = requestedAsOf;
  const entries = asOfBoundValueForAsOf(loadedEntries, requestedAsOf);
  React.useEffect(() => {
    // A new effective date makes any previous financial rows ineligible for
    // display. Invalidate issued sweeps too, so a late old-date response is
    // inert rather than being relabelled under the new date.
    requestVersion.current += 1;
    setLoadedEntries(null);
    setLoading(false);
    setError(null);
  }, [requestedAsOf]);

  React.useEffect(() => {
    requestVersion.current += 1;
    setLoadedEntries(null);
    setLoading(false);
    setError(null);
  }, [ageingAnchor]);

  React.useEffect(() => {
    let active = true;
    setGroupLabelsReady(false);
    void invoke<ClientGroupLabels>("load_client_group_labels")
      .then((labels) => {
        if (!active) return;
        persistedGroupLabels.current = labels;
        setGroupLabels(labels);
        setGroupLabelsReady(true);
      })
      // A label is optional. If its local config cannot be read, continue as
      // ungrouped instead of turning the report into a failed screen.
      .catch(() => {
        if (active) {
          persistedGroupLabels.current = {};
          setGroupLabels({});
          setGroupLabelsReady(true);
        }
      });
    return () => {
      active = false;
    };
  }, []);

  React.useEffect(() => {
    let active = true;
    void invoke<SortPreference | null>("load_client_sort_preference")
      .then((preference) => {
        if (active && isSortPreference(preference)) {
          setSort((current) => reconcileLoadedSortPreference(
            current,
            preference,
            sortChangedDuringLoad.current,
          ));
        }
      })
      // Sorting is an optional local display preference. Keep the safe default
      // if its config file cannot be read.
      .catch(() => {});
    return () => {
      active = false;
    };
  }, []);

  const load = React.useCallback(async () => {
    const argument = allCompaniesOutstandingsInvokeArgument(config, companies, asOf, ageingAnchor);
    if (companies.length === 0 || !argument) return;
    const requestedAsOfYyyymmdd = argument.request.as_of_yyyymmdd;
    const version = requestVersion.current + 1;
    requestVersion.current = version;
    setLoading(true);
    setError(null);
    try {
      const next = await invoke<Entry[]>("fetch_tally_outstandings_all_companies", argument);
      if (requestVersion.current !== version) return;
      const settled = settleAsOfBoundValue(
        currentRequestedAsOf.current,
        requestedAsOfYyyymmdd,
        next,
      );
      if (!settled) return;
      setLoadedEntries(settled);
    } catch (cause) {
      if (requestVersion.current !== version) return;
      setLoadedEntries(null);
      setError(
        cause && typeof cause === "object" && "message" in cause && typeof cause.message === "string"
          ? cause.message
          : "The local Tally read did not complete.",
      );
    } finally {
      if (requestVersion.current === version) setLoading(false);
    }
  }, [ageingAnchor, asOf, config.host, config.port, companies.map((company) => company.guid).join("|")]);

  const rows = React.useMemo(() => {
    if (!entries) return [];
    return entries
      .map((entry) => {
        const complete = entry.result.state === "complete" ? entry.result : null;
        const oldest = complete
          ? complete.report.top_parties.reduce<number | null>(
              (worst, party) =>
                party.oldest_bill_age_days === null
                  ? worst
                  : Math.max(worst ?? 0, party.oldest_bill_age_days),
              null,
            )
          : null;
        return {
          company: entry.company,
          // Group labels and React row keys must not merge year-end split
          // books that share a Tally GUID.
          companyGuid: companyIdentityKey({
            canonical_origin: entry.canonical_origin,
            company_guid: entry.company_guid,
            company_number: entry.company_number,
            company_name: entry.company,
            books_from_yyyymmdd: entry.books_from_yyyymmdd,
          }),
          sourceGuid: entry.company_guid,
          companyNumber: entry.company_number,
          booksFromYyyymmdd: entry.books_from_yyyymmdd,
          complete,
          reasonCode: entry.result.state === "partial" ? entry.result.reason_code : null,
          requestedAsOf: entry.result.state === "partial" ? entry.result.requested_as_of_yyyymmdd : undefined,
          tallyAsOf: entry.result.state === "partial" ? entry.result.tally_as_of_yyyymmdd : undefined,
          foreignCurrencyLedgerName: entry.result.state === "partial" ? entry.result.foreign_currency_ledger_name : undefined,
          receivable: complete ? amountOf(complete.report.receivable_total) : null,
          overdue: complete ? amountOf(complete.report.ageing.days_90_plus) : null,
          unallocated: complete ? amountOf(complete.unallocated_total) : null,
          exactAmounts: {
            receivable: complete ? complete.report.receivable_total : undefined,
            overdue: complete ? complete.report.ageing.days_90_plus : undefined,
            unallocated: complete ? complete.unallocated_total : undefined,
          },
          // How much of this book's exposure Tally cannot age. It is the
          // single best signal of whether the other numbers can be trusted,
          // and it varies enormously between books.
          unallocatedShare: complete
            && amountOf(complete.unallocated_total) !== null
            && amountOf(complete.report.receivable_total) !== null
            ? Math.round(
                amountOf(complete.unallocated_total)!
                  / Math.max(1, amountOf(complete.unallocated_total)! + amountOf(complete.report.receivable_total)!)
                  * 100,
              )
            : null,
          oldest,
        };
      })
      .sort((left, right) => {
        const direction = sort.desc ? -1 : 1;
        if (sort.key === "client") return left.company.localeCompare(right.company) * direction;
        if (sort.key === "oldest") {
          // A book with no aged bill has no "oldest" -- it must not sort as
          // zero and look the least urgent thing on the screen.
          const l = left.oldest ?? -1;
          const r = right.oldest ?? -1;
          return (l - r) * direction;
        }
        const leftAmount = left[sort.key];
        const rightAmount = right[sort.key];
        if (leftAmount === null && rightAmount === null) return 0;
        if (leftAmount === null) return 1;
        if (rightAmount === null) return -1;
        return (leftAmount - rightAmount) * direction;
      });
  }, [entries, sort]);

  const groupedRows = React.useMemo(
    () => groupClientRows(rows, groupLabels),
    [rows, groupLabels],
  );
  const readable = rows.filter((row) => row.complete).length;
  const largestExposure = Math.max(
    ...rows.map((row) => row.receivable === null || row.unallocated === null ? 0 : row.receivable + row.unallocated),
    0,
  );

  const updateGroupLabel = React.useCallback((companyKey: string, label: string) => {
    setGroupLabels((current) => applyClientGroupLabel(current, companyKey, label));
  }, []);

  const saveGroupLabel = React.useCallback((companyKey: string, label: string) => {
    const attemptedLabel = label.trim();
    setGroupLabelError(null);
    const { sequence, stamp } = issueClientGroupLabelSave(groupLabelSaveSequence.current, companyKey);
    groupLabelSaveSequence.current = sequence;
    void invoke("save_client_group_label", {
      request: {
        company_key: companyKey,
        label: attemptedLabel,
      },
    })
      .then(() => {
        // A later save for this company has already been issued: this
        // response is stale. Applying it would let a slow, superseded
        // success overwrite the record of whatever that newer save settles
        // to, so it is dropped instead.
        if (!isLatestClientGroupLabelSave(groupLabelSaveSequence.current, companyKey, stamp)) return;
        persistedGroupLabels.current = applyClientGroupLabel(
          persistedGroupLabels.current,
          companyKey,
          attemptedLabel,
        );
      })
      .catch(() => {
        // Same reasoning as above: a superseded failure must not roll back
        // the UI to this attempt's baseline, or claim (falsely) that the
        // previous label was restored.
        if (!isLatestClientGroupLabelSave(groupLabelSaveSequence.current, companyKey, stamp)) return;
        setGroupLabels((current) => rollbackFailedClientGroupLabel(
          current,
          companyKey,
          attemptedLabel,
          persistedGroupLabels.current,
        ));
        setGroupLabelError("Bridge could not save this group label. The previous label was restored; your figures are unchanged.");
      });
  }, []);

  const changeSort = React.useCallback((key: SortKey) => {
    const next = sort.key === key ? { key, desc: !sort.desc } : { key, desc: key !== "client" };
    sortChangedDuringLoad.current = true;
    setSort(next);
    void invoke("save_client_sort_preference", { preference: next }).catch(() => {});
  }, [sort]);

  const renderRow = (row: (typeof rows)[number]) => {
    const partial = row.reasonCode
      ? outstandingsPartialState(
        row.reasonCode,
        row.requestedAsOf,
        row.tallyAsOf,
        row.foreignCurrencyLedgerName,
      )
      : null;
    return (
      <button
        className="clients-row"
        role="row"
        type="button"
        key={row.companyGuid}
        onClick={() => {
          const match = companies.find((company) =>
            company.guid === row.sourceGuid
              && company.company_number === row.companyNumber
              && company.name === row.company
              && company.books_from_yyyymmdd === row.booksFromYyyymmdd,
          );
          if (match) onOpenCompany(match);
        }}
      >
        {/* Magnitude behind the row: which client is biggest is
            readable without comparing five columns of digits. */}
        <span
          className="clients-magnitude"
          style={{ width: `${largestExposure > 0 && row.receivable !== null && row.unallocated !== null ? Math.max(1, (row.receivable + row.unallocated) / largestExposure * 100) : 0}%` }}
          aria-hidden="true"
        />
        <span role="cell" className="clients-name">
          <strong>{row.company}</strong>
          {partial
            ? <><em>{partial.title}</em><em>{partial.message}</em></>
            : row.unallocatedShare !== null && (
              <em>{row.unallocatedShare}% carries no bill reference</em>
            )}
        </span>
        <span role="cell">{row.complete ? formatCompact(row.exactAmounts.receivable, "INR") : "—"}</span>
        <span role="cell" className={row.overdue !== null && row.overdue > 0 ? "is-overdue" : undefined}>
          {row.complete ? formatCompact(row.exactAmounts.overdue, "INR") : "—"}
        </span>
        <span role="cell">{row.complete ? formatCompact(row.exactAmounts.unallocated, "INR") : "—"}</span>
        <span role="cell" className="clients-age">
          {row.oldest === null
            ? <em className="age-chip is-none">none</em>
            : <em className={`age-chip tier-${ageTier(row.oldest)}`}>{row.oldest}d</em>}
          <ChevronRight size={16} aria-hidden="true" />
        </span>
      </button>
    );
  };

  return (
    <section className="clients-screen" aria-busy={loading}>
      <div className="outstandings-heading">
        <div>
          <h2>All clients</h2>
          <p>
            {entries
              ? `${readable} of ${rows.length} ${rows.length === 1 ? "book" : "books"} read`
              : `${companies.length} ${companies.length === 1 ? "book" : "books"} open in Tally`}
          </p>
          <p>As of {asOf}</p>
          <p>{outstandingsAgeingAnchorLabel(ageingAnchor)}</p>
        </div>
        <div className="outstandings-heading-actions">
          <label className="outstandings-as-of">
            <span>Age from</span>
            <select
              value={ageingAnchor}
              onChange={(event) => setAgeingAnchor(event.target.value as OutstandingsAgeingAnchor)}
              disabled={loading}
            >
              <option value="due_date">Due date</option>
              <option value="bill_date">Bill date</option>
            </select>
            <small>Applies to every client in this read.</small>
          </label>
          {onBack && (
            <button className="secondary-action" type="button" onClick={onBack}>
              Back to one client
            </button>
          )}
          <button type="button" onClick={() => void load()} disabled={loading || companies.length === 0 || !requestedAsOf}>
            <RefreshCw size={18} className={loading ? "spin" : undefined} />
            {loading ? "Reading each book…" : entries ? "Refresh" : "Read all clients"}
          </button>
        </div>
      </div>

      {error && <div className="outstandings-state error" role="alert"><strong>Read failed</strong><span>{error}</span></div>}

      {companies.length === 0 && (
        <div className="outstandings-state">
          <strong>No verified companies yet</strong>
          <span>Open your client books in Tally and choose them under Manage Tally. Bridge reads each one in turn.</span>
        </div>
      )}

      {!entries && !loading && companies.length > 0 && (
        <div className="outstandings-state">
          <strong>Ready</strong>
          <span>Bridge reads each book in turn, one request at a time. Roughly a third of a second per company.</span>
        </div>
      )}

      {entries && rows.length > 0 && (
        <>
          <section className="client-group-labels" aria-labelledby="client-group-labels-heading">
            <div>
              <h3 id="client-group-labels-heading">Client groups</h3>
              <p>Optional filing labels. Ungrouped clients stay separate.</p>
            </div>
            <div className="client-group-label-grid">
              {rows.map((row) => {
                return (
                  <label key={row.companyGuid}>
                    <span>{row.company}</span>
                    <input
                      aria-label={`Group label for ${row.company}`}
                      value={groupLabels[row.sourceGuid] ?? ""}
                      placeholder="No group"
                      disabled={!groupLabelsReady}
                      onChange={(event) => updateGroupLabel(
                        row.sourceGuid,
                        event.target.value,
                      )}
                      onBlur={(event) => saveGroupLabel(
                        row.sourceGuid,
                        event.target.value,
                      )}
                    />
                  </label>
                );
              })}
            </div>
            {groupLabelError && <p className="client-group-label-error" role="alert">{groupLabelError}</p>}
          </section>

          <div className="clients-table" role="table" aria-label="Outstandings by client" tabIndex={0}>
            <div className="clients-row is-head" role="row">
              {([
                ["client", "Client"],
                ["receivable", "Receivable"],
                ["overdue", "Overdue 90+"],
                ["unallocated", "Unallocated"],
                ["oldest", "Oldest"],
              ] as Array<[SortKey, string]>).map(([key, label]) => (
                <button
                  key={key}
                  role="columnheader"
                  type="button"
                  className={sort.key === key ? "is-sorted" : undefined}
                  aria-sort={sort.key === key ? (sort.desc ? "descending" : "ascending") : "none"}
                  onClick={() => changeSort(key)}
                >
                  {label}
                </button>
              ))}
            </div>
            {groupedRows.groups.map((group) => (
              <React.Fragment key={group.label}>
                <div className="clients-row clients-group-total" role="row" aria-label={`Totals for ${group.label}`}>
                  <span role="cell" className="clients-name"><strong>{group.label}</strong><em>Group total</em></span>
                  <span role="cell">{formatCompact(group.totals.receivable, "INR")}</span>
                  <span role="cell" className={(amountOf(group.totals.overdue) ?? 0) > 0 ? "is-overdue" : undefined}>
                    {formatCompact(group.totals.overdue, "INR")}
                  </span>
                  <span role="cell">{formatCompact(group.totals.unallocated, "INR")}</span>
                  <span role="cell" />
                </div>
                {group.rows.map(renderRow)}
              </React.Fragment>
            ))}
            {groupedRows.ungroupedRows.map(renderRow)}
          </div>
        </>
      )}
    </section>
  );
}
