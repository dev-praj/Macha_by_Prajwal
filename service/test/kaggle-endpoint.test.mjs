import test from "node:test";
import assert from "node:assert/strict";
import {
  createKaggleEndpointRegistry,
  registryTokenMatches,
  validateKaggleEndpoint,
} from "../src/kaggle-endpoint.mjs";

test("registry token comparison accepts only the shared token", () => {
  assert.equal(registryTokenMatches("secret-value", "secret-value"), true);
  assert.equal(registryTokenMatches("wrong-value", "secret-value"), false);
  assert.equal(registryTokenMatches("secret-value", ""), false);
});

test("registry accepts only an HTTPS trycloudflare origin", () => {
  assert.equal(validateKaggleEndpoint("https://abc123.trycloudflare.com"), true);
  for (const value of [
    "http://abc123.trycloudflare.com",
    "https://trycloudflare.com",
    "https://abc123.trycloudflare.com.attacker.example",
    "https://user@abc123.trycloudflare.com",
    "https://abc123.trycloudflare.com/path",
    "not a URL",
  ]) assert.equal(validateKaggleEndpoint(value), false, value);
});

test("registry stores the current endpoint and marks it stale after three minutes", () => {
  let clock = Date.parse("2026-01-01T00:00:00.000Z");
  const registry = createKaggleEndpointRegistry({ token: "registry-secret", now: () => clock });
  assert.deepEqual(registry.get(), { url: null, updatedAt: null, stale: true });
  assert.deepEqual(registry.register("wrong", "https://abc.trycloudflare.com"), {
    status: 401,
    body: { error: "unauthorized" },
  });
  const registered = registry.register("registry-secret", "https://abc.trycloudflare.com");
  assert.equal(registered.status, 200);
  assert.equal(registered.body.url, "https://abc.trycloudflare.com");
  assert.equal(registry.get().stale, false);
  clock += 3 * 60 * 1000 + 1;
  assert.equal(registry.get().stale, true);
});

test("registry clears the endpoint only with the shared token", () => {
  const registry = createKaggleEndpointRegistry({ token: "registry-secret" });
  registry.register("registry-secret", "https://abc.trycloudflare.com");
  assert.equal(registry.clear("wrong").status, 401);
  assert.equal(registry.get().url, "https://abc.trycloudflare.com");
  assert.equal(registry.clear("registry-secret").status, 204);
  assert.equal(registry.get().stale, true);
});
