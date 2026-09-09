// SPDX-License-Identifier: Apache-2.0
import assert from "node:assert/strict";
import test from "node:test";
import { obsoleteCaches, pruneCaches } from "./prune-package-compiler-cache.mjs";

const env = {
  GITHUB_REF: "refs/heads/master", GITHUB_EVENT_NAME: "push",
  GITHUB_REPOSITORY: "example/bridge", GH_TOKEN: "synthetic-test-token",
};
const cache = (id, os = "macOS", extra = {}) => ({
  id, key: `bridge-package-sccache-v1-${os}-ARM64-${"a".repeat(64)}-${"b".repeat(40)}-${id}`,
  ref: "refs/heads/master", created_at: new Date(Date.UTC(2026, 0, id)).toISOString(), size_in_bytes: 100, ...extra,
});
const response = (status, body) => ({ status, ok: status >= 200 && status < 300, json: async () => body });

test("keeps two newest snapshots per OS across source and toolchain generations", () => {
  const rows = [cache(1), cache(4), cache(2), cache(3), cache(5, "Windows"), cache(6, "Windows")];
  rows[1].key = rows[1].key.replace("a".repeat(64), "c".repeat(64));
  assert.deepEqual(obsoleteCaches(rows).map((row) => row.id), [2, 1]);
});

test("never selects dependency caches, PR refs, foreign namespaces or malformed keys", () => {
  const protectedRows = [
    cache(1, "macOS", { key: "v0-rust-bundle-smoke-Darwin-arm64" }),
    cache(2, "macOS", { ref: "refs/pull/7/merge" }),
    cache(3, "macOS", { key: cache(3).key.replace("-v1-", "-v2-") }),
    cache(4, "macOS", { key: `${cache(4).key}-extra` }),
  ];
  assert.deepEqual(obsoleteCaches([...protectedRows, cache(5), cache(6)]), []);
});

test("invalid or duplicate managed metadata rejects the entire plan", () => {
  for (const invalid of [cache(3, "macOS", { id: -1 }), cache(3, "macOS", { created_at: "unknown" }),
    cache(3, "macOS", { size_in_bytes: 0 }), cache(2)]) {
    assert.throws(() => obsoleteCaches([cache(1), cache(2), invalid]), TypeError);
  }
});

test("PR and untrusted event contexts refuse before accessing the API", async () => {
  const fetcher = async () => assert.fail("API must not be accessed");
  await assert.rejects(pruneCaches({ env: { ...env, GITHUB_REF: "refs/pull/1/merge" }, fetcher, apply: true }), TypeError);
  await assert.rejects(pruneCaches({ env: { ...env, GITHUB_EVENT_NAME: "pull_request_target" }, fetcher, apply: true }), TypeError);
});

test("dry run reports exact IDs without deleting", async () => {
  const fetcher = async (_url, options) => {
    assert.equal(options.method, "GET");
    return response(200, { total_count: 3, actions_caches: [cache(1), cache(2), cache(3)] });
  };
  assert.deepEqual(await pruneCaches({ env, fetcher }), { applied: false, obsoleteIds: [1], retainedPerOS: 2 });
});

test("apply deletes only the planned ID and verifies the remaining inventory", async () => {
  let rows = [cache(1), cache(2), cache(3)];
  const deleted = [];
  const fetcher = async (url, options) => {
    if (options.method === "GET") return response(200, { total_count: rows.length, actions_caches: rows });
    assert.equal(url, "https://api.github.com/repos/example/bridge/actions/caches/1");
    deleted.push(1); rows = rows.filter((row) => row.id !== 1);
    return response(404); // Concurrent eviction is harmless only if the postcheck confirms it.
  };
  await pruneCaches({ env, fetcher, apply: true });
  assert.deepEqual(deleted, [1]);
});

test("API failures and unsuccessful deletion cannot count as successful retention", async () => {
  await assert.rejects(pruneCaches({ env, fetcher: async () => response(403) }), { status: 403 });
  const fetcher = async (_url, options) => options.method === "GET"
    ? response(200, { total_count: 3, actions_caches: [cache(1), cache(2), cache(3)] }) : response(404);
  await assert.rejects(pruneCaches({ env, fetcher, apply: true }), { code: "retention_incomplete" });
});


test("pagination is complete before selecting any deletion", async () => {
  const rows = Array.from({ length: 101 }, (_, index) => cache(index + 1));
  const pages = [];
  const fetcher = async (url, options) => {
    assert.equal(options.method, "GET");
    const query = new URL(url).searchParams;
    assert.equal(query.get("ref"), "refs/heads/master");
    assert.equal(query.get("key"), "bridge-package-sccache-v1-");
    const page = Number(query.get("page")); pages.push(page);
    return response(200, { total_count: rows.length, actions_caches: rows.slice((page - 1) * 100, page * 100) });
  };
  const result = await pruneCaches({ env, fetcher });
  assert.deepEqual(pages, [1, 2]);
  assert.equal(result.obsoleteIds.length, 99);
  assert.deepEqual(result.obsoleteIds.slice(0, 2), [99, 98]);
});
