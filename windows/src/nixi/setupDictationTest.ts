import { onNixiDictateStart, onNixiDictateStop } from "../core/bridge";
import { NixiAudioRecorder } from "./audioRecorder";
import { insertTextAtCursor, isDictationHoldKey } from "./setupDictation";
import { transcribeAudio } from "./stt";

interface SetupDictationOptions {
  input: HTMLTextAreaElement;
  status: HTMLElement;
  visual?: HTMLElement | null;
}

export function registerSetupDictationTest(options: SetupDictationOptions): () => void {
  const recorder = new NixiAudioRecorder();
  let recordingGeneration = 0;
  let holding = false;
  let startedFromKey = false;
  const idleStatus = options.status.textContent
    || "Click the field, hold Right Alt while speaking, and release. The test transcription appears here.";

  const setListeningVisual = (listening: boolean) => {
    options.visual?.classList.toggle("is-listening", listening);
  };

  const start = () => {
    if (holding) return;
    holding = true;
    const session = ++recordingGeneration;
    options.status.textContent = "Listening… Keep holding Right Alt, then release to transcribe.";
    setListeningVisual(true);
    void recorder.start("dictate").then((started) => {
      if (started || session !== recordingGeneration) return;
      holding = false;
      startedFromKey = false;
      setListeningVisual(false);
      options.status.textContent = "Microphone unavailable. Allow microphone access, then try again.";
    });
  };

  const stop = () => {
    if (!holding) return;
    holding = false;
    startedFromKey = false;
    const session = ++recordingGeneration;
    setListeningVisual(false);
    options.status.textContent = "Transcribing…";
    void (async () => {
      const result = await recorder.stop();
      if (session !== recordingGeneration) return;
      if (!result || result.blob.size === 0) {
        options.status.textContent =
          "No dictation audio was captured. Click the field, hold Right Alt while speaking, then release.";
        return;
      }
      try {
        const transcript = await transcribeAudio(result.blob);
        if (session !== recordingGeneration) return;
        if (!transcript) {
          options.status.textContent = "No speech was detected. Try speaking a little longer.";
          return;
        }
        insertTextAtCursor(options.input, transcript);
      } catch (error) {
        if (session !== recordingGeneration) return;
        options.status.textContent = `Dictation test failed: ${
          error instanceof Error ? error.message : String(error)
        }`;
        console.error("[macha] setup dictation transcription failed", error);
      }
    })();
  };

  const onKeyDown = (event: KeyboardEvent) => {
    if (!isDictationHoldKey(event) || event.repeat) return;
    event.preventDefault();
    startedFromKey = true;
    start();
  };

  const onKeyUp = (event: KeyboardEvent) => {
    if (!isDictationHoldKey(event)) return;
    event.preventDefault();
    stop();
  };

  const onBlur = () => {
    if (holding) stop();
  };

  options.input.addEventListener("keydown", onKeyDown);
  window.addEventListener("keydown", onKeyDown, true);
  window.addEventListener("keyup", onKeyUp, true);
  window.addEventListener("blur", onBlur);

  const unlistenStart = onNixiDictateStart(() => {
    start();
  }).catch((error: unknown) => {
    console.error("[macha] could not listen for setup dictation start", error);
    return () => {};
  });
  const unlistenStop = onNixiDictateStop(() => {
    if (startedFromKey) return;
    stop();
  }).catch((error: unknown) => {
    console.error("[macha] could not listen for setup dictation stop", error);
    return () => {};
  });

  return () => {
    recordingGeneration += 1;
    holding = false;
    startedFromKey = false;
    setListeningVisual(false);
    recorder.destroy();
    options.input.removeEventListener("keydown", onKeyDown);
    window.removeEventListener("keydown", onKeyDown, true);
    window.removeEventListener("keyup", onKeyUp, true);
    window.removeEventListener("blur", onBlur);
    void unlistenStart.then((unlisten) => unlisten());
    void unlistenStop.then((unlisten) => unlisten());
    if (options.status.textContent?.startsWith("Listening") || options.status.textContent === "Transcribing…") {
      options.status.textContent = idleStatus;
    }
  };
}
