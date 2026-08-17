// SPDX-License-Identifier: Apache-2.0

import { spawnSync } from "node:child_process";
import { readFileSync, readdirSync } from "node:fs";
import { fileURLToPath } from "node:url";

const root = fileURLToPath(new URL("../", import.meta.url));
const result = spawnSync(
  "cargo",
  [
    "tree",
    "--locked",
    "--manifest-path",
    // bridge-tally-live-read now lives in the separate tools/ workspace. The
    // property this gate enforces is unchanged -- the read tool must never
    // pull in the app, tauri, or a database driver -- only its location moved.
    "tools/Cargo.toml",
    "-p",
    "bridge-tally-live-read",
    "--edges",
    "normal",
    "--prefix",
    "none",
    "--format",
    "{p}",
  ],
  { cwd: root, encoding: "utf8", maxBuffer: 64 * 1024 * 1024, windowsHide: true },
);
if (result.error || result.status !== 0) {
  throw new Error(`live-read dependency tree failed: ${result.stderr ?? result.error}`);
}

const packages = new Set();
for (const line of result.stdout.split(/\r?\n/)) {
  const match = line.match(/^([A-Za-z0-9_.+-]+) v\d/);
  if (match) packages.add(match[1]);
}
const forbiddenNames = [
  "bridge",
  "bridge-tally-canonical",
  "bridge-tally-core",
  "bridge-tally-incremental",
  "bridge-tally-observability",
  "bridge-tally-qualification",
  "libsqlite3-sys",
  "rusqlite",
  "sqlx",
  "tauri",
  "tauri-runtime",
];
const forbidden = forbiddenNames.filter((name) => packages.has(name));
if (forbidden.length) {
  throw new Error(`live-read boundary reached forbidden packages: ${forbidden.join(", ")}`);
}

const firstParty = [...packages].filter((name) => name.startsWith("bridge-tally-")).sort();
// `bridge-tally-primitives` was added 2026-07-31 to REMOVE `bridge-tally-core`
// from this set, not to widen it. Unit A's outstandings work needs `ExactDecimal`
// and `TallyDate`, which lived in `bridge-tally-core` alongside delivery
// capability (`AxalTallyGateway`, `DestinationAdapter` -- begin/deliver/finalize).
// Depending on core from the protocol dragged that write surface into this
// read-only controller. The two value types now live in a capability-free crate
// beneath both, so live-read reaches value types and no delivery surface at all.
// The forbidden list below is unchanged; this is a tightening, not a relaxation.
const expected = [
  "bridge-tally-compatibility",
  "bridge-tally-live-read",
  "bridge-tally-primitives",
  "bridge-tally-protocol",
  "bridge-tally-read-transport",
  "bridge-tally-transport",
];
if (JSON.stringify(firstParty) !== JSON.stringify(expected)) {
  throw new Error(`live-read first-party boundary changed: ${firstParty.join(", ")}`);
}

const manifest = readFileSync(new URL("../tools/bridge-tally-live-read/Cargo.toml", import.meta.url), "utf8");
const liveReadRoot = fileURLToPath(new URL("../tools/bridge-tally-live-read", import.meta.url)).replaceAll("\\", "/");
for (const forbiddenManifestText of [
  "bridge-tally-transport",
  // Previously `path = "../.."`, which was the app root while this crate lived
  // in src-tauri/crates/. From tools/ the app root is ../../src-tauri, so the
  // check is restated against what it was always protecting: the read tool must
  // never depend on the Tauri application crate itself.
  "path = \"../../src-tauri\"",
]) {
  if (manifest.includes(forbiddenManifestText)) {
    throw new Error(`live-read manifest exposes forbidden dependency: ${forbiddenManifestText}`);
  }
}
for (const path of walkFiles(liveReadRoot)) {
  if (!path.endsWith(".rs")) continue;
  const source = readFileSync(path, "utf8");
  for (const forbiddenSourceText of ["post_xml", "<TALLYMESSAGE", "<IMPORTDATA", "TallyHttpTransport"]) {
    if (source.includes(forbiddenSourceText)) {
      throw new Error(`live-read source exposes forbidden generic/write capability: ${forbiddenSourceText}`);
    }
  }
}

const qualificationOnlyFeatures = [
  "bills-native-outstandings-probe",
  "bills-native-outstandings-probe-receipt",
  "bills-native-outstandings-probe-runner",
  "bills-native-outstandings-probe-transport",
];
const nativeFeatureTree = spawnSync(
  "cargo",
  [
    "tree",
    "--locked",
    "--manifest-path",
    "src-tauri/Cargo.toml",
    "-p",
    "bridge",
    "--edges",
    "features",
    "--prefix",
    "none",
    "--format",
    "{p} {f}",
  ],
  { cwd: root, encoding: "utf8", maxBuffer: 64 * 1024 * 1024, windowsHide: true },
);
if (nativeFeatureTree.error || nativeFeatureTree.status !== 0) {
  throw new Error("native Bridge feature tree failed");
}
for (const feature of qualificationOnlyFeatures) {
  if (nativeFeatureTree.stdout.includes(feature)) {
    throw new Error(`qualification-only Bills probe feature is active in the native Bridge graph: ${feature}`);
  }
}

