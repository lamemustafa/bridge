// SPDX-License-Identifier: Apache-2.0

export type ClientGroupLabels = Record<string, string>;

export type GroupableClientRow = {
  companyGuid: string;
  receivable: number;
  overdue: number;
  unallocated: number;
};

export type ClientGroup<Row extends GroupableClientRow> = {
  label: string;
  rows: Row[];
  totals: { receivable: number; overdue: number; unallocated: number };
};

function totalRows(rows: readonly GroupableClientRow[]) {
  return rows.reduce(
    (total, row) => ({
      receivable: total.receivable + row.receivable,
      overdue: total.overdue + row.overdue,
      unallocated: total.unallocated + row.unallocated,
    }),
    { receivable: 0, overdue: 0, unallocated: 0 },
  );
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
