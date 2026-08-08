// SPDX-License-Identifier: Apache-2.0

import assert from "node:assert/strict";
import { execFileSync } from "node:child_process";
import { mkdir, mkdtemp, rm, writeFile } from "node:fs/promises";
import { join } from "node:path";
import test from "node:test";
import { fileURLToPath } from "node:url";

const root = fileURLToPath(new URL("../", import.meta.url));

test("an unregistered fixture directory fails the byte-integrity gate", async () => {
  const directory = await mkdtemp(join(root, ".fixture-integrity-"));
  const fixtureDirectory = join(directory, "fixtures");

  try {
    await mkdir(fixtureDirectory, { recursive: true });
    await writeFile(join(fixtureDirectory, "synthetic.xml"), "<fixture />\n");

    let failure = null;
    try {
      execFileSync(process.execPath, ["scripts/check-fixture-byte-integrity.mjs"], {
        cwd: root,
        encoding: "utf8",
        stdio: "pipe",
      });
    } catch (cause) {
      failure = cause;
    }

    assert.ok(failure, "an unregistered fixture directory must fail the gate");
    assert.match(`${failure.stderr}${failure.stdout}`, /unexpected fixture directories/);
  } finally {
    await rm(directory, { force: true, recursive: true });
  }
});
