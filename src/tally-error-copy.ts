// SPDX-License-Identifier: Apache-2.0

type TallyErrorDetails = {
  code?: string;
  message: string;
};

export function classifyTallyError({ code, message }: TallyErrorDetails) {
  if (code === "tally_request_deadline_exceeded") {
    return {
      category: "Tally is taking longer than expected",
      action: "Bridge did not change data in Tally. Do not repeat the same request. Wait until Tally is responsive, then review the connection and company before making a changed request.",
    };
  }
  if (code === "response_validation_failed") {
    return {
      category: "Tally returned a result Bridge could not safely use",
      action: "Keep the result unverified. Confirm that Tally is responsive and the intended company is open, then check Tally again.",
    };
  }
  if (code === "tally_company_context_failed" || code === "reviewed_company_scope_changed") {
    return {
      category: "Open the company in Tally first",
      action: "Open the intended company in Tally, then return here and check Tally again.",
    };
  }
  if (code === "endpoint_configuration_invalid") {
    return {
      category: "Check the Tally address",
      action: "Confirm the local address and Tally XML server, then check Tally again.",
    };
  }
  return classifyUnstructuredTallyError(message);
}

export function classifyUnstructuredTallyError(message: string) {
  const value = message.toLowerCase();
  if (value.includes("permission") || value.includes("education") || value.includes("mode")) {
    return { category: "Permission or mode", action: "Confirm this operation is supported by the active Tally mode and company permissions." };
  }
  if (value.includes("parse") || value.includes("xml") || value.includes("schema") || value.includes("payload")) {
    return { category: "Tally response needs review", action: "Bridge kept the result unverified. Check Tally, then start a fresh connection check." };
  }
  if (value.includes("reconcil") || value.includes("mismatch") || value.includes("proof")) {
    return { category: "Results need review", action: "Keep the result unverified and review the saved evidence before making another request." };
  }
  if (value.includes("status") || value.includes("company") || value.includes("tally_export")) {
    return { category: "Check the Tally company", action: "Make sure the intended company is open in Tally, then start a fresh connection check." };
  }
  if (value.includes("host") || value.includes("port") || value.includes("endpoint") || value.includes("connect")) {
    return { category: "Check the Tally address", action: "Confirm the local address and Tally XML server, then start a fresh connection check." };
  }
  return {
    category: "Bridge could not complete this request",
    action: "Bridge cannot confirm the final state. Do not retry the same request yet; check Tally and the connection first.",
  };
}
