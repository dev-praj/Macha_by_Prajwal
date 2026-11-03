// Claude Code hook events → island state.
// Port of HookServer.processEvent / processPermissionRequest from the macOS app.
// Difference from macOS: no terminal filter. On Windows the hook fires from any
// terminal (Windows Terminal, VS Code, PowerShell…) and all of them are handled.

import { Bridge, onEvent, type PhoneLinkApproval, type PhoneLinkSession } from "../core/bridge";
import { buildFileDiff, fileName, makeDiffStep } from "../core/diff";
import { Sound } from "../core/sound";
import { State } from "../core/state";
import type { Island } from "./island";

const CLAUDE_ID = "integration_claude";

/** Clears the approval card if no decision was made before the hook gave up. */
let pendingTimeout: number | null = null;

interface HookPayload {
  hook_event_name?: string;
  request_id?: string;
  session_id?: string;
  cwd?: string;
  message?: string;
  /** UserPromptSubmit carries `prompt`; `message` belongs to Notification/Stop. */
  prompt?: string;
  tool_name?: string;
  tool_input?: Record<string, unknown>;
  macha_diff_truncated?: boolean;
  /** Optional agent tag: lowercase, digits and hyphens, ≤ 24 chars. */
  macha_agent?: string;
}

/** Same rule as HookServer.validateAgent on macOS. "claude" is reserved. */
function validateAgent(raw: string | undefined): string | null {
  if (!raw || raw.length > 24 || raw === "claude") return null;
  if (!/^[a-z0-9-]+$/.test(raw)) return null;
  return raw;
}

const FALLBACK_COLORS = ["#22C55E", "#EAB308", "#60A5FA", "#E879F9"];

function agentColor(name: string): string {
  let h = 0;
  for (let i = 0; i < name.length; i++) {
    h = (Math.imul(31, h) + name.charCodeAt(i)) | 0;
  }
  return FALLBACK_COLORS[Math.abs(h) % FALLBACK_COLORS.length];
}

function agentName(id: string): string {
  return id === "copilot" ? "GitHub Copilot" : id;
}

const PROJECT_ALIASES: Record<string, string> = {
  "notch-buddy": "Notch Buddy",
  notchbuddy: "Notch Buddy",
  notch_buddy: "Notch Buddy",
};

function aliasProjectName(name: string): string {
  return PROJECT_ALIASES[name.toLowerCase()] ?? name;
}

function lastPathComponent(p: string): string {
  const cleaned = p.replace(/[\\/]+$/, "");
  const idx = Math.max(cleaned.lastIndexOf("\\"), cleaned.lastIndexOf("/"));
  return idx >= 0 ? cleaned.slice(idx + 1) : cleaned;
}

/** frenchStep() — same labels as the macOS app. */
const TOOL_LABELS: Record<string, string> = {
  Bash: "Exécute",
  Read: "Lit",
  Write: "Écrit",
  Edit: "Modifie",
  Glob: "Cherche",
  Grep: "Recherche",
  WebSearch: "Recherche web",
  WebFetch: "Récupère",
  TodoWrite: "Tâches",
  Task: "Agent",
  LS: "Liste",
  MultiEdit: "Modifie",
  NotebookEdit: "Notebook",
  PowerShell: "Exécute",
};

function stepLabel(tool: string, input: Record<string, unknown>): string {
  const label = TOOL_LABELS[tool] ?? tool;
  const str = (k: string) => (typeof input[k] === "string" ? (input[k] as string) : null);
  const cmd = str("command");
  if (cmd) return `${label} · ${cmd.slice(0, 40)}`;
  const path = str("path");
  if (path) return `${label} · ${lastPathComponent(path)}`;
  const file = str("file_path");
  if (file) return `${label} · ${lastPathComponent(file)}`;
  const query = str("query");
  if (query) return `${label} · ${query.slice(0, 40)}`;
  return label;
}

/**
 * What the Allow button actually authorises. Approving "Write" tells you nothing
 * — approving `Write · C:\…\.env` tells you everything, and the difference is
 * the whole point of approving from the island rather than blind.
 *
 * Ordered by how specific the field is, so an unfamiliar tool still shows
 * whatever identifying string it carries instead of falling back to its name.
 */
const APPROVAL_FIELDS = [
  "command", // Bash, PowerShell
  "file_path", // Write, Edit, MultiEdit, NotebookEdit
  "path", // Read, LS
  "url", // WebFetch
  "query", // WebSearch
  "pattern", // Glob, Grep
  "prompt", // Task
] as const;

function approvalTarget(tool: string, input: Record<string, unknown>): string {
  for (const field of APPROVAL_FIELDS) {
    const value = input[field];
    if (typeof value === "string" && value.trim()) {
      return `${tool} · ${value.trim()}`;
    }
  }
  return tool;
}

