type DrawerFocusTarget = Pick<HTMLElement, "focus" | "isConnected">;

type TabStopCandidate = {
  element: HTMLElement;
  index: number;
};

function isVisibleInDrawer(element: HTMLElement, drawer: HTMLElement) {
  for (
    let current: HTMLElement | null = element;
    current && current !== drawer.parentElement;
    current = current.parentElement
  ) {
    if (
      current.hidden
      || current.inert
      || current.hasAttribute("inert")
      || current.getAttribute("aria-hidden") === "true"
      || current.matches(":disabled")
    ) {
      return false;
    }

    const style = window.getComputedStyle(current);
    if (style.display === "none" || style.visibility === "hidden" || style.visibility === "collapse") {
      return false;
    }

    if (
      current.tagName === "DETAILS"
      && !current.hasAttribute("open")
      && element !== current
      && !(element.tagName === "SUMMARY" && element.parentElement === current)
    ) {
      return false;
    }
  }

  return element.getClientRects().length > 0;
}

function inBrowserTabOrder(candidates: TabStopCandidate[]) {
  const positive = candidates
    .filter(({ element }) => element.tabIndex > 0)
    .sort((left, right) => left.element.tabIndex - right.element.tabIndex || left.index - right.index);
  const sequential = candidates.filter(({ element }) => element.tabIndex === 0);
  return [...positive, ...sequential].map(({ element }) => element);
}

// `tabIndex` is the browser's own declaration of sequential focusability. It
// covers native controls and future native tab stops without maintaining a
// second, inevitably incomplete selector list in application code.
export function visibleDrawerTabStops(drawer: HTMLElement) {
  return inBrowserTabOrder(
    Array.from(drawer.querySelectorAll<HTMLElement>("*")).flatMap((element, index) => (
      element.tabIndex >= 0 && isVisibleInDrawer(element, drawer) ? [{ element, index }] : []
    )),
  );
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

export function drawerFocusBoundaryTarget(
  activeElement: Element | null,
  candidates: HTMLElement[],
  backwards: boolean,
) {
  const targetIndex = drawerFocusBoundaryIndex(candidates.indexOf(activeElement as HTMLElement), candidates.length, backwards);
  return targetIndex === null ? null : candidates[targetIndex];
}

export function shouldFocusMainContentAfterViewTransition({
  previousView,
  view,
  drawerWasOpen,
  drawerOpen,
}: {
  previousView: string;
  view: string;
  drawerWasOpen: boolean;
  drawerOpen: boolean;
}) {
  return previousView !== view && !drawerWasOpen && !drawerOpen;
}

export function ensureDrawerFocus(drawerOpen: boolean, target: DrawerFocusTarget | null) {
  if (!drawerOpen || !target?.isConnected) return false;
  target.focus();
  return true;
}

export function createDrawerFocusLifecycle() {
  let opener: DrawerFocusTarget | null = null;

  return {
    captureOpener(target: DrawerFocusTarget | null) {
      opener = target;
    },
    restoreOpener() {
      const capturedOpener = opener;
      opener = null;
      if (!capturedOpener?.isConnected) return false;
      capturedOpener.focus({ preventScroll: true });
      return true;
    },
  };
}
