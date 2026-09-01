// SPDX-License-Identifier: Apache-2.0

import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import test from "node:test";

import {
  createDrawerFocusLifecycle,
  drawerFocusBoundaryIndex,
  ensureDrawerFocus,
  shouldFocusMainContentAfterViewTransition,
  visibleDrawerTabStops,
} from "../src/evidence-drawer-focus.ts";

function fakeElement(name, options = {}) {
  const attributes = new Set(options.attributes ?? []);
  return {
    name,
    tagName: options.tagName ?? "BUTTON",
    tabIndex: options.tabIndex ?? 0,
    parentElement: options.parentElement ?? null,
    hidden: options.hidden ?? false,
    inert: options.inert ?? false,
    isConnected: options.isConnected ?? true,
    display: options.display ?? "block",
    visibility: options.visibility ?? "visible",
    rects: options.rects ?? [{}],
    disabled: options.disabled ?? false,
    getAttribute(attribute) {
      return attributes.has(attribute) ? "" : null;
    },
    hasAttribute(attribute) {
      return attributes.has(attribute);
    },
    matches(selector) {
      return selector === ":disabled" && this.disabled;
    },
    getClientRects() {
      return this.rects;
    },
    focus(options) {
      this.focusCalls = [...(this.focusCalls ?? []), options];
    },
  };
}

function installComputedStyle() {
  const previousWindow = globalThis.window;
  Object.defineProperty(globalThis, "window", {
    configurable: true,
    value: {
      getComputedStyle(element) {
        return { display: element.display, visibility: element.visibility };
      },
    },
  });
  return () => {
    if (previousWindow === undefined) delete globalThis.window;
    else Object.defineProperty(globalThis, "window", { configurable: true, value: previousWindow });
  };
}

test("evidence drawer derives collapsed and expanded native tab stops from browser tabIndex", (t) => {
  const restoreWindow = installComputedStyle();
  t.after(restoreWindow);

  const drawer = fakeElement("drawer", { tagName: "ASIDE", tabIndex: -1 });
  const details = fakeElement("details", { tagName: "DETAILS", parentElement: drawer });
  const summary = fakeElement("summary", { tagName: "SUMMARY", parentElement: details });
  const advancedButton = fakeElement("advanced button", { parentElement: details });
  const hiddenGroup = fakeElement("hidden group", { tagName: "DIV", parentElement: drawer, hidden: true, tabIndex: -1 });
  const inertGroup = fakeElement("inert group", { tagName: "DIV", parentElement: drawer, inert: true, tabIndex: -1 });
  const displayNoneGroup = fakeElement("display none group", { tagName: "DIV", parentElement: drawer, display: "none", tabIndex: -1 });
  const visibilityHiddenGroup = fakeElement("visibility hidden group", { tagName: "DIV", parentElement: drawer, visibility: "hidden", tabIndex: -1 });
  const close = fakeElement("close", { parentElement: drawer });
  const audio = fakeElement("audio controls", { tagName: "AUDIO", parentElement: drawer });
  const video = fakeElement("video controls", { tagName: "VIDEO", parentElement: drawer });
  const editable = fakeElement("editable", { tagName: "DIV", parentElement: drawer });
  const hiddenButton = fakeElement("hidden button", { parentElement: hiddenGroup });
  const inertButton = fakeElement("inert button", { parentElement: inertGroup });
  const displayNoneButton = fakeElement("display none button", { parentElement: displayNoneGroup });
  const visibilityHiddenButton = fakeElement("visibility hidden button", { parentElement: visibilityHiddenGroup });
  drawer.querySelectorAll = () => [close, details, summary, advancedButton, audio, video, editable, hiddenGroup, hiddenButton, inertGroup, inertButton, displayNoneGroup, displayNoneButton, visibilityHiddenGroup, visibilityHiddenButton];

  assert.deepEqual(
    visibleDrawerTabStops(drawer).map((element) => element.name),
    ["close", "details", "summary", "audio controls", "video controls", "editable"],
    "collapsed Advanced retains its native summary/details stops while all hidden or inert descendants stay out",
  );
  assert.equal(drawerFocusBoundaryIndex(5, 6, false), 0, "Tab from the last collapsed-state tab stop wraps to Close");

  details.hasAttribute = (attribute) => attribute === "open";
  assert.deepEqual(
    visibleDrawerTabStops(drawer).map((element) => element.name),
    ["close", "details", "summary", "advanced button", "audio controls", "video controls", "editable"],
    "expanded Advanced adds its rendered control without special-casing a control selector",
  );
  assert.equal(drawerFocusBoundaryIndex(0, 7, true), 6, "Shift+Tab from Close reaches the last expanded-state tab stop");
  assert.equal(drawerFocusBoundaryIndex(3, 7, false), null, "an expanded Advanced control keeps native forward tabbing");
});

