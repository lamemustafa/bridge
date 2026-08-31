// SPDX-License-Identifier: Apache-2.0
// @vitest-environment jsdom

import { readFile } from "node:fs/promises";
import { afterEach, expect, test } from "vitest";

import {
  createDrawerFocusLifecycle,
  ensureDrawerFocus,
  shouldFocusMainContentAfterViewTransition,
} from "../src/evidence-drawer-focus";

function connectedButton(label: string) {
  const button = document.createElement("button");
  button.textContent = label;
  document.body.append(button);
  return button;
}

function appSource() {
  return readFile("src/main.tsx", "utf8");
}

afterEach(() => {
  document.body.replaceChildren();
});

test("evidence drawer lifecycle restores the captured browser opener for Close and Escape", async () => {
  const app = await appSource();

  for (const dismissal of ["Close", "Escape"]) {
    const lifecycle = createDrawerFocusLifecycle();
    const opener = connectedButton(`${dismissal} opener`);
    lifecycle.captureOpener(opener);

    expect(lifecycle.restoreOpener()).toBe(true);
    expect(document.activeElement).toBe(opener);
  }

  expect(app).toMatch(/event\.key === "Escape"\) \{\s*closeEvidenceDrawer\(\);\s*return;/s);
  expect(app).toMatch(/ref=\{evidenceDrawerCloseRef\} onClick=\{closeEvidenceDrawer\}/);
  expect(app).toMatch(/captureOpener\(opener \?\? \(document\.activeElement instanceof HTMLElement/);
  expect(app).toMatch(/if \(evidenceDrawerOpen\) \{\s*ensureDrawerFocus\(evidenceDrawerOpen, evidenceDrawerCloseRef\.current\);/s);
});

test("a disconnected drawer opener falls back to the main-content focus owner", async () => {
  const app = await appSource();
  const lifecycle = createDrawerFocusLifecycle();
  const opener = connectedButton("unmounted opener");
  opener.remove();

  lifecycle.captureOpener(opener);
  expect(lifecycle.restoreOpener()).toBe(false);
  expect(app).toMatch(/setEvidenceDrawerRestorePending\(true\);/);
  expect(app).toMatch(/else if \(evidenceDrawerRestorePending\) \{\s*focusMainContent = !evidenceDrawerFocusLifecycle\.restoreOpener\(\);/s);
});

test("drawer replacement focuses a connected native control and keeps restoration authoritative", async () => {
  const app = await appSource();
  const drawerClose = connectedButton("drawer close");

  expect(ensureDrawerFocus(true, drawerClose)).toBe(true);
  expect(document.activeElement).toBe(drawerClose);
  expect(ensureDrawerFocus(false, drawerClose)).toBe(false);
  expect(app).toMatch(/if \(evidenceDrawerOpen\) setEvidenceDrawerFocusEpoch\(\(current\) => current \+ 1\);/);
  expect(app).toMatch(/\[view, evidenceDrawerFocusEpoch, evidenceDrawerOpen, evidenceDrawerRestorePending, evidenceDrawerFocusLifecycle\]/);
  expect((app.match(/mainContentRef\.current\?\.focus\(\)/g) ?? []).length).toBe(1);
});

test("ordinary view transitions focus main content without overriding drawer restoration", async () => {
  const app = await appSource();

  expect(shouldFocusMainContentAfterViewTransition({
    previousView: "clients",
    view: "outstandings",
    drawerWasOpen: false,
    drawerOpen: false,
  })).toBe(true);
  expect(shouldFocusMainContentAfterViewTransition({
    previousView: "outstandings",
    view: "companies",
    drawerWasOpen: true,
    drawerOpen: false,
  })).toBe(false);
  expect(app).toMatch(/shouldFocusMainContentAfterViewTransition\([\s\S]*?mainContentRef\.current\?\.focus\(\)/);
  expect(app).toMatch(/<main className="content" id="main-content" ref=\{mainContentRef\}/);
});
