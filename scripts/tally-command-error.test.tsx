// SPDX-License-Identifier: Apache-2.0

import { expect, test } from "vitest";
import { formatCommandErrorMessage } from "../src/tally-command-error";

test("formats structured command errors with every available operator detail", () => {
  expect(formatCommandErrorMessage({
    code: "source_draft_revision_conflict",
    message: "This draft changed while it was open.",
    remediation: "Review the current draft, then save again.",
  }, "unused fallback")).toBe(
    "This draft changed while it was open. [source_draft_revision_conflict] Review the current draft, then save again.",
  );
});

test("keeps usable message-only command errors and each caller's fallback", () => {
  expect(formatCommandErrorMessage({ message: "The command was refused." }, "unused fallback"))
    .toBe("The command was refused.");
  expect(formatCommandErrorMessage("The local Tally read did not complete.", "unused fallback"))
    .toBe("The local Tally read did not complete.");
  expect(formatCommandErrorMessage(42, "Bridge could not complete that source-draft action."))
    .toBe("Bridge could not complete that source-draft action.");
  expect(formatCommandErrorMessage(42, (cause) => (cause instanceof Error ? cause.message : String(cause))))
    .toBe("42");
});
