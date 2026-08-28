export function collapsedDetailsTabbable(detailsOpen: boolean, isSummary: boolean) {
  return detailsOpen || isSummary;
}

export function drawerFocusBoundaryIndex(
  activeIndex: number,
  candidateCount: number,
  backwards: boolean,
) {
  if (activeIndex < 0 || candidateCount === 0) return null;
  if (backwards && activeIndex === 0) return candidateCount - 1;
  if (!backwards && activeIndex === candidateCount - 1) return 0;
  return null;
}

const DRAWER_FOCUSABLE = "a[href], button:not([disabled]), input:not([disabled]), select:not([disabled]), textarea:not([disabled]), [tabindex]:not([tabindex=\"-1\"])";

export function visibleDrawerTabStops(drawer: HTMLElement) {
  return Array.from(drawer.querySelectorAll<HTMLElement>(DRAWER_FOCUSABLE)).filter((element) => {
    if (element.getAttribute("aria-hidden") === "true" || element.matches(":disabled")) return false;
    const closedDetails = element.closest("details:not([open])");
    if (closedDetails && !collapsedDetailsTabbable(false, element.tagName === "SUMMARY" && element.parentElement === closedDetails)) {
      return false;
    }
    const style = window.getComputedStyle(element);
    return style.visibility !== "hidden" && style.display !== "none" && element.getClientRects().length > 0;
  });
}

export function drawerFocusBoundaryTarget(
  activeElement: Element | null,
  candidates: HTMLElement[],
  backwards: boolean,
) {
  const targetIndex = drawerFocusBoundaryIndex(candidates.indexOf(activeElement as HTMLElement), candidates.length, backwards);
  return targetIndex === null ? null : candidates[targetIndex];
}
