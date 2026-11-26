import test from "node:test";
import assert from "node:assert/strict";
import { createMachaAiGateway, validateAiChatBody } from "../src/ai-gateway.mjs";

test("chat validation fixes the model and caps output tokens", () => {
  const valid = validateAiChatBody({
    model: "untrusted-model", max_tokens: 10_000,
    messages: [{ role: "user", content: "hello" }],
  }, 512, 10_000);
  assert.equal(valid.model, "qwen-local");
  assert.equal(valid.max_tokens, 512);
  assert.equal(validateAiChatBody({ messages: [] }, 512, 10_000), null);
  assert.equal(validateAiChatBody({ messages: [{ role: "system", content: "x".repeat(100) }] }, 512, 50), null);
});

test("gateway admission is FIFO, bounded, and releases the global slot", async () => {
  const gateway = createMachaAiGateway({
    env: {
      MACHA_REGISTRY_TOKEN: "registry", MACHA_PROXY_KEY: "proxy",
      MACHA_AI_MAX_CONCURRENT: "1", MACHA_AI_MAX_QUEUE: "1",
      MACHA_AI_REQUESTS_PER_HOUR: "10", MACHA_AI_REQUESTS_PER_DAY: "20",
      MACHA_AI_QUEUE_WAIT_MS: "500",
    },
  });
  const firstRelease = await gateway.acquire("user-a");
  let secondStarted = false;
  const secondPromise = gateway.acquire("user-b").then((release) => {
    secondStarted = true;
    return release;
  });
  assert.equal(gateway.queueLength, 1);
  await assert.rejects(gateway.acquire("user-c"), (error) => error.status === 429);
  assert.equal(secondStarted, false);
  firstRelease();
  const secondRelease = await secondPromise;
  assert.equal(gateway.activeCount, 1);
  secondRelease();
  assert.equal(gateway.activeCount, 0);
});
