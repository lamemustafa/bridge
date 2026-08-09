// SPDX-License-Identifier: Apache-2.0

import React from "react";
import { ChevronRight, RefreshCw } from "lucide-react";
import { invoke } from "@tauri-apps/api/core";
import { ClientGroupLabels, groupClientRows } from "./client-grouping";
import { outstandingsPartialState } from "./outstandings-copy";

type CompanyRef = { name: string; guid: string };

type Props = {
  config: { host: string; port: number };
  companies: CompanyRef[];
  onOpenCompany: (company: CompanyRef) => void;
  /// Returns to the single-company view. The two screens are the same
  /// question at two altitudes, so the switch has to work both ways.
  onBack?: () => void;
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
  | { state: "partial"; reason_code: string };

type Entry = { company: string; company_guid: string; result: LoadResult };

function amountOf(value: string | undefined) {
  if (!value || !/^-?\d+(?:\.\d+)?$/.test(value)) return null;
  const parsed = Number(value);
  return Number.isFinite(parsed) ? parsed : null;
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

/// Compact form for a wide table: a crore figure at full precision makes every
/// column unreadable, and at this altitude the reader is comparing clients, not
/// reconciling paise. The exact figure is one click away on the client's own
/// screen, and in the export.
function formatCompact(value: string | undefined) {
  const amount = amountOf(value);
  if (amount === null) return "Amount unavailable";
  if (amount === 0) return "—";
  if (amount >= 10_000_000) return `₹${(amount / 10_000_000).toFixed(2)} cr`;
  if (amount >= 100_000) return `₹${(amount / 100_000).toFixed(2)} L`;
  return `₹${Math.round(amount).toLocaleString("en-IN")}`;
}

type SortKey = "client" | "receivable" | "overdue" | "unallocated" | "oldest";

/// Severity tiers reuse the ageing ramp used on the single-client screen, so a
/// chip means the same thing in both places.
function ageTier(days: number | null) {
  if (days === null) return 0;
  if (days <= 30) return 1;
  if (days <= 60) return 2;
  if (days <= 90) return 3;
  return 4;
}

export function AllClientsScreen({ config, companies, onOpenCompany, onBack }: Props) {
  const [sort, setSort] = React.useState<{ key: SortKey; desc: boolean }>({ key: "overdue", desc: true });
  const [entries, setEntries] = React.useState<Entry[] | null>(null);
  const [loading, setLoading] = React.useState(false);
  const [error, setError] = React.useState<string | null>(null);
  const [groupLabels, setGroupLabels] = React.useState<ClientGroupLabels>({});
  const [groupLabelError, setGroupLabelError] = React.useState<string | null>(null);
  const requestVersion = React.useRef(0);

  React.useEffect(() => {
    let active = true;
    void invoke<ClientGroupLabels>("load_client_group_labels")
      .then((labels) => {
        if (active) setGroupLabels(labels);
      })
      // A label is optional. If its local config cannot be read, continue as
      // ungrouped instead of turning the report into a failed screen.
      .catch(() => {
        if (active) setGroupLabels({});
      });
    return () => {
      active = false;
    };
  }, []);

  const load = React.useCallback(async () => {
    if (companies.length === 0) return;
    const version = requestVersion.current + 1;
    requestVersion.current = version;
    setLoading(true);
    setError(null);
    try {
      const next = await invoke<Entry[]>("fetch_tally_outstandings_all_companies", {
        request: {
          config,
          companies: companies.map((company) => ({
            company: company.name,
            expected_company_guid: company.guid,
          })),
          currency_assertion: "INR",
        },
      });
      if (requestVersion.current !== version) return;
      setEntries(next);
    } catch (cause) {
      if (requestVersion.current !== version) return;
      setEntries(null);
      setError(
        cause && typeof cause === "object" && "message" in cause && typeof cause.message === "string"
          ? cause.message
          : "The local Tally read did not complete.",
      );
    } finally {
      if (requestVersion.current === version) setLoading(false);
    }
  }, [config.host, config.port, companies.map((company) => company.guid).join("|")]);

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
          companyGuid: entry.company_guid,
          complete,
          reasonCode: entry.result.state === "partial" ? entry.result.reason_code : null,
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

  const updateGroupLabel = React.useCallback((companyGuid: string, label: string) => {
    setGroupLabels((current) => {
      const next = { ...current };
      if (label.trim()) next[companyGuid] = label;
      else delete next[companyGuid];
      return next;
    });
  }, []);

  const saveGroupLabel = React.useCallback((companyGuid: string, label: string) => {
    setGroupLabelError(null);
    void invoke("save_client_group_label", {
      request: { company_guid: companyGuid, label },
    }).catch(() => setGroupLabelError("Bridge could not save this group label. Your figures are unchanged."));
  }, []);

  const renderRow = (row: (typeof rows)[number]) => {
    const partial = row.reasonCode ? outstandingsPartialState(row.reasonCode) : null;
    return (
      <button
        className="clients-row"
        role="row"
        type="button"
        key={row.companyGuid}
        onClick={() => {
          const match = companies.find((company) => company.guid === row.companyGuid);
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
            ? <em>{partial.title}</em>
            : row.unallocatedShare !== null && (
              <em>{row.unallocatedShare}% carries no bill reference</em>
            )}
        </span>
        <span role="cell">{row.complete ? formatCompact(row.exactAmounts.receivable) : "—"}</span>
        <span role="cell" className={row.overdue !== null && row.overdue > 0 ? "is-overdue" : undefined}>
          {row.complete ? formatCompact(row.exactAmounts.overdue) : "—"}
        </span>
        <span role="cell">{row.complete ? formatCompact(row.exactAmounts.unallocated) : "—"}</span>
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
        </div>
        <div className="outstandings-heading-actions">
          {onBack && (
            <button className="secondary-action" type="button" onClick={onBack}>
              Back to one client
            </button>
          )}
          <button type="button" onClick={() => void load()} disabled={loading || companies.length === 0}>
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
              {companies.map((company) => (
                <label key={company.guid}>
                  <span>{company.name}</span>
                  <input
                    aria-label={`Group label for ${company.name}`}
                    value={groupLabels[company.guid] ?? ""}
                    placeholder="No group"
                    onChange={(event) => updateGroupLabel(company.guid, event.target.value)}
                    onBlur={(event) => saveGroupLabel(company.guid, event.target.value)}
                  />
                </label>
              ))}
            </div>
            {groupLabelError && <p className="client-group-label-error" role="alert">{groupLabelError}</p>}
          </section>

          <div className="clients-table" role="table" aria-label="Outstandings by client">
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
                  onClick={() => setSort((current) =>
                    current.key === key
                      ? { key, desc: !current.desc }
                      : { key, desc: key !== "client" })}
                >
                  {label}
                </button>
              ))}
            </div>
            {groupedRows.groups.map((group) => (
              <React.Fragment key={group.label}>
                <div className="clients-row clients-group-total" role="row" aria-label={`Totals for ${group.label}`}>
                  <span role="cell" className="clients-name"><strong>{group.label}</strong><em>Group total</em></span>
                  <span role="cell">{formatCompact(group.totals.receivable)}</span>
                  <span role="cell" className={(amountOf(group.totals.overdue) ?? 0) > 0 ? "is-overdue" : undefined}>
                    {formatCompact(group.totals.overdue)}
                  </span>
                  <span role="cell">{formatCompact(group.totals.unallocated)}</span>
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
