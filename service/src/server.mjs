import { createServer } from "node:http";
import { createCopilotAccounts } from "./copilot-accounts.mjs";
import { copilotCompletion } from "./copilot-sdk.mjs";

const port = Number(process.env.PORT || 8788);
const supabaseUrl = (process.env.SUPABASE_URL || "").replace(/\/$/, "");
const anonKey = process.env.SUPABASE_ANON_KEY || "";
const serviceRoleKey = process.env.SUPABASE_SERVICE_ROLE_KEY || "";
const maxBodyBytes = 4096;
const copilotAccounts = createCopilotAccounts();

if (!supabaseUrl || !anonKey || !serviceRoleKey) {
  throw new Error("SUPABASE_URL, SUPABASE_ANON_KEY, and SUPABASE_SERVICE_ROLE_KEY are required.");
}
const parsedSupabaseUrl = new URL(supabaseUrl);
if (parsedSupabaseUrl.protocol !== "https:" && parsedSupabaseUrl.hostname !== "localhost" && parsedSupabaseUrl.hostname !== "127.0.0.1") {
  throw new Error("SUPABASE_URL must use HTTPS outside local development.");
}

function send(response, status, value) {
  if (response.destroyed || response.writableEnded) return;
  const body = JSON.stringify(value);
  response.writeHead(status, {
    "content-type": "application/json; charset=utf-8",
    "cache-control": "no-store",
    "content-length": Buffer.byteLength(body),
  });
  response.end(body);
}

async function authenticatedUser(request) {
  const authorization = request.headers.authorization || "";
  const match = /^Bearer ([^\s]+)$/.exec(authorization);
  if (!match) return null;

  const response = await fetch(`${supabaseUrl}/auth/v1/user`, {
    headers: { apikey: anonKey, authorization: `Bearer ${match[1]}` },
    signal: AbortSignal.timeout(10000),
  });
  if (response.status === 401 || response.status === 403) return null;
  if (!response.ok) throw new Error("Supabase identity verification failed");
  const user = await response.json();
  const google = Array.isArray(user.identities)
    ? user.identities.find((identity) => identity?.provider === "google")
    : null;
  const googleId = google?.identity_data?.sub || google?.provider_id;
  if (
    user.app_metadata?.provider !== "google" ||
    typeof user.id !== "string" ||
    typeof user.email !== "string" ||
    !user.email_confirmed_at ||
    typeof googleId !== "string" ||
    !googleId
  ) return null;
  return { supabaseUserId: user.id, googleId, email: user.email };
}

async function upsertUser(user) {
  const response = await fetch(
    `${supabaseUrl}/rest/v1/app_users?on_conflict=google_id&select=google_id,email`,
    {
      method: "POST",
      headers: {
        apikey: serviceRoleKey,
        authorization: `Bearer ${serviceRoleKey}`,
        "content-type": "application/json",
        prefer: "resolution=merge-duplicates,return=representation",
      },
      body: JSON.stringify({
        google_id: user.googleId,
        supabase_user_id: user.supabaseUserId,
        email: user.email,
        updated_at: new Date().toISOString(),
      }),
      signal: AbortSignal.timeout(10000),
    },
  );
  if (!response.ok) throw new Error("User upsert failed");
  const rows = await response.json();
  if (!Array.isArray(rows) || rows.length !== 1) throw new Error("User upsert returned no row");
  return { googleId: rows[0].google_id, email: rows[0].email };
}

async function findUser(user) {
  const params = new URLSearchParams({
    select: "google_id,email",
    supabase_user_id: `eq.${user.supabaseUserId}`,
    limit: "1",
  });
  const response = await fetch(`${supabaseUrl}/rest/v1/app_users?${params}`, {
    headers: {
      apikey: serviceRoleKey,
      authorization: `Bearer ${serviceRoleKey}`,
    },
    signal: AbortSignal.timeout(10000),
  });
  if (!response.ok) throw new Error("User lookup failed");
  const rows = await response.json();
  if (!Array.isArray(rows) || rows.length !== 1) return null;
  return { googleId: rows[0].google_id, email: rows[0].email };
}

async function readJson(request, byteLimit = maxBodyBytes) {
  let size = 0;
  const chunks = [];
  for await (const chunk of request) {
    size += chunk.length;
    if (size > byteLimit) throw new Error("request_too_large");
    chunks.push(chunk);
  }
  if (size === 0) return {};
  return JSON.parse(Buffer.concat(chunks).toString("utf8"));
}