test("evidence drawer lifecycle restores the captured opener for Close and Escape", async () => {
  const app = await readFile(new URL("../src/main.tsx", import.meta.url), "utf8");

  for (const dismissal of ["Close", "Escape"]) {
    const lifecycle = createDrawerFocusLifecycle();
    const opener = fakeElement(`${dismissal} opener`);
    lifecycle.captureOpener(opener);
    assert.equal(lifecycle.restoreOpener(), true);
    assert.deepEqual(opener.focusCalls, [{ preventScroll: true }], `${dismissal} returns focus to the original opener`);
  }

  assert.match(app, /event\.key === "Escape"\) \{\s*closeEvidenceDrawer\(\);\s*return;/s);
  assert.match(app, /ref=\{evidenceDrawerCloseRef\} onClick=\{closeEvidenceDrawer\}/);
  assert.match(app, /captureOpener\(opener \?\? \(document\.activeElement instanceof HTMLElement/);
  assert.match(app, /if \(evidenceDrawerOpen\) \{\s*ensureDrawerFocus\(evidenceDrawerOpen, evidenceDrawerCloseRef\.current\);/s);
});

test("a disconnected drawer opener falls back to the single main-content focus owner", async () => {
  const app = await readFile(new URL("../src/main.tsx", import.meta.url), "utf8");
  const lifecycle = createDrawerFocusLifecycle();
  const disconnectedOpener = fakeElement("unmounted opener", { isConnected: false });

  lifecycle.captureOpener(disconnectedOpener);
  assert.equal(lifecycle.restoreOpener(), false);
  assert.equal(disconnectedOpener.focusCalls, undefined);
  assert.match(app, /setEvidenceDrawerRestorePending\(true\);/);
  assert.match(app, /else if \(evidenceDrawerRestorePending\) \{\s*focusMainContent = !evidenceDrawerFocusLifecycle\.restoreOpener\(\);/s);
});

test("close and navigate decides focus after the opener's old view unmounts", async () => {
  const app = await readFile(new URL("../src/main.tsx", import.meta.url), "utf8");
  const lifecycle = createDrawerFocusLifecycle();
  const outgoingOutstandingsLink = fakeElement("outgoing Outstandings link");

  lifecycle.captureOpener(outgoingOutstandingsLink);
  outgoingOutstandingsLink.isConnected = false;
  assert.equal(lifecycle.restoreOpener(), false, "the post-render decision rejects an opener removed by the new view");
  assert.match(app, /onClick=\{\(\) => \{ closeEvidenceDrawer\(\); setView\("companies"\); \}\}/);
  assert.match(app, /else if \(evidenceDrawerRestorePending\) \{\s*focusMainContent = !evidenceDrawerFocusLifecycle\.restoreOpener\(\);/s);
  assert.equal((app.match(/mainContentRef\.current\?\.focus\(\)/g) ?? []).length, 1);
});

test("picker replacement while the drawer is open re-runs its focus owner", async () => {
  const app = await readFile(new URL("../src/main.tsx", import.meta.url), "utf8");
  const selectSavedCompany = app.slice(
    app.indexOf("function selectSavedCompany"),
    app.indexOf("function selectClientFromShell"),
  );
  const focusOwner = app.slice(
    app.indexOf("React.useEffect(() => {\n    const drawerWasOpen"),
    app.indexOf("const snapshotActive"),
  );

  assert.match(selectSavedCompany, /if \(evidenceDrawerOpen\) setEvidenceDrawerFocusEpoch\(\(current\) => current \+ 1\);/);
  const drawerClose = fakeElement("drawer close");
  assert.equal(ensureDrawerFocus(true, drawerClose), true);
  assert.deepEqual(drawerClose.focusCalls, [undefined], "the replacement path returns focus to a connected drawer control");
  assert.equal(ensureDrawerFocus(false, drawerClose), false);
  assert.match(focusOwner, /if \(evidenceDrawerOpen\) \{\s*ensureDrawerFocus\(evidenceDrawerOpen, evidenceDrawerCloseRef\.current\);/s);
  assert.match(focusOwner, /\[view, evidenceDrawerFocusEpoch, evidenceDrawerOpen, evidenceDrawerRestorePending, evidenceDrawerFocusLifecycle\]/);
  assert.equal((app.match(/mainContentRef\.current\?\.focus\(\)/g) ?? []).length, 1);
});

test("ordinary view transitions focus main content without overriding drawer restoration", async () => {
  const app = await readFile(new URL("../src/main.tsx", import.meta.url), "utf8");

  assert.equal(
    shouldFocusMainContentAfterViewTransition({
      previousView: "clients",
      view: "outstandings",
      drawerWasOpen: false,
      drawerOpen: false,
    }),
    true,
    "a non-drawer view transition moves focus to its new main content",
  );
  assert.equal(
    shouldFocusMainContentAfterViewTransition({
      previousView: "outstandings",
      view: "companies",
      drawerWasOpen: true,
      drawerOpen: false,
    }),
    false,
    "closing the drawer keeps its captured-opener restoration authoritative",
  );

  assert.match(app, /shouldFocusMainContentAfterViewTransition\([\s\S]*?mainContentRef\.current\?\.focus\(\)/);
  assert.match(app, /<main className="content" id="main-content" ref=\{mainContentRef\}/);
  assert.equal((app.match(/mainContentRef\.current\?\.focus\(\)/g) ?? []).length, 1);
});
