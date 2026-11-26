import test from "node:test";
import assert from "node:assert/strict";
import { createServer } from "node:http";
import { spawn } from "node:child_process";
import { once } from "node:events";
import { fileURLToPath } from "node:url";

async function freePort() {
  const server = createServer();
  server.listen(0, "127.0.0.1");
  await once(server, "listening");
  const { port } = server.address();
  await new Promise((resolve) => server.close(resolve));
  return port;
}

test("gateway authenticates, hides secrets, enforces limits, and relays SSE", async (t) => {
  const calls = [];
  const upstream = createServer(async (request, response) => {
    const chunks = [];
    for await (const chunk of request) chunks.push(chunk);
    const body = chunks.length ? JSON.parse(Buffer.concat(chunks).toString("utf8")) : null;
    calls.push({ path: request.url, authorization: request.headers.authorization, body });
    if (request.url === "/api/tags") {
      response.writeHead(200, { "content-type": "application/json" }).end('{"models":[]}');
      return;
    }
    response.writeHead(200, { "content-type": "text/event-stream", "cache-control": "no-cache" });
    response.write('data: {"choices":[{"delta":{"content":"first "}}]}\n\n');
    await new Promise((resolve) => setTimeout(resolve, 180));
    response.write('data: {"choices":[{"delta":{"content":"chunk"}}]}\n\n');
    await new Promise((resolve) => setTimeout(resolve, 50));
    response.end("data: [DONE]\n\n");
  });
  upstream.listen(0, "127.0.0.1");
  await once(upstream, "listening");
  const upstreamPort = upstream.address().port;
  t.after(() => upstream.close());

  const supabase = createServer((request, response) => {
    if (request.url === "/auth/v1/user") {
      const id = request.headers.authorization === "Bearer other-user-token"
        ? "223e4567-e89b-42d3-a456-426614174001"
        : "123e4567-e89b-42d3-a456-426614174000";
      response.writeHead(200, { "content-type": "application/json" }).end(JSON.stringify({
        id, email: "test@example.test", email_confirmed_at: "2026-01-01T00:00:00Z",
        app_metadata: { provider: "google" },
        identities: [{ provider: "google", provider_id: id, identity_data: { sub: id } }],
      }));
      return;
    }
    if (request.url?.startsWith("/rest/v1/app_users?")) {
      response.writeHead(200, { "content-type": "application/json" }).end(JSON.stringify([
        { google_id: "google-test", email: "test@example.test" },
      ]));
      return;
    }
    response.writeHead(404).end();
  });
  supabase.listen(0, "127.0.0.1");
  await once(supabase, "listening");
  t.after(() => supabase.close());

  const port = await freePort();
  const registryToken = "integration-registry-token";
  const proxyKey = "integration-proxy-key";
  let output = "";
  const service = spawn(process.execPath, [
    "--import", new URL("../test-support/fetch-shim.mjs", import.meta.url).href,
    fileURLToPath(new URL("../src/server.mjs", import.meta.url)),
  ], {
    env: {
      ...process.env, NODE_ENV: "test", PORT: String(port),
      SUPABASE_URL: `http://127.0.0.1:${supabase.address().port}`,
      SUPABASE_ANON_KEY: "test-anon-key", SUPABASE_SERVICE_ROLE_KEY: "test-service-role-key",
      MACHA_REGISTRY_TOKEN: registryToken, MACHA_PROXY_KEY: proxyKey,
      MACHA_AI_MAX_CONCURRENT: "1", MACHA_AI_MAX_QUEUE: "1",
      MACHA_AI_REQUESTS_PER_HOUR: "4", MACHA_AI_REQUESTS_PER_DAY: "10",
      MACHA_AI_QUEUE_WAIT_MS: "2000", MACHA_AI_MAX_OUTPUT_TOKENS: "128",
      MACHA_AI_TEST_UPSTREAM_URL: `http://127.0.0.1:${upstreamPort}`,
    },
    stdio: ["ignore", "pipe", "pipe"],
  });
  service.stdout.on("data", (chunk) => { output += chunk.toString(); });
  service.stderr.on("data", (chunk) => { output += chunk.toString(); });
  t.after(() => service.kill());

  const base = `http://127.0.0.1:${port}`;
  let ready = false;
  for (let attempt = 0; attempt < 40; attempt++) {
    try { if ((await fetch(`${base}/health`)).ok) { ready = true; break; } } catch {}
    await new Promise((resolve) => setTimeout(resolve, 100));
  }
  assert.equal(ready, true, `service should start for the route test: ${output}`);

  const endpoint = "https://test-session.trycloudflare.com";
  const denied = await fetch(`${base}/internal/kaggle-endpoint`, {
    method: "POST", headers: { "content-type": "application/json" }, body: JSON.stringify({ url: endpoint }),
  });
  assert.equal(denied.status, 401);
  const registered = await fetch(`${base}/internal/kaggle-endpoint`, {
    method: "POST", headers: { "content-type": "application/json", "x-registry-token": registryToken },
    body: JSON.stringify({ url: endpoint }),
  });
  assert.equal(registered.status, 200);

  assert.equal((await fetch(`${base}/ai/status`)).status, 401);
  const auth = { authorization: "Bearer valid-test-access-token" };
  const status = await fetch(`${base}/ai/status`, { headers: auth });
  const statusBody = await status.json();
  assert.deepEqual(statusBody, { available: true, queueLength: 0 });
  assert.equal(JSON.stringify(statusBody).includes(endpoint), false);
  assert.equal((await fetch(`${base}/ai/chat`, { method: "POST", headers: { "content-type": "application/json" }, body: "{}" })).status, 401);

  const chat = (content) => fetch(`${base}/ai/chat`, {
    method: "POST",
    headers: { ...auth, authorization: "Bearer client-supplied-secret", "content-type": "application/json" },
    body: JSON.stringify({ model: "attacker-model", max_tokens: 99999, stream: true, messages: [{ role: "user", content }] }),
  });
  const firstPromise = chat("one");
  for (let attempt = 0; attempt < 50 && calls.length === 0; attempt++) {
    await new Promise((resolve) => setTimeout(resolve, 10));
  }
  assert.equal(calls.length, 1);
  const secondPromise = chat("two");
  await new Promise((resolve) => setTimeout(resolve, 30));
  const fullQueue = await chat("three");
  assert.equal(fullQueue.status, 429);
  assert.ok(Number(fullQueue.headers.get("retry-after")) >= 1);
  const first = await firstPromise;
  assert.equal(first.status, 200);
  const reader = first.body.getReader();
  const firstChunk = await reader.read();
  assert.equal(firstChunk.done, false);
  const rest = [Buffer.from(firstChunk.value)];
  while (true) {
    const item = await reader.read();
    if (item.done) break;
    rest.push(Buffer.from(item.value));
  }
  const second = await secondPromise;
  assert.equal(second.status, 200);
  await second.arrayBuffer();
  assert.equal(calls.filter((call) => call.path === "/v1/chat/completions").length, 2);
  const chatCalls = calls.filter((entry) => entry.path === "/v1/chat/completions");
  assert.deepEqual(chatCalls.map((call) => call.body.messages[0].content), ["one", "two"]);
  for (const call of chatCalls) {
    assert.equal(call.authorization, `Bearer ${proxyKey}`);
    assert.equal(call.body.model, "qwen-local");
    assert.equal(call.body.max_tokens, 128);
  }

  const withinLimit = await chat("four");
  assert.equal(withinLimit.status, 200);
  await withinLimit.arrayBuffer();
  const rateLimited = await chat("five");
  assert.equal(rateLimited.status, 429);
  assert.ok(Number(rateLimited.headers.get("retry-after")) >= 1);
  assert.equal(output.includes(proxyKey), false);
  assert.equal(output.includes(registryToken), false);
  assert.equal(output.includes("client-supplied-secret"), false);

  const deleted = await fetch(`${base}/internal/kaggle-endpoint`, {
    method: "DELETE", headers: { "x-registry-token": registryToken },
  });
  assert.equal(deleted.status, 204);
  assert.deepEqual(await (await fetch(`${base}/ai/status`, { headers: auth })).json(), { available: false, queueLength: 0 });
  const offline = await chat("offline");
  assert.equal(offline.status, 503);
  assert.deepEqual(await offline.json(), { error: "offline" });
});
