#!/usr/bin/env node
// Contract tests for scripts/check-parser-test-symmetry.mjs.
import assert from "node:assert/strict";
import { execFileSync, spawnSync } from "node:child_process";
import { mkdir, mkdtemp, rm, writeFile } from "node:fs/promises";
import { join } from "node:path";
import test from "node:test";
import { tmpdir } from "node:os";
import { fileURLToPath } from "node:url";

const here = fileURLToPath(new URL(".", import.meta.url));
const GATE = join(here, "check-parser-test-symmetry.mjs");

function git(cwd, ...args) {
  const out = spawnSync("git", args, { cwd, encoding: "utf8" });
  if (out.status !== 0) throw new Error(`git ${args.join(" ")}: ${out.stderr}`);
  return out.stdout;
}

async function makeRepo(files) {
  const root = await mkdtemp(join(tmpdir(), ".parser-symmetry-"));
  for (const [path, content] of Object.entries(files)) {
    const directory = path.split("/").slice(0, -1).join("/");
    if (directory) await mkdir(join(root, directory), { recursive: true });
    await writeFile(join(root, path), content);
  }
  git(root, "init", "-q", ".");
  git(root, "config", "user.email", "test@example.invalid");
  git(root, "config", "user.name", "test");
  git(root, "add", "-A");
  git(root, "commit", "-qm", "base");
  const base = git(root, "rev-parse", "HEAD").trim();
  return { root, base };
}

function runGate(root, base) {
  return execFileSync("node", [GATE, "--root", root, "--base", base], {
    encoding: "utf8",
    stdio: "pipe",
  });
}

function runGateExpectingFailure(root, base) {
  try {
    runGate(root, base);
  } catch (error) {
    return `${error.stdout ?? ""}${error.stderr ?? ""}`;
  }
  throw new Error("expected the gate to fail, but it passed");
}

const PARSER_V1 = `pub fn parse_amount(text: &str) -> Result<i64, String> {
    text.parse::<i64>().map_err(|_| "bad amount".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
}
`;

function withAddedTests(extraTestBody) {
  return PARSER_V1.replace(
    "mod tests {\n    use super::*;\n}\n",
    `mod tests {\n    use super::*;\n${extraTestBody}}\n`,
  );
}

test("no changes passes", async () => {
  const { root, base } = await makeRepo({ "src-tauri/src/amount_parser.rs": PARSER_V1 });
  try {
    const output = runGate(root, base);
    assert.match(output, /0 parser module\(s\) touched/);
  } finally {
    await rm(root, { recursive: true, force: true });
  }
});

test("a new accept-path test with no reject-path test fails", async () => {
  const { root, base } = await makeRepo({ "src-tauri/src/amount_parser.rs": PARSER_V1 });
  try {
    await writeFile(
      join(root, "src-tauri/src/amount_parser.rs"),
      withAddedTests(
        `\n    #[test]\n    fn parses_valid_amount() {\n        assert_eq!(parse_amount("100").unwrap(), 100);\n    }\n`,
      ),
    );
    const output = runGateExpectingFailure(root, base);
    assert.match(output, /amount_parser\.rs/);
    assert.match(output, /parses_valid_amount/);
    assert.match(output, /no new reject-path test/);
  } finally {
    await rm(root, { recursive: true, force: true });
  }
});

test("a new accept-path test paired with a new reject-path test passes", async () => {
  const { root, base } = await makeRepo({ "src-tauri/src/amount_parser.rs": PARSER_V1 });
  try {
    await writeFile(
      join(root, "src-tauri/src/amount_parser.rs"),
      withAddedTests(
        `\n    #[test]\n    fn parses_valid_amount() {\n        assert_eq!(parse_amount("100").unwrap(), 100);\n    }\n\n` +
          `    #[test]\n    fn rejects_invalid_amount() {\n        assert!(parse_amount("abc").is_err());\n    }\n`,
      ),
    );
    const output = runGate(root, base);
    assert.match(output, /1 parser module\(s\) touched/);
  } finally {
    await rm(root, { recursive: true, force: true });
  }
});

test("adding only a reject-path test (no accept test) passes", async () => {
  const { root, base } = await makeRepo({ "src-tauri/src/amount_parser.rs": PARSER_V1 });
  try {
    await writeFile(
      join(root, "src-tauri/src/amount_parser.rs"),
      withAddedTests(
        `\n    #[test]\n    fn rejects_malformed_amount() {\n        assert!(parse_amount("abc").is_err());\n    }\n`,
      ),
    );
    assert.doesNotThrow(() => runGate(root, base));
  } finally {
    await rm(root, { recursive: true, force: true });
  }
});

test("an unclassifiable new test name does not trigger a false failure", async () => {
  const { root, base } = await makeRepo({ "src-tauri/src/amount_parser.rs": PARSER_V1 });
  try {
    await writeFile(
      join(root, "src-tauri/src/amount_parser.rs"),
      withAddedTests(
        `\n    #[test]\n    fn amount_roundtrips_through_serde() {\n        let _ = parse_amount("100");\n    }\n`,
      ),
    );
    // "roundtrips" is not a whole token ("roundtrip" is, "roundtrips" is not),
    // so this new test is unclassifiable and must not force a failure.
    assert.doesNotThrow(() => runGate(root, base));
  } finally {
    await rm(root, { recursive: true, force: true });
  }
});

test("a non-parser Rust file (no `parse` function) is not checked at all", async () => {
  const source = `pub fn add(a: i64, b: i64) -> i64 { a + b }

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn adds_valid_numbers() {
        assert_eq!(add(1, 2), 3);
    }
}
`;
  const { root, base } = await makeRepo({ "src-tauri/src/math.rs": source });
  try {
    const output = runGate(root, base);
    assert.match(output, /0 parser module\(s\) touched/);
  } finally {
    await rm(root, { recursive: true, force: true });
  }
});

test("a file with an existing accept test gaining only a NEW accept test still fails", async () => {
  // Pre-existing tests (already on the base) do not count — only what the
  // diff itself adds. Otherwise a file that already has one reject test ever
  // in its history would permanently exempt every future accept-only change.
  const withExistingRejectTest = `pub fn parse_amount(text: &str) -> Result<i64, String> {
    text.parse::<i64>().map_err(|_| "bad amount".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_invalid_amount_long_ago() {
        assert!(parse_amount("abc").is_err());
    }
}
`;
  const withNewAcceptTestToo = `pub fn parse_amount(text: &str) -> Result<i64, String> {
    text.parse::<i64>().map_err(|_| "bad amount".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_invalid_amount_long_ago() {
        assert!(parse_amount("abc").is_err());
    }

    #[test]
    fn parses_valid_amount() {
        assert_eq!(parse_amount("100").unwrap(), 100);
    }
}
`;
  const { root, base } = await makeRepo({
    "src-tauri/src/amount_parser.rs": withExistingRejectTest,
  });
  try {
    await writeFile(join(root, "src-tauri/src/amount_parser.rs"), withNewAcceptTestToo);
    const output = runGateExpectingFailure(root, base);
    assert.match(output, /no new reject-path test/);
  } finally {
    await rm(root, { recursive: true, force: true });
  }
});

console.log("\nrunning check-parser-test-symmetry contract tests via node:test above");
