// SPDX-License-Identifier: Apache-2.0

import assert from "node:assert/strict";
import test from "node:test";

import {
  DEFAULT_TALLY_ENDPOINT_HINT,
  loadEndpointReconnectHint,
  saveEndpointReconnectHint,
} from "../src/tally-endpoint-reconnect-hint.ts";

function storage(initial = new Map()) {
  return {
    getItem(key) { return initial.get(key) ?? null; },
    setItem(key, value) { initial.set(key, value); },
  };
}

test("restores a host and valid port hint without applying endpoint admission", () => {
  const hintStorage = storage(new Map([["bridge.tally.endpoint-reconnect-hint.v1", JSON.stringify({ host: " 127.1.2.3 ", port: 9001 })]]));

  assert.deepEqual(loadEndpointReconnectHint(hintStorage), { host: "127.1.2.3", port: 9001 });
});

test("corrupt, incomplete, and unavailable storage falls back", () => {
  for (const value of ["{", JSON.stringify({ host: "   ", port: 9000 }), JSON.stringify({ host: "localhost", port: 0 })]) {
    assert.deepEqual(loadEndpointReconnectHint(storage(new Map([["bridge.tally.endpoint-reconnect-hint.v1", value]]))), DEFAULT_TALLY_ENDPOINT_HINT);
  }
  assert.deepEqual(loadEndpointReconnectHint({ getItem() { throw new Error("unavailable"); }, setItem() {} }), DEFAULT_TALLY_ENDPOINT_HINT);
});

test("does not persist an incomplete endpoint hint", () => {
  const values = new Map();
  saveEndpointReconnectHint({ host: "", port: 9000 }, storage(values));
  assert.equal(values.size, 0);
});

test("successful hint round-trip retains only form fields, and storage failures stay optional", () => {
  const values = new Map();
  const store = storage(values);
  saveEndpointReconnectHint({ host: "127.1.2.3", port: 9001, passport: { forged: true } }, store);
  assert.deepEqual(loadEndpointReconnectHint(store), { host: "127.1.2.3", port: 9001 });
  assert.deepEqual(JSON.parse([...values.values()][0]), { host: "127.1.2.3", port: 9001 });
  assert.doesNotThrow(() => saveEndpointReconnectHint({ host: "localhost", port: 9001 }, {
    getItem() { return null; }, setItem() { throw new Error("storage unavailable"); },
  }));
});
