import { expect, test, type Page, type TestInfo } from "@playwright/test";

declare global {
  interface Window {
    evidenceDrawerFocus: {
      drawerFocusBoundaryTarget: (
        activeElement: Element | null,
        candidates: HTMLElement[],
        backwards: boolean,
      ) => HTMLElement | null;
      trapDrawerTabKeydown: (event: KeyboardEvent) => void;
      visibleDrawerTabStops: (drawer: HTMLElement) => HTMLElement[];
    };
  }
}

async function pressUntilNativeFocus(
  page: Page,
  key: "Tab" | "Shift+Tab",
  currentFocusName: string,
  expectedFocusName: string,
) {
  for (let pressCount = 0; pressCount < 12; pressCount += 1) {
    await page.keyboard.press(key);
    const focusName = await page.evaluate(() => document.activeElement?.getAttribute("data-focus-name"));
    if (focusName === expectedFocusName) {
      return;
    }
    if (focusName !== currentFocusName) {
      throw new Error(`native ${key} sequence reached unexpected ${focusName ?? "document"} before ${expectedFocusName}`);
    }
  }
  throw new Error(`native ${key} sequence did not reach ${expectedFocusName}`);
}

async function nativeFocusRoute(
  page: Page,
  startFocusName: string,
  key: "Tab" | "Shift+Tab",
  pressCount = 12,
) {
  await page.locator(`[data-focus-name="${startFocusName}"]`).focus();
  const route = [startFocusName];
  for (let index = 0; index < pressCount; index += 1) {
    await page.keyboard.press(key);
    route.push(await page.evaluate(() => document.activeElement?.getAttribute("data-focus-name") ?? "document"));
  }
  return route;
}

async function visibleDrawerTabStops(page: Page) {
  return page.locator("#drawer").evaluate((drawer) => (
    window.evidenceDrawerFocus.visibleDrawerTabStops(drawer as HTMLElement)
      .map((element) => element.dataset.focusName)
  ));
}

async function attachFocusDiagnostics(page: Page, testInfo: TestInfo) {
  const diagnostics = {
    project: testInfo.project.name,
    platform: process.platform,
    collapsed: {
      helperStops: await visibleDrawerTabStops(page),
      forwardFromPositive: await nativeFocusRoute(page, "positive tabindex editable", "Tab"),
      reverseFromEditable: await nativeFocusRoute(page, "editable", "Shift+Tab"),
    },
  };

  await page.locator("summary").click();
  const expanded = {
    helperStops: await visibleDrawerTabStops(page),
    forwardFromSummary: await nativeFocusRoute(page, "advanced summary", "Tab"),
    reverseFromEditable: await nativeFocusRoute(page, "editable", "Shift+Tab"),
  };
  await page.locator("summary").click();

  const body = JSON.stringify({ ...diagnostics, expanded });
  console.log(`evidence-drawer native focus diagnostics: ${body}`);
  await testInfo.attach("evidence-drawer-native-focus-routes.json", {
    body,
    contentType: "application/json",
  });
}

