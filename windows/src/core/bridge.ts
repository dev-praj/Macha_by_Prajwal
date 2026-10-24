// Thin wrapper over the Tauri commands/events. Every call is a no-op when the
// page is opened in a plain browser, so the island can be iterated on with
// `npm run dev` alone.

import { invoke } from "@tauri-apps/api/core";
import { emit, listen } from "@tauri-apps/api/event";
import { getCurrentWebview } from "@tauri-apps/api/webview";
import { open } from "@tauri-apps/plugin-dialog";
import type {
  ChatMessage,
  DictationReplacement,
  Settings,
  SharedConversationContext,
} from "./state";

export const IS_TAURI =
  typeof window !== "undefined" && "__TAURI_INTERNALS__" in window;

async function call<T>(cmd: string, args?: Record<string, unknown>): Promise<T | null> {
  if (!IS_TAURI) return null;
  try {
    return await invoke<T>(cmd, args);
  } catch (err) {
    console.error(`[macha] ${cmd} failed`, err);
    return null;
  }
}

export interface BootInfo {
  settings: Settings;
  /** Logical screen rect of the monitor the island lives on. */
  screen: { x: number; y: number; width: number; height: number; scale: number };
  version: string;
  hookPath: string;
  /** False where the OS has no global cursor (Wayland): see Island.followPageCursor. */
  cursorPoll: boolean;
}

export interface OpenRouterAccount {
  id: string;
  name: string;
}

export interface MessageNotification {
  id: string;
  appName: string;
  title: string;
  body: string;
}

export interface SpotifyTrack {
  album: string;
  artworkDataUrl: string | null;
  name: string;
  artists: string;
  durationMs: number;
}

export interface SpotifySnapshot {
  connected: boolean;
  playing: boolean;
  progressMs: number;
  shuffle: boolean;
  repeat: boolean;
  volume: number;
  spotifyApp: boolean;
  track: SpotifyTrack | null;
}