const server = createServer(async (request, response) => {
  const url = new URL(request.url || "/", "http://localhost");
  if (request.method === "GET" && url.pathname === "/health") {
    return send(response, 200, { status: "ok" });
  }
  if (request.method === "GET" && url.pathname === "/v1/auth/me") {
    try {
      const user = await authenticatedUser(request);
      if (!user) return send(response, 401, { error: "unauthorized" });
      const account = await findUser(user);
      if (!account) return send(response, 401, { error: "unauthorized" });
      return send(response, 200, account);
    } catch {
      return send(response, 503, { error: "account_service_unavailable" });
    }
  }
  if (url.pathname.startsWith("/v1/copilot/")) {
    try {
      const user = await authenticatedUser(request);
      if (!user) return send(response, 401, { error: "unauthorized" });
      const account = await findUser(user);
      if (!account) return send(response, 401, { error: "unauthorized" });
      if (request.method === "POST" && url.pathname === "/v1/copilot/device/start") {
        return send(response, 200, await copilotAccounts.start(user));
      }
      if (request.method === "POST" && url.pathname === "/v1/copilot/device/poll") {
        const body = await readJson(request);
        if (typeof body.flowId !== "string" || body.flowId.length > 80) return send(response, 400, { error: "invalid_request" });
        return send(response, 200, await copilotAccounts.poll(user, body.flowId));
      }
      if (request.method === "POST" && ["/v1/copilot/provider/chat", "/v1/copilot/provider/stream"].includes(url.pathname)) {
        const body = await readJson(request, 1024 * 1024);
        if (typeof body.accountId !== "string" || !/^[0-9a-f-]{36}$/i.test(body.accountId)
          || typeof body.model !== "string" || body.model.length > 120
          || !Array.isArray(body.messages) || body.messages.length > 100
          || (body.tools !== undefined && (!Array.isArray(body.tools) || body.tools.length > 100))) {
          return send(response, 400, { error: "invalid_request" });
        }
        const credential = await copilotAccounts.providerCredential(user, body.accountId);
        if (url.pathname.endsWith("/stream")) {
          const controller = new AbortController();
          response.on("close", () => controller.abort());
          response.writeHead(200, {
            "content-type": "text/event-stream; charset=utf-8",
            "cache-control": "no-store, no-transform",
            connection: "keep-alive",
            "x-accel-buffering": "no",
          });
          const emit = (event) => {
            if (!response.destroyed && !response.writableEnded) response.write(`data: ${JSON.stringify(event)}\n\n`);
          };
          try {
            const result = await copilotCompletion(credential.accessToken, {
              model: body.model,
              messages: body.messages,
              tools: body.tools,
              signal: controller.signal,
              onDelta: (delta) => emit({ type: "delta", delta }),
            });
            emit({ type: "result", response: result });
            response.end();
          } catch (error) {
            if (error?.code === "copilot_needs_reconnect") await copilotAccounts.markProviderStatus(user, body.accountId, "needs_reconnect");
            if (error?.code === "copilot_subscription_inactive") await copilotAccounts.markProviderStatus(user, body.accountId, "inactive");
            const safeCode = ["copilot_needs_reconnect", "copilot_subscription_inactive", "copilot_rate_limited", "copilot_request_timeout"].includes(error?.code)
              ? error.code
              : "copilot_request_failed";
            emit({ type: "error", error: safeCode });
            response.end();
          }
          return;
        }
        const controller = new AbortController();
        response.on("close", () => controller.abort());
        let result;
        try {
          result = await copilotCompletion(credential.accessToken, {
            model: body.model,
            messages: body.messages,
            tools: body.tools,
            signal: controller.signal,
          });
        } catch (error) {
          if (error?.code === "copilot_needs_reconnect") await copilotAccounts.markProviderStatus(user, body.accountId, "needs_reconnect");
          if (error?.code === "copilot_subscription_inactive") await copilotAccounts.markProviderStatus(user, body.accountId, "inactive");
          throw error;
        }
        return send(response, 200, result);
      }
      if (request.method === "GET" && url.pathname === "/v1/copilot/accounts") {
        return send(response, 200, { accounts: await copilotAccounts.list(user) });
      }
      if (request.method === "GET" && url.pathname === "/v1/copilot/provider/accounts") {
        return send(response, 200, { accounts: await copilotAccounts.providerAccounts(user) });
      }
      if (request.method === "DELETE" && url.pathname.startsWith("/v1/copilot/accounts/")) {
        const accountId = decodeURIComponent(url.pathname.slice("/v1/copilot/accounts/".length));
        if (!/^[0-9a-f-]{36}$/i.test(accountId)) return send(response, 400, { error: "invalid_request" });
        await copilotAccounts.disconnect(user, accountId);
        return send(response, 204, {});
      }
      return send(response, 404, { error: "not_found" });
    } catch (error) {
      // Do not log request headers, body, OAuth credentials, or upstream errors.
      const code = error?.code;
      if (code === "copilot_account_not_found") return send(response, 404, { error: code });
      if (code === "copilot_subscription_inactive" || code === "copilot_subscription_unverified" || code === "copilot_needs_reconnect") {
        return send(response, 409, { error: code });
      }
      if (code === "copilot_rate_limited") return send(response, 429, { error: code });
      if (code === "copilot_request_timeout") return send(response, 504, { error: code });
      return send(response, 503, { error: "copilot_service_unavailable" });
    }
  }
  if (request.method !== "POST" || url.pathname !== "/v1/auth/session") {
    return send(response, 404, { error: "not_found" });
  }

  try {
    const user = await authenticatedUser(request);
    if (!user) return send(response, 401, { error: "unauthorized" });
    const account = await upsertUser(user);
    return send(response, 200, account);
  } catch {
    // Do not log request headers, body, OAuth credentials, or upstream errors.
    return send(response, 503, { error: "account_service_unavailable" });
  }
});

server.listen(port, "0.0.0.0", () => {
  process.stdout.write(`Macha account service listening on port ${port}\n`);
});

for (const signal of ["SIGINT", "SIGTERM"]) {
  process.on(signal, () => server.close(() => process.exit(0)));
}
