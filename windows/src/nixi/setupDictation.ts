export function isDictationHoldKey(event: Pick<KeyboardEvent, "code" | "key">): boolean {
  return event.code === "AltRight" || event.key === "AltGraph";
}

export function insertTextAtCursor(input: HTMLTextAreaElement, transcript: string): void {
  const start = input.selectionStart;
  const end = input.selectionEnd;
  input.setRangeText(transcript, start, end, "end");
  input.dispatchEvent(new Event("input", { bubbles: true }));
  input.focus();
}

export function isDictationSetupTestPayload(payload: unknown): boolean {
  return typeof payload === "object"
    && payload !== null
    && "setupTest" in payload
    && (payload as { setupTest?: unknown }).setupTest === true;
}