export const Bridge = {
  boot: () => call<BootInfo>("boot"),

  saveSettings: (settings: Settings) => call<void>("save_settings", { settings }),
  saveSettingsStrict: (settings: Settings) => callOrThrow<void>("save_settings", { settings }),

  /** Shrink the window down to the invisible wake strip (hidden) or back to full. */
  setCollapsed: (collapsed: boolean) => call<void>("set_collapsed", { collapsed }),

  /**
   * Pushes the island shape in window coordinates. Rust flips click-through from
   * its own cursor poll, so the flag is never a frame behind a click.
   */
  setIslandRect: (x: number, y: number, width: number, height: number) =>
    call<void>("set_island_rect", { x, y, width, height }),

  /** Give the window keyboard focus (chat field) and take it away again. */
  focusWindow: (focused: boolean) => call<void>("focus_window", { focused }),

  reposition: () => call<void>("reposition"),

  openUrl: (url: string) => call<void>("open_url", { url }),
  openUrlStrict: (url: string) => callOrThrow<void>("open_url", { url }),

  /** "Open terminal" → opens the folder in VS Code when `code` is on PATH. */
  openInVSCode: (path: string | null) => call<boolean>("open_in_vscode", { path }),

  quit: () => call<void>("quit_app"),

  openSettingsWindow: () => call<void>("open_settings_window"),
  openFirstRunSetupWindow: () => callOrThrow<void>("open_first_run_setup_window"),
  finishFirstRunSetupWindow: () => callOrThrow<void>("finish_first_run_setup_window"),
  startVisionOnboardingTest: () => callOrThrow<void>("start_vision_onboarding_test"),
  /** Activates the island for Vision Mode without hiding the Settings window. */
  startVisionOnboardingInline: () => callOrThrow<void>("start_vision_onboarding_inline"),
  completeVisionOnboardingTest: async (success: boolean, message?: string) => {
    if (!IS_TAURI) return;
    try {
      await emit("vision-onboarding-result", { success, message });
    } finally {
      await callOrThrow<void>("open_settings_window");
    }
  },
  setDictationSetupTestActive: async (active: boolean) => {
    if (!IS_TAURI) return;
    try {
      await callOrThrow<void>("set_dictation_setup_test_active", { active });
    } catch (error) {
      console.error("[macha] could not synchronize setup dictation state", error);
      await emit("dictation-setup-test-active", active);
    }
  },
  getDictationSetupTestActive: async () => {
    if (!IS_TAURI) return false;
    return await callOrThrow<boolean>("dictation_setup_test_active");
  },
  completeDictationSetupTest: async (transcript: string, error?: string) => {
    if (IS_TAURI) await emit("dictation-setup-test-result", { transcript, error });
  },

  /** Writes to %LOCALAPPDATA%\Macha\macha.log, next to the Rust lines. */
  log: (message: string) => call<void>("log_line", { message }),

  // ── Claude Code hooks ─────────────────────────────────────────────────────
  hooksStatus: () => call<HookStatus>("hooks_status"),
  /** Diff to show before anything is written. `install: false` previews removal. */
  hooksPreview: (install: boolean) => callOrThrow<HookPreview>("hooks_preview", { install }),
  /**
   * Writes ~/.claude/settings.json — only ever after an explicit click, and only
   * when the file still matches the preview the user looked at.
   */
  hooksApply: (install: boolean, fingerprint: string) =>
    callOrThrow<string>("hooks_apply", { install, fingerprint }),
  agentHooksList: () => callOrThrow<AgentHookStatus[]>("agent_hooks_list"),
  agentHooksPreview: (agent: string, install: boolean) =>
    callOrThrow<AgentHookPlan>("agent_hooks_preview", { agent, install }),
  agentHooksApply: (agent: string, install: boolean, fingerprint: string) =>
    callOrThrow<string>("agent_hooks_apply", { agent, install, fingerprint }),

  approvalDecision: (requestId: string, decision: "allow" | "deny") =>
    call<void>("approval_decision", { requestId, decision }),
  /** "The card is up" — until this lands the relay only waits a moment. */
  approvalAck: (requestId: string) => call<void>("approval_ack", { requestId }),
  /** "Nobody can act on this" — Claude Code asks in the terminal right away. */
  approvalDecline: (requestId: string) => call<void>("approval_decline", { requestId }),

  // ── Chat, files, secrets ──────────────────────────────────────────────────
  /** One chat turn. The API key and any file bytes never leave Rust. */
  chatSend: (
    conversationId: string,
    history: ChatMessage[],
    query: string,
    context: ChatContext | null,
    sharedContext: SharedConversationContext[],
    requestId: string,
    localOnly = false,
    voiceRequest = false,
    screenImage: string | null = null,
    workflowPhase?: "plan" | "action" | "verify",
  ) => callOrThrow<ChatReply>("chat_send", {
    conversationId,
    history,
    query,
    context,
    sharedContext,
    requestId,
    localOnly,
    voiceRequest,
    screenImage,
    workflowPhase,
  }),
  chatAction: (
    conversationId: string,
    approved: boolean,
    requestId: string,
    selectedAppId?: string,
    workflowStep = false,
  ) => callOrThrow<ChatReply>("chat_action", {
    conversationId,
    approved,
    requestId,
    selectedAppId,
    workflowStep,
  }),
  chatCancel: (requestId: string) =>
    callOrThrow<void>("chat_cancel", { requestId }),
  setVoiceActive: (active: boolean) =>
    callOrThrow<void>("set_voice_active", { active }),
  setWakeConversationActive: (active: boolean) =>
    callOrThrow<void>("set_wake_conversation_active", { active }),
  setWakeCalibration: async (active: boolean) => {
    if (!IS_TAURI) return;
    await emit("wake-calibration", active);
  },
  ttsSpeak: (text: string, rate: number, volume: number) =>
    callOrThrow<void>("tts_speak", { text, rate, volume }),
  ttsStop: () => callOrThrow<void>("tts_stop"),
  ttsIsSpeaking: () => callOrThrow<boolean>("tts_is_speaking"),
  ollamaStatus: (refresh = false) =>
    callOrThrow<OllamaStatus>("ollama_status", { refresh }),
  chatDelete: (conversationId: string) =>
    callOrThrow<void>("chat_delete", { conversationId }),
  memoryList: () => callOrThrow<MemoryRecord[]>("memories_list"),
  memoryRemember: (category: MemoryRecord["category"], text: string) =>
    callOrThrow<MemoryRecord>("memory_remember", { category, text }),
  memoryForget: (id: string) => callOrThrow<void>("memory_forget", { id }),
  workflowsList: () => callOrThrow<SavedWorkflow[]>("workflows_list"),
  workflowSave: (workflow: { id?: string; name: string; prompt: string }) =>
    callOrThrow<SavedWorkflow>("workflow_save", { workflow }),
  workflowSetArchived: (id: string, archived: boolean) =>
    callOrThrow<void>("workflow_set_archived", { id, archived }),
  workflowDelete: (id: string) => callOrThrow<void>("workflow_delete", { id }),
  workflowRun: (id: string) => callOrThrow<void>("workflow_run", { id }),
  recapSnapshot: () => callOrThrow<RecapSnapshot>("recap_snapshot"),
  recapSetEnabled: (enabled: boolean) => callOrThrow<void>("recap_set_enabled", { enabled }),
  recapClear: () => callOrThrow<void>("recap_clear"),
  phoneLinkStatus: () => call<PhoneLinkStatus>("phone_link_status"),
  phoneLinkSetEnabled: (enabled: boolean) =>
    callOrThrow<PhoneLinkStatus>("phone_link_set_enabled", { enabled }),
  phoneLinkPairing: () => callOrThrow<PhoneLinkPairing>("phone_link_pairing"),
  phoneLinkNewPairing: () => callOrThrow<PhoneLinkPairing>("phone_link_new_pairing"),
  phoneDetailsSetEnabled: (enabled: boolean) =>
    callOrThrow<{ enabled: boolean }>("phone_details_set_enabled", { enabled }),
  phoneLinkPublish: (sessions: PhoneLinkSession[], approval: PhoneLinkApproval | null) =>
    call<void>("phone_link_publish", { sessions, approval }),
  chatReset: () => call<void>("chat_reset"),
  /** Copies a dropped file into the inbox. */
  ingestFile: (path: string) => callOrThrow<DroppedFile>("ingest_file", { path }),
  /** Opens the native file picker; selected files stay on the Rust side. */
  async pickFile(): Promise<string | null> {
    if (!IS_TAURI) return null;
    const selected = await open({ multiple: false, directory: false });
    return Array.isArray(selected) ? selected[0] ?? null : selected;
  },
  async pickAutomationFolder(): Promise<string | null> {
    if (!IS_TAURI) return null;
    const selected = await open({ multiple: false, directory: true });
    return Array.isArray(selected) ? selected[0] ?? null : selected;
  },
  /** Only ever tells you whether a key exists — never its value. */
  secretPresent: (key: string) => call<boolean>("secret_present", { key }),
  secretSet: (key: string, value: string) => callOrThrow<void>("secret_set", { key, value }),
  secretClear: (key: string) => callOrThrow<void>("secret_clear", { key }),
  openRouterAccounts: () => callOrThrow<OpenRouterAccount[]>("openrouter_accounts"),
  openRouterAccountAdd: (name: string, key: string) =>
    callOrThrow<OpenRouterAccount>("openrouter_account_add", { name, key }),
  openRouterAccountRemove: (id: string) =>
    callOrThrow<void>("openrouter_account_remove", { id }),
  openRouterAccountReveal: (id: string) =>
    callOrThrow<string>("openrouter_account_reveal", { id }),

  // ── Integrations ──────────────────────────────────────────────────────────
  refreshIntegration: (id: string) => call<void>("refresh_integration", { id }),
  requestMessageAccess: () => callOrThrow<string>("request_message_access"),
  messageAccessStatus: () => call<boolean>("message_access_status"),
  openMessageSource: (id: string) =>
    callOrThrow<void>("open_message_source", { notificationId: id }),
  spotifyControl: (
    action: "play" | "pause" | "next" | "previous" | "seek" | "shuffle" | "repeat" | "volume",
    value?: number,
  ) => callOrThrow<void>("spotify_control", {
    action,
    positionMs: value ?? null,
  }),
  refreshSpotify: () => callOrThrow<SpotifySnapshot>("refresh_spotify"),
  openSpotify: () => callOrThrow<void>("open_spotify"),

  // ── Vision Mode ─────────────────────────────────────────────────────────
  gestureMouseStart: () => callOrThrow<void>("gesture_mouse_start"),
  gestureMouseStop: () => callOrThrow<void>("gesture_mouse_stop"),
  gestureMouseIsRunning: () => callOrThrow<boolean>("gesture_mouse_is_running"),

  /** Tray → Pause. Stops the integration pollers, not just the island. */
  setPaused: (paused: boolean) => call<void>("set_paused", { paused }),

  /** Tray → toggle the native auto-hide state. */
  toggleAutoHide: () => call<boolean>("toggle_auto_hide"),

  // ── Nixi Commands & Dictation ─────────────────────────────────────────────
  nixiRunCommand: (input: string) =>
    callOrThrow<NixiCommandResult>("nixi_run_command", { input }),
  nixiUndoLastCommand: () =>
    callOrThrow<string>("nixi_undo_last_command"),
  nixiForegroundApplication: () =>
    callOrThrow<string | null>("nixi_foreground_application"),
  nixiDictateInsert: (
    text: string,
    replacements: DictationReplacement[],
    style: "standard" | "verbatim",
  ) => callOrThrow<string>("nixi_dictate_insert", { text, replacements, style }),
  nixiSearchFiles: (query: string, limit?: number) =>
    callOrThrow<NixiFileMatch[]>("nixi_search_files", { query, limit }),
  nixiRevealFile: (path: string) =>
    callOrThrow<void>("nixi_reveal_file", { path }),
  nixiAdjustVolume: (action: string, steps?: number) =>
    callOrThrow<string>("nixi_adjust_volume", { action, steps }),
  nixiSendMedia: (action: string) =>
    callOrThrow<string>("nixi_send_media", { action }),
  nixiToggleTheme: (enableDark?: boolean) =>
    callOrThrow<boolean>("nixi_toggle_theme", { enableDark }),
  nixiCaptureScreenshot: () =>
    callOrThrow<ScreenshotResult>("nixi_capture_screenshot"),
  nixiCaptureScreenRegion: (region: { x: number; y: number; width: number; height: number }) =>
    callOrThrow<string>("nixi_capture_screen_region", region),
};

