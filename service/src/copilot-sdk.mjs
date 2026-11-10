// All @github/copilot-sdk calls stay in this module. Each check owns and closes
// its runtime process; tokens are passed only to the SDK's Node-side client.
import { randomUUID } from "node:crypto";
import { CopilotClient, defineTool } from "@github/copilot-sdk";

const FIRST_RESPONSE_TIMEOUT_MS = 180_000;
const MAX_RUN_TIMEOUT_MS = 24 * 60 * 60 * 1000;

export function createCopilotClient(accessToken) {
  if (typeof accessToken !== "string" || !/^(gho_|ghu_)/.test(accessToken)) {
    throw new Error("A supported GitHub OAuth token is required");
  }
  return new CopilotClient({
    gitHubToken: accessToken,
    useLoggedInUser: false,
    logLevel: "none",
  });
}

function contentText(content) {
  if (typeof content === "string") return content;
  if (!Array.isArray(content)) return JSON.stringify(content ?? "");
  return content.map((part) => part?.type === "text" ? part.text || "" : JSON.stringify(part)).join("\n");
}

function promptFromMessages(messages) {
  return messages.map((message) => {
    const role = typeof message?.role === "string" ? message.role : "user";
    const toolCalls = Array.isArray(message?.tool_calls) ? `\nTool calls: ${JSON.stringify(message.tool_calls)}` : "";
    const toolId = typeof message?.tool_call_id === "string" ? ` (${message.tool_call_id})` : "";
    return `<${role}${toolId}>\n${contentText(message?.content)}${toolCalls}\n</${role}>`;
  }).join("\n\n");
}

function mapCopilotError(error) {
  const message = error instanceof Error ? error.message.toLowerCase() : "";
  const code = /rate.?limit|too many requests|http 429/.test(message)
    ? "copilot_rate_limited"
    : /subscription|entitlement|copilot access|no active plan/.test(message)
      ? "copilot_subscription_inactive"
      : /unauthorized|bad credentials|token.*(expired|invalid|revoked)|http 401/.test(message)
        ? "copilot_needs_reconnect"
        : /timed? ?out|timeout|deadline/.test(message)
          ? "copilot_request_timeout"
          : "copilot_request_failed";
  const mapped = new Error("Copilot request failed");
  mapped.code = code;
  return mapped;
}

export async function copilotCompletion(accessToken, { model, messages, tools, onDelta, signal }) {
  if (typeof model !== "string" || !model.trim()) throw new Error("A Copilot model is required");
  if (signal?.aborted) throw new Error("Copilot request cancelled");
  const client = createCopilotClient(accessToken);
  const toolCalls = [];
  let session;
  let content = "";
  let firstResponseTimer;
  const sdkTools = Array.isArray(tools) ? tools.map((tool) => {
    const fn = tool?.function;
    if (!fn || typeof fn.name !== "string") return null;
    return defineTool(fn.name, {
      description: typeof fn.description === "string" ? fn.description : "",
      parameters: fn.parameters && typeof fn.parameters === "object" ? fn.parameters : { type: "object", properties: {} },
      handler: async (args) => {
        toolCalls.push({
          id: randomUUID(),
          type: "function",
          function: { name: fn.name, arguments: JSON.stringify(args ?? {}) },
        });
        if (session) void session.abort();
        throw new Error("Tool call handed to the desktop application");
      },
    });
  }).filter(Boolean) : [];
  let stopTimer;
  const abortRequest = () => {
    if (session) void session.abort().catch(() => {});
    else void client.forceStop().catch(() => {});
  };
  signal?.addEventListener("abort", abortRequest, { once: true });
  try {
    await client.start();
    const sessionConfig = { streaming: typeof onDelta === "function", tools: sdkTools };
    if (model !== "default") sessionConfig.model = model;
    session = await client.createSession(sessionConfig);
    if (signal?.aborted) abortRequest();
    if (typeof onDelta === "function") {
      session.on("assistant.message_delta", (event) => {
        const delta = event.data?.deltaContent || "";
        if (!delta) return;
        clearTimeout(firstResponseTimer);
        content += delta;
        onDelta(delta);
      });
    }
    try {
      const firstResponseTimeout = new Promise((_, reject) => {
        firstResponseTimer = setTimeout(
          () => reject(new Error("Copilot request timed out before the response started")),
          FIRST_RESPONSE_TIMEOUT_MS,
        );
      });
      const answer = await Promise.race([
        firstResponseTimeout,
        session.sendAndWait({ prompt: promptFromMessages(messages) }, MAX_RUN_TIMEOUT_MS),
      ]);
      if (!content && typeof answer?.data?.content === "string") content = answer.data.content;
    } catch (error) {
      if (!toolCalls.length) throw error;
    } finally {
    }
    return {
      id: `copilot-${randomUUID()}`,
      object: "chat.completion",
      model,
      choices: [{
        index: 0,
        message: { role: "assistant", content: content || null, ...(toolCalls.length ? { tool_calls: toolCalls } : {}) },
        finish_reason: toolCalls.length ? "tool_calls" : "stop",
      }],
    };
  } catch (error) {
    throw mapCopilotError(error);
  } finally {
    clearTimeout(firstResponseTimer);
    signal?.removeEventListener("abort", abortRequest);
    try {
      const stopping = client.stop();
      const stopped = await Promise.race([
        stopping.then(() => true),
        new Promise((resolve) => { stopTimer = setTimeout(() => resolve(false), 8000); }),
      ]);
      if (!stopped) await client.forceStop();
    } catch {
      try { await client.forceStop(); } catch { /* shutdown errors never reach logs or clients */ }
    } finally { clearTimeout(stopTimer); }
  }
}

export async function checkCopilotSubscription(accessToken) {
  const client = createCopilotClient(accessToken);
  let result = "unverified";
  let checkTimer;
  try {
    result = await Promise.race([
      (async () => {
        await client.start();
        const auth = await client.getAuthStatus();
        if (!auth.isAuthenticated) return "inactive";
        const models = await client.listModels();
        return models.length ? "active" : "inactive";
      })(),
      new Promise((_, reject) => { checkTimer = setTimeout(() => reject(new Error("Copilot check timed out")), 20000); }),
    ]);
  } catch (error) {
    const message = error instanceof Error ? error.message.toLowerCase() : "";
    if (/subscription|entitlement|copilot access|not entitled|no active plan/.test(message)) result = "inactive";
  } finally {
    clearTimeout(checkTimer);
    try {
      const stopped = client.stop();
      let stopTimer;
      const didStop = await Promise.race([
        stopped.then(() => true),
        new Promise((resolve) => { stopTimer = setTimeout(() => resolve(false), 8000); }),
      ]);
      clearTimeout(stopTimer);
      if (!didStop) await client.forceStop();
    } catch {
      try { await client.forceStop(); } catch { /* keep shutdown errors out of responses/logs */ }
    }
  }
  return result;
}
