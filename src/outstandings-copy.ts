// SPDX-License-Identifier: Apache-2.0

export type OutstandingsAgeingAnchor = "due_date" | "bill_date";

export function outstandingsAgeingAnchorLabel(anchor: OutstandingsAgeingAnchor) {
  return anchor === "due_date" ? "aged from due date" : "aged from bill date";
}

export function outstandingsPartialReason(
  value: string,
  requestedAsOf?: string,
  tallyAsOf?: string,
) {
  if (value === "native_outstandings_as_of_refused") {
    if (requestedAsOf && tallyAsOf) {
      return `Tally refused the requested as-of date (${requestedAsOf}) and returned overdue days as of ${tallyAsOf}, so Bridge withheld the totals`;
    }
    return "Tally did not use the requested as-of date, so Bridge withheld the totals";
  }
  if (value === "native_overdue_crosscheck_mismatch") {
    return "Tally's overdue-day cross-check disagreed with the bill due dates, so Bridge withheld the totals";
  }
  if (value === "native_outstandings_as_of_unconfirmed_without_bill_references") {
    return "Tally returned no bill references while the ledger still carried a balance, so Bridge could not confirm the requested as-of date and withheld the totals";
  }
  if (value === "company_currency_probe_failed") {
    return "Bridge could not verify this company's base currency";
  }
  if (value === "company_base_currency_not_inr") {
    return "this company's verified base currency is not INR";
  }
  if (value === "company_outstandings_read_failed") {
    return "this company read failed while the remaining companies continued";
  }
  if (value === "tally_segment_latency_trending_restart_recommended") {
    return "comparable segments kept slowing toward the safety deadline; Tally may need a restart before another sync";
  }
  if (value === "tally_segment_deadline_restart_recommended") {
    return "a segment reached the safety deadline; Tally may need a restart before another sync";
  }
  if (value === "segment_response_size_limit_exceeded") {
    return "a wildcard segment exceeded Bridge's response safety bound";
  }
  if (value === "outstandings_segment_sizing_uncalibrated") {
    return "Bridge has no approved production segment size yet; no voucher read was sent";
  }
  if (value === "outstandings_segment_plan_exceeds_budget") {
    return "this book needs more read segments than Bridge can safely verify in one sync; no voucher scan started";
  }
  if (value === "company_voucher_alter_id_high_water_missing") {
    return "Tally did not return the voucher limit Bridge needs to prove complete coverage";
  }
  if (value === "ledger_opening_bills_not_covered") {
    return "Bridge found bill-wise opening balances that the voucher scan cannot verify, so totals stay withheld";
  }
  if (value === "unallocated_direct_postings_not_covered") {
    return "Bridge cannot yet prove balances posted without a bill reference, so totals stay withheld before any voucher read";
  }
  if (value === "whole_book_false_empty") {
    return "Tally reported existing vouchers but the complete tiled date scan returned no rows";
  }
  if (value === "empty_segment_contradicted_by_wider_read") {
    return "a wider date check found vouchers inside a supposedly empty period";
  }
  if (value === "voucher_outside_requested_window") {
    return "Tally returned a voucher outside the requested date period";
  }
  return value.replace(/_/g, " ");
}

export type OutstandingsPartialState = {
  title: string;
  message: string;
  retryable: boolean;
  tallyReadAttempted: boolean;
};

export function outstandingsPartialState(
  reasonCode: string,
  requestedAsOf?: string,
  tallyAsOf?: string,
): OutstandingsPartialState {
  if (reasonCode === "native_outstandings_as_of_refused") {
    return {
      title: "Tally did not accept this as-of date",
      message: outstandingsPartialReason(reasonCode, requestedAsOf, tallyAsOf),
      retryable: true,
      tallyReadAttempted: true,
    };
  }
  if (reasonCode === "native_outstandings_as_of_unconfirmed_without_bill_references") {
    return {
      title: "Tally did not confirm this as-of date",
      message: outstandingsPartialReason(reasonCode, requestedAsOf, tallyAsOf),
      retryable: true,
      tallyReadAttempted: true,
    };
  }
  if (reasonCode === "outstandings_segment_sizing_uncalibrated") {
    return {
      title: "Outstandings aren’t available yet",
      message: "Bridge isn’t ready to calculate this report safely. It didn’t read anything from Tally or calculate totals. Changing Tally settings won’t resolve this.",
      retryable: false,
      tallyReadAttempted: false,
    };
  }
  if (reasonCode === "unallocated_direct_postings_not_covered") {
    return {
      title: "Outstandings are not available for this company",
      message: "Bridge cannot yet verify balances posted without a bill reference. It did not calculate totals. Changing Tally settings won’t resolve this.",
      retryable: false,
      tallyReadAttempted: false,
    };
  }
  if (reasonCode === "ledger_opening_bills_not_covered") {
    return {
      title: "Outstandings are not available for this company",
      message: "Bridge completed a coverage check, but bill-wise opening balances fall outside the current read scope. It did not calculate totals. Repeating the same scan won't resolve this.",
      retryable: false,
      tallyReadAttempted: true,
    };
  }
  return {
    title: "Partial result withheld",
    message: `Bridge could not prove every requested segment complete (${outstandingsPartialReason(reasonCode)}). No totals were calculated.`,
    retryable: true,
    tallyReadAttempted: true,
  };
}

export function isNonRetryableOutstandingsBoundary(value: string) {
  return !outstandingsPartialState(value).retryable;
}

export function outstandingsAgeingDisclosure(
  hasUnagedReceivable: boolean,
  unallocatedTotalKnown = false,
) {
  if (!hasUnagedReceivable) return null;
  if (unallocatedTotalKnown) {
    // The native bills path recovers the unallocated balance exactly from the
    // party ledgers, so the honest disclosure is now "shown separately" rather
    // than "cannot be proven".
    return "Receivable includes entries with no bill reference. Tally gives them no bill and no age, so they are excluded from these buckets and shown as Unallocated above.";
  }
  return "Receivable includes On Account entries that are excluded from these buckets. Tally gives them no bill reference or age. Bridge does not show an On Account amount because this voucher read cannot prove the full unallocated balance.";
}