export interface IntegrationUpdate {
  id: string;
  data: Record<string, unknown>;
  error: string | null;
  event: { success: boolean; label: string; detail: string | null } | null;
}

export type ChatContext =
  | { kind: "file"; name: string; path: string }
  | { kind: "window"; appName: string; title: string; url?: string };

export interface DroppedFile {
  name: string;
  path: string;
  size: number;
}

export interface ChatReply {
  text: string;
  model: string;
  action: AutomationAction | null;
  provider: string;
  fallbackNotice: string | null;
}

export interface MemoryRecord {
  id: string;
  category: "identity" | "preference" | "goal" | "project" | "relationship" | "emotional" | "behavior";
  text: string;
  createdAt: number;
  updatedAt: number;
}

export interface RecapSnapshot {
  enabled: boolean;
  sessions: number;
  filesChanged: number;
  linesAdded: number;
  linesRemoved: number;
  commandsRun: number;
  permissionsAllowed: number;
  permissionsDenied: number;
}

export interface PhoneLinkStatus {
  enabled: boolean;
  running: boolean;
  port: number;
  host: string;
  name: string;
  clients: number;
  error: string | null;
  relayState: string;
}

export interface PhoneLinkPairing {
  link: string;
  qrSvg: string;
  host: string;
  port: number;
  name: string;
  relay: string | null;
}

