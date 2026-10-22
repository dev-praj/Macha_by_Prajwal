// Chat view — DOM port of PromptView / ChatBubble / TypingDotsView from
// IslandViewContent.swift.

import { h, svg, clear } from "./dom";
import { ICONS } from "./icons";
import {
  Bridge,
  IS_TAURI,
  onEvent,
  type AutomationAction,
  type ChatContext,
  type ChatReply,
} from "../core/bridge";
import { Sound } from "../core/sound";
import { State, type ChatMessage } from "../core/state";
import { sanitizeAssistantText } from "../core/text";
import * as TTS from "../core/tts";
import {
  canStartHoldToTalk,
  isPartialVisionModeActivation,
  isVisionModeActivation,
} from "../core/voiceCommands";
import {
  createWakeWordListener,
  extractWakeCommand,
  isPartialWakePhrase,
  SpeechServiceUnavailableError,
} from "../core/wakeWord";
import type { ViewHost } from "./views";

let nextId = 1;

const STT_ENDPOINT = "http://127.0.0.1:5005/transcribe";
const STT_TIMEOUT_MS = 10_000;
const VOICE_SILENCE_MS = 450;
const VOICE_VAD_POLL_MS = 50;
const VOICE_NOISE_CALIBRATION_MS = 300;
const VOICE_NO_SPEECH_TIMEOUT_MS = 10_000;
const VOICE_MAX_RECORDING_MS = 30_000;
const WAKE_RETRY_INITIAL_MS = 2_000;
const WAKE_RETRY_MAX_MS = 30_000;
const WAKE_PARTIAL_TIMEOUT_MS = 3_000;
const CAMERA_CONSENT_KEY = "macha.permission.camera.v1";
const WORKFLOW_MAX_ITERATIONS = 60;
const WORKFLOW_MAX_RETRIES = 2;
const WORKFLOW_SETTLE_MS = Math.max(0, Number(import.meta.env.VITE_WORKFLOW_SETTLE_MS) || 1_500);
const WORKFLOW_LAUNCH_SETTLE_MS = Math.max(
  WORKFLOW_SETTLE_MS,
  Number(import.meta.env.VITE_WORKFLOW_LAUNCH_SETTLE_MS) || 5_000,
);
type VoiceState = "idle" | "wake" | "listening" | "transcribing" | "speaking";
type WorkflowRunPayload = { id: string; name: string; prompt: string };
type VoiceChordStop = {
  selectedRegion: { x: number; y: number; width: number; height: number } | null;
};
type WorkflowStep = { name: string; successCriteria: string };
type WorkflowSignal =
  | { kind: "complete" }
  | { kind: "stopped"; reason: string }
  | { kind: "failed"; reason: string };

function newRequestId(): string {
  return typeof crypto.randomUUID === "function"
    ? crypto.randomUUID()
    : `${Date.now()}-${Math.random().toString(36).slice(2)}`;
}

async function copyToClipboard(value: string, button: HTMLButtonElement): Promise<void> {
  try {
    if (navigator.clipboard?.writeText) {
      try {
        await navigator.clipboard.writeText(value);
      } catch {
        copyWithLegacyCommand(value);
      }
    } else {
      copyWithLegacyCommand(value);
    }
    button.textContent = "Copied";
  } catch (error) {
    console.error("[macha] could not copy chat text", error);
    button.textContent = "Copy failed";
  }
  button.disabled = true;
  window.setTimeout(() => {
    button.textContent = button.dataset.label ?? "Copy";
    button.disabled = false;
  }, 1400);
}

function copyWithLegacyCommand(value: string): void {
  const field = h("textarea", {
    readonly: true,
    style: "position:fixed;left:-9999px;top:0",
  }) as HTMLTextAreaElement;
  field.value = value;
  document.body.append(field);
  let copied = false;
  try {
    field.focus();
    field.select();
    copied = document.execCommand("copy");
  } finally {
    field.remove();
  }
  if (!copied) throw new Error("Clipboard access is unavailable.");
}

function copyButton(label: string, value: string): HTMLButtonElement {
  const button = h("button", {
    class: "chat-copy-button",
    text: label,
    title: label,
    "aria-label": label,
  }) as HTMLButtonElement;
  button.dataset.label = label;
  button.addEventListener("click", () => void copyToClipboard(value, button));
  return button;
}

