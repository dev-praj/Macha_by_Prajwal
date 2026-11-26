import { createHash, timingSafeEqual } from "node:crypto";

const STALE_AFTER_MS = 3 * 60 * 1000;

function digest(value) {
  return createHash("sha256").update(String(value ?? ""), "utf8").digest();
}

export function registryTokenMatches(supplied, expected) {
  if (typeof expected !== "string" || expected.length === 0) return false;
  return timingSafeEqual(digest(supplied), digest(expected));
}

export function validateKaggleEndpoint(value) {
  if (typeof value !== "string" || value.length > 512) return false;
  try {
    const url = new URL(value);
    return url.protocol === "https:"
      && /^([a-z0-9-]+\.)+trycloudflare\.com$/i.test(url.hostname)
      && !url.username
      && !url.password
      && !url.port
      && url.pathname === "/"
      && !url.search
      && !url.hash;
  } catch {
    return false;
  }
}

export function createKaggleEndpointRegistry({ token, now = Date.now } = {}) {
  let current = null;
  return {
    get configured() {
      return typeof token === "string" && token.length > 0;
    },
    register(suppliedToken, url) {
      if (!this.configured) return { status: 503, body: { error: "registry_not_configured" } };
      if (!registryTokenMatches(suppliedToken, token)) return { status: 401, body: { error: "unauthorized" } };
      if (!validateKaggleEndpoint(url)) return { status: 400, body: { error: "invalid_endpoint" } };
      current = { url, updatedAt: new Date(now()).toISOString() };
      return { status: 200, body: { ...current, stale: false } };
    },
    clear(suppliedToken) {
      if (!this.configured) return { status: 503, body: { error: "registry_not_configured" } };
      if (!registryTokenMatches(suppliedToken, token)) return { status: 401, body: { error: "unauthorized" } };
      current = null;
      return { status: 204, body: null };
    },
    get() {
      if (!current) return { url: null, updatedAt: null, stale: true };
      return {
        ...current,
        stale: now() - Date.parse(current.updatedAt) > STALE_AFTER_MS,
      };
    },
  };
}
