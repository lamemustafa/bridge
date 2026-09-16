// SPDX-License-Identifier: Apache-2.0

import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import { mkdirSync, mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join, resolve } from "node:path";
import test from "node:test";
import { fileURLToPath } from "node:url";

import { collect, EXPECTED_TEST_MODULE_FILES } from "./check-tally-request-builder-hazards.mjs";

const repositoryRoot = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const CARGO = '[package]\nname = "fixture"\nversion = "0.0.0"\n';
// A request-builder hazard the scanner reports wherever it is scanned.
const HAZARD = 'pub fn build() -> &\'static str { "<REPORT NAME=\\"Fixture Hazard\\">" }\n';

function fixture(files) {
  const root = mkdtempSync(join(tmpdir(), "hazards-"));
  mkdirSync(join(root, "tools"));
  for (const [path, content] of Object.entries({ "src-tauri/Cargo.toml": CARGO, ...files })) {
    mkdirSync(dirname(join(root, path)), { recursive: true });
    writeFileSync(join(root, path), content);
  }
  try {
    return collect(root);
  } finally {
    rmSync(root, { recursive: true, force: true });
  }
}

const scanned = (result, file) => [...result.violations].some((violation) => violation.includes(`|src-tauri/src/${file}::`));
const skipped = (result, file) => result.skipped.has(`src-tauri/src/${file}`);

function assertSkipped(result, file) {
  assert.ok(skipped(result, file), `${file} should be skipped as a test module`);
  assert.ok(!scanned(result, file), `${file} should not be scanned`);
}

function assertScanned(result, file) {
  assert.ok(!skipped(result, file), `${file} must not be skipped`);
  assert.ok(scanned(result, file), `${file} must be scanned and its hazard reported`);
}

// --- the case this exists for, and the existing rules it must not disturb ---

test("an extracted test module loaded under #[cfg(test)] #[path] is skipped", () => {
  const result = fixture({
    "src-tauri/src/lib.rs": '#[cfg(test)]\n#[path = "lib_tests.rs"]\nmod tests;\n',
    "src-tauri/src/lib_tests.rs": HAZARD,
  });
  assertSkipped(result, "lib_tests.rs");
});

test("an inline #[cfg(test)] mod body is still skipped and a bare #[cfg(test)] fn is still scanned", () => {
  const result = fixture({
    "src-tauri/src/lib.rs":
      '#[cfg(test)]\nmod tests {\n    fn inline() -> &\'static str { "<REPORT NAME=\\"Inline\\">" }\n}\n' +
      '#[cfg(test)]\nfn legacy_request() -> &\'static str { "<REPORT NAME=\\"Bare Test Fn\\">" }\n',
  });
  assert.ok(![...result.violations].some((violation) => violation.endsWith("|Inline")));
  assert.ok([...result.violations].some((violation) => violation.endsWith("|Bare Test Fn")));
});

// --- file names are never trusted ---

test("a file named *_tests.rs loaded without #[cfg(test)] is scanned", () => {
  const result = fixture({
    "src-tauri/src/lib.rs": '#[path = "evil_tests.rs"]\nmod evil;\n',
    "src-tauri/src/evil_tests.rs": HAZARD,
  });
  assertScanned(result, "evil_tests.rs");
});

// --- cfg shapes ---

test("cfg(all(test, ...)) implies test and is skipped", () => {
  const result = fixture({
    "src-tauri/src/lib.rs": '#[cfg(all(test, target_os = "macos"))]\n#[path = "mac_tests.rs"]\nmod mac_tests;\n',
    "src-tauri/src/mac_tests.rs": HAZARD,
  });
  assertSkipped(result, "mac_tests.rs");
});

test("cfg(any(test, ...)), cfg(not(test)) and cfg_attr(..., cfg(test)) do not imply test and are scanned", () => {
  const result = fixture({
    "src-tauri/src/lib.rs":
      '#[cfg(any(test, feature = "x"))]\n#[path = "any_tests.rs"]\nmod any_tests;\n' +
      '#[cfg(not(test))]\n#[path = "not_tests.rs"]\nmod not_tests;\n' +
      '#[cfg_attr(feature = "x", cfg(test))]\n#[path = "attr_tests.rs"]\nmod attr_tests;\n',
    "src-tauri/src/any_tests.rs": HAZARD,
    "src-tauri/src/not_tests.rs": HAZARD,
    "src-tauri/src/attr_tests.rs": HAZARD,
  });
  assertScanned(result, "any_tests.rs");
  assertScanned(result, "not_tests.rs");
  assertScanned(result, "attr_tests.rs");
});

// --- attribute group parsing ---

test("#[path] before #[cfg(test)], a raw-string path, and an attribute containing ] are all recognised", () => {
  const result = fixture({
    "src-tauri/src/lib.rs":
      '#[path = "first_tests.rs"]\n#[cfg(test)]\nmod first;\n' +
      '#[cfg(test)]\n#[path = r"raw_tests.rs"]\nmod raw;\n' +
      '#[doc = "a]"]\n#[cfg(test)]\n#[path = "bracket_tests.rs"]\nmod bracket;\n',
    "src-tauri/src/first_tests.rs": HAZARD,
    "src-tauri/src/raw_tests.rs": HAZARD,
    "src-tauri/src/bracket_tests.rs": HAZARD,
  });
  assertSkipped(result, "first_tests.rs");
  assertSkipped(result, "raw_tests.rs");
  assertSkipped(result, "bracket_tests.rs");
});

