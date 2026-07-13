// SPDX-License-Identifier: Apache-2.0

import { readFile } from "node:fs/promises";

const expectedLicense = "Apache-2.0";
const read = (path) => readFile(new URL(`../${path}`, import.meta.url), "utf8");

const [packageText, cargo, tauriText, license, notice, readme, frontendNotices, rustNotices] =
  await Promise.all([
    read("package.json"),
    read("src-tauri/Cargo.toml"),
    read("src-tauri/tauri.conf.json"),
    read("LICENSE"),
    read("NOTICE"),
    read("README.md"),
    read("THIRD_PARTY_LICENSES.txt"),
    read("THIRD_PARTY_LICENSES_RUST.txt"),
  ]);

const packageJson = JSON.parse(packageText);
const tauri = JSON.parse(tauriText);
const failures = [];

if (packageJson.license !== expectedLicense) failures.push("package.json license");
if (!/^license = "Apache-2\.0"$/m.test(cargo)) failures.push("Cargo.toml license");
if (tauri.bundle?.license !== expectedLicense) failures.push("Tauri bundle license");
if (tauri.bundle?.licenseFile !== "../LICENSE") failures.push("Tauri licenseFile");

const resources = tauri.bundle?.resources ?? {};
for (const [source, destination] of Object.entries({
  "../LICENSE": "LICENSE",
  "../NOTICE": "NOTICE",
  "../THIRD_PARTY_LICENSES.txt": "THIRD_PARTY_LICENSES.txt",
  "../THIRD_PARTY_LICENSES_RUST.txt": "THIRD_PARTY_LICENSES_RUST.txt",
})) {
  if (resources[source] !== destination) failures.push(`Tauri resource ${source}`);
}

if (!license.includes("Apache License") || !license.includes("Grant of Patent License")) {
  failures.push("complete Apache-2.0 LICENSE");
}
if (!notice.includes("Rust PKCS#11 Library") || !notice.includes("OASIS IPR Policy")) {
  failures.push("required PKCS#11 NOTICE attribution");
}
if (!readme.includes("Apache License, Version 2.0")) failures.push("README license declaration");
if (!frontendNotices.includes("lucide-react") || !frontendNotices.includes("react 19.2.7")) {
  failures.push("frontend third-party notices");
}
if (!rustNotices.includes("pkcs11 0.5.0") || !rustNotices.includes("Mozilla Public License 2.0")) {
  failures.push("native third-party report");
}

const portableFiles = [license, notice, readme, frontendNotices, rustNotices];
const localPath = /(?:[A-Za-z]:\\(?:Users|dev)\\|\/Users\/[^/]+\/)/i;
if (portableFiles.some((content) => localPath.test(content))) failures.push("local absolute path");

if (failures.length) {
  throw new Error(`License validation failed: ${failures.join(", ")}`);
}

console.log("Apache-2.0 metadata, notices, and bundle resources are consistent.");