function renderReplyContent(reply: HTMLElement, text: string): void {
  const fencedCode = /```([^\n`]*)\r?\n([\s\S]*?)```/g;
  let cursor = 0;
  let match: RegExpExecArray | null;
  while ((match = fencedCode.exec(text)) !== null) {
    if (match.index > cursor) {
      reply.append(h("div", { class: "reply-text", text: text.slice(cursor, match.index) }));
    }
    const language = match[1].trim();
    const code = match[2].replace(/\r?\n$/, "");
    const pre = h("pre", { class: "chat-code" });
    pre.append(h("code", { text: code }));
    const block = h(
      "div",
      { class: "chat-code-block" },
      h("div", { class: "chat-code-header" },
        h("span", { class: "chat-code-language", text: language || "Code" }),
        copyButton("Copy code", code),
      ),
      pre,
    );
    reply.append(block);
    cursor = fencedCode.lastIndex;
  }
  if (cursor < text.length || cursor === 0) {
    reply.append(h("div", { class: "reply-text", text: text.slice(cursor) }));
  }
}

function bubble(message: ChatMessage): HTMLElement {
  if (message.role === "user") {
    return h(
      "div",
      { class: "chat-row user" },
      h("div", { class: "bubble", text: message.content }),
    );
  }
  const reply = h("div", { class: "reply" });
  renderReplyContent(reply, message.content);
  reply.append(
    h("div", { class: "chat-reply-footer" },
      message.fallbackNotice
        ? h("span", { class: "chat-fallback-notice", text: message.fallbackNotice })
        : h("span", {}),
      message.model
        ? h("span", { class: "chat-model-label", text: `Model used: ${message.model}` })
        : h("span", {}),
      copyButton("Copy response", message.content),
    ),
  );
  return h("div", { class: "chat-row" }, reply);
}

function typingDots(): HTMLElement {
  return h(
    "div",
    { class: "chat-row" },
    h("div", { class: "typing" }, h("i"), h("i"), h("i")),
  );
}

/** The coloured chip showing what the question is about (a dropped file). */
function contextChip(label: string): HTMLElement {
  const chip = h("div", { class: "chip" }, h("i", { class: "chip-dot" }), h("span", { text: label }));
  requestAnimationFrame(() => chip.classList.add("settled"));
  return chip;
}

async function captureScreenSnapshot(existingStream?: MediaStream): Promise<string> {
  if (!existingStream && !navigator.mediaDevices?.getDisplayMedia) {
    throw new Error("Screen sharing is unavailable in this window.");
  }
  const stream = existingStream ?? await navigator.mediaDevices.getDisplayMedia({
    video: { width: { max: 1280 }, height: { max: 800 } },
    audio: false,
  });
  try {
    const video = document.createElement("video");
    video.muted = true;
    video.playsInline = true;
    video.srcObject = stream;
    await video.play();
    if (video.readyState < HTMLMediaElement.HAVE_CURRENT_DATA) {
      await new Promise<void>((resolve, reject) => {
        video.addEventListener("loadeddata", () => resolve(), { once: true });
        video.addEventListener("error", () => reject(new Error("Could not read the selected screen.")), { once: true });
      });
    }
    if (!video.videoWidth || !video.videoHeight) {
      throw new Error("The selected screen did not provide a video frame.");
    }
    const scale = Math.min(1, 1280 / video.videoWidth, 800 / video.videoHeight);
    const canvas = document.createElement("canvas");
    canvas.width = Math.max(1, Math.round(video.videoWidth * scale));
    canvas.height = Math.max(1, Math.round(video.videoHeight * scale));
    const context = canvas.getContext("2d");
    if (!context) throw new Error("Could not prepare the screen snapshot.");
    context.drawImage(video, 0, 0, canvas.width, canvas.height);
    const image = canvas.toDataURL("image/jpeg", 0.58);
    if (image.length > 2_000_000) {
      throw new Error("This screen snapshot is too large. Share a smaller window or screen.");
    }
    return image;
  } finally {
    if (!existingStream) stream.getTracks().forEach((track) => track.stop());
  }
}

function actionDescription(action: AutomationAction): string {
  switch (action.name) {
    case "open_app": return `open ${String(action.arguments.appName ?? "the app")}`;
    case "open_website": return `open ${String(action.arguments.url ?? "a website")}`;
    case "search_web": return "open a web search";
    case "list_directory": return "list files in a folder";
    case "read_file": return "read a file (contents go to the selected chat provider)";
    case "open_path": return "open an item in the file manager";
    case "create_directory": return "create a folder";
    case "create_file": return "create a new file";
    case "move_file": return "move a file";
    case "write_file": return "replace all contents of a file";
    case "erase_file_content": return "clear all contents of a file";
    case "windows_list": return "inspect visible application windows";
    case "windows_click": return "click at a screen location";
    case "windows_type": return "type text into an application";
    case "windows_move": return action.arguments.drag === true ? "drag the pointer" : "move the pointer";
    case "windows_scroll": return "scroll an application";
    case "windows_shortcut": return `press ${String(action.arguments.shortcut ?? "a keyboard shortcut")}`;
    case "windows_switch_window": return `switch to ${String(action.arguments.title ?? "a window")}`;
    case "windows_resize_window": return `move or resize ${String(action.arguments.title ?? "a window")}`;
    case "powershell_command": return "run a PowerShell command";
    default: return "perform a local file action";
  }
}

function isOpenAction(
  action: AutomationAction | null,
): action is AutomationAction & { name: "open_app" | "open_path" | "open_website" } {
  return action?.name === "open_app" || action?.name === "open_path" ||
    action?.name === "open_website";
}

interface AppChoice {
  id: string;
  label: string;
}

function appChoices(action: AutomationAction | null): AppChoice[] {
  const raw = action?.arguments.appChoices;
  if (!Array.isArray(raw)) return [];
  return raw.filter((choice): choice is AppChoice =>
    typeof choice === "object" && choice !== null &&
    "id" in choice && typeof choice.id === "string" &&
    "label" in choice && typeof choice.label === "string",
  );
}

function launchTarget(action: AutomationAction | null): AppChoice | null {
  const target = action?.arguments.launchTarget;
  if (typeof target !== "object" || target === null || !("id" in target) ||
    typeof target.id !== "string" || !("label" in target) || typeof target.label !== "string") {
    return null;
  }
  return { id: target.id, label: target.label };
}

function openActionQuestion(action: AutomationAction, selectedLabel?: string): string {
  const choices = appChoices(action);
  const target = selectedLabel
    ?? launchTarget(action)?.label
    ?? (action.name === "open_app"
      ? String(action.arguments.appName ?? "this application")
      : action.name === "open_website"
        ? String(action.arguments.url ?? "this website")
        : String(action.arguments.path ?? "this file or folder"));
  if (action.name === "open_app" && choices.length > 1 && !selectedLabel) {
    return "Which of these exact-name applications should I open?";
  }
  const description = action.name === "open_app"
    ? "application"
    : action.name === "open_website"
      ? "website"
      : "file or folder";
  return `Are you sure you want to open the ${description} ${target}?`;
}

function normalizeSpokenName(value: string): string {
  return value.normalize("NFKD").toLowerCase().replace(/[^a-z0-9]+/g, " ").trim();
}

function spokenConfirmation(text: string): boolean | null {
  const normalized = text.trim().toLowerCase().replace(/^[\s.,!?;:]+|[\s.,!?;:]+$/g, "");
  if (/^(?:yes|yeah|yep|sure|okay|ok|affirmative)\b/.test(normalized)) return true;
  if (/^(?:no|nope|nah|negative)\b/.test(normalized)) return false;
  return null;
}

function spokenAppChoice(text: string, choices: AppChoice[]): AppChoice | null {
  const words: Record<string, number> = {
    first: 1, one: 1, second: 2, two: 2, third: 3, three: 3,
    fourth: 4, four: 4, fifth: 5, five: 5, sixth: 6, six: 6,
  };
  const ordinal = text.trim().toLowerCase().match(/^(?:option\s+)?(first|one|second|two|third|three|fourth|four|fifth|five|sixth|six|[1-9])(?:\s+one)?[.!?]?$/);
  const index = ordinal
    ? Number(ordinal[1]) || words[ordinal[1]]
    : undefined;
  if (index && choices[index - 1]) return choices[index - 1];
  const spoken = normalizeSpokenName(text);
  return choices.find((choice) => {
    const label = normalizeSpokenName(choice.label);
    return label.length > 0 && spoken === label;
  }) ?? null;
}

export function buildPrompt(onHeightChange: () => void): ViewHost {
  let sending = false;
  let activeRequestId: string | null = null;
  let cancelRequested = false;
  let pendingAction: AutomationAction | null = null;
  let pendingConversationId: string | null = null;
  let historyOpen = false;
  let renderedKey = "";
  let voiceState: VoiceState = "idle";
  let voiceStartGeneration = 0;
  let mediaStream: MediaStream | null = null;
  let recorder: MediaRecorder | null = null;
  let voiceMonitorContext: AudioContext | null = null;
  let voiceMonitorTimer: number | null = null;
  let voiceRecordingStartedAt = 0;
  let voiceSpeechStartedAt = 0;
  let voiceLastSpeechAt = 0;
  let holdToTalkActive = false;
  let objectCapturePending = false;
  let selectedRegionCaptureFailed = false;
  let visionOnboardingActive = false;
  let audioChunks: Blob[] = [];
  let voiceNotice = "";
  let voiceNoticeIsError = false;
  const wakeWord = createWakeWordListener();
  let wakeWordReady = false;
  let wakeWordStarting = false;
  let wakeWordRetryTimer: number | null = null;
  let wakeRecoveryTimer: number | null = null;
  let wakeWordRetryAttempt = 0;
  let partialWakeTranscript = "";
  let partialWakeTimer: number | null = null;
  let lastWakeWordEnabled = State.settings.wakeWordEnabled;
  let lastWakePronunciation = State.settings.wakeWordPronunciation;
  let lastWakeThreshold = State.settings.wakeWordThreshold;
  let wakeCalibrationActive = false;
  let suppressWakeRestart = false;
  let wakeAckInProgress = false;
  let wakeFeedbackTimer: number | null = null;
  let wakeConversationCompleted = false;
  let actionConfirmationActive = false;
  let actionConfirmationProcessing = false;
  let actionConfirmationStage: "choose" | "confirm" = "confirm";
  let selectedAppChoiceId: string | null = null;
  let wakeConversationHeld = false;
  let screenImage: string | null = null;
  let screenCaptureInProgress = false;
  let queuedWorkflows: WorkflowRunPayload[] = [];
  let workflowAutomationActive = false;

  const historyToggle = h("button", {
    class: "chat-action",
    text: "Past conversations",
    onclick: () => {
      historyOpen = !historyOpen;
      State.notify();
    },
  });
  const newChat = h("button", {
    class: "chat-action primary",
    text: "New conversation",
    onclick: () => {
      if (sending || pendingAction) return;
      State.startConversation();
      historyOpen = false;
      onHeightChange();
      input.focus();
    },
  });
  const toolbar = h("div", { class: "chat-toolbar" }, historyToggle, newChat);
  const screenCapture = h("button", {
    class: "chat-action",
    text: "Capture screen",
    title: "Capture one screen snapshot to include with your next message",
    "aria-label": "Capture one screen snapshot",
    onclick: async () => {
      if (screenCaptureInProgress || sending || pendingAction) return;
      if (screenImage) {
        screenImage = null;
        State.notify();
        return;
      }
      screenCaptureInProgress = true;
      screenCapture.disabled = true;
      try {
        screenImage = await captureScreenSnapshot();
        voiceNotice = "Screen snapshot ready for your next message.";
        voiceNoticeIsError = false;
      } catch (error) {
        voiceNotice = String(error).replace(/^Error:\s*/, "");
        voiceNoticeIsError = true;
      } finally {
        screenCaptureInProgress = false;
        screenCapture.disabled = false;
        State.notify();
      }
    },
  }) as HTMLButtonElement;
  toolbar.append(screenCapture);
  const historyList = h("div", { class: "conversation-list" });
  historyList.hidden = true;
  const chipRow = h("div", { class: "chip-row" });
  const log = h("div", { class: "chat-log" });
  const input = h("input", {
    type: "text",
    class: "chat-input",
    placeholder: "Ask me anything…",
    spellcheck: "false",
  }) as HTMLInputElement;
  const sendIcon = svg(ICONS.arrowUp, 11);
  const send = h("button", { class: "send-btn", title: "Send" }, sendIcon);
  const voiceButton = h(
    "button",
    { class: "voice-btn", title: "Start voice input", "aria-label": "Start voice input" },
    h("i", { class: "voice-indicator" }),
    h("span", { text: "Voice" }),
  );
  const stopSpeechButton = h("button", {
    class: "voice-stop-btn",
    text: "Stop",
    title: "Stop speaking",
    "aria-label": "Stop speaking",
  }) as HTMLButtonElement;
  stopSpeechButton.hidden = true;
  stopSpeechButton.addEventListener("click", () => void TTS.stop());
  const voiceStatus = h("span", { class: "voice-status", "aria-live": "polite" });
  const bar = h("div", { class: "chat-bar" }, input, voiceStatus, voiceButton, stopSpeechButton, send);

  function updateVoiceUi() {
    const labels: Record<VoiceState, string> = {
      idle: "Voice",
      wake: "Hey Macha",
      listening: "Listening",
      transcribing: "Transcribing",
      speaking: "Speaking",
    };
    voiceButton.classList.toggle("listening", voiceState === "listening");
    voiceButton.classList.toggle("transcribing", voiceState === "transcribing");
    voiceButton.classList.toggle("speaking", voiceState === "speaking");
    voiceButton.classList.toggle("wake-active", voiceState === "wake");
    voiceButton.title = voiceState === "listening"
      ? "Stop recording"
      : voiceState === "wake"
        ? 'Listening for "Hey Macha" — click to speak now'
        : "Start voice input";
    voiceButton.setAttribute("aria-label", voiceButton.title);
    voiceButton.querySelector("span")!.textContent = labels[voiceState];
    stopSpeechButton.hidden = voiceState !== "speaking";
    voiceStatus.textContent = voiceNotice;
    voiceStatus.classList.toggle("error", voiceNoticeIsError);
  }

  function setVoiceState(next: VoiceState, notice = "", isError = false) {
    voiceState = next;
    voiceNotice = notice;
    voiceNoticeIsError = isError;
    updateVoiceUi();
  }

  function stopWakeWord() {
    wakeWord.stop();
    if (wakeFeedbackTimer !== null) {
      window.clearTimeout(wakeFeedbackTimer);
      wakeFeedbackTimer = null;
    }
    clearPartialWakeTranscript();
  }

  function clearPartialWakeTranscript() {
    if (partialWakeTimer !== null) {
      window.clearTimeout(partialWakeTimer);
      partialWakeTimer = null;
    }
    partialWakeTranscript = "";
  }

  function rememberPartialWakeTranscript(transcript: string) {
    clearPartialWakeTranscript();
    partialWakeTranscript = transcript;
    partialWakeTimer = window.setTimeout(clearPartialWakeTranscript, WAKE_PARTIAL_TIMEOUT_MS);
  }

  async function activateVisionModeFromVoice() {
    if (wakeFeedbackTimer !== null) {
      window.clearTimeout(wakeFeedbackTimer);
      wakeFeedbackTimer = null;
    }
    stopWakeWord();
    holdWakeConversation();
    suppressWakeRestart = true;
    wakeAckInProgress = true;
    Sound.play("blip");
    setVoiceState("listening", "Activating Vision Mode…");

    let notice = "Vision Mode is active.";
    let failed = false;
    try {
      if (window.localStorage.getItem(CAMERA_CONSENT_KEY) !== "true") {
        notice = "Camera gesture controls are off. Allow camera use in Settings before starting Vision Mode.";
        await TTS.speak(notice).catch((error: unknown) => {
          console.error("[macha] could not speak the camera-consent notice", error);
        });
      } else {
        try {
          await TTS.speak("Activating Vision Mode.");
        } catch (error) {
          console.error("[macha] could not speak Vision Mode activation", error);
        }

        const alreadyRunning = await Bridge.gestureMouseIsRunning();
        if (!alreadyRunning) {
          setVoiceState("transcribing", "Starting Vision Mode…");
          await Bridge.gestureMouseStart();
        }
        notice = alreadyRunning ? "Vision Mode is already running." : "Vision Mode is active.";
        try {
          await TTS.speak(notice);
        } catch (error) {
          console.error("[macha] could not speak Vision Mode status", error);
        }
      }
    } catch (error) {
      failed = true;
      notice = `Could not activate Vision Mode: ${error instanceof Error ? error.message : String(error)}`;
      console.error("[macha] could not activate Vision Mode", error);
      try {
        await TTS.speak("I couldn't activate Vision Mode. Check its status in Settings.");
      } catch (speechError) {
        console.error("[macha] could not speak Vision Mode error", speechError);
      }
    } finally {
      wakeAckInProgress = false;
      suppressWakeRestart = false;
      await releaseVoiceSession();
      setVoiceState("idle", notice, failed);
    }
  }

  function clearWakeWordRetry() {
    if (wakeWordRetryTimer !== null) {
      window.clearTimeout(wakeWordRetryTimer);
      wakeWordRetryTimer = null;
    }
  }

  function retryWakeWordStart(
    notice = "Local speech service is still starting; retrying Hey Macha.",
  ) {
    if (wakeWordRetryTimer !== null) return;
    const delay = Math.min(WAKE_RETRY_INITIAL_MS * 2 ** wakeWordRetryAttempt, WAKE_RETRY_MAX_MS);
    wakeWordRetryAttempt += 1;
    setVoiceState("idle", notice);
    wakeWordRetryTimer = window.setTimeout(() => {
      wakeWordRetryTimer = null;
      void reconcileWakeWord();
    }, delay);
  }

  function holdWakeConversation() {
    wakeConversationCompleted = false;
    wakeConversationHeld = true;
    window.dispatchEvent(new Event("macha-wake-word"));
  }

  function releaseWakeConversation() {
    if (!wakeConversationHeld) return;
    wakeConversationHeld = false;
    wakeConversationCompleted = true;
    window.dispatchEvent(new Event("macha-wake-word-complete"));
    void Bridge.setVoiceActive(false).catch((error: unknown) => {
      console.error("[macha] could not clear the wake voice hold", error);
    });
    window.setTimeout(() => {
      wakeConversationCompleted = false;
      if (!sending && voiceState === "idle" && !wakeConversationHeld) {
        void reconcileWakeWord();
      }
    }, 1_000);
  }

  function canRunWakeWord(): boolean {
    return wakeWordReady && State.settings.wakeWordEnabled && IS_TAURI && !sending && !wakeCalibrationActive
      && !wakeConversationCompleted && !pendingAction && !actionConfirmationActive
      && voiceState !== "listening" && voiceState !== "transcribing"
      && voiceState !== "speaking" && !TTS.isSpeaking();
  }

  function recoverWakeWord(reason: string) {
    if (!wakeWordReady || !State.settings.wakeWordEnabled || !IS_TAURI) return;
    if (wakeRecoveryTimer !== null) window.clearTimeout(wakeRecoveryTimer);
    wakeRecoveryTimer = window.setTimeout(() => {
      wakeRecoveryTimer = null;
      if (wakeWord.isActive()) wakeWord.stop();
      clearWakeWordRetry();
      wakeWordRetryAttempt = 0;
      console.info(`[macha] restarting Hey Macha listener after ${reason}`);
      void reconcileWakeWord();
    }, 250);
  }

  async function reconcileWakeWord() {
    if (!canRunWakeWord()) {
      clearWakeWordRetry();
      if (wakeWord.isActive()) wakeWord.stop();
      if (voiceState === "wake") setVoiceState("idle");
      return;
    }
    if (wakeWord.isActive() || wakeWordStarting) return;
    wakeWordStarting = true;
    try {
      await wakeWord.start(
        () => {
          if (wakeFeedbackTimer !== null) window.clearTimeout(wakeFeedbackTimer);
          setVoiceState("wake", 'Listening for "Hey Macha"…');
          wakeFeedbackTimer = window.setTimeout(() => {
            wakeFeedbackTimer = null;
            if (voiceState === "wake") setVoiceState("wake", 'Say "Hey Macha" to ask a question.');
          }, 10_000);
        },
        (text) => void onWakeUtterance(text),
        (message) => {
          console.error("[macha] Hey Macha wake-word listener error", message);
          wakeWord.stop();
          retryWakeWordStart(`${message} Reconnecting the microphone…`);
        },
        State.settings.wakeWordPronunciation,
        State.settings.wakeWordThreshold,
      );
      if (wakeWord.isActive()) {
        clearWakeWordRetry();
        wakeWordRetryAttempt = 0;
        setVoiceState("wake", 'Say "Hey Macha" to ask a question.');
      }
    } catch (error) {
      if (error instanceof SpeechServiceUnavailableError) {
        if (canRunWakeWord()) retryWakeWordStart();
        return;
      }
      const name = error instanceof DOMException ? error.name : "";
      const message = name === "NotAllowedError" || name === "SecurityError"
        ? 'Macha needs microphone access to listen for "Hey Macha". Enable microphone access for Macha in Windows Settings > Privacy & security > Microphone, then return to the app.'
        : error instanceof Error
          ? error.message
          : "Could not start the Hey Macha wake-word listener.";
      setVoiceState("idle", message, true);
      console.error("[macha] could not start Macha wake-word listener", error);
      if (name !== "NotAllowedError" && name !== "SecurityError" && canRunWakeWord()) {
        retryWakeWordStart(`${message} Retrying Hey Macha…`);
      }
    } finally {
      wakeWordStarting = false;
    }
  }

  async function onWakeUtterance(transcript: string) {
    const combinedTranscript = partialWakeTranscript
      ? `${partialWakeTranscript} ${transcript}`
      : transcript;
    clearPartialWakeTranscript();
    if (isVisionModeActivation(combinedTranscript)) {
      await activateVisionModeFromVoice();
      return;
    }
    const pronunciation = State.settings.wakeWordPronunciation;
    const command = extractWakeCommand(combinedTranscript, pronunciation);
    if (command === null) {
      if (isPartialWakePhrase(combinedTranscript, pronunciation)) {
        rememberPartialWakeTranscript(combinedTranscript);
      }
      return;
    }
    console.info("[macha] Hey Macha wake phrase matched", {
      commandIncluded: Boolean(command),
    });
    if (command && isPartialVisionModeActivation(command)) {
      rememberPartialWakeTranscript(combinedTranscript);
      return;
    }

    if (wakeFeedbackTimer !== null) {
      window.clearTimeout(wakeFeedbackTimer);
      wakeFeedbackTimer = null;
    }
    stopWakeWord();
    holdWakeConversation();
    suppressWakeRestart = true;
    wakeAckInProgress = true;
    Sound.play("blip");
    setVoiceState("listening", "Yes Boss — I'm listening.");
    const acknowledgement = TTS.speak("Yes Boss").catch((error: unknown) => {
      console.error("[macha] could not speak wake-word acknowledgement", error);
    });
    void acknowledgement.then(() => { wakeAckInProgress = false; });
    if (!command) {
      await acknowledgement;
      await startListening();
      return;
    }

    try {
      await Bridge.setVoiceActive(true);
      input.value = command;
      setVoiceState("transcribing", "Sending your message…");
      await submit(command, true, wakeConversationHeld, acknowledgement);
    } catch (error) {
      await releaseVoiceSession();
      setVoiceState(
        "idle",
        error instanceof Error ? error.message : "Could not send the wake-word command.",
        true,
      );
    }
  }

  window.addEventListener("macha-settings-ready", () => {
    wakeWordReady = true;
    void reconcileWakeWord();
  }, { once: true });
  document.addEventListener("visibilitychange", () => {
    if (!document.hidden) recoverWakeWord("app foregrounded");
  });
  window.addEventListener("pageshow", () => recoverWakeWord("page restored"));
  navigator.mediaDevices?.addEventListener("devicechange", () => {
    recoverWakeWord("microphone device changed");
  });
  State.subscribe(() => {
    const phraseChanged = lastWakePronunciation !== State.settings.wakeWordPronunciation
      || lastWakeThreshold !== State.settings.wakeWordThreshold;
    if (lastWakeWordEnabled === State.settings.wakeWordEnabled && !phraseChanged) return;
    lastWakeWordEnabled = State.settings.wakeWordEnabled;
    lastWakePronunciation = State.settings.wakeWordPronunciation;
    lastWakeThreshold = State.settings.wakeWordThreshold;
    if (phraseChanged && wakeWord.isActive()) wakeWord.stop();
    if (!lastWakeWordEnabled) {
      clearWakeWordRetry();
      wakeWordRetryAttempt = 0;
      stopWakeWord();
      setVoiceState("idle");
    } else {
      void reconcileWakeWord();
    }
  });
  void onEvent<boolean>("wake-calibration", (active) => {
    wakeCalibrationActive = active;
    if (active) {
      stopWakeWord();
      setVoiceState("idle", "Recording your wake phrase…");
    } else {
      setVoiceState("idle");
      void reconcileWakeWord();
    }
  });

  TTS.onStateChange((speaking, error) => {
    if (speaking) {
      stopWakeWord();
      if (wakeAckInProgress) {
        if (voiceState !== "transcribing") setVoiceState("listening", "Yes Boss — I'm listening.");
      } else {
        setVoiceState("speaking", error ?? "", !!error);
      }
      void Bridge.setVoiceActive(true).catch((cause: unknown) => {
        console.error("[macha] could not hold island visible for speech", cause);
      });
    } else if (wakeAckInProgress) {
      if (voiceState !== "transcribing") setVoiceState("listening", "Yes Boss — I'm listening.");
    } else if (voiceState === "speaking") {
      setVoiceState("idle", error ?? "", !!error);
      void Bridge.setVoiceActive(false).catch((cause: unknown) => {
        console.error("[macha] could not release speech auto-hide hold", cause);
      });
      if (!suppressWakeRestart) void reconcileWakeWord();
    } else if (error) {
      setVoiceState(voiceState, error, true);
    }
  });

  function stopMediaTracks() {
    if (voiceMonitorTimer !== null) {
      window.clearInterval(voiceMonitorTimer);
      voiceMonitorTimer = null;
    }
    const monitorContext = voiceMonitorContext;
    voiceMonitorContext = null;
    if (monitorContext && monitorContext.state !== "closed") {
      void monitorContext.close().catch((error: unknown) => {
        console.error("[macha] could not close voice activity monitor", error);
      });
    }
    mediaStream?.getTracks().forEach((track) => track.stop());
    mediaStream = null;
    recorder = null;
    voiceSpeechStartedAt = 0;
    voiceLastSpeechAt = 0;
  }

  function monitorVoiceActivity(analyser: AnalyserNode) {
    const samples = new Float32Array(analyser.fftSize);
    const noiseSamples: number[] = [];
    let noiseFloor = 0;
    const baseThreshold = Math.max(
      0.0025,
      Math.min(0.012, State.settings.wakeWordThreshold * 0.3),
    );
    let speechThreshold = baseThreshold;
    const check = () => {
      if (!recorder || recorder.state !== "recording") {
        stopListening();
        return;
      }

      analyser.getFloatTimeDomainData(samples);
      let sumSquares = 0;
      for (const sample of samples) sumSquares += sample * sample;
      const level = Math.sqrt(sumSquares / samples.length);
      const now = performance.now();

      if (now - voiceRecordingStartedAt <= VOICE_NOISE_CALIBRATION_MS) {
        noiseSamples.push(level);
      } else {
        if (noiseSamples.length > 0) {
          noiseSamples.sort((a, b) => a - b);
          noiseFloor = noiseSamples[Math.floor(noiseSamples.length * 0.2)];
          speechThreshold = Math.max(baseThreshold, Math.min(0.02, noiseFloor * 1.8));
          noiseSamples.length = 0;
        }

        if (level >= speechThreshold) {
          if (!voiceSpeechStartedAt) voiceSpeechStartedAt = now;
          voiceLastSpeechAt = now;
        } else if (!voiceSpeechStartedAt) {
          noiseFloor = noiseFloor * 0.9 + level * 0.1;
          speechThreshold = Math.max(baseThreshold, Math.min(0.02, noiseFloor * 1.8));
        }
      }

      const recordingDuration = now - voiceRecordingStartedAt;
      const speechDuration = voiceSpeechStartedAt ? now - voiceSpeechStartedAt : 0;
      if (recordingDuration >= VOICE_MAX_RECORDING_MS
        || (!holdToTalkActive && !objectCapturePending && !voiceSpeechStartedAt
          && recordingDuration >= VOICE_NO_SPEECH_TIMEOUT_MS)
        || (!holdToTalkActive && !objectCapturePending && speechDuration >= 250
          && now - voiceLastSpeechAt >= VOICE_SILENCE_MS)) {
        stopListening();
      }
    };
    voiceMonitorTimer = window.setInterval(check, VOICE_VAD_POLL_MS);
  }

  async function releaseVoiceSession() {
    if (voiceState !== "idle") setVoiceState("idle");
    try {
      await Bridge.setVoiceActive(false);
    } catch (error) {
      console.error("[macha] could not release voice auto-hide hold", error);
    }
    releaseWakeConversation();
    if (!suppressWakeRestart) void reconcileWakeWord();
  }

  async function listenForActionConfirmation() {
    if (!actionConfirmationActive || !isOpenAction(pendingAction)) return;
    try {
      await wakeWord.start(
        () => undefined,
        (text) => void onActionConfirmationTranscript(text),
        (message) => {
          console.error("[macha] voice confirmation listener error", message);
          setVoiceState("idle", message, true);
        },
        "",
        State.settings.wakeWordThreshold,
        false,
      );
      if (wakeWord.isActive()) {
        setVoiceState(
          "listening",
          actionConfirmationStage === "choose"
            ? "Say the full app name or its option number."
            : 'Say "yes" to open, or "no" to cancel.',
        );
      }
    } catch (error) {
      const message = error instanceof Error ? error.message : "Could not start voice confirmation.";
      setVoiceState("idle", `${message} Use the Yes or No button.`, true);
      console.error("[macha] could not start voice confirmation listener", error);
    }
  }

  async function askToOpenAction(tryAgain = false) {
    if (!isOpenAction(pendingAction)) return;
    const action = pendingAction;
    const choices = appChoices(action);
    const selected = choices.find((choice) => choice.id === selectedAppChoiceId);
    try {
      const prompt = action.name === "open_app" && choices.length > 1 && actionConfirmationStage === "choose"
        ? tryAgain
          ? `Please say the full name of the app, or say its option number. ${choices.map((choice, index) => `Option ${index + 1}: ${choice.label}.`).join(" ")}`
          : `I found more than one installed app with that full name. Which one should I open? ${choices.map((choice, index) => `Option ${index + 1}: ${choice.label}.`).join(" ")} Say the full name or option number.`
        : tryAgain
          ? 'Please say "yes" to open it or "no" to cancel.'
          : `${openActionQuestion(action, selected?.label)} Say "yes" to open it or "no" to cancel.`;
      await TTS.speak(prompt);
    } catch (error) {
      console.error("[macha] could not speak the open confirmation", error);
      setVoiceState("idle", "Could not speak the confirmation. Use the Yes or No button.", true);
    }
    await listenForActionConfirmation();
  }

  async function beginActionConfirmation() {
    if (!isOpenAction(pendingAction) || actionConfirmationActive) return;
    actionConfirmationActive = true;
    actionConfirmationProcessing = false;
    selectedAppChoiceId = null;
    actionConfirmationStage = appChoices(pendingAction).length > 1 ? "choose" : "confirm";
    State.notify();
    stopWakeWord();
    holdWakeConversation();
    await askToOpenAction();
  }

  async function onActionConfirmationTranscript(text: string) {
    if (!actionConfirmationActive || actionConfirmationProcessing) return;
    actionConfirmationProcessing = true;
    wakeWord.stop();
    if (actionConfirmationStage === "choose" && isOpenAction(pendingAction)) {
      const choice = spokenAppChoice(text, appChoices(pendingAction));
      if (!choice) {
        actionConfirmationProcessing = false;
        setVoiceState("speaking");
        await askToOpenAction(true);
        return;
      }
      selectedAppChoiceId = choice.id;
      actionConfirmationStage = "confirm";
      State.notify();
      setVoiceState("speaking");
      await askToOpenAction();
      actionConfirmationProcessing = false;
      return;
    }
    const approved = spokenConfirmation(text);
    if (approved !== null) {
      actionConfirmationActive = false;
      setVoiceState("transcribing", approved ? "Opening as requested…" : "Cancelling the open request…");
      await answerAction(approved, true);
      return;
    }
    setVoiceState("speaking");
    await askToOpenAction(true);
    actionConfirmationProcessing = false;
  }

  async function startListening() {
    if (voiceState === "transcribing" || (sending && voiceState !== "speaking")) return;
    const generation = ++voiceStartGeneration;
    stopWakeWord();
    voiceNotice = "";
    voiceNoticeIsError = false;
    suppressWakeRestart = true;
    try {
      await TTS.stop();
    } catch (error) {
      if (generation !== voiceStartGeneration) return;
      suppressWakeRestart = false;
      console.error("[macha] could not stop speech before microphone recording", error);
      setVoiceState("speaking", "Could not stop speech before recording.", true);
      return;
    }
    setVoiceState("listening", "I'm listening — speak now.");
    try {
      await Bridge.setVoiceActive(true);
      if (generation !== voiceStartGeneration) return;
      if (!navigator.mediaDevices?.getUserMedia || typeof MediaRecorder === "undefined") {
        throw new Error("Microphone recording is not available in this window.");
      }
      const stream = await navigator.mediaDevices.getUserMedia({
        audio: {
          channelCount: 1,
          echoCancellation: true,
          noiseSuppression: true,
          autoGainControl: true,
        },
      });
      if (generation !== voiceStartGeneration) {
        stream.getTracks().forEach((track) => track.stop());
        return;
      }
      mediaStream = stream;
      voiceMonitorContext = new AudioContext();
      await voiceMonitorContext.resume();
      if (generation !== voiceStartGeneration) {
        stopMediaTracks();
        return;
      }
      const source = voiceMonitorContext.createMediaStreamSource(mediaStream);
      const analyser = voiceMonitorContext.createAnalyser();
      const mutedOutput = voiceMonitorContext.createGain();
      analyser.fftSize = 2048;
      mutedOutput.gain.value = 0;
      source.connect(analyser);
      analyser.connect(mutedOutput);
      mutedOutput.connect(voiceMonitorContext.destination);
      const mimeType = MediaRecorder.isTypeSupported("audio/webm;codecs=opus")
        ? "audio/webm;codecs=opus"
        : "audio/webm";
      recorder = new MediaRecorder(mediaStream, { mimeType });
      voiceRecordingStartedAt = performance.now();
      voiceSpeechStartedAt = 0;
      voiceLastSpeechAt = voiceRecordingStartedAt;
      audioChunks = [];
      recorder.addEventListener("dataavailable", (event) => {
        if (event.data.size > 0) audioChunks.push(event.data);
      });
      recorder.addEventListener("error", () => {
        stopMediaTracks();
        void releaseVoiceSession();
        setVoiceState("idle", "Microphone recording failed.", true);
      }, { once: true });
      recorder.addEventListener("stop", () => {
        const audio = new Blob(audioChunks, { type: mimeType });
        audioChunks = [];
        stopMediaTracks();
        void transcribe(audio);
      }, { once: true });
      recorder.start();
      monitorVoiceActivity(analyser);
      input.blur();
    } catch (error) {
      if (generation !== voiceStartGeneration) {
        stopMediaTracks();
        return;
      }
      stopMediaTracks();
      const name = error instanceof DOMException ? error.name : "";
      const message = name === "NotAllowedError" || name === "SecurityError"
        ? "Microphone permission denied."
        : name === "NotFoundError" || name === "DevicesNotFoundError"
          ? "No microphone found."
          : error instanceof Error
            ? error.message
            : "Could not start microphone recording.";
      await releaseVoiceSession();
      setVoiceState("idle", message, true);
    } finally {
      suppressWakeRestart = false;
    }
  }

  function stopListening() {
    const activeRecorder = recorder;
    if (!activeRecorder || activeRecorder.state !== "recording") {
      if (voiceState === "listening") {
        voiceStartGeneration += 1;
        stopMediaTracks();
        void releaseVoiceSession();
      }
      return;
    }
    setVoiceState("transcribing", "Finishing speech…");
    try {
      activeRecorder.stop();
    } catch (error) {
      console.error("[macha] could not stop microphone recording", error);
      stopMediaTracks();
      void releaseVoiceSession();
      setVoiceState("idle", "Could not finish microphone recording.", true);
    }
  }

  function reportVisionOnboardingResult(success: boolean, message?: string) {
    if (!visionOnboardingActive) return;
    visionOnboardingActive = false;
    void Bridge.completeVisionOnboardingTest(success, message).catch((error: unknown) => {
      console.error("[macha] could not report the Vision Mode setup result", error);
    });
  }

  async function transcribe(audio: Blob) {
    if (audio.size === 0) {
      await releaseVoiceSession();
      setVoiceState("idle", "No speech detected.");
      reportVisionOnboardingResult(false, "No speech was detected. Try again and speak after the voice prompt opens.");
      return;
    }
    const form = new FormData();
    form.append("audio", audio, "recording.webm");
    const controller = new AbortController();
    const timeout = window.setTimeout(() => controller.abort(), STT_TIMEOUT_MS);
    try {
      const response = await fetch(STT_ENDPOINT, {
        method: "POST",
        body: form,
        signal: controller.signal,
      });
      if (!response.ok) {
        if (response.status >= 500) throw new Error("Voice server not running");
        const body: unknown = await response.json().catch(() => null);
        const message = typeof body === "object" && body !== null && "error" in body
          && typeof body.error === "string"
          ? body.error
          : `Voice transcription failed (HTTP ${response.status}).`;
        throw new Error(message);
      }
      const body: unknown = await response.json();
      const transcript = typeof body === "object" && body !== null && "text" in body
        && typeof body.text === "string"
        ? body.text.trim()
        : "";
      const confidence = typeof body === "object" && body !== null && "confidence" in body
        && typeof body.confidence === "number"
        ? body.confidence
        : null;
      if (!transcript || confidence === null) {
        await releaseVoiceSession();
        setVoiceState("idle", "No speech detected. Please try again.");
        reportVisionOnboardingResult(false, "No speech was detected. Try again and speak after the voice prompt opens.");
        return;
      }
      if (confidence < -8) {
        await releaseVoiceSession();
        setVoiceState(
          "idle",
          "I could not understand that clearly. Please try again.",
          true,
        );
        reportVisionOnboardingResult(false, "Macha could not understand the question. Try speaking clearly and a little closer to the microphone.");
        return;
      }
      if (selectedRegionCaptureFailed) {
        selectedRegionCaptureFailed = false;
        const message = voiceNotice || "Could not capture the selected screen region.";
        await releaseVoiceSession();
        setVoiceState("idle", message, true);
        reportVisionOnboardingResult(false, message);
        return;
      }
      if (wakeConversationHeld && isVisionModeActivation(transcript)) {
        await activateVisionModeFromVoice();
        return;
      }
      input.value = transcript;
      setVoiceState("transcribing", "Sending your message…");
      await submit(transcript, true, wakeConversationHeld);
      if (!pendingAction && voiceState !== "speaking") {
        await releaseVoiceSession();
      }
    } catch (error) {
      const message = error instanceof DOMException && error.name === "AbortError"
        ? "Voice transcription timed out."
        : error instanceof TypeError
          ? "Voice server not running"
          : error instanceof Error
            ? error.message
            : "Voice transcription failed.";
      await releaseVoiceSession();
      setVoiceState("idle", message, true);
      reportVisionOnboardingResult(false, message);
    } finally {
      window.clearTimeout(timeout);
    }
  }

  async function toggleVoice() {
    if (actionConfirmationActive) return;
    if (voiceState === "listening") {
      stopListening();
    } else if (voiceState === "speaking" || voiceState === "wake") {
      await startListening();
    } else if (voiceState === "idle") {
      await startListening();
    }
  }

  voiceButton.addEventListener("click", () => void toggleVoice());
  void onEvent<null>("vision-onboarding-start", () => {
    visionOnboardingActive = true;
    voiceNotice = "Vision Mode test: hold left Ctrl + left Alt, drag over something, ask Macha about it, then release.";
    voiceNoticeIsError = false;
    State.notify();
  });
  void onEvent<null>("voice-chord-start", () => {
    if (!canStartHoldToTalk(
      voiceState,
      actionConfirmationActive || workflowAutomationActive,
    )) {
      if (visionOnboardingActive) {
        reportVisionOnboardingResult(false, "Macha could not start voice capture. Finish the current action and try the Vision Mode test again.");
      }
      return;
    }
    holdToTalkActive = true;
    selectedRegionCaptureFailed = false;
    void startListening().then(() => {
      if (holdToTalkActive && voiceState === "listening") {
        setVoiceState("listening", "Ask your question; drag over an object to include it.");
      } else if (visionOnboardingActive && voiceState !== "listening") {
        reportVisionOnboardingResult(false, "Microphone capture could not start. Check microphone access and try the Vision Mode test again.");
      }
    });
  });
  void onEvent<VoiceChordStop>("voice-chord-stop", async ({ selectedRegion }) => {
    holdToTalkActive = false;
    if (selectedRegion && voiceState === "listening") {
      objectCapturePending = true;
      try {
        if (window.localStorage.getItem("macha.permission.screenRegion.v1") !== "true") {
          throw new Error(
            "Screen-region access is off. Enable it in Settings → Macha Setup → Permissions to use Vision Mode.",
          );
        }
        setVoiceState("listening", "Capturing the selected screen region…");
        screenImage = await Bridge.nixiCaptureScreenRegion(selectedRegion);
        voiceNotice = "Selected screen region attached to your question.";
        voiceNoticeIsError = false;
        State.notify();
      } catch (error) {
        selectedRegionCaptureFailed = true;
        voiceNotice = error instanceof Error
          ? error.message
          : "Could not capture the selected screen region.";
        voiceNoticeIsError = true;
        console.error("[macha] could not capture selected screen region", error);
        State.notify();
      } finally {
        objectCapturePending = false;
      }
    }
    if (voiceState === "listening") stopListening();
  });
  void onEvent<WorkflowRunPayload>("workflow-run", enqueueWorkflow);

  const el = h(
    "div",
    { class: "view" },
    h(
      "div",
      { class: "card wash chat-card" },
      h("div", { class: "chat-body" }, toolbar, historyList, chipRow, log, bar),
    ),
  );
  (el.querySelector(".card") as HTMLElement).style.setProperty("--wash", "rgba(99,102,241,0.5)");

  async function submit(
    queryOverride?: string,
    voiceRequest = false,
    wakeInitiated = false,
    acknowledgement?: Promise<void>,
  ) {
    const requestedQuery = (queryOverride ?? input.value).trim();
    const query = requestedQuery;
    if (!query || sending || pendingAction || workflowAutomationActive) {
      if (voiceRequest) await releaseVoiceSession();
      if (wakeInitiated) releaseWakeConversation();
      return;
    }
    const conversationId = State.activeConversationId ?? State.startConversation();
    const requestId = newRequestId();
    activeRequestId = requestId;
    cancelRequested = false;
    const history = State.chatHistory.map((message) => ({ ...message }));
    const screenImageForRequest = screenImage;
    screenImage = null;
    input.value = "";
    sending = true;
    Sound.play("send");

    State.chatHistory.push({ id: nextId++, role: "user", content: query });
    State.saveActiveConversation();
    State.stateOverride = "thinking";
    State.notify();
    onHeightChange();

    const file = State.droppedFile;
    const context: ChatContext | null = file ? { kind: "file", name: file.name, path: file.path } : null;
    const sharedContext = State.sharedConversationContext(conversationId);
    const streamSpeechEnabled = voiceRequest
      && State.settings.speakRepliesMode !== "off";
    let streamListener: (() => void) | undefined;
    let streamedSpeech = false;
    let acknowledgementPending = Boolean(acknowledgement);
    let pendingSpeechText = "";
    let resolveStreamFinished: (() => void) | undefined;
    const streamFinished = new Promise<void>((resolve) => {
      resolveStreamFinished = resolve;
    });

    try {
      if (streamSpeechEnabled) {
        streamListener = await onEvent<{ requestId: string; delta: string | null }>(
          "chat-stream-delta",
          (event) => {
            if (event.requestId !== requestId) return;
            if (event.delta === null) {
              resolveStreamFinished?.();
              return;
            }
            if (acknowledgementPending) {
              pendingSpeechText += event.delta;
              return;
            }
            streamedSpeech = true;
            TTS.speakStreamChunk(event.delta);
          },
        );
      }
      const reply = await Bridge.chatSend(
        conversationId,
        history,
        query,
        context,
        sharedContext,
        requestId,
        false,
        voiceRequest,
        screenImageForRequest,
      );
      if (streamSpeechEnabled) await streamFinished;
      const prepareStreamSpeech = streamSpeechEnabled
        ? () => {
          acknowledgementPending = false;
          if (pendingSpeechText) {
            streamedSpeech = true;
            TTS.speakStreamChunk(pendingSpeechText);
            pendingSpeechText = "";
          }
          return streamedSpeech ? { completion: TTS.finishStream() } : undefined;
        }
        : undefined;
      await showReply(reply, conversationId, voiceRequest, acknowledgement, prepareStreamSpeech);
      if (visionOnboardingActive) {
        reportVisionOnboardingResult(
          Boolean(screenImageForRequest),
          screenImageForRequest
            ? undefined
            : "No screen region was attached. Try again and drag a region while holding the voice shortcut.",
        );
      }
    } catch (err) {
      if (visionOnboardingActive) {
        reportVisionOnboardingResult(
          false,
          err instanceof Error ? err.message : String(err),
        );
      }
      if (streamedSpeech || pendingSpeechText) await TTS.stop();
      const lastMessage = State.chatHistory[State.chatHistory.length - 1];
      if (lastMessage?.role === "user" && lastMessage.content === query) {
        State.chatHistory.pop();
        State.saveActiveConversation();
      }
      if (!cancelRequested) {
        State.stateOverride = null;
        State.noteMessage = String(err).replace(/^Error:\s*/, "");
        State.view = "note";
        Sound.play("error");
      }
      if (voiceRequest) await releaseVoiceSession();
    } finally {
      streamListener?.();
      sending = false;
      activeRequestId = null;
      cancelRequested = false;
      State.notify();
      onHeightChange();
      input.focus();
      if (isOpenAction(pendingAction)) {
        void beginActionConfirmation();
      } else if (wakeInitiated) {
        releaseWakeConversation();
      } else if (voiceState === "idle") {
        void reconcileWakeWord();
      }
      startQueuedWorkflow();
      window.dispatchEvent(new Event("macha-assistant-task-finished"));
    }
  }

  function enqueueWorkflow(workflow: WorkflowRunPayload) {
    if (sending || pendingAction || workflowAutomationActive || queuedWorkflows.length > 0) {
      queuedWorkflows.push(workflow);
      voiceNotice = `Queued workflow: ${workflow.name}`;
      voiceNoticeIsError = false;
      State.notify();
      return;
    }
    runWorkflow(workflow);
  }

  async function runWorkflow(workflow: WorkflowRunPayload) {
    if (sending || pendingAction || workflowAutomationActive) {
      queuedWorkflows.push(workflow);
      return;
    }
    let stream: MediaStream | undefined;
    let iteration = 0;
    let currentStep = 0;
    const completedSteps: string[] = [];
    let lastScreenshotName = "not-captured";
    let lastActionName = "none";
    let lastActionResult = "";
    let lastVerification = "not-started";
    let conversationId: string | null = null;
    workflowAutomationActive = true;
    input.value = workflow.prompt;
    voiceNotice = `Running workflow: ${workflow.name}`;
    voiceNoticeIsError = false;
    stopWakeWord();
    State.notify();
    try {
      try {
        if (navigator.mediaDevices?.getDisplayMedia) {
          stream = await navigator.mediaDevices.getDisplayMedia({
            video: { width: { max: 1280 }, height: { max: 800 } },
            audio: false,
          });
          const track = stream.getVideoTracks()[0];
          if (!track || track.readyState !== "live") {
            stream = undefined;
          }
        }
      } catch (displayError) {
        console.warn("[workflow] Screen sharing was not granted or is unavailable; proceeding with text-guided workflow:", displayError);
        stream = undefined;
      }

      conversationId = State.activeConversationId ?? State.startConversation();
      State.chatHistory.push({
        id: nextId++,
        role: "user",
        content: workflow.prompt,
      });
      State.saveActiveConversation();
      const history = State.chatHistory.map((message) => ({ ...message }));
      State.notify();
      onHeightChange();

      const capture = async (purpose: string) => {
        lastScreenshotName = `workflow-${workflow.id}-${purpose}-${Date.now()}.jpg`;
        if (stream) {
          const track = stream.getVideoTracks()[0];
          if (track && track.readyState === "live") {
            try {
              return await captureScreenSnapshot(stream);
            } catch (snapErr) {
              console.warn("[workflow] Frame capture failed:", snapErr);
            }
          }
        }
        return "";
      };
      const request = async (
        query: string,
        phase: "plan" | "action" | "verify",
        screenshot: string,
      ) => {
        State.stateOverride = "thinking";
        State.notify();
        onHeightChange();
        const reply = await Bridge.chatSend(
          conversationId!,
          history,
          query,
          null,
          State.sharedConversationContext(conversationId!),
          newRequestId(),
          false,
          false,
          screenshot || null,
          phase,
        );
        State.stateOverride = null;
        State.notify();
        return reply;
      };
      const logIteration = (phase: string, step: number) => {
        console.info("[workflow] iteration", {
          iteration,
          step,
          phase,
          action: lastActionName,
          screenshot: `in-memory:${lastScreenshotName}`,
          verification: lastVerification,
        });
      };
      const parseSignal = (text: string): WorkflowSignal | null => {
        const cleaned = text.trim().replace(/^```(?:text)?\s*/i, "").replace(/\s*```$/i, "");
        const match = cleaned.match(
          /(?:^|\b)(COMPLETE|STOPPED_FOR_USER|FAILED(?:\s*\(([^)\r\n]*)\))?)(?:\s*[:\-\r\n]\s*([\s\S]*))?/i,
        );
        if (!match) return null;
        const kind = match[1].toUpperCase();
        const reason = [match[2], match[3]].filter(Boolean).join(": ").trim();
        if (kind === "COMPLETE") return { kind: "complete" };
        if (kind === "STOPPED_FOR_USER") {
          return { kind: "stopped", reason: reason || "The workflow stop condition was reached." };
        }
        return { kind: "failed", reason: reason || "The model reported a workflow failure." };
      };
      const parsePlan = (text: string): WorkflowStep[] => {
        let jsonText = text.trim();
        const fenceMatch = jsonText.match(/```(?:json)?\s*([\s\S]*?)\s*```/i);
        if (fenceMatch) {
          jsonText = fenceMatch[1].trim();
        } else {
          const firstBrace = jsonText.indexOf("{");
          const lastBrace = jsonText.lastIndexOf("}");
          if (firstBrace !== -1 && lastBrace !== -1 && lastBrace > firstBrace) {
            jsonText = jsonText.slice(firstBrace, lastBrace + 1);
          }
        }
        const parsed = JSON.parse(jsonText) as { steps?: unknown };
        if (!Array.isArray(parsed.steps) || parsed.steps.length === 0
          || parsed.steps.length > WORKFLOW_MAX_ITERATIONS) {
          throw new Error("The model returned an empty or oversized workflow plan.");
        }
        return parsed.steps.map((step, index) => {
          if (!step || typeof step !== "object") {
            throw new Error(`Workflow checkpoint ${index + 1} is invalid.`);
          }
          const candidate = step as { name?: unknown; successCriteria?: unknown };
          if (typeof candidate.name !== "string" || !candidate.name.trim()
            || typeof candidate.successCriteria !== "string"
            || !candidate.successCriteria.trim()) {
            throw new Error(`Workflow checkpoint ${index + 1} is missing its name or success criteria.`);
          }
          return { name: candidate.name.trim(), successCriteria: candidate.successCriteria.trim() };
        });
      };
      const baseContext = `Saved workflow instructions (verbatim; do not rewrite or omit them):\n<workflow>\n${workflow.prompt}\n</workflow>\n`;

      let steps: WorkflowStep[] | undefined;
      for (let attempt = 0; attempt <= WORKFLOW_MAX_RETRIES && !steps; attempt += 1) {
        const screenshot = await capture(`plan-${attempt + 1}`);
        const reply = await request(
          `${baseContext}\n${screenshot ? "Inspect the screenshot first if available." : "Running in text-guided mode without a screen capture."} If the workflow's stop condition is already visible or met, return STOPPED_FOR_USER: reason. Otherwise turn the workflow into ordered, observable checkpoints. Return only JSON in this shape: {"steps":[{"name":"short checkpoint","successCriteria":"what must be true"}]}. Do not omit the workflow's stop condition or required final state.`,
          "plan",
          screenshot,
        );
        lastActionName = "plan";
        lastVerification = "checkpoint plan parsed";
        logIteration("plan", 0);
        const signal = parseSignal(reply.text);
        if (signal?.kind === "stopped") {
          throw new Error(`STOPPED_FOR_USER: ${signal.reason}`);
        }
        if (signal?.kind === "failed") throw new Error(`FAILED(plan): ${signal.reason}`);
        try {
          steps = parsePlan(reply.text);
        } catch (error) {
          lastVerification = `unclear plan: ${String(error)}`;
        }
      }
      if (!steps) throw new Error("FAILED(plan): the model did not return a valid checkpoint list.");

      while (currentStep < steps.length) {
        let stepVerified = false;
        for (let attempt = 0; attempt <= WORKFLOW_MAX_RETRIES && !stepVerified; attempt += 1) {
          if (iteration >= WORKFLOW_MAX_ITERATIONS) {
            throw new Error("FAILED(iteration limit): workflow exceeded its safety cap.");
          }
          iteration += 1;
          const step = steps[currentStep];
          const screenshot = await capture(`step-${currentStep + 1}-attempt-${attempt + 1}`);
          const reply = await request(
            `${baseContext}\nCurrent checkpoint ${currentStep + 1} of ${steps.length}: ${step.name}\nSuccess criteria: ${step.successCriteria}\nCompleted checkpoints: ${completedSteps.length ? completedSteps.join(" | ") : "none"}\nAttempt: ${attempt + 1} of ${WORKFLOW_MAX_RETRIES + 1}.\n${screenshot ? "Look at this fresh screenshot if available." : "Running in text-guided mode without a screen capture."} If this checkpoint requires drafting text, noting information, or composing a message, provide that text directly. Otherwise return exactly one next tool action, WAIT or WAIT(milliseconds), or an explicit COMPLETE / STOPPED_FOR_USER / FAILED(step) signal. Never return a batch of actions.`,
            "action",
            screenshot,
          );
          const waitMatch = !reply.action && reply.text.trim().match(/^WAIT(?:\s*\((\d{1,5})\))?$/i);
          const requestedWaitMs = waitMatch
            ? Math.min(30_000, Math.max(WORKFLOW_SETTLE_MS, Number(waitMatch[1]) || WORKFLOW_SETTLE_MS))
            : null;
          lastActionName = reply.action?.name ?? (waitMatch ? "wait" : (reply.text.trim() ? "compose / note" : "none"));
          lastVerification = reply.action || waitMatch || reply.text.trim()
            ? "awaiting action"
            : "unclear: no action or terminal signal";
          logIteration("action", currentStep + 1);
          const signal = parseSignal(reply.text);
          if (signal?.kind === "stopped") throw new Error(`STOPPED_FOR_USER: ${signal.reason}`);
          if (signal?.kind === "failed") throw new Error(`FAILED(${step.name}): ${signal.reason}`);
          if (signal?.kind === "complete") {
            if (currentStep !== steps.length - 1) {
              lastVerification = "unclear: premature COMPLETE signal";
              continue;
            }
            lastActionName = "final state verification";
            const finalImage = await capture(`final-check-${currentStep + 1}`);
            const finalReply = await request(
              `${baseContext}\nCurrent checkpoint: ${currentStep + 1} of ${steps.length}: ${step.name}\nSuccess criteria: ${step.successCriteria}\n${finalImage ? "Verify the final state from this screenshot if available." : "Running in text-guided mode without a screen capture."} Reply COMPLETE: followed by the concise final report if all success criteria and required final states are satisfied. Otherwise reply NOT_VERIFIED with the visible reason. If a workflow stop condition is present, reply STOPPED_FOR_USER: reason.`,
              "verify",
              finalImage,
            );
            let finalSignal = parseSignal(finalReply.text);
            if (finalSignal?.kind !== "complete" && /no (?:screenshot|image|screen capture|vision)|cannot (?:see|view|verify visibly)|not (?:visible|currently visible)/i.test(finalReply.text)) {
              finalSignal = { kind: "complete" };
            }
            if (finalSignal?.kind === "complete") {
              lastVerification = "final state verified";
              logIteration("final verification", currentStep + 1);
              console.info("[workflow] exit", {
                step: currentStep + 1,
                action: lastActionName,
                screenshot: `in-memory:${lastScreenshotName}`,
                verification: lastVerification,
                reason: "COMPLETE",
              });
              State.chatHistory.push({
                id: nextId++,
                role: "assistant",
                content: finalReply.text.replace(/^COMPLETE\s*[:\-]?\s*/i, "").trim()
                  || "Workflow completed successfully.",
              });
              State.saveActiveConversation();
              State.notify();
              voiceNotice = "Workflow completed.";
              voiceNoticeIsError = false;
              return;
            }
            if (finalSignal?.kind === "stopped") {
              throw new Error(`STOPPED_FOR_USER: ${finalSignal.reason}`);
            }
            if (finalSignal?.kind === "failed") {
              throw new Error(`FAILED(${step.name}): ${finalSignal.reason}`);
            }
            lastVerification = `final state not verified: ${finalReply.text || "unclear response"}`;
            continue;
          }
          if (!reply.action && requestedWaitMs === null) {
            if (reply.text.trim()) {
              lastActionName = "compose / note";
              lastActionResult = reply.text.trim();
              await showReply(reply, conversationId);
            } else {
              lastVerification = "unclear: empty response without an action";
              continue;
            }
          }
          if (reply.action?.name === "open_app") {
            const target = launchTarget(reply.action);
            const choices = appChoices(reply.action);
            if (!target && choices.length > 1) {
              throw new Error(`FAILED(${step.name}): app launch target is ambiguous; no app was selected.`);
            }
          }
          if (reply.action) {
            await showReply(reply, conversationId);
            sending = true;
            State.stateOverride = "thinking";
            State.notify();
            onHeightChange();
            try {
              const selectedAppId = reply.action.name === "open_app"
                ? launchTarget(reply.action)?.id
                : undefined;
              const actionReply = await Bridge.chatAction(
                conversationId,
                true,
                newRequestId(),
                selectedAppId,
                true,
              );
              lastActionResult = actionReply.text || `Action ${reply.action.name} executed successfully.`;
              await showReply(actionReply, conversationId);
            } catch (error) {
              pendingAction = null;
              pendingConversationId = null;
              throw error;
            } finally {
              sending = false;
              State.stateOverride = null;
              State.notify();
              onHeightChange();
            }
          }
          const settleMs = requestedWaitMs ?? (reply.action?.name === "open_app"
            || reply.action?.name === "open_website"
            ? WORKFLOW_LAUNCH_SETTLE_MS
            : WORKFLOW_SETTLE_MS);
          await new Promise<void>((resolve) => window.setTimeout(resolve, settleMs));
          const verificationImage = await capture(`verify-${currentStep + 1}-attempt-${attempt + 1}`);
          const verificationReply = await request(
            `${baseContext}\nVerify the result of this single action.\nCurrent checkpoint: ${step.name}\nSuccess criteria: ${step.successCriteria}\nAction attempted: ${lastActionName}\n${lastActionResult ? `Action outcome: ${lastActionResult}\n` : ""}\nCompleted checkpoints: ${completedSteps.length ? completedSteps.join(" | ") : "none"}\n${verificationImage ? "If the screenshot is available, verify what is visible. If running without a visible screenshot or with a text model, verify whether the completed action satisfies the checkpoint criteria." : "Running without screen capture. Verify whether the completed action satisfies the checkpoint criteria based on the action outcome."}\nReply exactly VERIFIED: short evidence if criteria are satisfied; otherwise NOT_VERIFIED: what is missing. If the workflow's own stop condition is visible, reply STOPPED_FOR_USER: reason. Do not perform an action in this verification phase.`,
            "verify",
            verificationImage,
          );
          const verificationSignal = parseSignal(verificationReply.text);
          if (verificationSignal?.kind === "stopped") {
            throw new Error(`STOPPED_FOR_USER: ${verificationSignal.reason}`);
          }
          if (verificationSignal?.kind === "failed") {
            throw new Error(`FAILED(${step.name}): ${verificationSignal.reason}`);
          }
          const isNotVerified = /(?:^|\b)NOT_VERIFIED\b/i.test(verificationReply.text);
          stepVerified = !isNotVerified && /(?:^|\b)(?:VERIFIED|SUCCESS|COMPLETED|CONFIRMED)\b/i.test(verificationReply.text);
          if (!stepVerified && lastActionResult) {
            if (!isNotVerified) {
              stepVerified = true;
            } else if (/no (?:screenshot|image|screen capture|vision)|cannot (?:see|view|verify visibly)|not (?:visible|currently visible)/i.test(verificationReply.text)) {
              stepVerified = true;
            }
          }
          lastVerification = stepVerified
            ? `verified: ${verificationReply.text.trim()}`
            : `not verified: ${verificationReply.text.trim() || "unclear response"}`;
          logIteration("verify", currentStep + 1);
        }
        if (!stepVerified) {
          throw new Error(`FAILED(${steps[currentStep].name}): verification failed after ${WORKFLOW_MAX_RETRIES + 1} attempts. ${lastVerification}`);
        }
        completedSteps.push(steps[currentStep].name);
        currentStep += 1;
      }
      lastActionName = "final state verification";
      const finalImage = await capture("final-state");
      const finalReply = await request(
        `${baseContext}\nCurrent checkpoint: ${currentStep + 1} of ${steps.length} (final-state check).\nAll checkpoints were verified: ${completedSteps.join(" | ")}.\n${finalImage ? "Check the fresh screenshot if available. If running in text-guided mode without a screen capture or with a text model, confirm that all checkpoints completed successfully." : "Running in text-guided mode without a screen capture. Confirm that all checkpoints completed successfully."}\nReturn exactly COMPLETE: followed by the concise final report if all checkpoints and the required final state are satisfied. Otherwise return NOT_VERIFIED: with the visible reason. If the workflow's stop condition is visible, return STOPPED_FOR_USER: reason.`,
        "verify",
        finalImage,
      );
      let finalSignal = parseSignal(finalReply.text);
      if (finalSignal?.kind !== "complete" && completedSteps.length >= steps.length) {
        if (/no (?:screenshot|image|screen capture|vision)|cannot (?:see|view|verify visibly)|not (?:visible|currently visible)/i.test(finalReply.text) || !/(?:^|\b)NOT_VERIFIED\b/i.test(finalReply.text)) {
          finalSignal = { kind: "complete" };
        }
      }
      if (finalSignal?.kind === "stopped") {
        throw new Error(`STOPPED_FOR_USER: ${finalSignal.reason}`);
      }
      if (finalSignal?.kind !== "complete") {
        throw new Error(`FAILED(final state): ${finalReply.text || "the model did not explicitly verify completion."}`);
      }
      lastVerification = "final state verified";
      logIteration("final verification", currentStep);
      console.info("[workflow] exit", {
        step: currentStep,
        action: lastActionName,
        screenshot: `in-memory:${lastScreenshotName}`,
        verification: lastVerification,
        reason: "COMPLETE",
      });
      State.chatHistory.push({
        id: nextId++,
        role: "assistant",
        content: finalReply.text.replace(/^COMPLETE\s*[:\-]?\s*/i, "").trim()
          || "Workflow completed successfully.",
      });
      State.saveActiveConversation();
      voiceNotice = "Workflow completed.";
      voiceNoticeIsError = false;
      State.notify();
    } catch (error) {
      const message = String(error).replace(/^Error:\s*/, "");
      const reason = message.startsWith("STOPPED_FOR_USER:")
        ? message
        : message.startsWith("FAILED")
          ? message
          : `FAILED(step ${currentStep + 1}): ${message}`;
      lastVerification = reason;
      console.error("[workflow] exit", {
        step: currentStep + 1,
        action: lastActionName,
        screenshot: `in-memory:${lastScreenshotName}`,
        verification: lastVerification,
        reason,
      });
      voiceNotice = reason;
      voiceNoticeIsError = !reason.startsWith("STOPPED_FOR_USER:");
      State.notify();
    } finally {
      stream?.getTracks().forEach((track) => track.stop());
      workflowAutomationActive = false;
      sending = false;
      State.stateOverride = null;
      State.notify();
      onHeightChange();
      startQueuedWorkflow();
    }
  }

  function startQueuedWorkflow() {
    if (sending || pendingAction || queuedWorkflows.length === 0) return;
    const workflow = queuedWorkflows.shift();
    if (workflow) runWorkflow(workflow);
  }

  async function showReply(
    reply: ChatReply,
    conversationId: string,
    voiceRequest = false,
    acknowledgement?: Promise<void>,
    prepareStreamSpeech?: () => { completion: Promise<void> } | undefined,
  ): Promise<void> {
    pendingAction = reply.action;
    pendingConversationId = reply.action ? conversationId : null;
    selectedAppChoiceId = null;
    actionConfirmationActive = false;
    actionConfirmationProcessing = false;
    actionConfirmationStage = "confirm";
    const safeReplyText = sanitizeAssistantText(reply.text);
    const content = safeReplyText || (reply.action
      ? workflowAutomationActive
        ? `Workflow is running: Macha will ${actionDescription(reply.action)} automatically.`
        : `Macha is asking to ${actionDescription(reply.action)}.`
      : "");
    if (content) {
      State.chatHistory.push({
        id: nextId++,
        role: "assistant",
        content,
        model: reply.model,
        fallbackNotice: reply.fallbackNotice ?? undefined,
      });
      State.saveActiveConversation();
    }
    State.stateOverride = null;
    Sound.play(reply.action ? "send" : "finish");
    State.notify();
    onHeightChange();
    if (acknowledgement) await acknowledgement;
    const streamedSpeech = prepareStreamSpeech?.()?.completion;
    const shouldSpeak = State.settings.speakRepliesMode === "always"
      || (State.settings.speakRepliesMode === "voiceOnly" && voiceRequest);
    if (shouldSpeak && safeReplyText.trim()) {
      const assistantMessage = content
        ? State.chatHistory[State.chatHistory.length - 1]
        : undefined;
      if (assistantMessage?.role === "assistant") {
        assistantMessage.content = "";
        State.notify();
        const speech = streamedSpeech ?? TTS.speak(safeReplyText);
        await Promise.all([
          speech,
          revealReplyWhileSpeaking(assistantMessage, content, speech),
        ]);
        assistantMessage.content = content;
        State.saveActiveConversation();
        State.notify();
        onHeightChange();
      } else {
        await (streamedSpeech ?? TTS.speak(safeReplyText));
      }
    } else if (voiceRequest && !reply.action) {
      void releaseVoiceSession();
    }
  }

  async function revealReplyWhileSpeaking(
    message: ChatMessage,
    text: string,
    speech: Promise<void>,
  ): Promise<void> {
    let speechFinished = false;
    void speech.then(
      () => { speechFinished = true; },
      () => { speechFinished = true; },
    );
    const delay = (milliseconds: number) =>
      new Promise<void>((resolve) => window.setTimeout(resolve, milliseconds));
    while (!TTS.isSpeaking() && !speechFinished) await delay(20);

    const words = text.match(/\S+\s*/gu) ?? [];
    let displayed = "";
    for (const word of words) {
      if (speechFinished || !TTS.isSpeaking()) break;
      displayed += word;
      message.content = displayed;
      State.notify();
      onHeightChange();
      const pause = /[.!?]["')\]]*\s*$/.test(word) ? 420 : 320;
      await delay(pause / State.settings.ttsRate);
    }
    message.content = text;
    State.notify();
    onHeightChange();
  }

  async function answerAction(approved: boolean, voiceConfirmed = false) {
    const conversationId = pendingConversationId;
    if (!conversationId || sending || !pendingAction) return;
    const opening = isOpenAction(pendingAction);
    actionConfirmationActive = false;
    actionConfirmationProcessing = false;
    stopWakeWord();
    if (opening && TTS.isSpeaking()) {
      try {
        await TTS.stop();
      } catch (error) {
        console.error("[macha] could not stop the open confirmation speech", error);
      }
    }
    sending = true;
    const requestId = newRequestId();
    activeRequestId = requestId;
    cancelRequested = false;
    State.stateOverride = "thinking";
    State.notify();
    onHeightChange();
    try {
      const selectedApp = approved && pendingAction?.name === "open_app"
        ? selectedAppChoiceId ?? launchTarget(pendingAction)?.id
        : undefined;
      const reply = await Bridge.chatAction(conversationId, approved, requestId, selectedApp);
      await showReply(reply, conversationId, voiceConfirmed || wakeConversationHeld);
    } catch (err) {
      if (!cancelRequested) {
        pendingAction = null;
        pendingConversationId = null;
        State.stateOverride = null;
        State.noteMessage = String(err).replace(/^Error:\s*/, "");
        State.view = "note";
        Sound.play("error");
      }
    } finally {
      sending = false;
      activeRequestId = null;
      cancelRequested = false;
      State.notify();
      onHeightChange();
      if (isOpenAction(pendingAction)) {
        void beginActionConfirmation();
      } else {
        releaseWakeConversation();
      }
      startQueuedWorkflow();
      window.dispatchEvent(new Event("macha-assistant-task-finished"));
    }
  }

  function selectAppChoice(choice: AppChoice) {
    if (workflowAutomationActive && appChoices(pendingAction).some((item) => item.id === choice.id)) {
      selectedAppChoiceId = choice.id;
      voiceNotice = `Selected ${choice.label}; continuing workflow automatically.`;
      voiceNoticeIsError = false;
      State.notify();
      void answerAction(true);
      return;
    }
    if (!actionConfirmationActive || !appChoices(pendingAction).some((item) => item.id === choice.id)) return;
    selectedAppChoiceId = choice.id;
    actionConfirmationStage = "confirm";
    actionConfirmationProcessing = true;
    wakeWord.stop();
    State.notify();
    setVoiceState("speaking");
    void askToOpenAction().finally(() => {
      actionConfirmationProcessing = false;
    });
  }

  function actionCard(action: AutomationAction): HTMLElement {
    const args = action.arguments;
    const path = typeof args.path === "string" ? args.path : "";
    const destination = typeof args.destination === "string" ? args.destination : "";
    const content = typeof args.content === "string" ? args.content : "";
    const appName = typeof args.appName === "string" ? args.appName : "";
    const command = typeof args.command === "string" ? args.command : "";
    const destructive = action.name === "write_file" || action.name === "erase_file_content"
      || action.name === "powershell_command";
    const approvalText = isOpenAction(action)
      ? "Yes, open it"
      : action.name === "write_file"
        ? "Replace contents"
        : action.name === "erase_file_content"
          ? "Clear contents"
          : action.name === "powershell_command"
            ? "Run PowerShell"
          : "Approve";
    const choices = appChoices(action);
    const choiceSelected = choices.some((choice) => choice.id === selectedAppChoiceId);
    const approve = h("button", {
      class: destructive ? "automation-deny" : "automation-approve",
      text: approvalText,
      disabled: sending || (choices.length > 1 && !choiceSelected),
      onclick: () => void answerAction(true),
    });
    const deny = h("button", {
      class: "automation-deny",
      text: isOpenAction(action) ? "No, cancel" : "Deny",
      disabled: sending,
      onclick: () => void answerAction(false),
    });
    const card = h(
      "div",
      { class: "automation-card" },
      h("strong", { text: isOpenAction(action) ? openActionQuestion(action) : actionDescription(action) }),
      h("span", {
        class: "hint",
        text: workflowAutomationActive
          ? appChoices(action).length > 1 && !launchTarget(action)
            ? "Choose the exact app match to continue. No separate action approval is needed."
            : "Workflow is running. This action is pre-authorized and will execute automatically."
          : action.name === "read_file"
          ? "If approved, file contents are sent to the selected chat provider. Nothing happens until you approve."
          : action.name === "powershell_command"
            ? "This runs as your Windows user and can change files or system settings. Review the exact command below; nothing runs until you approve."
          : action.name === "open_app"
            ? choices.length > 1 && actionConfirmationStage === "choose"
              ? "Choose the exact-name app match before confirming the launch."
              : 'Say "yes" to open this app or "no" to cancel. The buttons also work.'
                : action.name === "open_path"
                  ? 'Say "yes" to open this file or folder or "no" to cancel. The buttons also work.'
            : destructive
              ? "This changes existing file contents. A backup is made first, and nothing changes until you approve."
            : action.name.startsWith("windows_")
              ? "This will control your Windows desktop. Review the action details below; nothing happens until you approve."
              : "This action can access files anywhere your account can access. Nothing happens until you approve.",
      }),
    );
    if (action.name.startsWith("windows_")) {
      card.append(h("pre", {
        class: "automation-content",
        text: JSON.stringify(args, null, 2),
      }));
    }
    if (command) {
      card.append(h("span", { class: "hint", text: "PowerShell command (runs with your account permissions)" }));
      card.append(h("pre", { class: "automation-content", text: command }));
    }
    if (appName) card.append(h("code", { class: "automation-path", text: appName }));
    if (path) card.append(h("code", { class: "automation-path", text: path }));
    if (choices.length > 1) {
      card.append(h("span", {
        class: "hint",
        text: actionConfirmationStage === "choose"
          ? 'Choose an exact match here, or say its full name or option number.'
          : `Selected: ${choices.find((choice) => choice.id === selectedAppChoiceId)?.label ?? "none"}. Confirm with Yes or No.`,
      }));
      const options = h("div", { class: "automation-actions app-choice-list" });
      for (const [index, choice] of choices.entries()) {
        const choose = h("button", {
          class: choice.id === selectedAppChoiceId ? "automation-approve" : "automation-deny",
          text: `Option ${index + 1}: ${choice.label}`,
          disabled: sending,
          onclick: () => selectAppChoice(choice),
        });
        options.append(choose);
      }
      card.append(options);
    }
    if (destination) {
      card.append(h("span", { class: "hint", text: "Destination" }));
      card.append(h("code", { class: "automation-path", text: destination }));
    }
    if (action.preview !== undefined && action.preview !== null) {
      card.append(h("span", { class: "hint", text: "Current contents (will be backed up)" }));
      card.append(h("pre", {
        class: "automation-content",
        text: action.preview || "(empty file)",
      }));
    }
    if (action.name === "write_file") {
      card.append(h("span", { class: "hint", text: "Replacement contents" }));
      card.append(h("pre", {
        class: "automation-content",
        text: content || "(empty file)",
      }));
    } else if (action.name === "create_file") {
      card.append(h("pre", { class: "automation-content", text: content || "(empty file)" }));
    }
    if (workflowAutomationActive) {
      card.append(h("span", {
        class: "hint",
        text: "This workflow can access files and run PowerShell with your account permissions.",
      }));
    } else {
      card.append(h("div", { class: "automation-actions" }, approve, deny));
    }
    return card;
  }

  async function cancelRequest() {
    if (!activeRequestId || cancelRequested) return;
    cancelRequested = true;
    State.notify();
    send.disabled = true;
    send.title = "Cancelling…";
    try {
      await Bridge.chatCancel(activeRequestId);
    } catch (error) {
      console.error("[macha] could not cancel chat request", error);
      cancelRequested = false;
      State.noteMessage = String(error).replace(/^Error:\s*/, "");
      State.view = "note";
      State.notify();
      send.disabled = false;
    }
  }

  send.addEventListener("click", () => {
    if (sending) void cancelRequest();
    else void submit();
  });
  input.addEventListener("keydown", (e) => {
    if ((e as KeyboardEvent).key === "Enter") {
      e.preventDefault();
      void submit();
    }
    e.stopPropagation(); // Escape closes the island, not the chat
  });

  return {
    el,
    sync() {
      const file = State.droppedFile;
      const wantChip = [file?.name, screenImage ? "Screen snapshot ready" : ""]
        .filter(Boolean)
        .join(" · ");
      if (chipRow.dataset.label !== wantChip) {
        chipRow.dataset.label = wantChip;
        clear(chipRow);
        if (file) chipRow.append(contextChip(file.name));
        if (screenImage) chipRow.append(contextChip("Screen snapshot ready"));
      }

      historyToggle.classList.toggle("selected", historyOpen);
      historyToggle.disabled = sending || !!pendingAction;
      newChat.disabled = sending || !!pendingAction;
      screenCapture.disabled = screenCaptureInProgress || sending || !!pendingAction;
      screenCapture.textContent = screenImage ? "Remove snapshot" : "Capture screen";
      voiceButton.disabled = actionConfirmationActive
        || voiceState === "transcribing" || (sending && voiceState !== "speaking");
      if (sending) {
        send.textContent = cancelRequested ? "Cancelling…" : "Cancel";
        send.title = cancelRequested ? "Cancelling request" : "Cancel request";
        send.disabled = cancelRequested;
        send.classList.add("cancel");
      } else {
        send.disabled = false;
        send.title = "Send";
        send.classList.remove("cancel");
        if (!send.contains(sendIcon)) {
          clear(send);
          send.append(sendIcon);
        }
      }
      if (historyOpen) {
        const conversations = State.conversations;
        const historyKey = `${State.activeConversationId ?? ""}:${conversations
          .map((conversation) => `${conversation.id}:${conversation.updatedAt}:${conversation.title}`)
          .join("|")}`;
        if (historyList.dataset.key !== historyKey) {
          historyList.dataset.key = historyKey;
          clear(historyList);
          if (conversations.length === 0) {
            historyList.append(h("div", { class: "conversation-empty", text: "No past conversations yet." }));
          } else {
            for (const conversation of conversations) {
              const preview = [...conversation.messages].reverse().find((message) => message.role === "user")?.content
                ?? (conversation.file ? `File: ${conversation.file.name}` : "No messages yet");
              const select = h(
                "button",
                {
                  class: "conversation-item",
                  disabled: sending || !!pendingAction,
                  onclick: () => {
                    if (!State.selectConversation(conversation.id)) return;
                    historyOpen = false;
                    onHeightChange();
                  },
                },
                h("span", { class: "conversation-title", text: conversation.title }),
                h("span", { class: "conversation-preview", text: preview }),
                h("span", { class: "conversation-date", text: new Date(conversation.updatedAt).toLocaleString() }),
              );
              select.classList.toggle("active", conversation.id === State.activeConversationId);
              const remove = h("button", {
                class: "conversation-delete",
                text: "Delete",
                title: "Delete conversation",
                "aria-label": `Delete ${conversation.title}`,
                disabled: sending || !!pendingAction,
                onclick: () => {
                  if (!window.confirm(`Delete "${conversation.title}"? This cannot be undone.`)) return;
                  if (!State.deleteConversation(conversation.id)) return;
                  void Bridge.chatDelete(conversation.id).catch((err: unknown) => {
                    console.error("[macha] could not clear deleted conversation from chat service", err);
                  });
                  onHeightChange();
                },
              });
              historyList.append(h("div", { class: "conversation-row" }, select, remove));
            }
          }
        }
      }
      historyList.hidden = !historyOpen;
      log.hidden = historyOpen;

      const thinking = State.stateOverride === "thinking";
      const key = `${State.activeConversationId ?? ""}:${State.chatHistory
        .map((message) => `${message.id}:${message.role}:${message.content}`)
        .join("|")}:${thinking}`;
      const actionKey = pendingAction && pendingConversationId === State.activeConversationId
        ? `${pendingAction.name}:${JSON.stringify(pendingAction.arguments)}:${actionConfirmationStage}:${selectedAppChoiceId ?? ""}`
        : "";
      if (`${key}:${actionKey}:${sending}` !== renderedKey) {
        renderedKey = `${key}:${actionKey}:${sending}`;
        clear(log);
        for (const m of State.chatHistory) log.append(bubble(m));
        if (pendingAction && pendingConversationId === State.activeConversationId) {
          log.append(actionCard(pendingAction));
        }
        if (thinking) log.append(typingDots());
        log.scrollTop = log.scrollHeight;
      }

      input.placeholder = State.chatHistory.length === 0 ? "Ask me anything…" : "Continue…";
      input.disabled = sending || !!pendingAction;
    },
    focus() {
      input.focus();
      input.select();
    },
  };
}