export interface PhoneLinkSession {
  pillId: string;
  agent: string;
  state: string;
  statusText: string;
  stepIndex: number;
  stepCount: number;
  steps?: string[];
  project?: string;
  color?: string;
}

export interface PhoneLinkApproval {
  requestId: string;
  sessionId: string;
  pillId: string;
  tool: string;
  command: string;
}

export interface SavedWorkflow {
  id: string;
  name: string;
  prompt: string;
  archived: boolean;
  createdAt: number;
  updatedAt: number;
}

export interface OllamaStatus {
  reachable: boolean;
  models: string[];
  error: string | null;
}

export interface AutomationAction {
  name: string;
  arguments: Record<string, unknown>;
  preview?: string | null;
  previewHash?: number | null;
}

export interface HookStatus {
  installed: boolean;
  settingsPath: string;
  hookPath: string;
  hookReady: boolean;
}

export interface AgentHookStatus {
  id: string;
  name: string;
  installed: boolean;
  path: string;
  hookReady: boolean;
  approvals: boolean;
  note: string;
}

export interface AgentHookPlan {
  diff: string;
  backup: string;
  path: string;
  fingerprint: string;
}

export interface HookPreview {
  diff: string;
  backup: string;
  settingsPath: string;
  /** Hand back to hooksApply so only the reviewed diff is ever written. */
  fingerprint: string;
}

