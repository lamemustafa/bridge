// SPDX-License-Identifier: Apache-2.0

// Locks in the README's central privacy promise (README.md, 'What it does not do'):
//
//   "Your Tally data is never uploaded. Bridge reads it over a local
//   connection and hands it to the assistant you are talking to; nothing in
//   the Tally path sends it to a server of ours."
//
// and the separation promise for the upload-capable parts of the app
// (README.md, 'One part of the app does upload'):
//
//   "Bridge also contains a document feature that uploads files you choose
//   to ComplyEaze cloud storage, and an AXAL sign-in. Those are separate and
//   user-initiated, and share no code with the Tally path described here"
//
// Before this gate, both sentences were prose: nothing stopped a new
// `reqwest` call site from landing anywhere in the tree, including inside
// the Tally read/write path itself.
//
// Two checks, deliberately different in what they can see:
//
// 1. A `cargo tree` boundary (modelled on
//    scripts/check-tally-live-read-boundary.mjs): asserts which first-party
//    crates are allowed to declare a *direct* dependency on an outbound HTTP
//    client (reqwest) or its transport (hyper) at all. This is a
//    compile-time property -- cargo will not link a crate against reqwest
//    unless its Cargo.toml says so -- but it only sees crate boundaries. It
//    cannot see what a crate that *is* allowed to depend on reqwest
//    (`bridge`, the app crate, which legitimately needs it for
//    axal.rs/documents.rs -- see the note above APP_CRATE: both ship in the
//    extension binary too, not only in the desktop app) does with that
//    dependency inside its own files. It follows normal, build and dev
//    edges alike: a dev- or build-dependency on reqwest in a crate outside
//    the allow-list is refused too, since a test double or build script
//    that can open a connection is still egress from a developer's machine.
//    The sets are pinned exactly and the tree must be seen: a crate that
//    drops out, a missing root line, a failed `cargo` or an unparseable line
//    all fail, so "nothing found" cannot stand in for "nothing was read".
//
// 2. A source scan of the app crate (`src-tauri/src`): asserts that
//    `reqwest::`, `hyper::`, and raw socket construction appear only in a
//    pinned allow-list of files. This is what catches a new call site added
//    to an unlisted file inside `bridge` -- exactly the gap the cargo-tree
//    check above cannot close, because `bridge` already has the dependency
//    edge and adding a new caller inside it changes no Cargo.toml.
//
// What this gate does NOT prove (read before relying on it further):
//  - It does not prove the Tally transport's loopback restriction
//    (`non_loopback_forbidden` in bridge-tally-transport, which rejects any
//    non-loopback host at request-construction time) is itself correct or
//    still wired up -- that is a runtime property with its own tests in
//    bridge-tally-transport, not this gate.
//  - It does not catch an HTTP client built through indirection this scan
//    does not pattern-match: a re-exported alias (`use reqwest as http;`
//    then `http::Client`), a macro that expands to a reqwest call, a crate
//    obtained through a build script, or a raw `std::net` connect spelled
//    some way other than the literal patterns below (e.g. through a helper
//    function whose *name* doesn't mention sockets).
//  - It does not catch network egress performed by a non-Rust dependency
//    (a native library, a downloaded binary, a JS/webview call outside the
//    scanned source) -- the webview CSP (`ipc:` only) is the control for
//    that surface, not this file.
//  - The source scan only covers `src-tauri/src`. A new call site inside
//    `crates/*` or `tools/*` is instead caught by the cargo-tree half
//    (those crates would need a new Cargo.toml dependency edge to compile
//    one), not by pattern-matching source text.
//  - The scan matches `reqwest::`/`hyper::` as plain substrings, including
//    inside comments and string literals. That is deliberately
//    conservative (a mention in a comment must still live in an
//    allow-listed file) rather than a source of missed real call sites.

import { spawnSync } from "node:child_process";
import { readFileSync, readdirSync } from "node:fs";
import { fileURLToPath } from "node:url";

const root = fileURLToPath(new URL("../", import.meta.url));

// ---------------------------------------------------------------------------
// Check 1: which first-party crates may directly depend on an outbound HTTP
// client at all.
// ---------------------------------------------------------------------------

// The one crate that is allowed to carry reqwest as a *direct* dependency
// for talking to Tally itself: the loopback-only HTTP transport. It is
// reached by `bridge` (the app crate) and, through it, by the read-only
// tools in tools/bridge-tally-live-read -- both expected, both already
// covered by scripts/check-tally-live-read-boundary.mjs's own first-party
// boundary list. This gate only asks one narrower question: which crates
// declare reqwest *directly*, not which crates reach it transitively.
const TALLY_HTTP_TRANSPORT_CRATE = "bridge-tally-transport";

