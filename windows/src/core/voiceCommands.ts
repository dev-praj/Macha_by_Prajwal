function normalizeCommand(text: string): string {
  return text.toLowerCase().replace(/[^a-z0-9]+/g, " ").trim();
}

const VISION_MODE_ACTIVATION = /\b(?:activate|start|open|enable)\s+(?:the\s+)?vision\s+mode\b/;
const VISION_MODE_ACTIVATION_PREFIX =
  /^(?:(?:and|then)\s+)?(?:(?:tell\s+(?:it|me|you)\s+to)\s+)?(?:activate|start|open|enable)(?:\s+the)?(?:\s+vision)?$/;
const COMMAND_CONNECTOR_PREFIX =
  /^(?:(?:and|then)\s+)?tell(?:\s+(?:it|me|you))?(?:\s+to)?$/;

export function canStartHoldToTalk(
  voiceState: "idle" | "wake" | "listening" | "transcribing" | "speaking",
  blocked: boolean,
): boolean {
  return !blocked && (voiceState === "idle" || voiceState === "wake");
}

export function isVisionModeActivation(transcript: string): boolean {
  return VISION_MODE_ACTIVATION.test(normalizeCommand(transcript));
}

export function isPartialVisionModeActivation(command: string): boolean {
  const normalized = normalizeCommand(command);
  return VISION_MODE_ACTIVATION_PREFIX.test(normalized)
    || COMMAND_CONNECTOR_PREFIX.test(normalized);
}
