// SPDX-License-Identifier: Apache-2.0

export function outstandingsPartialReason(value: string) {
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