function upsert(projectName: string, cwd: string) {
  const t = State.tasks.find((x) => x.id === CLAUDE_ID);
  if (!t) return;
  t.name = projectName;
  if (cwd) t.sessionCwd = cwd;
}

function clearSession() {
  const t = State.tasks.find((x) => x.id === CLAUDE_ID);
  if (!t) return;
  t.steps = [];
  t.stepIndex = 0;
  t.name = "VS Code";
  t.pillBadge = null;
  State.clearSessionDiffs(CLAUDE_ID);
}

function recordDiff(agentId: string, payload: HookPayload) {
  if (payload.macha_diff_truncated || !State.tasks.some((task) => task.id === agentId)) return;
  const diff = buildFileDiff(payload.tool_name ?? "", payload.tool_input ?? {});
  if (!diff) return;
  const id = State.appendSessionDiff(agentId, diff);
  State.appendStep(agentId, makeDiffStep(fileName(diff.path), diff.added, diff.removed, id));
}

export function registerHookHandlers(island: Island) {
  void onEvent<HookPayload>("hook", (payload) => handleHook(island, payload));
  void onEvent<string>("phone-link-decided", (requestId) => {
    if (State.pendingApproval?.requestId !== requestId) return;
    if (pendingTimeout != null) window.clearTimeout(pendingTimeout);
    pendingTimeout = null;
    const agentId = State.pendingApproval.agentId ?? CLAUDE_ID;
    State.pendingApproval = null;
    State.isPinned = false;
    island.dropPin();
    State.updateTask(agentId, "working");
    State.setPillBadge(agentId, null);
    if (State.view === "approval") island.setView(State.defaultView());
    State.notify();
    void publishPhoneState();
  });
}

function publishPhoneState(): Promise<void> {
  const sessions: PhoneLinkSession[] = State.tasks
    .filter((task) => task.id === CLAUDE_ID || (task.source === "agent" && !task.isIntegration))
    .map((task) => {
      const session: PhoneLinkSession = {
        pillId: task.id,
        agent: task.id === CLAUDE_ID ? "Claude Code" : agentName(task.id.replace(/^agent_/, "")),
        state: task.state,
        statusText: task.steps.at(-1) ?? task.name,
        stepIndex: task.stepIndex,
        stepCount: task.steps.length,
        steps: task.steps.slice(-20),
        project: task.name,
        color: task.color,
      };
      return session;
    });
  let approval: PhoneLinkApproval | null = null;
  const pending = State.pendingApproval;
  if (pending) {
    approval = {
      requestId: pending.requestId,
      sessionId: pending.sessionId,
      pillId: pending.agentId ?? CLAUDE_ID,
      tool: pending.tool,
      command: pending.command,
    };
  }
  return Bridge.phoneLinkPublish(sessions, approval).then(() => undefined);
}

