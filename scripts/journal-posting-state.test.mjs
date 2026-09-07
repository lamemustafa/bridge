import assert from "node:assert/strict";
import test from "node:test";
import { deriveJournalActionState } from "../src/journal-posting-state.ts";

const freshReview = { dispatched: false, responseRecorded: false };

test("approval cancellation keeps Post Journal available", () => {
  const state = deriveJournalActionState(freshReview, {
    state: null,
    attemptRecorded: false,
    hasError: true,
  }, false);

  assert.equal(state.canPost, true);
  assert.equal(state.canReconcile, false);
});

test("a thrown post command keeps the review and offers safe reconciliation", () => {
  const state = deriveJournalActionState(freshReview, null, true);

  assert.equal(state.canPost, false);
  assert.equal(state.canReconcile, true);
  assert.equal(state.canChooseAnother, false);
});

test("verified posting never exposes a second Post button", () => {
  const state = deriveJournalActionState(freshReview, {
    state: "posted_verified",
    hasError: false,
  }, false);

  assert.equal(state.verified, true);
  assert.equal(state.canPost, false);
  assert.equal(state.canChooseAnother, true);
});

test("no-dispatch reconciliation returns to safe retry", () => {
  const state = deriveJournalActionState(freshReview, {
    state: "not_dispatched",
    hasError: true,
  }, false);

  assert.equal(state.canPost, true);
  assert.equal(state.canReconcile, false);
});

test("a reconciled original batch can choose another file", () => {
  const state = deriveJournalActionState({ dispatched: true, responseRecorded: true }, {
    state: "previous_attempt_reconciled",
    hasError: false,
  }, false);

  assert.equal(state.verified, true);
  assert.equal(state.canChooseAnother, true);
});
