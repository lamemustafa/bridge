// SPDX-License-Identifier: Apache-2.0

export type OutstandingsReadProvenance = {
  source_voucher_count: number;
  open_receivable_bill_count: number;
};

// Describes what was actually read. The native bills path reads no vouchers at
// all, so reporting a voucher count there would be a false provenance claim —
// and "0 vouchers verified" reads as a failure rather than as a different,
// cheaper read.
export function readProvenance(report: OutstandingsReadProvenance) {
  if (report.source_voucher_count > 0) {
    return `${report.source_voucher_count.toLocaleString("en-IN")} vouchers verified`;
  }
  const bills = report.open_receivable_bill_count.toLocaleString("en-IN");
  return `${bills} open ${report.open_receivable_bill_count === 1 ? "bill" : "bills"} read from Tally`;
}
