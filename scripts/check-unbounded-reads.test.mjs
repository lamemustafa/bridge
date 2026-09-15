#!/usr/bin/env node
// Contract tests for scripts/check-unbounded-reads.mjs.
//
// Each case is a synthetic --root tree with one real git repository (the
// scanner shells out to `git ls-files`, so it needs one) holding a single
// Rust file exercising one branch of the scanner's logic.
import assert from "node:assert/strict";
import { execFileSync, spawnSync } from "node:child_process";
import { mkdir, mkdtemp, rm, writeFile } from "node:fs/promises";
import { join } from "node:path";
import test from "node:test";
import { tmpdir } from "node:os";
import { fileURLToPath } from "node:url";

const here = fileURLToPath(new URL(".", import.meta.url));
const GATE = join(here, "check-unbounded-reads.mjs");

function git(cwd, ...args) {
  const out = spawnSync("git", args, { cwd, encoding: "utf8" });
  if (out.status !== 0) throw new Error(`git ${args.join(" ")}: ${out.stderr}`);
  return out.stdout;
}

async function makeRepo(rustFileRelativePath, rustSource) {
  const root = await mkdtemp(join(tmpdir(), ".unbounded-reads-"));
  const directory = rustFileRelativePath.split("/").slice(0, -1).join("/");
  await mkdir(join(root, directory), { recursive: true });
  await writeFile(join(root, rustFileRelativePath), rustSource);
  git(root, "init", "-q", ".");
  git(root, "config", "user.email", "test@example.invalid");
  git(root, "config", "user.name", "test");
  git(root, "add", "-A");
  git(root, "commit", "-qm", "seed");
  return root;
}

function runGate(root) {
  return execFileSync("node", [GATE, "--root", root], { encoding: "utf8", stdio: "pipe" });
}

function runGateExpectingFailure(root) {
  try {
    runGate(root);
  } catch (error) {
    return `${error.stdout ?? ""}${error.stderr ?? ""}`;
  }
  throw new Error("expected the gate to fail, but it passed");
}

test("a .take()-guarded read_to_end passes", async () => {
  const root = await makeRepo(
    "src-tauri/src/example.rs",
    `fn read_bounded(mut reader: impl std::io::Read) -> std::io::Result<Vec<u8>> {
    let mut output = Vec::new();
    reader.take(1024 + 1).read_to_end(&mut output)?;
    Ok(output)
}
`,
  );
  try {
    const output = runGate(root);
    assert.match(output, /0 unbounded/);
  } finally {
    await rm(root, { recursive: true, force: true });
  }
});

test("an unguarded read_to_end fails the gate", async () => {
  const root = await makeRepo(
    "src-tauri/src/example.rs",
    `fn read_unbounded(mut reader: impl std::io::Read) -> std::io::Result<Vec<u8>> {
    let mut output = Vec::new();
    reader.read_to_end(&mut output)?;
    Ok(output)
}
`,
  );
  try {
    const output = runGateExpectingFailure(root);
    assert.match(output, /example\.rs:3/);
    assert.match(output, /unbounded Read::read_to_end/);
  } finally {
    await rm(root, { recursive: true, force: true });
  }
});

// quick_xml::Reader::read_to_end(QName) is a same-named, different method —
// "skip to this closing tag", not a byte sink — and must not be flagged.
test("quick_xml's read_to_end(QName) is not a false positive", async () => {
  const root = await makeRepo(
    "src-tauri/src/example.rs",
    `fn skip_element(reader: &mut quick_xml::Reader<&[u8]>, name: &[u8]) -> quick_xml::Result<()> {
    reader.read_to_end(quick_xml::name::QName(name).to_owned())?;
    Ok(())
}
`,
  );
  try {
    const output = runGate(root);
    assert.match(output, /0 read_to_end\/read_to_string call site\(s\) scanned|0 unbounded/);
  } finally {
    await rm(root, { recursive: true, force: true });
  }
});

test("an unguarded read inside a #[test] function is excluded", async () => {
  const root = await makeRepo(
    "src-tauri/src/example.rs",
    `fn production_code() {}

#[cfg(test)]
mod tests {
    #[test]
    fn reads_a_small_fixture() {
        let mut reader = std::io::Cursor::new(b"fixture bytes".to_vec());
        let mut output = Vec::new();
        std::io::Read::read_to_end(&mut reader, &mut output).expect("read fixture");
    }
}
`,
  );
  try {
    assert.doesNotThrow(() => runGate(root));
  } finally {
    await rm(root, { recursive: true, force: true });
  }
});

test("an unguarded read in a #[cfg(test)] mod without a per-fn #[test] attribute is still excluded", async () => {
  const root = await makeRepo(
    "src-tauri/src/example.rs",
    `#[cfg(test)]
mod tests {
    fn helper_reads_fixture(mut reader: impl std::io::Read) -> Vec<u8> {
        let mut output = Vec::new();
        reader.read_to_end(&mut output).expect("read fixture");
        output
    }
}
`,
  );
  try {
    assert.doesNotThrow(() => runGate(root));
  } finally {
    await rm(root, { recursive: true, force: true });
  }
});

test("scope tracking exits the test module: code after it is still checked", async () => {
  const root = await makeRepo(
    "src-tauri/src/example.rs",
    `#[cfg(test)]
mod tests {
    #[test]
    fn reads_a_small_fixture() {
        let mut reader = std::io::Cursor::new(b"x".to_vec());
        let mut output = Vec::new();
        std::io::Read::read_to_end(&mut reader, &mut output).expect("read fixture");
    }
}

fn production_code_after_tests(mut reader: impl std::io::Read) -> std::io::Result<Vec<u8>> {
    let mut output = Vec::new();
    reader.read_to_end(&mut output)?;
    Ok(output)
}
`,
  );
  try {
    const output = runGateExpectingFailure(root);
    assert.match(output, /example\.rs:13/);
  } finally {
    await rm(root, { recursive: true, force: true });
  }
});

test("a path under a tests/ directory is excluded regardless of content", async () => {
  const root = await mkdtemp(join(tmpdir(), ".unbounded-reads-"));
  try {
    await mkdir(join(root, "src-tauri/tests"), { recursive: true });
    await writeFile(
      join(root, "src-tauri/tests/integration.rs"),
      `fn helper(mut reader: impl std::io::Read) -> std::io::Result<Vec<u8>> {
    let mut output = Vec::new();
    reader.read_to_end(&mut output)?;
    Ok(output)
}
`,
    );
    git(root, "init", "-q", ".");
    git(root, "config", "user.email", "test@example.invalid");
    git(root, "config", "user.name", "test");
    git(root, "add", "-A");
    git(root, "commit", "-qm", "seed");
    assert.doesNotThrow(() => runGate(root));
  } finally {
    await rm(root, { recursive: true, force: true });
  }
});

console.log("\nrunning check-unbounded-reads contract tests via node:test above");
