// SPDX-License-Identifier: Apache-2.0
import { pathToFileURL } from "node:url";

const prefix = "bridge-package-sccache-v1-";
const managedKey = /^bridge-package-sccache-v1-(macOS|Windows)-(ARM64|X64)-[a-f0-9]{64}-[a-f0-9]{40}-\d+$/;

export function obsoleteCaches(caches) {
  if (!Array.isArray(caches)) throw new TypeError("Expected a cache inventory");
  const groups = new Map();
  const ids = new Set();
  for (const cache of caches) {
    const match = typeof cache?.key === "string" && cache.key.match(managedKey);
    if (cache?.ref !== "refs/heads/master" || !match) continue;
    if (!Number.isSafeInteger(cache.id) || cache.id <= 0 || ids.has(cache.id) ||
        !Number.isSafeInteger(cache.size_in_bytes) || cache.size_in_bytes <= 0 ||
        typeof cache.created_at !== "string" || !Number.isFinite(Date.parse(cache.created_at))) {
      throw new TypeError("Invalid managed cache metadata");
    }
    ids.add(cache.id);
    const group = groups.get(match[1]) ?? [];
    group.push(cache);
    groups.set(match[1], group);
  }
  return [...groups.values()].flatMap((group) =>
    group.sort((a, b) => Date.parse(b.created_at) - Date.parse(a.created_at) || b.id - a.id).slice(2));
}

export async function pruneCaches({ env = process.env, fetcher = fetch, apply = false } = {}) {
  if (env.GITHUB_REF !== "refs/heads/master" ||
      !["push", "workflow_dispatch"].includes(env.GITHUB_EVENT_NAME)) {
    throw new TypeError("Cache retention requires a master push or manual run");
  }
  if (!/^[\w.-]+\/[\w.-]+$/.test(env.GITHUB_REPOSITORY ?? "") || !env.GH_TOKEN) {
    throw new TypeError("Missing repository or cache-management credential");
  }
  const endpoint = `https://api.github.com/repos/${env.GITHUB_REPOSITORY}/actions/caches`;
  async function request(method, url) {
    const response = await fetcher(url, {
      method, redirect: "error", signal: AbortSignal.timeout(30_000),
      headers: { Authorization: `Bearer ${env.GH_TOKEN}`, Accept: "application/vnd.github+json" },
    });
    // Cache eviction can remove a previously listed entry before this DELETE.
    if (method === "DELETE" && response.status === 404) return;
    if (!response.ok) throw Object.assign(new Error("GitHub cache API request failed"), { status: response.status });
    return method === "GET" ? response.json() : undefined;
  }
  async function inventory() {
    const caches = [];
    for (let page = 1; page <= 10; page += 1) {
      const query = new URLSearchParams({ key: prefix, ref: "refs/heads/master", per_page: "100", page: String(page) });
      const result = await request("GET", `${endpoint}?${query}`);
      if (!Number.isSafeInteger(result.total_count) || result.total_count < 0 || result.total_count > 1000 ||
          !Array.isArray(result.actions_caches)) throw new TypeError("Invalid or excessive cache inventory");
      caches.push(...result.actions_caches);
      if (caches.length === result.total_count) return caches;
      if (caches.length > result.total_count || result.actions_caches.length === 0) break;
    }
    throw new TypeError("Cache inventory is incomplete");
  }
  const obsolete = obsoleteCaches(await inventory());
  if (apply) {
    for (const cache of obsolete) await request("DELETE", `${endpoint}/${cache.id}`);
    if (obsoleteCaches(await inventory()).length) throw Object.assign(new Error("Compiler-cache retention did not converge"), { code: "retention_incomplete" });
  }
  return { applied: apply, obsoleteIds: obsolete.map((cache) => cache.id), retainedPerOS: 2 };
}

if (process.argv[1] && pathToFileURL(process.argv[1]).href === import.meta.url) {
  if (process.argv.slice(2).some((arg) => arg !== "--apply")) throw new TypeError("Unexpected argument");
  console.log(JSON.stringify(await pruneCaches({ apply: process.argv.includes("--apply") })));
}