// `bridge` is the app crate. It legitimately depends on reqwest directly for
// two things that are NOT the Tally path: axal.rs (AXAL sign-in / cloud
// storage) and documents.rs (the document upload feature). The README paragraph
// 'One part of the app does upload' names both explicitly as the parts of the
// app that DO upload.
//
// Do not read "app crate" as "Tauri only". Both modules are declared
// unconditionally in lib.rs, with no cfg(feature) gate; src/bin/bridge_mcp.rs
// links bridge_lib; and scripts/package-mcpb.mjs ships bridge_mcp as the
// extension binary. So this reqwest edge is compiled into the artifact a user
// installs, not just into the desktop app. The honest claim is "present and
// unreachable from the agent surface", not "absent" -- and "unreachable" is
// what source check 2 below exists to keep true.
//
// The standard this gate is modelled on is the Tally transport's own loopback
// guard, which is stronger than a file allow-list: `endpoint_url` special-cases
// only the literal string "localhost" and hardcodes 127.0.0.1 without ever
// resolving DNS, so a hosts-file entry aiming a hostname at loopback cannot
// pass. It runs on every network method rather than once at construction,
// redirects are disabled, and no environment variable or cargo feature relaxes
// it. Where a future control can be written that way, prefer it to a list.
const APP_CRATE = "bridge";

function directDependents(manifestPath, packageName) {
  const result = spawnSync(
    "cargo",
    [
      "tree",
      "--locked",
      "--manifest-path",
      manifestPath,
      "-p",
      packageName,
      "--invert",
      "--depth",
      "1",
      "--edges",
      "normal,build,dev",
      "--prefix",
      "none",
      "--format",
      "{p}",
    ],
    { cwd: root, encoding: "utf8", maxBuffer: 64 * 1024 * 1024, windowsHide: true },
  );
  // Both workspaces resolve both packages, so there is no "nothing to check"
  // exit here: any failure, including "did not match any packages", means
  // the tree was not read.
  if (result.error) {
    throw new Error(`dependency tree for ${packageName} (${manifestPath}) could not run cargo: ${result.error.message}`);
  }
  if (result.status !== 0) {
    throw new Error(`dependency tree for ${packageName} (${manifestPath}) exited ${result.status}: ${result.stderr}`);
  }
  const lines = result.stdout.split(/\r?\n/).filter((line) => line !== "");
  const parsed = lines.map((line) => {
    const match = line.match(/^([A-Za-z0-9_.+-]+) v\S+(?: \((.+)\))?$/);
    if (!match) {
      throw new Error(`dependency tree for ${packageName} (${manifestPath}) printed an unparseable line: ${JSON.stringify(line)}`);
    }
    return { name: match[1], source: match[2] };
  });
  // `--invert` prints the package itself first. Without that line no tree
  // was produced, and an empty dependent list would prove nothing.
  if (parsed[0]?.name !== packageName) {
    throw new Error(
      `dependency tree for ${packageName} (${manifestPath}) did not start with ${packageName}; ` +
        `got ${JSON.stringify(lines[0] ?? "")} (${lines.length} line(s))`,
    );
  }
  // Only lines carrying a parenthesized on-disk path are first-party
  // (workspace or path) dependencies -- `cargo tree`'s `{p}` format appends
  // that path for anything not resolved from a registry. A third-party
  // crate that happens to also depend on `packageName` directly (e.g.
  // hyper-util and hyper-rustls both depend on hyper directly, same as
  // reqwest does) has no such path and is not what this gate is asking
  // about: it cares which crates *we* wrote declare the dependency, not
  // reqwest's own internal transport plumbing.
  const names = new Set();
  for (const { name, source } of parsed.slice(1)) {
    if (source?.startsWith(root.slice(0, -1))) names.add(name);
  }
  return [...names].sort();
}

// The exact first-party crates with a direct dependency on each package, per
// workspace, as `cargo tree` reports them today. The tools workspace reaches
// reqwest only through the transport; hyper is reqwest's own transport, and
// a first-party crate using it directly would build an HTTP client that
// bypasses reqwest and bridge-tally-transport's loopback check entirely.
const workspaces = [
  {
    label: "src-tauri",
    manifestPath: "src-tauri/Cargo.toml",
    expected: { reqwest: [APP_CRATE, TALLY_HTTP_TRANSPORT_CRATE], hyper: [] },
  },
  {
    label: "tools",
    manifestPath: "tools/Cargo.toml",
    expected: { reqwest: [TALLY_HTTP_TRANSPORT_CRATE], hyper: [] },
  },
];

const egressViolations = [];

