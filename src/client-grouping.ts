// SPDX-License-Identifier: Apache-2.0

export type ClientGroupLabels = Record<string, string>;

export type ClientGroupLabelResolution = {
  label: string | undefined;
  /// A pre-composite raw-GUID label exists but cannot safely be assigned to
  /// one of several year-end-split books.
  ambiguousLegacyLabel: boolean;
};

function legacyLabelForGuid(labels: ClientGroupLabels, guid: string): string | undefined {
  const normalizedGuid = guid.trim().toLowerCase();
  if (!normalizedGuid) return undefined;
  for (const [key, label] of Object.entries(labels)) {
    if (key.trim().toLowerCase() === normalizedGuid) return label.trim() || undefined;
  }
  return undefined;
}

/// Resolves a label for the complete observed company identity. Labels from
/// releases that keyed by GUID alone remain usable for a single listed book,
/// but are never guessed onto either side of a split-GUID collision.
export function resolveClientGroupLabel(
  labels: ClientGroupLabels,
  companyKey: string,
  sourceGuid: string | undefined,
  listedSourceGuids: readonly (string | undefined)[],
): ClientGroupLabelResolution {
  const exact = labels[companyKey]?.trim();
  if (exact) return { label: exact, ambiguousLegacyLabel: false };
  if (!sourceGuid) return { label: undefined, ambiguousLegacyLabel: false };

  const legacy = legacyLabelForGuid(labels, sourceGuid);
  if (!legacy) return { label: undefined, ambiguousLegacyLabel: false };
  const normalizedGuid = sourceGuid.trim().toLowerCase();
  const matchingBooks = listedSourceGuids.filter(
    (guid) => guid?.trim().toLowerCase() === normalizedGuid,
  ).length;
  return matchingBooks === 1
    ? { label: legacy, ambiguousLegacyLabel: false }
    : { label: undefined, ambiguousLegacyLabel: true };
}

export function applyClientGroupLabel(
  labels: ClientGroupLabels,
  companyKey: string,
  label: string,
): ClientGroupLabels {
  const next = { ...labels };
  const normalized = label.trim();
  if (normalized) next[companyKey] = normalized;
  else delete next[companyKey];
  return next;
}

export function rollbackFailedClientGroupLabel(
  current: ClientGroupLabels,
  companyKey: string,
  attemptedLabel: string,
  persisted: ClientGroupLabels,
): ClientGroupLabels {
  if ((current[companyKey] ?? "").trim() !== attemptedLabel.trim()) {
    return current;
  }
  return applyClientGroupLabel(current, companyKey, persisted[companyKey] ?? "");
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
  companyKey: string,
): { sequence: ClientGroupLabelSaveSequence; stamp: number } {
  const stamp = (sequence[companyKey] ?? 0) + 1;
  return { sequence: { ...sequence, [companyKey]: stamp }, stamp };
}

/// True only for the response belonging to the most recently issued save for
/// this company. A save superseded by a later one (whether it goes on to
/// succeed or fail) must be treated as inert by the caller: it must not
/// touch `persisted`, roll back the UI, or surface an error -- doing so
/// would report on a request the user has already moved past.
export function isLatestClientGroupLabelSave(
  sequence: ClientGroupLabelSaveSequence,
  companyKey: string,
  stamp: number,
): boolean {
  return (sequence[companyKey] ?? 0) === stamp;
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
  sourceGuid?: string;
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
  const listedSourceGuids = rows.map((row) => row.sourceGuid);

  for (const row of rows) {
    const { label } = resolveClientGroupLabel(
      labels,
      row.companyGuid,
      row.sourceGuid,
      listedSourceGuids,
    );
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