function walkFiles(directory) {
  const files = [];
  for (const entry of readdirSync(directory, { withFileTypes: true })) {
    if (entry.name === "target") continue;
    const path = `${directory}/${entry.name}`;
    if (entry.isDirectory()) files.push(...walkFiles(path));
    else files.push(path);
  }
  return files;
}

const tauriRoot = fileURLToPath(new URL("../src-tauri", import.meta.url)).replaceAll("\\", "/");
const protocolManifest = `${tauriRoot}/crates/bridge-tally-protocol/Cargo.toml`;
const toolsRoot = fileURLToPath(new URL("../tools", import.meta.url)).replaceAll("\\", "/");
const protocolModule = `${tauriRoot}/crates/bridge-tally-protocol/src/bills_native_outstandings_probe.rs`;
const protocolLib = `${tauriRoot}/crates/bridge-tally-protocol/src/lib.rs`;
const allowedManifests = new Set([
  protocolManifest,
  `${toolsRoot}/bridge-tally-compatibility/Cargo.toml`,
  `${toolsRoot}/bridge-tally-live-read/Cargo.toml`,
  `${toolsRoot}/bridge-tally-read-transport/Cargo.toml`,
]);
const allowedRust = new Set([
  protocolModule,
  protocolLib,
  `${toolsRoot}/bridge-tally-compatibility/src/bills_native_outstandings_probe_receipt.rs`,
  `${toolsRoot}/bridge-tally-compatibility/src/lib.rs`,
  `${toolsRoot}/bridge-tally-live-read/src/bin/native_outstandings_probe.rs`,
  `${toolsRoot}/bridge-tally-live-read/src/lib.rs`,
  `${toolsRoot}/bridge-tally-live-read/src/native_outstandings_qualification.rs`,
  `${toolsRoot}/bridge-tally-read-transport/src/lib.rs`,
]);
for (const path of [...walkFiles(tauriRoot), ...walkFiles(toolsRoot)]) {
  if (path.endsWith("/Cargo.toml") && !allowedManifests.has(path)) {
    const contents = readFileSync(path, "utf8");
    for (const feature of qualificationOnlyFeatures) {
      if (contents.includes(feature)) {
        throw new Error(`qualification-only Bills probe enabled outside reviewed manifests: ${relativeCheckedPath(path)}`);
      }
    }
  }
  if (path.endsWith(".rs") && !allowedRust.has(path)) {
    const contents = readFileSync(path, "utf8");
    for (const identifier of [
      "bills_native_outstandings_probe",
      "SealedNativeLedgerOutstandingsProbe",
      "LedgerOutstandingsCandidateV0",
    ]) {
      if (contents.includes(identifier)) {
        throw new Error(`qualification-only Bills probe referenced outside its module: ${relativeCheckedPath(path)}`);
      }
    }
  }
}

function relativeCheckedPath(path) {
  const scannedRoot = path.startsWith(`${toolsRoot}/`) ? toolsRoot : tauriRoot;
  return path.slice(scannedRoot.length + 1);
}

const productionSurfaces = [
  "src-tauri/Cargo.toml",
  "src-tauri/src/sync/reconciliation.rs",
  "src-tauri/src/tally/capability_packs.rs",
  "src-tauri/src/tally/connector.rs",
  "src-tauri/src/tally/runtime.rs",
  "src-tauri/src/tally/tdl_engine.rs",
];
for (const relativePath of productionSurfaces) {
  const contents = readFileSync(new URL(`../${relativePath}`, import.meta.url), "utf8");
  for (const feature of qualificationOnlyFeatures) {
    if (contents.includes(feature)) {
      throw new Error(`qualification-only Bills probe reached production surface: ${relativePath}`);
    }
  }
}

const runnerTree = spawnSync(
  "cargo",
  [
    "tree",
    "--locked",
    "--manifest-path",
    // bridge-tally-live-read now lives in the separate tools/ workspace. The
    // property this gate enforces is unchanged -- the read tool must never
    // pull in the app, tauri, or a database driver -- only its location moved.
    "tools/Cargo.toml",
    "-p",
    "bridge-tally-live-read",
    "--features",
    "bills-native-outstandings-probe-runner",
    "--edges",
    "normal",
    "--prefix",
    "none",
    "--format",
    "{p}",
  ],
  { cwd: root, encoding: "utf8", maxBuffer: 64 * 1024 * 1024, windowsHide: true },
);
if (runnerTree.error || runnerTree.status !== 0) {
  throw new Error("native outstandings runner dependency tree failed");
}
for (const forbiddenPackage of forbiddenNames) {
  if (new RegExp(`^${forbiddenPackage.replaceAll("-", "\\-")} v`, "m").test(runnerTree.stdout)) {
    throw new Error(`qualification runner reached forbidden package: ${forbiddenPackage}`);
  }
}

console.log(`Tally live-read dependency boundary is sealed (${packages.size} normal packages).`);
