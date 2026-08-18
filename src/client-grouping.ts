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