test("the evidence drawer records each browser engine's native Tab order for collapsed and expanded details", async ({ page }, testInfo) => {
  await page.goto("/scripts/evidence-drawer-focus.fixture.html");
  await expect.poll(() => page.locator("#drawer").evaluate((drawer) => Boolean(window.evidenceDrawerFocus))).toBe(true);
  // Preserve bounded native routes and helper output before strict assertions so
  // a new browser/platform divergence is visible in the CI log and artifact.
  await attachFocusDiagnostics(page, testInfo);

  await page.locator('[data-focus-name="positive tabindex editable"]').focus();
  // The engines differ here: Chromium follows the positive-tabindex editable
  // host with Close, while Playwright WebKit moves straight to the summary.
  // This records browser-engine behavior only; it is not packaged-shell proof.
  if (testInfo.project.name === "webkit") {
    await pressUntilNativeFocus(page, "Tab", "positive tabindex editable", "advanced summary");
  } else {
    await pressUntilNativeFocus(page, "Tab", "positive tabindex editable", "close");
    await page.locator('[data-focus-name="close"]').focus();
    await pressUntilNativeFocus(page, "Tab", "close", "advanced summary");
  }
  await pressUntilNativeFocus(page, "Tab", "advanced summary", "audio controls");
  await pressUntilNativeFocus(page, "Tab", "audio controls", "video controls");
  await pressUntilNativeFocus(page, "Tab", "video controls", "editable");
  if (testInfo.project.name === "webkit") {
    // WebKit leaves the document's focusable sequence once after this editing
    // host, then wraps to the positive-tabindex editable host instead of
    // reaching the following button.
    await page.keyboard.press("Tab");
    await expect.poll(() => page.evaluate(() => document.activeElement === document.body)).toBe(true);
    await page.keyboard.press("Tab");
    await expect(page.locator('[data-focus-name="positive tabindex editable"]')).toBeFocused();
  } else {
    await pressUntilNativeFocus(page, "Tab", "editable", "after drawer");
    await pressUntilNativeFocus(page, "Shift+Tab", "after drawer", "editable");
    await pressUntilNativeFocus(page, "Shift+Tab", "editable", "video controls");
    await pressUntilNativeFocus(page, "Shift+Tab", "video controls", "audio controls");
    await pressUntilNativeFocus(page, "Shift+Tab", "audio controls", "advanced summary");
  }

  const collapsed = await page.locator("#drawer").evaluate((drawer) => (
    window.evidenceDrawerFocus.visibleDrawerTabStops(drawer as HTMLElement)
      .map((element) => element.dataset.focusName)
  ));
  // WebKit's headless media controls do not expose client rects to the helper,
  // although its native Tab path above still visits them. Keep that browser-test
  // observation distinct from a claim about the packaged macOS shell.
  expect(collapsed).toEqual(testInfo.project.name === "webkit"
    ? ["positive tabindex editable", "close", "advanced summary", "editable"]
    : ["positive tabindex editable", "close", "advanced summary", "audio controls", "video controls", "editable"]);

  await page.locator("summary").click();
  if (testInfo.project.name === "webkit") {
    // WebKit also keeps the details button out of its native Tab path after
    // expansion, despite the element being present in the helper's list.
    await pressUntilNativeFocus(page, "Tab", "advanced summary", "audio controls");
  } else {
    await pressUntilNativeFocus(page, "Tab", "advanced summary", "advanced button");
    await pressUntilNativeFocus(page, "Tab", "advanced button", "audio controls");
  }
  await pressUntilNativeFocus(page, "Tab", "audio controls", "video controls");
  await pressUntilNativeFocus(page, "Tab", "video controls", "editable");

  const expanded = await page.locator("#drawer").evaluate((drawer) => (
    window.evidenceDrawerFocus.visibleDrawerTabStops(drawer as HTMLElement)
      .map((element) => element.dataset.focusName)
  ));
  expect(expanded).toEqual(testInfo.project.name === "webkit"
    ? ["positive tabindex editable", "close", "advanced summary", "advanced button", "editable"]
    : ["positive tabindex editable", "close", "advanced summary", "advanced button", "audio controls", "video controls", "editable"]);

  const boundary = await page.locator("#drawer").evaluate((drawer) => {
    const candidates = window.evidenceDrawerFocus.visibleDrawerTabStops(drawer as HTMLElement);
    return {
      forward: window.evidenceDrawerFocus.drawerFocusBoundaryTarget(candidates.at(-1) ?? null, candidates, false)?.dataset.focusName,
      backward: window.evidenceDrawerFocus.drawerFocusBoundaryTarget(candidates[0] ?? null, candidates, true)?.dataset.focusName,
    };
  });
  expect(boundary).toEqual({ forward: "positive tabindex editable", backward: "editable" });

  await page.locator("#drawer").evaluate((drawer) => {
    drawer.addEventListener("keydown", window.evidenceDrawerFocus.trapDrawerTabKeydown);
  });
  await page.locator('[data-focus-name="editable"]').focus();
  await page.keyboard.press("Tab");
  await expect(page.locator('[data-focus-name="positive tabindex editable"]')).toBeFocused();
});
