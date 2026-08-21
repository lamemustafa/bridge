// SPDX-License-Identifier: Apache-2.0

export type ClientGroupLabels = Record<string, string>;

export function applyClientGroupLabel(
  labels: ClientGroupLabels,
  companyGuid: string,
  label: string,
): ClientGroupLabels {
  const next = { ...labels };
  const normalized = label.trim();
  if (normalized) next[companyGuid] = normalized;
  else delete next[companyGuid];
  return next;
}

export function rollbackFailedClientGroupLabel(
  current: ClientGroupLabels,
  companyGuid: string,
  attemptedLabel: string,
  persisted: ClientGroupLabels,
): ClientGroupLabels {
  if ((current[companyGuid] ?? "").trim() !== attemptedLabel.trim()) {
    return current;
  }
  return applyClientGroupLabel(current, companyGuid, persisted[companyGuid] ?? "");
}

/// Per-company counter of the most recently ISSUED group-label save. There is
/// no lock here -- callers keep firing saves as fast as the user types -- but
/// every in-flight request can be stamped with the counter's value at the
/// moment it was issued, so a later settle can tell whether it is still the
/// save that matters.
export type ClientGroupLabelSaveSequence = Record<string, number>;

/// Call when a save is about to be fired. Returns the updated sequence table
/// (store it back) and the stamp to carry on that one request.
export function issueClientGroupLabelSave(
  sequence: ClientGroupLabelSaveSequence,
  companyGuid: string,
): { sequence: ClientGroupLabelSaveSequence; stamp: number } {
  const stamp = (sequence[companyGuid] ?? 0) + 1;
  return { sequence: { ...sequence, [companyGuid]: stamp }, stamp };
}

/// True only for the response belonging to the most recently issued save for
/// this company. A save superseded by a later one (whether it goes on to
/// succeed or fail) must be treated as inert by the caller: it must not
/// touch `persisted`, roll back the UI, or surface an error -- doing so
/// would report on a request the user has already moved past.
export function isLatestClientGroupLabelSave(
  sequence: ClientGroupLabelSaveSequence,
  companyGuid: string,
  stamp: number,
): boolean {
  return (sequence[companyGuid] ?? 0) === stamp;
}

export function reconcileLoadedSortPreference<Sort>(
  current: Sort,
  persisted: Sort,
  userChangedSort: boolean,
): Sort {
  return userChangedSort ? current : persisted;
}

export type GroupableClientRow = {
  companyGuid: string;
  exactAmounts: {
    receivable: string | undefined;
    overdue: string | undefined;
    unallocated: string | undefined;
  };
};

export type ClientGroup<Row extends GroupableClientRow> = {
  label: string;
  rows: Row[];
  totals: { receivable: string | undefined; overdue: string | undefined; unallocated: string | undefined };
};

function totalRows(rows: readonly GroupableClientRow[]) {
  return {
    receivable: sumExactDecimals(rows.map((row) => row.exactAmounts.receivable)),
    overdue: sumExactDecimals(rows.map((row) => row.exactAmounts.overdue)),
    unallocated: sumExactDecimals(rows.map((row) => row.exactAmounts.unallocated)),
  };
}

type ExactParts = { negative: boolean; whole: string; fraction: string };

function parseExactDecimal(value: string | undefined): ExactParts | undefined {
  const match = value?.match(/^(-?)(\d+)(?:\.(\d+))?$/);
  if (!match) return undefined;
  return { negative: match[1] === "-", whole: match[2], fraction: match[3] ?? "" };
}

/// Adds source decimal strings with `BigInt`, never through IEEE-754. If any
/// row is not a valid exact decimal, the group total is unavailable rather than
/// rounded into a believable figure.
export function sumExactDecimals(values: readonly (string | undefined)[]): string | undefined {
  const parsed = values.map(parseExactDecimal);
  if (parsed.some((value) => value === undefined)) return undefined;
  const exact = parsed as ExactParts[];
  const scale = Math.max(...exact.map((value) => value.fraction.length), 0);
  const total = exact.reduce((sum, value) => {
    const digits = `${value.whole}${value.fraction.padEnd(scale, "0")}`;
    const scaled = BigInt(digits) * (value.negative ? -1n : 1n);
    return sum + scaled;
  }, 0n);
  const negative = total < 0n;
  const unsigned = (negative ? -total : total).toString().padStart(scale + 1, "0");
  const whole = scale === 0 ? unsigned : unsigned.slice(0, -scale);
  const fraction = scale === 0 ? "" : unsigned.slice(-scale).replace(/0+$/, "");
  if (whole === "0" && !fraction) return "0";
  return `${negative ? "-" : ""}${whole}${fraction ? `.${fraction}` : ""}`;
}

/// Groups only labeled rows. Ungrouped companies stay as individual rows so
/// the screen never presents a synthetic catch-all total.
export function groupClientRows<Row extends GroupableClientRow>(
  rows: readonly Row[],
  labels: ClientGroupLabels,
): { groups: ClientGroup<Row>[]; ungroupedRows: Row[] } {
  const grouped = new Map<string, Row[]>();
  const ungroupedRows: Row[] = [];

  for (const row of rows) {
    const label = labels[row.companyGuid]?.trim();
    if (!label) {
      ungroupedRows.push(row);
      continue;
    }
    const group = grouped.get(label) ?? [];
    group.push(row);
    grouped.set(label, group);
  }

  const groups = [...grouped.entries()]
    .sort(([left], [right]) => left.localeCompare(right))
    .map(([label, groupRows]) => ({ label, rows: groupRows, totals: totalRows(groupRows) }));

  return { groups, ungroupedRows };
}
