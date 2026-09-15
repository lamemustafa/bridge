// SPDX-License-Identifier: Apache-2.0
//
// Regenerates THIRD_PARTY_LICENSES.txt from the resolved production frontend
// dependency graph (pnpm-lock.yaml / package.json), the same source of truth
// `scripts/check-dependency-inventory.mjs --frontend` validates against.
//
// The license text of an "A OR B" package is chosen from PREFERRED_LICENSE_IDS,
// but the printed `License:` line always keeps pnpm's full SPDX expression, so
// the checker (which only greps for `name version` tokens) and this generator
// can never disagree on what is "in" the inventory -- only on which license
// text is quoted for a dual-licensed package.

import { readFile, readdir, writeFile } from "node:fs/promises";
import { spawnSync } from "node:child_process";
import { join } from "node:path";
import { fileURLToPath } from "node:url";

const root = fileURLToPath(new URL("../", import.meta.url));
const reportUrl = new URL("../THIRD_PARTY_LICENSES.txt", import.meta.url);

const HEADER = `Bridge frontend third-party licenses
====================================

The native Rust dependency inventory and full license texts are provided in
THIRD_PARTY_LICENSES_RUST.txt. This file covers the production frontend
dependency graph from pnpm-lock.yaml.
`;

// Preference order used only to pick which license TEXT to quote when a
// package publishes an "A OR B" expression (e.g. "Apache-2.0 OR MIT"). The
// reported `License:` field always keeps the full expression regardless.
const PREFERRED_LICENSE_IDS = [
  "MIT",
  "ISC",
  "0BSD",
  "BSD-2-Clause",
  "BSD-3-Clause",
  "Apache-2.0",
  "CC0-1.0",
  "Unlicense",
];

// Extensions that mark a license file as the package's single, generic
// license text rather than one specific alternative of an "A OR B" choice.
const GENERIC_SUFFIXES = new Set(["", "MD", "TXT", "RST"]);

const idKey = (value) => value.toUpperCase().replace(/[^A-Z0-9]/g, "");

const runJson = (command, args, label) => {
  const result = spawnSync(command, args, {
    cwd: root,
    encoding: "utf8",
    maxBuffer: 64 * 1024 * 1024,
    windowsHide: true,
  });
  if (result.error || result.status !== 0) {
    throw new Error(`${label} command failed`);
  }
  try {
    return JSON.parse(result.stdout);
  } catch {
    throw new Error(`${label} command returned invalid JSON`);
  }
};

// Same normalization the Rust generator applies: strip trailing
// per-line whitespace and outer blank lines, so a stray space or CRLF
// in an upstream LICENSE file can never cause spurious inventory drift.
export function normalizeLicenseText(text) {
  return text.replace(/\r\n/g, "\n").replace(/[ \t]+$/gm, "").trim();
}

