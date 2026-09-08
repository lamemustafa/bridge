// SPDX-License-Identifier: Apache-2.0

export type TallyEndpointHint = {
  host: string;
  port: number;
};

export const DEFAULT_TALLY_ENDPOINT_HINT: TallyEndpointHint = {
  host: "localhost",
  port: 9000,
};

const STORAGE_KEY = "bridge.tally.endpoint-reconnect-hint.v1";

type EndpointHintStorage = Pick<Storage, "getItem" | "setItem">;

function browserStorage(): EndpointHintStorage | null {
  try {
    return typeof window === "undefined" ? null : window.localStorage;
  } catch {
    return null;
  }
}

function validHint(value: unknown): TallyEndpointHint | null {
  if (!value || typeof value !== "object") return null;
  const { host, port } = value as Record<string, unknown>;
  if (typeof host !== "string" || typeof port !== "number" || !Number.isInteger(port)) return null;
  const normalizedHost = host.trim();
  if (!normalizedHost || port < 1 || port > 65_535) return null;
  return { host: normalizedHost, port };
}

/**
 * Restores a reconnect convenience only. It deliberately has no company,
 * capability, review, or canonical-origin fields; a fresh Check Tally owns
 * all current-source authority after launch.
 */
export function loadEndpointReconnectHint(storage = browserStorage()): TallyEndpointHint {
  try {
    const saved = storage?.getItem(STORAGE_KEY);
    if (!saved) return DEFAULT_TALLY_ENDPOINT_HINT;
    return validHint(JSON.parse(saved)) ?? DEFAULT_TALLY_ENDPOINT_HINT;
  } catch {
    return DEFAULT_TALLY_ENDPOINT_HINT;
  }
}

/** Persists only an input hint after a successful probe. Rust admits the endpoint. */
export function saveEndpointReconnectHint(hint: TallyEndpointHint, storage = browserStorage()): void {
  const validated = validHint(hint);
  if (!validated) return;
  try {
    storage?.setItem(STORAGE_KEY, JSON.stringify(validated));
  } catch {
    // A reconnect hint is optional and must never block the checked session.
  }
}
