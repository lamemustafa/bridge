// SPDX-License-Identifier: Apache-2.0

import { execFile } from "node:child_process";
import { mkdtemp, readdir, rm, stat } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const attachTimeoutMilliseconds = 60_000;
const detachTimeoutMilliseconds = 15_000;
const argumentsList = process.argv.slice(2);
const appOnly = argumentsList.includes("--app-only");
const pathArguments = argumentsList.filter((argument) => argument !== "--app-only");

function execFileAsync(file, argumentsList, options, input) {
  return new Promise((resolvePromise, rejectPromise) => {
    const child = execFile(
      file,
      argumentsList,
      options,
      (error, stdout, stderr) => {
        if (error) {
          error.stdout = stdout;
          error.stderr = stderr;
          rejectPromise(error);
        } else {
          resolvePromise({ stdout, stderr });
        }
      },
    );
    if (input !== undefined) {
      child.stdin.end(input);
    }
  });
}

if (pathArguments.length > 1) {
  throw new Error("Expected at most one bundle directory argument");
}

const bundleDirectory = pathArguments[0]
  ? resolve(pathArguments[0])
  : fileURLToPath(new URL("../src-tauri/target/release/bundle/", import.meta.url));
const expectedResources = [
  "LICENSE",
  "NOTICE",
  "THIRD_PARTY_LICENSES.txt",
  "THIRD_PARTY_LICENSES_RUST.txt",
];

async function findOnlyApp(directory, label) {
  const entries = await readdir(directory, { withFileTypes: true });
  const apps = entries.filter(
    (entry) => entry.isDirectory() && entry.name.endsWith(".app"),
  );

  if (apps.length !== 1) {
    throw new Error(`Expected exactly one ${label} app bundle, found ${apps.length}`);
  }

  return join(directory, apps[0].name);
}

async function assertAppResources(app, label) {
  const resources = join(app, "Contents", "Resources");
  for (const resource of expectedResources) {
    const metadata = await stat(join(resources, resource));
    if (!metadata.isFile() || metadata.size === 0) {
      throw new Error(`${label} resource is missing or empty: ${resource}`);
    }
  }
}

const stagedApp = await findOnlyApp(join(bundleDirectory, "macos"), "staged macOS");
await assertAppResources(stagedApp, "Staged macOS app");

if (!appOnly) {
  const dmgDirectory = join(bundleDirectory, "dmg");
  const dmgEntries = await readdir(dmgDirectory, { withFileTypes: true });
  const dmgs = dmgEntries.filter(
    (entry) => entry.isFile() && entry.name.endsWith(".dmg"),
  );
  if (dmgs.length !== 1) {
    throw new Error(`Expected exactly one macOS DMG, found ${dmgs.length}`);
  }

  const mountPoint = await mkdtemp(join(tmpdir(), "bridge-dmg-"));
  let mounted = false;
  try {
    await execFileAsync(
      "hdiutil",
      [
        "attach",
        "-readonly",
        "-nobrowse",
        "-mountpoint",
        mountPoint,
        join(dmgDirectory, dmgs[0].name),
      ],
      { timeout: attachTimeoutMilliseconds },
      "Y\n",
    );
    mounted = true;
    const packagedApp = await findOnlyApp(mountPoint, "DMG-packaged macOS");
    await assertAppResources(packagedApp, "DMG-packaged macOS app");
  } finally {
    try {
      if (mounted) {
        try {
          await execFileAsync("hdiutil", ["detach", mountPoint], {
            timeout: detachTimeoutMilliseconds,
          });
        } catch {
          await execFileAsync("hdiutil", ["detach", "-force", mountPoint], {
            timeout: detachTimeoutMilliseconds,
          });
        }
      }
    } finally {
      await rm(mountPoint, { recursive: true, force: true });
    }
  }
}

console.log(
  appOnly
    ? "macOS app bundle contains all required legal resources."
    : "macOS app and DMG contain all required legal resources.",
);