// Reduces a package.json `repository` field (string, object, git+/git:///
// scp-style URL, or a bare "owner/repo") plus a `homepage` fallback to the
// plain https URL this file has always printed on its `Source:` line.
export function normalizeRepositoryUrl({ repository, homepage } = {}) {
  let raw = typeof repository === "string" ? repository : repository?.url;
  if (raw) {
    if (raw.startsWith("github:")) raw = `https://github.com/${raw.slice("github:".length)}`;
    raw = raw.replace(/^git\+/, "");
    const scp = raw.match(/^git@([^:]+):(.+)$/);
    if (scp) raw = `https://${scp[1]}/${scp[2]}`;
    raw = raw.replace(/^git:\/\//, "https://");
    raw = raw.replace(/^ssh:\/\/git@/, "https://");
    raw = raw.replace(/\.git$/, "");
    return raw;
  }
  if (homepage) {
    return homepage.replace(/#readme$/, "").replace(/\/+$/, "");
  }
  return null;
}

// Finds every LICENSE*/COPYING* file at the top of a package directory and
// classifies each as either the package's one generic license text, or the
// text for one specific alternative of an "A OR B" SPDX expression.
async function findLicenseFiles(packageDir) {
  let entries;
  try {
    entries = await readdir(packageDir, { withFileTypes: true });
  } catch (cause) {
    throw new Error(`could not read package directory ${packageDir}: ${cause.message}`);
  }
  const generic = [];
  const byId = new Map();
  for (const entry of entries) {
    if (!entry.isFile()) continue;
    const match = entry.name.match(/^(licen[cs]e|copying)([._-](.+))?$/i);
    if (!match) continue;
    const suffix = idKey(match[3] ?? "");
    if (GENERIC_SUFFIXES.has(suffix)) {
      generic.push(entry.name);
    } else {
      byId.set(suffix, entry.name);
    }
  }
  generic.sort();
  return { genericFile: generic[0] ?? null, byId };
}

// Resolves the (id printed on Source's companion License: line is always the
// full pnpm expression -- this only decides which file's TEXT gets quoted)
// license text for one resolved package.
async function resolveLicenseText(name, version, licenseExpression, packageDir) {
  const { genericFile, byId } = await findLicenseFiles(packageDir);
  const alternatives = licenseExpression.includes(" AND ")
    ? null
    : licenseExpression.replace(/^\(|\)$/g, "").split(" OR ").map((part) => part.trim());
  if (!alternatives) {
    throw new Error(
      `${name} ${version}: compound license expression "${licenseExpression}" (AND) is not ` +
        "handled automatically; resolve its license text manually and extend the generator.",
    );
  }

  let chosenFile = null;
  if (alternatives.length === 1) {
    chosenFile = byId.get(idKey(alternatives[0])) ?? genericFile;
  } else {
    for (const preferred of PREFERRED_LICENSE_IDS) {
      if (!alternatives.some((alt) => idKey(alt) === idKey(preferred))) continue;
      const file = byId.get(idKey(preferred));
      if (file) {
        chosenFile = file;
        break;
      }
    }
    if (!chosenFile && genericFile) {
      console.warn(
        `${name} ${version}: "${licenseExpression}" has no per-license file for any of ` +
          `${PREFERRED_LICENSE_IDS.join(", ")}; using its single ${genericFile} for all alternatives.`,
      );
      chosenFile = genericFile;
    }
  }

  if (!chosenFile) {
    throw new Error(
      `${name} ${version}: could not find a license file for "${licenseExpression}" in ` +
        `${packageDir}. Add a documented override or a matching LICENSE_<ID> file upstream.`,
    );
  }
  const text = await readFile(join(packageDir, chosenFile), "utf8");
  return normalizeLicenseText(text);
}

// Groups resolved packages that quote byte-identical license text (e.g.
// react/react-dom/scheduler) into the single inventory entry the existing
// file format expects, and orders entries/members alphabetically by name --
// the order the committed file has always used.
export function buildGroups(resolved) {
  const byText = new Map();
  for (const pkg of resolved) {
    const key = pkg.text;
    if (!byText.has(key)) byText.set(key, []);
    byText.get(key).push(pkg);
  }
  const groups = [...byText.values()].map((members) => {
    members.sort((a, b) => a.name.localeCompare(b.name, "en"));
    const licenses = new Set(members.map((m) => m.license));
    if (licenses.size > 1) {
      throw new Error(
        `packages sharing identical license text disagree on their SPDX expression: ` +
          `${members.map((m) => `${m.name}@${m.license}`).join(", ")}`,
      );
    }
    const sources = new Set(members.map((m) => m.source).filter(Boolean));
    if (sources.size > 1) {
      console.warn(
        `${members.map((m) => m.name).join(", ")} share license text but report different ` +
          `repository URLs (${[...sources].join(" vs ")}); using ${members[0].name}'s: ` +
          `${members[0].source}.`,
      );
    }
    if (!members[0].source) {
      throw new Error(`${members[0].name} ${members[0].version}: no repository or homepage to source from`);
    }
    return {
      members,
      license: members[0].license,
      source: members[0].source,
      text: members[0].text,
    };
  });
  groups.sort((a, b) => a.members[0].name.localeCompare(b.members[0].name, "en"));
  return groups;
}

export function renderInventory(groups) {
  const entries = groups.map((group) => {
    const header = group.members.map((m) => `${m.name} ${m.version}`).join(", ");
    const underline = "-".repeat(header.length);
    return `${header}\n${underline}\nLicense: ${group.license}\nSource: ${group.source}\n\n${group.text}`;
  });
  return `${HEADER}\n${entries.join("\n\n")}\n`;
}

async function resolvePackageJson(packageDir) {
  const raw = await readFile(join(packageDir, "package.json"), "utf8");
  return JSON.parse(raw);
}

async function collectResolvedPackages() {
  const packageManager = process.env.npm_execpath;
  if (!packageManager) {
    throw new Error("Run the frontend license generator through the pinned pnpm script");
  }
  const licenses = runJson(
    process.execPath,
    [packageManager, "licenses", "list", "--prod", "--json"],
    "frontend license inventory",
  );

  const resolved = [];
  for (const [licenseExpression, packages] of Object.entries(licenses)) {
    for (const dependency of packages) {
      for (let i = 0; i < dependency.versions.length; i += 1) {
        const version = dependency.versions[i];
        const packageDir = dependency.paths[i] ?? dependency.paths[0];
        const pkgJson = await resolvePackageJson(packageDir);
        const text = await resolveLicenseText(dependency.name, version, licenseExpression, packageDir);
        const source = normalizeRepositoryUrl(pkgJson);
        resolved.push({
          name: dependency.name,
          version,
          license: licenseExpression,
          source,
          text,
        });
      }
    }
  }
  return resolved;
}

async function main() {
  const resolved = await collectResolvedPackages();
  const groups = buildGroups(resolved);
  const inventory = renderInventory(groups);
  await writeFile(reportUrl, inventory, "utf8");
  console.log(
    `Regenerated THIRD_PARTY_LICENSES.txt for ${resolved.length} production frontend ` +
      `dependencies (${groups.length} license entries).`,
  );
}

if (import.meta.main) await main();
