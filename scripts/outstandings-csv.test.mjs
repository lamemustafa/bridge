import assert from "node:assert/strict";
import test from "node:test";

import { csvNumericCell, csvRow, csvTextCell } from "../src/outstandings-csv.ts";

test("neutralizes every active spreadsheet prefix in text cells", () => {
  for (const [value, expected] of [
    ["=BVL Zeta Formula", "'=BVL Zeta Formula"],
    ["+SUM(A1:A2)", "'+SUM(A1:A2)"],
    ["-1+2", "'-1+2"],
    ["@SUM(A1:A2)", "'@SUM(A1:A2)"],
    ["\t=SUM(A1:A2)", "'\t=SUM(A1:A2)"],
    ["\r=SUM(A1:A2)", "\"'\r=SUM(A1:A2)\""],
  ]) {
    assert.equal(csvRow(csvTextCell(value)), expected);
  }
});

test("preserves numeric amount and count cells", () => {
  assert.equal(
    csvRow(csvNumericCell("-11111.00"), csvNumericCell(5)),
    "-11111.00,5",
  );
});

test("quotes neutralized text with CSV syntax after prefixing", () => {
  assert.equal(csvRow(csvTextCell('=HYPERLINK("https://example.invalid","x")')), '"\'=HYPERLINK(""https://example.invalid"",""x"")"');
});