function handleHook(island: Island, payload: HookPayload) {
  if (State.paused) {
    // Silence here used to cost Claude Code nearly two minutes: the relay waited
    // for a decision from an island that had already decided not to look. Say so,
    // and the terminal takes the question immediately.
    if (payload.request_id) void Bridge.approvalDecline(payload.request_id);
    return;
  }

  const name = payload.hook_event_name ?? "";
  const cwd = payload.cwd ?? "";
  const raw = lastPathComponent(cwd);
  const projectName = aliasProjectName(raw || "Session");

  // Route to the right pill. Valid macha_agent → dynamic "agent_<name>" pill.
  // "claude" is reserved; absent or invalid → Claude Code pill unchanged.
  const validAgent = validateAgent(payload.macha_agent);
  const agentId = validAgent ? `agent_${validAgent}` : CLAUDE_ID;
  const isExternalAgent = validAgent !== null;
  const focused = State.focusId === agentId;

  /** Alerts force the island open; work events only reveal the compact island. */
  const surface = (view: Parameters<Island["alert"]>[0], isAlert: boolean) => {
    if (State.mode === "expanded") {
      if (isAlert) island.setView(view);
    } else if (isAlert) {
      island.alert(view);
    } else if (State.mode === "hidden") {
      island.reveal();
    }
  };

  /** Ensure the agent pill exists (no-op for Claude Code). */
  const ensurePill = () => {
    if (isExternalAgent) {
      State.upsertExternalAgent(agentId, agentName(validAgent!), agentColor(validAgent!));
    } else {
      upsert(projectName, cwd);
    }
  };

  switch (name) {
    case "SessionStart":
      ensurePill();
      surface("overview", false);
      Sound.play("work");
      break;

    case "UserPromptSubmit": {
      ensurePill();
      State.updateTask(agentId, "thinking");
      // The field is `prompt`; reading `message` meant this step was always blank.
      const asked = payload.prompt ?? payload.message;
      if (asked) State.appendStep(agentId, asked.slice(0, 60));
      surface("overview", false);
      break;
    }

    case "PreToolUse": {
      ensurePill();
      State.updateTask(agentId, "working");
      const tool = payload.tool_name ?? "Tool";
      State.appendStep(agentId, stepLabel(tool, payload.tool_input ?? {}));
      surface("overview", false);
      break;
    }

    case "PostToolUse":
      State.updateTask(agentId, "working");
      recordDiff(agentId, payload);
      break;

    case "PostToolUseFailure":
      State.updateTask(agentId, "working");
      State.appendStep(agentId, "⚠ failed");
      break;

    case "Notification": {
      const message = payload.message?.trim() ?? "";
      if (!message) break;

      ensurePill();
      const lower = message.toLowerCase();
      const isRateLimit = lower.includes("rate limit") || lower.includes("limite d");
      if (isRateLimit) {
        State.updateTask(agentId, "ratelimit");
        Sound.play("rate");
      } else {
        State.updateTask(agentId, "question");
        Sound.play("question");
      }
      State.appendStep(agentId, message.slice(0, 200));

      // A pending approval owns the expanded view until it is answered. Otherwise
      // bring every agent notification into view, even when it is not phrased as
      // a question or arrives from an agent that was not focused already.
      if (State.pendingApproval) {
        island.reveal();
      } else {
        State.setFocus(agentId);
        island.alert(isRateLimit ? "overview" : "question");
      }
      break;
    }

    case "Stop":
      State.updateTask(agentId, "finished");
      if (payload.message) State.appendStep(agentId, payload.message.slice(0, 60));
      Sound.play("finish");
      if (focused) surface("finished", true);
      else State.setPillBadge(agentId, "finished");
      window.setTimeout(() => {
        if (isExternalAgent) {
          State.removeTask(agentId);
        } else {
          State.updateTask(agentId, "idle");
          State.setPillBadge(agentId, null);
        }
      }, 5200);
      break;

    case "StopFailure":
      State.updateTask(agentId, "error");
      Sound.play("error");
      if (focused) surface("error", true);
      else State.setPillBadge(agentId, "error");
      break;

    case "SessionEnd":
      if (isExternalAgent) {
        State.clearSessionDiffs(agentId);
        State.removeTask(agentId);
      } else {
        State.updateTask(agentId, "idle");
        clearSession();
      }
      break;

    case "SubagentStart":
      State.appendStep(agentId, "+ subagent");
      break;

    case "SubagentStop":
      State.appendStep(agentId, "• subagent done");
      break;

    case "PermissionRequest": {
      // These external agents have a decision-capable relay. Other agents leave
      // permission handling to their own UI because Macha cannot answer them.
      if (isExternalAgent && !["codex", "copilot", "muse"].includes(validAgent ?? "")) {
        if (payload.request_id) void Bridge.approvalDecline(payload.request_id);
        break;
      }

      const requestId = payload.request_id ?? "";
      // One card, one request. A second one must never quietly replace the first
      // — that would leave a human staring at request B while request A waits for
      // a decision nobody can give. Hand it straight back to the terminal.
      if (State.pendingApproval && State.pendingApproval.requestId !== requestId) {
        if (requestId) void Bridge.approvalDecline(requestId);
        break;
      }
      if (isExternalAgent) ensurePill();
      else upsert(projectName, cwd);
      if (pendingTimeout != null) window.clearTimeout(pendingTimeout);
      const tool = payload.tool_name ?? "Tool";
      const input = payload.tool_input ?? {};
      State.pendingApproval = {
        requestId,
        sessionId: payload.session_id ?? "",
        tool,
        command: approvalTarget(tool, input),
        agentId,
      };
      // The relay's short ack window closes in 800 ms; everything below this
      // line is synchronous, so the card really is up by the time it lands.
      if (requestId) void Bridge.approvalAck(requestId);
      State.updateTask(agentId, "approval");
      State.isPinned = true;
      Sound.play("approval");
      State.setFocus(agentId);
      // Approval needs a visible decision card even when another agent held focus.
      island.alert("approval");
      // Macha answers within 108 s or not at all; after that the terminal has
      // taken over and the card would be lying.
      pendingTimeout = window.setTimeout(() => {
        pendingTimeout = null;
        if (!State.pendingApproval) return;
        const agentId = State.pendingApproval.agentId ?? CLAUDE_ID;
        State.pendingApproval = null;
        State.isPinned = false;
        island.dropPin();
        State.updateTask(agentId, "working");
        State.setPillBadge(agentId, null);
        if (State.view === "approval") island.setView(State.defaultView());
        State.notify();
        void publishPhoneState();
      }, 110_000);
      break;
    }

    default:
      break;
  }
  State.notify();
  void publishPhoneState();
}