/** Same as `call`, but surfaces the error so the UI can show what went wrong. */
async function callOrThrow<T>(cmd: string, args?: Record<string, unknown>): Promise<T> {
  if (!IS_TAURI) throw new Error("not running inside Macha");
  return invoke<T>(cmd, args);
}

export type BridgeEvent =
  | { name: "cursor"; payload: { x: number; y: number } }
  | { name: "tray"; payload: string }
  | { name: "hook"; payload: Record<string, unknown> }
  | { name: "screen-changed"; payload: null };

export interface DragDropPayload {
  type: "enter" | "over" | "drop" | "leave";
  paths?: string[];
}

/** Files dragged onto the island. Only reaches us when the window takes the mouse. */
export async function onDragDrop(handler: (e: DragDropPayload) => void) {
  if (!IS_TAURI) return () => {};
  return getCurrentWebview().onDragDropEvent((event) => {
    handler(event.payload as DragDropPayload);
  });
}

export async function onEvent<T>(name: string, handler: (payload: T) => void) {
  if (!IS_TAURI) return () => {};
  return listen<T>(name, (e) => handler(e.payload));
}

export interface NixiStatus {
  text: string;
  detail?: string;
}

export interface NixiFileMatch {
  path: string;
  name: string;
  folder: string;
  sizeBytes: number;
}

export interface ScreenshotResult {
  success: boolean;
  path: string;
  message: string;
}

export interface NixiCommandResult {
  success: boolean;
  intentName: string;
  summary: string;
  detail?: string;
  canUndo: boolean;
}

export type NixiHotkeyPayload = null | Record<string, unknown> | void;

export function onNixiStatus(handler: (payload: NixiStatus) => void) {
  return onEvent<NixiStatus>("nixi-status", handler);
}

export function onNixiPttStart(handler: (payload: NixiHotkeyPayload) => void) {
  return onEvent<NixiHotkeyPayload>("nixi-ptt-start", handler);
}

export function onNixiPttStop(handler: (payload: NixiHotkeyPayload) => void) {
  return onEvent<NixiHotkeyPayload>("nixi-ptt-stop", handler);
}

export function onNixiDictateStart(handler: (payload: NixiHotkeyPayload) => void) {
  return onEvent<NixiHotkeyPayload>("nixi-dictate-start", handler);
}

export function onNixiDictateStop(handler: (payload: NixiHotkeyPayload) => void) {
  return onEvent<NixiHotkeyPayload>("nixi-dictate-stop", handler);
}
