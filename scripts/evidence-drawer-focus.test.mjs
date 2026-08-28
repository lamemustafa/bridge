// SPDX-License-Identifier: Apache-2.0

import assert from "node:assert/strict";
import test from "node:test";

import { collapsedDetailsTabbable, drawerFocusBoundaryIndex } from "../src/evidence-drawer-focus.ts";

test("evidence drawer focus boundaries exclude collapsed Advanced controls and include them once expanded", () => {
  assert.equal(collapsedDetailsTabbable(false, false), false, "a button inside collapsed Advanced is not tabbable");
  assert.equal(collapsedDetailsTabbable(false, true), true, "the Advanced summary stays tabbable while collapsed");
  assert.equal(collapsedDetailsTabbable(true, false), true, "Advanced controls rejoin the tab order once expanded");

  assert.equal(drawerFocusBoundaryIndex(0, 2, true), 1, "Shift+Tab from Close wraps to the last visible collapsed-state control");
  assert.equal(drawerFocusBoundaryIndex(1, 2, false), 0, "Tab from the last visible collapsed-state control wraps to Close");
  assert.equal(drawerFocusBoundaryIndex(2, 4, false), null, "an expanded-state middle control keeps native forward tabbing");
  assert.equal(drawerFocusBoundaryIndex(3, 4, false), 0, "Tab from the last expanded-state control wraps to Close");
});
