// SPDX-License-Identifier: Apache-2.0
import { pathToFileURL } from "node:url";

const managedKey = /^bridge-package-sccache-v1-(macOS|Windows)-(ARM64|X64)-[a-f0-9]{64}-[a-f0-9]{40}-\d+$/;
// Swatinem/rust-cache keys end in an environment hash and a lockfile hash. A new master key in the
// same job/OS family supersedes the older one: restores match the family prefix and take the newest.
const rustKey = /^v0-rust-([\w.-]+)-[a-f0-9]{8}-[a-f0-9]{8}$/;
const families = [
  { prefix: "bridge-package-sccache-v1-", key: managedKey, keep: 2 },
  { prefix: "v0-rust-", key: rustKey, keep: 1 },
];

export function obsoleteCaches(caches, { key = managedKey, keep = 2 } = {}) {
  if (!Array.isArray(caches)) throw new TypeError("Expected a cache inventory");
  const groups = new Map();
  const ids = new Set();
  for (const cache of caches) {
    const match = typeof cache?.key === "string" && cache.key.match(key);
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
    group.sort((a, b) => Date.parse(b.created_at) - Date.parse(a.created_at) || b.id - a.id).slice(keep));
}

// The cache list is not a snapshot: a cache saved or evicted while it is paged moves total_count or
// the pages, so one listing can come back incomplete. It is listed again, a bounded number of times,
// before refusing; nothing is ever deleted from an incomplete listing.
const INVENTORY_ATTEMPTS = 3;
const INVENTORY_RETRY_MS = 10_000;
const wait = (ms) => new Promise((resolve) => setTimeout(resolve, ms));

export async function pruneCaches({ env = process.env, fetcher = fetch, apply = false, sleep = wait } = {}) {
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
  async function inventory(prefix) {
    const caches = [];
    for (let page = 1; page <= 10; page += 1) {
      const query = new URLSearchParams({ key: prefix, ref: "refs/heads/master", per_page: "100", page: String(page) });
      const result = await request("GET", `${endpoint}?${query}`);
      if (!Number.isSafeInteger(result.total_count) || result.total_count < 0 || result.total_count > 1000 ||
          !Array.isArray(result.actions_caches)) throw new TypeError("Invalid or excessive cache inventory");
      caches.push(...result.actions_caches);
      // A page that shifted under a concurrent save or eviction can repeat a cache: incomplete too.
      if (new Set(caches.map((cache) => cache?.id)).size !== caches.length) break;
      if (caches.length === result.total_count) return caches;
      if (caches.length > result.total_count || result.actions_caches.length === 0) break;
    }
    throw Object.assign(new TypeError("Cache inventory is incomplete"), { code: "inventory_incomplete" });
  }
  async function consistentInventory(prefix) {
    for (let attempt = 1; ; attempt += 1) {
      try {
        return await inventory(prefix);
      } catch (error) {
        if (error?.code !== "inventory_incomplete" || attempt >= INVENTORY_ATTEMPTS) throw error;
        await sleep(INVENTORY_RETRY_MS);
      }
    }
  }
  // Every family is listed completely before anything is deleted.
  const plans = [];
  for (const family of families) plans.push(obsoleteCaches(await consistentInventory(family.prefix), family));
  const obsolete = plans.flat();
  if (apply) {
    for (const cache of obsolete) await request("DELETE", `${endpoint}/${cache.id}`);
    for (const family of families) {
      if (obsoleteCaches(await consistentInventory(family.prefix), family).length) throw Object.assign(new Error("Compiler-cache retention did not converge"), { code: "retention_incomplete" });
    }
  }
  return { applied: apply, obsoleteIds: obsolete.map((cache) => cache.id), retainedPerOS: 2, retainedPerRustFamily: 1 };
}

if (process.argv[1] && pathToFileURL(process.argv[1]).href === import.meta.url) {
  if (process.argv.slice(2).some((arg) => arg !== "--apply")) throw new TypeError("Unexpected argument");
  console.log(JSON.stringify(await pruneCaches({ apply: process.argv.includes("--apply") })));
}
