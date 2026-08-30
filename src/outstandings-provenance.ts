// SPDX-License-Identifier: Apache-2.0

export type OutstandingsReadProvenance = {
  read_strategy: "native_bills" | "voucher_scan";
  source_voucher_count: number;
  open_receivable_bill_count: number;
};

// Strategy comes from the Rust result. A count can be zero on either path, so
// it cannot establish which read produced the report.
export function readProvenance(report: OutstandingsReadProvenance) {
  if (report.read_strategy === "voucher_scan") {
    const vouchers = report.source_voucher_count.toLocaleString("en-IN");
    return `${vouchers} ${report.source_voucher_count === 1 ? "voucher" : "vouchers"} verified`;
  }
  const bills = report.open_receivable_bill_count.toLocaleString("en-IN");
  return `${bills} open receivable ${report.open_receivable_bill_count === 1 ? "bill" : "bills"} read from Tally`;
}
