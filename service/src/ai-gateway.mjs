import { createKaggleEndpointRegistry } from "./kaggle-endpoint.mjs";

const MINUTE = 60_000;
const HOUR = 60 * MINUTE;
const DAY = 24 * HOUR;

function limitFromEnv(env, name, fallback, minimum = 1) {
  const value = Number(env[name]);
  return Number.isSafeInteger(value) && value >= minimum ? value : fallback;
}

export function createMachaAiGateway({ env = process.env, now = Date.now } = {}) {
  const registry = createKaggleEndpointRegistry({ token: env.MACHA_REGISTRY_TOKEN, now });
  const settings = {
    proxyKey: env.MACHA_PROXY_KEY || "",
    maxConcurrentPerUser: limitFromEnv(env, "MACHA_AI_MAX_CONCURRENT_PER_USER", 1),
    requestsPerHour: limitFromEnv(env, "MACHA_AI_REQUESTS_PER_HOUR", 20),
    requestsPerDay: limitFromEnv(env, "MACHA_AI_REQUESTS_PER_DAY", 200),
    maxConcurrent: limitFromEnv(env, "MACHA_AI_MAX_CONCURRENT", 4),
    maxQueue: limitFromEnv(env, "MACHA_AI_MAX_QUEUE", 16, 0),
    queueWaitMs: limitFromEnv(env, "MACHA_AI_QUEUE_WAIT_MS", 60_000),
    maxOutputTokens: limitFromEnv(env, "MACHA_AI_MAX_OUTPUT_TOKENS", 4096),
    maxRequestBytes: limitFromEnv(env, "MACHA_AI_MAX_REQUEST_BYTES", 1_048_576),
  };
  let active = 0;
  let upstreamFailed = false;
  const activeUsers = new Map();
  const waiters = [];
  const requestTimes = new Map();

  function pruneRequests(userId) {
    const recent = (requestTimes.get(userId) || []).filter((at) => now() - at < DAY);
    if (recent.length) requestTimes.set(userId, recent);
    else requestTimes.delete(userId);
    return recent;
  }

  function rateLimit(userId) {
    const recent = pruneRequests(userId);
    const hour = recent.filter((at) => now() - at < HOUR);
    if (hour.length >= settings.requestsPerHour) {
      return Math.max(1, Math.ceil((hour[0] + HOUR - now()) / 1000));
    }
    if (recent.length >= settings.requestsPerDay) {
      return Math.max(1, Math.ceil((recent[0] + DAY - now()) / 1000));
    }
    recent.push(now());
    requestTimes.set(userId, recent);
    return 0;
  }

  function release(userId) {
    active = Math.max(0, active - 1);
    const count = Math.max(0, (activeUsers.get(userId) || 0) - 1);
    if (count) activeUsers.set(userId, count);
    else activeUsers.delete(userId);
    drain();
  }

  function grant(userId) {
    active += 1;
    activeUsers.set(userId, (activeUsers.get(userId) || 0) + 1);
    return () => release(userId);
  }

  function drain() {
    while (waiters.length && active < settings.maxConcurrent) {
      const waiter = waiters[0];
      if ((activeUsers.get(waiter.userId) || 0) >= settings.maxConcurrentPerUser) break;
      waiters.shift();
      clearTimeout(waiter.timer);
      waiter.signal?.removeEventListener("abort", waiter.abort);
      waiter.resolve(grant(waiter.userId));
    }
  }

  function acquire(userId, signal) {
    const retryAfter = rateLimit(userId);
    if (retryAfter) return Promise.reject({ status: 429, retryAfter });
    if (signal?.aborted) return Promise.reject({ status: 499, retryAfter: 1 });
    if (active < settings.maxConcurrent && (activeUsers.get(userId) || 0) < settings.maxConcurrentPerUser && waiters.length === 0) {
      return Promise.resolve(grant(userId));
    }
    if (waiters.length >= settings.maxQueue) return Promise.reject({ status: 429, retryAfter: 1 });
    return new Promise((resolve, reject) => {
      const waiter = { userId, resolve, reject, signal, timer: null, abort: null };
      const remove = () => {
        const index = waiters.indexOf(waiter);
        if (index < 0) return false;
        waiters.splice(index, 1);
        clearTimeout(waiter.timer);
        signal?.removeEventListener("abort", waiter.abort);
        return true;
      };
      waiter.abort = () => {
        if (remove()) reject({ status: 499, retryAfter: 1 });
        drain();
      };
      waiter.timer = setTimeout(() => {
        if (remove()) reject({ status: 429, retryAfter: 1 });
        drain();
      }, settings.queueWaitMs);
      signal?.addEventListener("abort", waiter.abort, { once: true });
      waiters.push(waiter);
      drain();
    });
  }

  function currentEndpoint() {
    const endpoint = registry.get();
    return endpoint.url && !endpoint.stale && !upstreamFailed ? endpoint : null;
  }

  function setUpstreamFailed(value) { upstreamFailed = value; }

  return {
    registry,
    settings,
    acquire,
    currentEndpoint,
    setUpstreamFailed,
    get queueLength() { return waiters.length; },
    get activeCount() { return active; },
  };
}

export function validateAiChatBody(body, maxOutputTokens, maxRequestBytes) {
  if (!body || typeof body !== "object" || Array.isArray(body)) return null;
  try {
    if (Buffer.byteLength(JSON.stringify(body), "utf8") > maxRequestBytes) return null;
  } catch { return null; }
  if (!Array.isArray(body.messages) || body.messages.length === 0 || body.messages.length > 100) return null;
  if (!body.messages.every((message) => message && typeof message === "object"
    && ["system", "user", "assistant", "tool"].includes(message.role)
    && (typeof message.content === "string" || Array.isArray(message.content) || message.content === null))) return null;
  if (body.tools !== undefined && (!Array.isArray(body.tools) || body.tools.length > 100)) return null;
  const requested = Number(body.max_tokens ?? body.max_completion_tokens ?? maxOutputTokens);
  const maxTokens = Number.isSafeInteger(requested) && requested > 0 ? Math.min(requested, maxOutputTokens) : maxOutputTokens;
  return { ...body, model: "qwen-local", max_tokens: maxTokens };
}
