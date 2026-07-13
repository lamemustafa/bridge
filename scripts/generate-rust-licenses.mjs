// SPDX-License-Identifier: Apache-2.0

import { spawnSync } from "node:child_process";
import { readFile, writeFile } from "node:fs/promises";
import { fileURLToPath } from "node:url";

const cargoAboutVersion = "0.9.1";
const tauriDir = fileURLToPath(new URL("../src-tauri/", import.meta.url));
const cargo = process.platform === "win32" ? "cargo.exe" : "cargo";

const run = (args, label, capture = false) => {
  const result = spawnSync(cargo, args, {
    cwd: tauriDir,
    encoding: "utf8",
    stdio: capture ? "pipe" : "inherit",
    windowsHide: true,
  });
  if (result.error || result.status !== 0) {
    throw new Error(`${label} failed`);
  }
  return result.stdout?.trim() ?? "";
};

const installed = run(["about", "--version"], "cargo-about version check", true);
if (!installed.includes(cargoAboutVersion)) {
  throw new Error(
    `cargo-about ${cargoAboutVersion} is required; install it with ` +
      `cargo install --locked cargo-about --version ${cargoAboutVersion} --features cli`,
  );
}

run(
  [
    "about",
    "generate",
    "--locked",
    "--config",
    "about.toml",
    "--output-file",
    "../THIRD_PARTY_LICENSES_RUST.txt",
    "about.hbs",
  ],
  "Rust license generation",
);

const reportUrl = new URL("../THIRD_PARTY_LICENSES_RUST.txt", import.meta.url);
const report = await readFile(reportUrl, "utf8");
const normalized = `${report.replace(/[ \t]+$/gm, "").trimEnd()}\n`;
await writeFile(reportUrl, normalized, "utf8");

console.log("Regenerated THIRD_PARTY_LICENSES_RUST.txt for Windows and macOS targets.");
