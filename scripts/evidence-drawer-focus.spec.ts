import { expect, test } from "@playwright/test";

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

test("the evidence drawer uses Chromium's native tab order for collapsed and expanded details", async ({ page }) => {
  await page.goto("/scripts/evidence-drawer-focus.fixture.html");
  await expect.poll(() => page.locator("#drawer").evaluate((drawer) => Boolean(window.evidenceDrawerFocus))).toBe(true);

  const collapsed = await page.locator("#drawer").evaluate((drawer) => (
    window.evidenceDrawerFocus.visibleDrawerTabStops(drawer as HTMLElement)
      .map((element) => element.dataset.focusName)
  ));
  expect(collapsed).toEqual(["close", "advanced summary", "audio controls", "video controls"]);

  await page.locator("summary").click();
  const expanded = await page.locator("#drawer").evaluate((drawer) => (
    window.evidenceDrawerFocus.visibleDrawerTabStops(drawer as HTMLElement)
      .map((element) => element.dataset.focusName)
  ));
  expect(expanded).toEqual(["close", "advanced summary", "advanced button", "audio controls", "video controls"]);

  const boundary = await page.locator("#drawer").evaluate((drawer) => {
    const candidates = window.evidenceDrawerFocus.visibleDrawerTabStops(drawer as HTMLElement);
    return {
      forward: window.evidenceDrawerFocus.drawerFocusBoundaryTarget(candidates.at(-1) ?? null, candidates, false)?.dataset.focusName,
      backward: window.evidenceDrawerFocus.drawerFocusBoundaryTarget(candidates[0] ?? null, candidates, true)?.dataset.focusName,
    };
  });
  expect(boundary).toEqual({ forward: "close", backward: "video controls" });
});
