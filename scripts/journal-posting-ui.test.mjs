import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import test from "node:test";

test("Journal review is reachable from Overview without becoming a top-level nav route", async () => {
  const app = await readFile(new URL("../src/main.tsx", import.meta.url), "utf8");

  assert.match(app, /import \{ JournalPostingScreen \} from "\.\/JournalPostingScreen";/);
  assert.match(app, /type View = .*"journal"/);
  assert.match(app, /setView\("journal"\)/);
  assert.match(app, /<JournalPostingScreen config=\{config\} onBusyChange=\{setJournalActionBusy\} \/>/);
  assert.match(app, /disabled=\{shellNavigationLocked\}/);
  assert.match(app, /journal-action-busy-note/);
  assert.doesNotMatch(app, /Bridge only reads from Tally/);
  assert.match(app, /posting a Journal always requires your explicit approval/);
  const nav = app.slice(app.indexOf('<nav aria-label="Bridge operations">'), app.indexOf("</nav>"));
  assert.doesNotMatch(nav, /Review Journal/);
});

test("Journal review uses the bounded native commands and preserves reconciliation identity", async () => {
  const screen = await readFile(new URL("../src/JournalPostingScreen.tsx", import.meta.url), "utf8");

  assert.match(screen, /desktop_pick_journal_for_review/);
  assert.match(screen, /desktop_post_reviewed_journal/);
  assert.match(screen, /desktop_reconcile_reviewed_journal/);
  assert.match(screen, /batchId: review\.batchId/);
  assert.match(screen, /sha256: review\.sha256/);
  assert.match(screen, /companyGuid: review\.company\.guid/);
  assert.match(screen, /deriveJournalActionState/);
  assert.match(screen, /attemptRecorded: actionResult/);
  assert.match(screen, /uncertainAttempt/);
  assert.match(screen, /setUncertainAttempt\(true\)/);
  assert.match(screen, /requestedConfig = reviewConfig \?\? config/);
  assert.match(screen, /Review the approval dialog; Bridge then checks and posts this saved batch/);
  assert.match(screen, /journalState\.canReconcile/);
  assert.doesNotMatch(screen, /setReview\(\(current\).*dispatched/);
  assert.match(screen, /Reconcile original batch/);
  assert.match(screen, /will not rebuild or resend it/);
  assert.doesNotMatch(screen, /<textarea/);
  assert.doesNotMatch(screen, /build_import_xml|render_import_xml/);
  assert.match(screen, /onBusyChange\?\.\(true\)/);
  assert.match(screen, /onBusyChange\?\.\(false\)/);
});

test("Journal entries stay bounded and scrollable at narrow widths", async () => {
  const styles = await readFile(new URL("../src/styles.css", import.meta.url), "utf8");

  assert.match(styles, /\.journal-review\s*\{[\s\S]*?min-width:\s*0;/);
  assert.match(styles, /\.journal-entry-table-wrap\s*\{[\s\S]*?overflow-x:\s*auto;/);
  assert.match(styles, /\.journal-recovery-details\s*\{[\s\S]*?border-top:/);
  assert.match(styles, /\.journal-review-details\s*\{[\s\S]*?repeat\(auto-fit/);
});