test("a bare #[cfg(test)] mod tests; in mod.rs resolves beside it and is skipped", () => {
  const result = fixture({
    "src-tauri/src/lib.rs": "mod report;\n",
    "src-tauri/src/report/mod.rs": "#[cfg(test)]\nmod tests;\n",
    "src-tauri/src/report/tests.rs": HAZARD,
  });
  assertSkipped(result, "report/tests.rs");
});

// --- declarations that are not trusted edges ---

test("declaration text inside a comment or a string is not an edge", () => {
  const result = fixture({
    "src-tauri/src/lib.rs":
      '// #[cfg(test)] #[path = "commented_tests.rs"] mod a;\n' +
      'const S: &str = r#"\n#[cfg(test)]\n#[path = "quoted_tests.rs"]\nmod b;\n"#;\n',
    // No production declaration loads these: the scanner reads every .rs file
    // under its roots, and a production declaration would veto the skip and
    // hide a lexer that mistook the text for a real declaration.
    "src-tauri/src/commented_tests.rs": HAZARD,
    "src-tauri/src/quoted_tests.rs": HAZARD,
  });
  assertScanned(result, "commented_tests.rs");
  assertScanned(result, "quoted_tests.rs");
});

test("a #[cfg(test)] #[path] declaration inside an inline module is not an edge", () => {
  const result = fixture({
    "src-tauri/src/lib.rs": 'mod outer {\n    #[cfg(test)]\n    #[path = "nested_tests.rs"]\n    mod nested;\n}\n',
    "src-tauri/src/nested_tests.rs": HAZARD,
  });
  assertScanned(result, "nested_tests.rs");
});

test("edges from a file whose braces do not balance are not trusted", () => {
  const result = fixture({
    "src-tauri/src/lib.rs": '}\n#[cfg(test)]\n#[path = "unbalanced_tests.rs"]\nmod tests;\n',
    "src-tauri/src/unbalanced_tests.rs": HAZARD,
  });
  assertScanned(result, "unbalanced_tests.rs");
});

// --- propagation ---

test("a module declared by a test-only file is test-only, unless production also declares it", () => {
  const result = fixture({
    "src-tauri/src/lib.rs": '#[cfg(test)]\n#[path = "parent_tests.rs"]\nmod tests;\n#[path = "shared_tests.rs"]\nmod shared;\n',
    "src-tauri/src/parent_tests.rs": '#[path = "child_tests.rs"]\nmod child;\n#[path = "shared_tests.rs"]\nmod shared;\n',
    "src-tauri/src/child_tests.rs": HAZARD,
    "src-tauri/src/shared_tests.rs": HAZARD,
  });
  assertSkipped(result, "parent_tests.rs");
  assertSkipped(result, "child_tests.rs");
  assertScanned(result, "shared_tests.rs");
});

// --- vetoes: anything that might load the file as production keeps it scanned ---

test("a bare production mod declaration of the same file vetoes the skip", () => {
  const result = fixture({
    "src-tauri/src/lib.rs": '#[cfg(test)]\n#[path = "vetoed_tests.rs"]\nmod tests;\nmod vetoed_tests;\n',
    "src-tauri/src/vetoed_tests.rs": HAZARD,
  });
  assertScanned(result, "vetoed_tests.rs");
});

test("cfg_attr path, a macro body, include!, an inline module and a case variant each veto the skip", () => {
  const cases = {
    "cfg_attr_tests.rs": '#[cfg_attr(not(test), path = "cfg_attr_tests.rs")]\nmod production;\n',
    "macro_tests.rs": "macro_rules! declare { () => { mod macro_tests; }; }\n",
    "include_tests.rs": 'const X: &str = include_str!("include_tests.rs");\n',
    "inline_tests.rs": "mod outer {\n    mod inline_tests;\n}\n",
    "case_tests.rs": '#[path = "CASE_TESTS.rs"]\nmod case_variant;\n',
  };
  for (const [file, veto] of Object.entries(cases)) {
    const result = fixture({
      "src-tauri/src/lib.rs": `#[cfg(test)]\n#[path = "${file}"]\nmod tests;\n`,
      "src-tauri/src/other.rs": veto,
      [`src-tauri/src/${file}`]: HAZARD,
    });
    assertScanned(result, file);
  }
});

test("a Cargo target path vetoes the skip", () => {
  const result = fixture({
    "src-tauri/Cargo.toml": `${CARGO}[[bin]]\nname = "x"\npath = "src/bin_tests.rs"\n`,
    "src-tauri/src/lib.rs": '#[cfg(test)]\n#[path = "bin_tests.rs"]\nmod tests;\n',
    "src-tauri/src/bin_tests.rs": HAZARD,
  });
  assertScanned(result, "bin_tests.rs");
});

// --- the real repository ---

test("the gate itself runs, matches the pinned violations and pins the quarantine size", () => {
  const run = spawnSync(process.execPath, [join(repositoryRoot, "scripts/check-tally-request-builder-hazards.mjs")], {
    encoding: "utf8",
  });
  assert.equal(run.status, 0, run.stderr);
  assert.match(run.stdout, new RegExp(`\\(10 violations; ${EXPECTED_TEST_MODULE_FILES} test-only module files skipped\\)`));
});