for (const workspace of workspaces) {
  for (const [packageName, expectedNames] of Object.entries(workspace.expected)) {
    const expected = [...expectedNames].sort();
    const actual = directDependents(workspace.manifestPath, packageName);
    const gained = actual.filter((name) => !expected.includes(name));
    const lost = expected.filter((name) => !actual.includes(name));
    if (gained.length) {
      egressViolations.push(
        `${workspace.label}: crate(s) gained a direct ${packageName} dependency outside the pinned set ` +
          `(${expected.join(", ") || "none"}): ${gained.join(", ")}`,
      );
    }
    if (lost.length) {
      egressViolations.push(
        `${workspace.label}: pinned crate(s) no longer show a direct ${packageName} dependency: ` +
          `${lost.join(", ")}. Either the tree was not read in full or the dependency moved; narrow the ` +
          "pinned set in scripts/check-tally-egress-boundary.mjs only after confirming which.",
      );
    }
  }
}

// ---------------------------------------------------------------------------
// Check 2: inside the app crate, only these files may build an outbound
// HTTP request or a raw socket.
// ---------------------------------------------------------------------------

// Exact set, matched both ways (extra files using a forbidden pattern, and
// allow-listed files that no longer need to be) so the list cannot drift
// silently in either direction -- same shape as
// admission_and_egress_files_stay_pinned.rs's pin check.
const APP_CRATE_HTTP_ALLOW_LIST = new Set([
  // AXAL sign-in and document upload: the two features the README paragraph
  // 'One part of the app does upload' names as the parts of the app that DO
  // upload, on purpose, user-initiated.
  "src-tauri/src/axal.rs",
  "src-tauri/src/documents.rs",
  // The Tally HTTP transport wrapper: reqwest is used here, but only to
  // reach Tally itself over loopback (bridge-tally-transport's
  // `canonical_loopback_origin` rejects any other host before a request is
  // ever built). Its test file constructs the same client for test doubles.
  "src-tauri/src/tally/connection.rs",
  "src-tauri/src/tally/connection_tests.rs",
]);

const FORBIDDEN_SOURCE_PATTERNS = [
  "reqwest::",
  "hyper::",
  "TcpStream::connect",
  "UdpSocket::bind",
  "std::net::TcpStream",
  "std::net::UdpSocket",
];

function rustFiles(directory) {
  const files = [];
  for (const entry of readdirSync(directory, { withFileTypes: true })) {
    if (entry.name === "target") continue;
    const path = `${directory}/${entry.name}`;
    if (entry.isDirectory()) files.push(...rustFiles(path));
    else if (entry.name.endsWith(".rs")) files.push(path);
  }
  return files;
}

const appCrateSourceRoot = fileURLToPath(new URL("../src-tauri/src", import.meta.url)).replaceAll("\\", "/");
const filesWithForbiddenPatterns = new Set();
for (const path of rustFiles(appCrateSourceRoot)) {
  const relativePath = `src-tauri/src${path.slice(appCrateSourceRoot.length)}`;
  const source = readFileSync(path, "utf8");
  if (FORBIDDEN_SOURCE_PATTERNS.some((pattern) => source.includes(pattern))) {
    filesWithForbiddenPatterns.add(relativePath);
  }
}

const unlistedCallSites = [...filesWithForbiddenPatterns]
  .filter((path) => !APP_CRATE_HTTP_ALLOW_LIST.has(path))
  .sort();
const staleAllowListEntries = [...APP_CRATE_HTTP_ALLOW_LIST]
  .filter((path) => !filesWithForbiddenPatterns.has(path))
  .sort();

if (unlistedCallSites.length) {
  egressViolations.push(
    "src-tauri/src: found an outbound HTTP client or raw socket construction outside the pinned " +
      `allow-list (${[...APP_CRATE_HTTP_ALLOW_LIST].sort().join(", ")}): ${unlistedCallSites.join(", ")}. ` +
      'This falsifies the README promise "nothing in the Tally path sends it to a server of ours" ' +
      "(README.md, 'What it does not do') unless the new call site is one of the app's already-documented upload " +
      "features (README.md, 'One part of the app does upload'). If it is, add it to APP_CRATE_HTTP_ALLOW_LIST in " +
      "scripts/check-tally-egress-boundary.mjs with a reviewed reason; if it is not, it does not belong.",
  );
}
if (staleAllowListEntries.length) {
  egressViolations.push(
    "src-tauri/src: allow-listed file(s) no longer contain an outbound HTTP client or raw socket " +
      `construction; narrow APP_CRATE_HTTP_ALLOW_LIST in scripts/check-tally-egress-boundary.mjs: ` +
      staleAllowListEntries.join(", "),
  );
}

if (egressViolations.length) {
  throw new Error(
    "Tally-path egress boundary violated -- this protects the README promise " +
      '"Your Tally data is never uploaded ... nothing in the Tally path sends it to a server of ours" ' +
      "(README.md, 'What it does not do'):\n" +
      egressViolations.map((violation) => `- ${violation}`).join("\n"),
  );
}

console.log(
  "Tally-path egress boundary is sealed: reqwest/hyper are confined to the pinned crates and, inside " +
    `the app crate, to ${APP_CRATE_HTTP_ALLOW_LIST.size} pinned files.`,
);
