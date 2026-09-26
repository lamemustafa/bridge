// SPDX-License-Identifier: Apache-2.0
// Drives the egress gate's cargo-tree check with a stand-in `cargo` on PATH.
// The first row is the control: the same stand-in, given the trees cargo
// prints today, must pass, so each failing row fails on its tree and not on
// the stand-in. Check 2 (the source scan) runs against the real tree
// throughout.
import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import { chmodSync, existsSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { delimiter, join } from "node:path";
import { after, before, test } from "node:test";
import { fileURLToPath } from "node:url";

const root = fileURLToPath(new URL("../", import.meta.url)).slice(0, -1);
const gate = fileURLToPath(new URL("./check-tally-egress-boundary.mjs", import.meta.url));
const skip = process.platform === "win32" && "the stand-in cargo is a POSIX shell script";

// Each call records its package, then prints `<workspace>.<package>.out`
// from the row's directory, if the row wrote one. A call without
// `--target all` fails, so the control row also guards that flag.
const STAND_IN = `#!/bin/sh
manifest=""; package=""; target=""
while [ $# -gt 0 ]; do
  case "$1" in
    --manifest-path) manifest="$2"; shift ;;
    -p) package="$2"; shift ;;
    --target) target="$2"; shift ;;
  esac
  shift
done
echo "\${manifest%%/*}.$package" >> "$0.calls"
if [ "$target" != "all" ]; then
  echo "stand-in cargo: expected --target all, got '$target'" >&2
  exit 2
fi
[ -f "$TREES/\${manifest%%/*}.$package.out" ] && cat "$TREES/\${manifest%%/*}.$package.out"
[ -n "$STAND_IN_STDERR" ] && echo "$STAND_IN_STDERR" >&2
exit "\${STAND_IN_EXIT:-0}"
`;

// As printed by `cargo tree --invert --depth 1 --prefix none --format {p}
// --target all` on 2026-09-26.
const TODAY = {
  "src-tauri.reqwest": [
    "reqwest v0.13.5",
    `bridge v0.2.0 (${root}/src-tauri)`,
    `bridge-tally-transport v0.1.0 (${root}/src-tauri/crates/bridge-tally-transport)`,
    "tauri v2.11.5",
  ],
  "src-tauri.hyper": ["hyper v1.11.0", "hyper-rustls v0.27.9", "hyper-util v0.1.20", "reqwest v0.13.5"],
  "tools.reqwest": [
    "reqwest v0.13.4",
    `bridge-tally-transport v0.1.0 (${root}/src-tauri/crates/bridge-tally-transport)`,
  ],
  "tools.hyper": ["hyper v1.11.0", "hyper-rustls v0.27.9", "hyper-util v0.1.20", "reqwest v0.13.4"],
};

let bin;
before(() => {
  bin = mkdtempSync(join(tmpdir(), "egress-gate-"));
  writeFileSync(join(bin, "cargo"), STAND_IN);
  chmodSync(join(bin, "cargo"), 0o755);
});
after(() => rmSync(bin, { recursive: true, force: true }));

function runGate(trees, env = {}) {
  const dir = mkdtempSync(join(bin, "trees-"));
  for (const [key, lines] of Object.entries(trees)) writeFileSync(join(dir, `${key}.out`), lines.map((l) => `${l}\n`).join(""));
  rmSync(join(bin, "cargo.calls"), { force: true });
  const result = spawnSync(process.execPath, [gate], {
    cwd: root,
    encoding: "utf8",
    env: { ...process.env, PATH: `${bin}${delimiter}${process.env.PATH}`, TREES: dir, ...env },
  });
  const calls = existsSync(join(bin, "cargo.calls")) ? readFileSync(join(bin, "cargo.calls"), "utf8").trim().split("\n") : [];
  return { ...result, calls };
}

function assertRefused(result, message) {
  assert.notEqual(result.status, 0, `gate passed:\n${result.stdout}`);
  assert.ok(result.stderr.includes(message), `expected ${JSON.stringify(message)} in:\n${result.stderr}`);
  assert.ok(!result.stdout.includes("is sealed"), result.stdout);
}

test("control: the trees cargo prints today pass, and all four are read", { skip }, () => {
  const result = runGate(TODAY);
  assert.equal(result.status, 0, result.stderr);
  assert.match(result.stdout, /^Tally-path egress boundary is sealed:/);
  assert.deepEqual(result.calls, ["src-tauri.reqwest", "src-tauri.hyper", "tools.reqwest", "tools.hyper"]);
});

test("an empty tree from a cargo that exits 0 fails closed", { skip }, () => {
  const result = runGate({});
  assertRefused(result, 'dependency tree for reqwest (src-tauri/Cargo.toml) did not start with reqwest; got "" (0 line(s))');
  assert.deepEqual(result.calls, ["src-tauri.reqwest"]);
});

// hyper's pinned set is empty, so the pinned-set comparison cannot tell an
// empty read from a clean one; only the root-line check refuses this.
test("an empty hyper tree fails closed although its pinned set is empty", { skip }, () => {
  const { "tools.hyper": _omitted, ...rest } = TODAY;
  const result = runGate(rest);
  assertRefused(result, 'dependency tree for hyper (tools/Cargo.toml) did not start with hyper; got "" (0 line(s))');
  assert.deepEqual(result.calls, ["src-tauri.reqwest", "src-tauri.hyper", "tools.reqwest", "tools.hyper"]);
});

test("a tree with only its root line reports every pinned crate as lost", { skip }, () => {
  const result = runGate({ ...TODAY, "src-tauri.reqwest": ["reqwest v0.13.5"] });
  assertRefused(result, "src-tauri: pinned crate(s) no longer show a direct reqwest dependency: bridge, bridge-tally-transport.");
});

test("cargo failing, even with 'did not match any packages', is not an empty workspace", { skip }, () => {
  const result = runGate(TODAY, {
    STAND_IN_EXIT: "101",
    STAND_IN_STDERR: "error: package ID specification `reqwest` did not match any packages",
  });
  assertRefused(result, "dependency tree for reqwest (src-tauri/Cargo.toml) exited 101: error: package ID specification");
});

test("a new first-party dependent is refused", { skip }, () => {
  const extra = `bridge-tally-protocol v0.1.0 (${root}/src-tauri/crates/bridge-tally-protocol)`;
  const result = runGate({ ...TODAY, "tools.hyper": [...TODAY["tools.hyper"], extra] });
  assertRefused(result, "tools: crate(s) gained a direct hyper dependency outside the pinned set (none): bridge-tally-protocol");
});

test("an unparseable line fails instead of being skipped", { skip }, () => {
  const result = runGate({ ...TODAY, "tools.reqwest": [...TODAY["tools.reqwest"], "warning: something else"] });
  assertRefused(result, 'dependency tree for reqwest (tools/Cargo.toml) printed an unparseable line: "warning: something else"');
});
