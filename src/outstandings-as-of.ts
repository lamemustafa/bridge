// SPDX-License-Identifier: Apache-2.0

/** Local calendar date for a native date input; UTC conversion would show a
 * different day for operators west/east of the UTC boundary. */
export function todayAsDateInput(now = new Date()) {
  const year = now.getFullYear();
  const month = String(now.getMonth() + 1).padStart(2, "0");
  const day = String(now.getDate()).padStart(2, "0");
  return `${year}-${month}-${day}`;
}

/** The Tauri contract accepts only canonical YYYYMMDD values. */
export function asOfYyyymmdd(value: string) {
  return /^\d{4}-\d{2}-\d{2}$/.test(value) ? value.replace(/-/g, "") : null;
}
