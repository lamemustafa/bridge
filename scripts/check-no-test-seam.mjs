// Proves the test-only native-approval seam (bridge#583) is absent from every
// shipped executable, and present where it must be, so that its absence means
// something.
//
// The seam lives in src-tauri/src/tally/approved_import.rs under bare
// `#[cfg(test)]` and carries SEAM_MARKER, which it uses at runtime so the
// optimiser keeps it. A binary holding the marker was compiled with the seam.
//
//   node scripts/check-no-test-seam.mjs <file-or-directory>...
//       Fails if any regular file at or under the paths holds the marker.
//   node scripts/check-no-test-seam.mjs --expect-present <file>
//       Fails unless the file holds the marker (a positive control).
//   node scripts/check-no-test-seam.mjs --tauri-bundle-hook
//       Tauri's beforeBundleCommand: scans the bridge and bridge_mcp
//       executables `tauri build` just produced.
//   node scripts/check-no-test-seam.mjs --test-harness [--release]
//       Builds (or reuses) the bridge lib unit-test executable and requires
//       the marker in it: proof the scan can see the seam when it is compiled
//       in, in that profile.
//
// Only uncompressed executables prove anything. A .dmg, .msi, .zip, .mcpb or
// installer compresses its contents, so a clean scan of one would pass
// whatever it holds; those are refused rather than scanned.
import { spawnSync } from "node:child_process";
import { existsSync, readdirSync, readFileSync, statSync } from "node:fs";
import { basename, dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

export const SEAM_MARKER = "bridge-test-approval-seam-5f1c9e7a";

const root = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const COMPRESSED = /\.(dmg|msi|zip|mcpb|gz|tgz|xz|bz2|7z|pkg|appimage|deb|rpm)$|-setup\.exe$/i;
const SHIPPED_BINARIES = ["bridge", "bridge_mcp"];

export function holdsMarker(path) {
  return readFileSync(path).includes(Buffer.from(SEAM_MARKER, "utf8"));
}

/** Every regular file at or under `path`. */
export function filesUnder(path) {
  const stat = statSync(path);
  if (stat.isFile()) return [path];
  if (!stat.isDirectory()) return [];
  return readdirSync(path).flatMap((entry) => filesUnder(join(path, entry)));
}

/** The files among `paths` that hold the marker. Refuses compressed archives. */
export function markedFiles(paths) {
  if (paths.length === 0) throw new Error("no files given to scan");
  const files = [];
  for (const path of paths) {
    if (COMPRESSED.test(path)) {
      throw new Error(`${path} is compressed: scan the executables it was built from instead`);
    }
    if (!existsSync(path)) throw new Error(`${path} does not exist`);
    files.push(...filesUnder(path));
  }
  if (files.length === 0) throw new Error(`no regular files under ${paths.join(", ")}`);
  return files.filter(holdsMarker);
}

/** Throws if any of `paths` holds the marker; for callers such as package-mcpb. */
export function assertNoTestSeam(paths) {
  const marked = markedFiles(paths);
  if (marked.length > 0) {
    throw new Error(
      `test-only approval seam compiled into: ${marked.join(", ")} (bridge#583); ` +
        "a shipped binary was built with cfg(test)",
    );
  }
}

/**
 * The executables a `tauri build` produced: bridge and bridge_mcp in the
 * profile directory the build used, for the host target and any explicit
 * `--target` directory. Tauri gives the hook TAURI_ENV_DEBUG but no target
 * triple, so every candidate directory is scanned; scanning a stale binary
 * too is harmless. Finding none is a failure: a hook that saw nothing proved
 * nothing.
 */
export function tauriBuildExecutables(environment = process.env, sourceRoot = root) {
  const profile = environment.TAURI_ENV_DEBUG === "true" ? "debug" : "release";
  const target = environment.CARGO_TARGET_DIR
    ? resolve(sourceRoot, environment.CARGO_TARGET_DIR)
    : resolve(sourceRoot, "src-tauri", "target");
  const directories = [join(target, profile)];
  if (existsSync(target)) {
    for (const entry of readdirSync(target)) {
      const candidate = join(target, entry, profile);
      if (entry !== profile && existsSync(candidate)) directories.push(candidate);
    }
  }
  const executables = directories.flatMap((directory) =>
    SHIPPED_BINARIES.flatMap((name) => [name, `${name}.exe`])
      .map((name) => join(directory, name))
      .filter((path) => existsSync(path) && statSync(path).isFile()),
  );
  if (executables.length === 0) {
    throw new Error(`no bridge or bridge_mcp executable under ${target} (${profile})`);
  }
  return executables;
}

/** The bridge lib unit-test executable Cargo builds for `profile`. */
export function testHarnessExecutable(release, sourceRoot = root) {
  const argumentsList = [
    "test", "--locked", "--no-run", "--lib", "--message-format=json",
    "--manifest-path", resolve(sourceRoot, "src-tauri", "Cargo.toml"), "-p", "bridge",
  ];
  if (release) argumentsList.push("--release");
  const build = spawnSync("cargo", argumentsList, {
    cwd: sourceRoot,
    encoding: "utf8",
    maxBuffer: 256 * 1024 * 1024,
    stdio: ["ignore", "pipe", "inherit"],
  });
  if (build.status !== 0) throw new Error(`cargo test --no-run failed (${build.status})`);
  const executables = build.stdout
    .split("\n")
    .filter((line) => line.startsWith("{"))
    .map((line) => JSON.parse(line))
    .filter(
      (message) =>
        message.reason === "compiler-artifact" &&
        message.target?.name === "bridge_lib" &&
        message.profile?.test === true &&
        message.executable,
    )
    .map((message) => message.executable);
  if (executables.length !== 1) {
    throw new Error(`expected one bridge lib test executable, found ${executables.length}`);
  }
  return executables[0];
}

function main(argumentsList) {
  if (argumentsList[0] === "--expect-present") {
    const [file] = argumentsList.slice(1);
    if (!file || !holdsMarker(file)) {
      throw new Error(`${file ?? "(no file)"} does not hold the seam marker: the scan cannot see the seam`);
    }
    console.log(`seam marker present in ${basename(file)}, as a positive control requires`);
    return;
  }
  if (argumentsList[0] === "--test-harness") {
    const release = argumentsList.includes("--release");
    const executable = testHarnessExecutable(release);
    main(["--expect-present", executable]);
    return;
  }
  const paths = argumentsList[0] === "--tauri-bundle-hook" ? tauriBuildExecutables() : argumentsList;
  assertNoTestSeam(paths);
  console.log(`no test-only approval seam in ${paths.length} path(s): ${paths.map((path) => basename(path)).join(", ")}`);
}

if (process.argv[1] === fileURLToPath(import.meta.url)) {
  try {
    main(process.argv.slice(2));
  } catch (error) {
    console.error(`check-no-test-seam: ${error.message}`);
    process.exit(1);
  }
}
