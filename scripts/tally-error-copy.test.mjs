// SPDX-License-Identifier: Apache-2.0

import assert from "node:assert/strict";
import test from "node:test";

import { classifyTallyError, classifyUnstructuredTallyError } from "../src/tally-error-copy.ts";

test("a deadline explains the safe next step without exposing protocol jargon", () => {
  assert.deepEqual(
    classifyTallyError({
      code: "tally_request_deadline_exceeded",
      message: "The bounded Tally read exceeded its production deadline.",
    }),
    {
      category: "Tally is taking longer than expected",
      action: "Tally may still be processing the last check. Wait until it is responsive, make sure the right company is open, then check Tally again. Bridge did not change data in Tally.",
    },
  );
});

test("an unstructured request failure remains fail-closed without internal navigation jargon", () => {
  assert.deepEqual(
    classifyUnstructuredTallyError("Cannot read properties of undefined (reading 'invoke')"),
    {
      category: "Bridge could not complete this request",
      action: "Bridge cannot confirm the final state. Do not retry the same request yet; check Tally and the connection first.",
    },
  );
});

test("a plain endpoint failure gives the operator one clear next step", () => {
  assert.deepEqual(
    classifyUnstructuredTallyError("endpoint connection failed"),
    {
      category: "Check the Tally address",
      action: "Confirm the local address and Tally XML server, then start a fresh connection check.",
    },
  );
});
