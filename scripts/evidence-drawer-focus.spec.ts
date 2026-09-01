import { expect, test, type Page } from "@playwright/test";

declare global {
  interface Window {
    evidenceDrawerFocus: {
      drawerFocusBoundaryTarget: (
        activeElement: Element | null,
        candidates: HTMLElement[],
        backwards: boolean,
      ) => HTMLElement | null;
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

test("the evidence drawer follows Chromium's native Tab order for collapsed and expanded details", async ({ page }) => {
  await page.goto("/scripts/evidence-drawer-focus.fixture.html");
  await expect.poll(() => page.locator("#drawer").evaluate((drawer) => Boolean(window.evidenceDrawerFocus))).toBe(true);

  await page.locator('[data-focus-name="close"]').focus();
  await pressUntilNativeFocus(page, "Tab", "close", "advanced summary");
  await pressUntilNativeFocus(page, "Tab", "advanced summary", "audio controls");
  await pressUntilNativeFocus(page, "Tab", "audio controls", "video controls");
  await pressUntilNativeFocus(page, "Tab", "video controls", "editable");
  await pressUntilNativeFocus(page, "Shift+Tab", "editable", "video controls");
  await pressUntilNativeFocus(page, "Shift+Tab", "video controls", "audio controls");
  await pressUntilNativeFocus(page, "Shift+Tab", "audio controls", "advanced summary");

  const collapsed = await page.locator("#drawer").evaluate((drawer) => (
    window.evidenceDrawerFocus.visibleDrawerTabStops(drawer as HTMLElement)
      .map((element) => element.dataset.focusName)
  ));
  expect(collapsed).toEqual(["close", "advanced summary", "audio controls", "video controls", "editable"]);

  await page.locator("summary").click();
  await pressUntilNativeFocus(page, "Tab", "advanced summary", "advanced button");
  await pressUntilNativeFocus(page, "Tab", "advanced button", "audio controls");
  await pressUntilNativeFocus(page, "Tab", "audio controls", "video controls");
  await pressUntilNativeFocus(page, "Tab", "video controls", "editable");

  const expanded = await page.locator("#drawer").evaluate((drawer) => (
    window.evidenceDrawerFocus.visibleDrawerTabStops(drawer as HTMLElement)
      .map((element) => element.dataset.focusName)
  ));
  expect(expanded).toEqual(["close", "advanced summary", "advanced button", "audio controls", "video controls", "editable"]);

  const boundary = await page.locator("#drawer").evaluate((drawer) => {
    const candidates = window.evidenceDrawerFocus.visibleDrawerTabStops(drawer as HTMLElement);
    return {
      forward: window.evidenceDrawerFocus.drawerFocusBoundaryTarget(candidates.at(-1) ?? null, candidates, false)?.dataset.focusName,
      backward: window.evidenceDrawerFocus.drawerFocusBoundaryTarget(candidates[0] ?? null, candidates, true)?.dataset.focusName,
    };
  });
  expect(boundary).toEqual({ forward: "close", backward: "editable" });

  await page.locator("#drawer").evaluate((drawer) => {
    drawer.addEventListener("keydown", (event) => {
      if (event.key !== "Tab") return;
      const candidates = window.evidenceDrawerFocus.visibleDrawerTabStops(drawer as HTMLElement);
      const target = window.evidenceDrawerFocus.drawerFocusBoundaryTarget(document.activeElement, candidates, event.shiftKey);
      if (!target) return;
      event.preventDefault();
      target.focus();
    });
  });
  await page.locator('[data-focus-name="editable"]').focus();
  await page.keyboard.press("Tab");
  await expect(page.locator('[data-focus-name="close"]')).toBeFocused();
});
