export type CsvCell =
  | { kind: "text"; value: string }
  | { kind: "number"; value: string | number };

const ACTIVE_SPREADSHEET_PREFIX = /^[=+\-@\t\r]/;

export function csvTextCell(value: string | number): CsvCell {
  return { kind: "text", value: String(value) };
}

export function csvNumericCell(value: string | number): CsvCell {
  return { kind: "number", value };
}

export function csvRow(...values: Array<CsvCell>) {
  return values.map(serializeCsvCell).join(",");
}

function serializeCsvCell(cell: CsvCell) {
  const raw = String(cell.value);
  // A leading apostrophe makes spreadsheet applications treat an untrusted
  // label as text. Numeric cells are deliberately distinct: prefixing a
  // negative amount would silently break downstream SUM operations.
  const text = cell.kind === "text" && ACTIVE_SPREADSHEET_PREFIX.test(raw)
    ? `'${raw}`
    : raw;
  return /[",\r\n]/.test(text) ? `"${text.replace(/"/g, '""')}"` : text;
}
