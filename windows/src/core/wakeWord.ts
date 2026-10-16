const STT_ENDPOINT = "http://127.0.0.1:5005/transcribe";
const STT_HEALTH_ENDPOINT = "http://127.0.0.1:5005/health";
const STT_TIMEOUT_MS = 15_000;
const STT_STARTUP_TIMEOUT_MS = 20_000;
const STT_STARTUP_RETRY_MS = 250;
const DEFAULT_SPEECH_THRESHOLD = 0.006;
const PRE_ROLL_MS = 280;
const SILENCE_MS = 220;
const WAKE_SILENCE_MS = 420;
const WAKE_MIN_SPEECH_MS = 180;
const WAKE_TRIGGER_FRAMES = 2;
const MAX_SPEECH_MS = 12_000;
const CALIBRATION_MS = 5000;
const FILLER_WORDS = new Set(["um", "uh", "oh", "ah", "er", "okay", "ok", "so", "well"]);
const GREETINGS = new Set(["hey", "hay", "hai", "hi"]);
const NAMES = new Set([
  "macha", "matcha", "masha", "machaa", "maacha", "maccha", "macho", "mocha",
  "marcha", "mascha", "matcher", "macher", "mochi",
]);
const SPLIT_NAMES: readonly (readonly [string, string])[] = [
  ["ma", "cha"],
  ["my", "cha"],
  ["mo", "chi"],
];
const PARTIAL_NAMES = new Set(["ma", "my", "mo", "mach", "mat", "mash"]);
const WAKE_MIN_CONFIDENCE = -1.5;
const WAKE_MAX_NO_SPEECH = 0.5;
const WAKE_MAX_COMPRESSION = 2.4;
const VOICE_MIN_CONFIDENCE = -1.5;
const VOICE_MAX_NO_SPEECH = 0.6;

export class SpeechServiceUnavailableError extends Error {
  constructor() {
    super("Local speech service is not ready. Start Coucou with the voice service running.");
    this.name = "SpeechServiceUnavailableError";
  }
}

async function waitForSttReady(isCurrentRun: () => boolean): Promise<void> {
  const deadline = performance.now() + STT_STARTUP_TIMEOUT_MS;
  while (isCurrentRun() && performance.now() < deadline) {
    try {
      const response = await fetch(STT_HEALTH_ENDPOINT, { signal: AbortSignal.timeout(1_000) });
      if (response.ok) return;
    } catch {
      // The backend may still be loading the Whisper model. Retry briefly.
    }
    await new Promise((resolve) => window.setTimeout(resolve, STT_STARTUP_RETRY_MS));
  }
  if (isCurrentRun()) throw new SpeechServiceUnavailableError();
}

function normalizeWakeWord(value: string): string {
  return value.normalize("NFD").replace(/\p{Diacritic}/gu, "").toLocaleLowerCase();
}

function wakeWords(value: string): string[] {
  return normalizeWakeWord(value).match(/[\p{L}\p{N}]+/gu) ?? [];
}

function configuredWakeWords(pronunciation: string): [string, string] {
  const configured = wakeWords(pronunciation);
  let greeting = configured[0] ?? "hey";
  let name = configured[1] ?? "macha";
  if (name === "ma" && configured[2] === "cha") name = "macha";
  // Older builds stored loose mishears ("a", "mark") as the phrase itself.
  if (!GREETINGS.has(greeting)) greeting = "hey";
  if (name === "mark") name = "macha";
  return [greeting, name];
}

function isGreeting(word: string, expected: string): boolean {
  if (GREETINGS.has(expected)) return GREETINGS.has(word) || word === "a";
  return word === expected;
}

function differsByOneEdit(left: string, right: string): boolean {
  if (Math.abs(left.length - right.length) > 1) return false;
  let leftIndex = 0;
  let rightIndex = 0;
  let edits = 0;
  while (leftIndex < left.length && rightIndex < right.length) {
    if (left[leftIndex] === right[rightIndex]) {
      leftIndex += 1;
      rightIndex += 1;
      continue;
    }
    edits += 1;
    if (edits > 1) return false;
    if (
      left.length === right.length
      && left[leftIndex] === right[rightIndex + 1]
      && left[leftIndex + 1] === right[rightIndex]
    ) {
      leftIndex += 2;
      rightIndex += 2;
    } else if (left.length > right.length) {
      leftIndex += 1;
    } else if (right.length > left.length) {
      rightIndex += 1;
    } else {
      leftIndex += 1;
      rightIndex += 1;
    }
  }
  return edits + (left.length - leftIndex) + (right.length - rightIndex) <= 1;
}

function isKnownName(word: string): boolean {
  return NAMES.has(word) || [...NAMES].some((variant) => differsByOneEdit(word, variant));
}

function isName(word: string, expected: string): boolean {
  if (NAMES.has(expected)) return isKnownName(word);
  return word === expected;
}

function splitGluedWake(word: string, expectedGreeting: string, expectedName: string): boolean {
  if (!GREETINGS.has(expectedGreeting) || !NAMES.has(expectedName)) return false;
  for (const greeting of ["hey", "hay", "hai", "hi"]) {
    if (!word.startsWith(greeting) || word.length <= greeting.length) continue;
    if (isName(word.slice(greeting.length), expectedName)) return true;
  }
  return false;
}

function contentTokens(transcript: string): RegExpMatchArray[] {
  const tokens = [...transcript.matchAll(/[\p{L}\p{N}]+/gu)];
  let start = 0;
  while (start < tokens.length && FILLER_WORDS.has(normalizeWakeWord(tokens[start][0]))) {
    start += 1;
  }
  return tokens.slice(start);
}

function nameEndIndex(words: string[], start: number, expectedName: string): number | null {
  if (start >= words.length) return null;
  if (isName(words[start], expectedName)) return start;
  const next = words[start + 1];
  if (
    NAMES.has(expectedName)
    && next
    && (
      SPLIT_NAMES.some(([left, right]) => words[start] === left && next === right)
      || isName(`${words[start]}${next}`, expectedName)
    )
  ) {
    return start + 1;
  }
  return null;
}

function isWakeGreeting(word: string, expected: string): boolean {
  return isGreeting(word, expected);
}

export interface WakeMatch {
  command: string;
}

export function matchWakePhrase(transcript: string, pronunciation = "Hey Macha"): WakeMatch | null {
  const tokens = contentTokens(transcript);
  if (!tokens.length) return null;
  const [expectedGreeting, expectedName] = configuredWakeWords(pronunciation);
  const words = tokens.map((token) => normalizeWakeWord(token[0]));
  let nameEnd: number | null = null;
  if (splitGluedWake(words[0], expectedGreeting, expectedName)) {
    nameEnd = 0;
  } else if (isWakeGreeting(words[0], expectedGreeting)) {
    nameEnd = nameEndIndex(words, 1, expectedName);
  } else {
    nameEnd = nameEndIndex(words, 0, expectedName);
  }
  if (nameEnd === null) return null;
  const phraseEnd = tokens[nameEnd].index! + tokens[nameEnd][0].length;
  return { command: transcript.slice(phraseEnd).replace(/^[\s\p{P}\p{S}]+/u, "").trim() };
}

export function extractWakeCommand(
  transcript: string,
  pronunciation = "Hey Macha",
): string | null {
  return matchWakePhrase(transcript, pronunciation)?.command ?? null;
}

export function isPartialWakePhrase(transcript: string, pronunciation = "Hey Macha"): boolean {
  const tokens = contentTokens(transcript);
  const words = tokens.map((token) => normalizeWakeWord(token[0]));
  const [expectedGreeting, expectedName] = configuredWakeWords(pronunciation);
  if (words.length === 1) return words[0] !== "a" && isGreeting(words[0], expectedGreeting);
  return words.length === 2
    && isGreeting(words[0], expectedGreeting)
    && NAMES.has(expectedName)
    && PARTIAL_NAMES.has(words[1]);
}

export function isWakeLeadOnly(transcript: string, pronunciation = "Hey Macha"): boolean {
  const words = contentTokens(transcript).map((token) => normalizeWakeWord(token[0]));
  const [expectedGreeting] = configuredWakeWords(pronunciation);
  return words.length === 1 && isGreeting(words[0], expectedGreeting);
}

export interface SpeechTranscript {
  text: string;
  confidence: number | null;
  noSpeechProb: number | null;
  compressionRatio: number | null;
}

export function readSpeechTranscript(result: unknown): SpeechTranscript | null {
  if (typeof result !== "object" || result === null) return null;
  const text = "text" in result && typeof result.text === "string" ? result.text.trim() : "";
  const confidence = "confidence" in result && typeof result.confidence === "number"
    ? result.confidence
    : null;
  const noSpeechProb = "no_speech_prob" in result && typeof result.no_speech_prob === "number"
    ? result.no_speech_prob
    : null;
  const compressionRatio = "compression_ratio" in result && typeof result.compression_ratio === "number"
    ? result.compression_ratio
    : null;
  return { text, confidence, noSpeechProb, compressionRatio };
}

function rejectsTranscript(
  result: SpeechTranscript,
  minConfidence: number,
  maxNoSpeech: number,
): boolean {
  if (!result.text) return true;
  if (result.noSpeechProb !== null && result.noSpeechProb >= maxNoSpeech) return true;
  if (result.confidence !== null && result.confidence < minConfidence) return true;
  if (result.compressionRatio !== null && result.compressionRatio > WAKE_MAX_COMPRESSION) return true;
  return false;
}

/** True when a wake-word clip is real speech rather than silence or a hallucination. */
export function isAcceptedWakeTranscript(result: SpeechTranscript): boolean {
  return !rejectsTranscript(result, WAKE_MIN_CONFIDENCE, WAKE_MAX_NO_SPEECH);
}

/** True when a push-to-talk clip is clear enough to send. */
export function isAcceptedVoiceTranscript(result: SpeechTranscript): boolean {
  if (result.confidence === null) return false;
  return !rejectsTranscript(result, VOICE_MIN_CONFIDENCE, VOICE_MAX_NO_SPEECH);
}

export interface WakeWordListener {
  start(
    onWakeActivity: () => void,
    onUtterance: (text: string) => void,
    onError: (message: string) => void,
    pronunciation: string,
    threshold: number,
    useWakeHint?: boolean,
  ): Promise<void>;
  stop(): void;
  isActive(): boolean;
}

export interface WakeWordCalibration {
  transcript: string;
  threshold: number;
}

function encodeWav(frames: Float32Array[], sampleRate: number): Blob {
  const samples = frames.reduce((total, frame) => total + frame.length, 0);
  const buffer = new ArrayBuffer(44 + samples * 2);
  const view = new DataView(buffer);
  const writeText = (offset: number, value: string) => {
    for (let index = 0; index < value.length; index += 1) {
      view.setUint8(offset + index, value.charCodeAt(index));
    }
  };
  writeText(0, "RIFF");
  view.setUint32(4, 36 + samples * 2, true);
  writeText(8, "WAVE");
  writeText(12, "fmt ");
  view.setUint32(16, 16, true);
  view.setUint16(20, 1, true);
  view.setUint16(22, 1, true);
  view.setUint32(24, sampleRate, true);
  view.setUint32(28, sampleRate * 2, true);
  view.setUint16(32, 2, true);
  view.setUint16(34, 16, true);
  writeText(36, "data");
  view.setUint32(40, samples * 2, true);

  let offset = 44;
  for (const frame of frames) {
    for (const sample of frame) {
      const value = Math.max(-1, Math.min(1, sample));
      view.setInt16(offset, value < 0 ? value * 0x8000 : value * 0x7fff, true);
      offset += 2;
    }
  }
  return new Blob([buffer], { type: "audio/wav" });
}

function streamRms(samples: Float32Array): number {
  let sumSquares = 0;
  for (const sample of samples) sumSquares += sample * sample;
  return Math.sqrt(sumSquares / samples.length);
}

async function transcribeWakeClip(audio: Blob, pronunciation: string): Promise<string> {
  const form = new FormData();
  form.append("audio", audio, "hey-macha.wav");
  form.append("wake_hint", pronunciation);
  const controller = new AbortController();
  const timeout = window.setTimeout(() => controller.abort(), STT_TIMEOUT_MS);
  try {
    const response = await fetch(STT_ENDPOINT, {
      method: "POST",
      body: form,
      signal: controller.signal,
    });
    if (!response.ok) {
      const body: unknown = await response.json().catch(() => null);
      const message = typeof body === "object" && body !== null && "error" in body
        && typeof body.error === "string"
        ? body.error
        : `Wake-word transcription failed (HTTP ${response.status}).`;
      throw new Error(message);
    }
    const result: unknown = await response.json();
    if (typeof result === "object" && result !== null && "text" in result
      && typeof result.text === "string" && result.text.trim()) {
      return result.text.trim();
    }
    throw new Error("No speech detected. Say “Hey Macha” clearly and try again.");
  } catch (error) {
    if (error instanceof DOMException && error.name === "AbortError") {
      throw new Error("Local Whisper timed out. Try recording again.");
    }
    throw error;
  } finally {
    window.clearTimeout(timeout);
  }
}

export async function recordWakePronunciation(): Promise<WakeWordCalibration> {
  if (!navigator.mediaDevices?.getUserMedia) {
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
  let context: AudioContext | null = null;
  let source: MediaStreamAudioSourceNode | null = null;
  let analyser: ScriptProcessorNode | null = null;
  let mutedOutput: GainNode | null = null;
  let recorder: MediaRecorder | null = null;
  const voiceLevels: number[] = [];
  let collectLevels = true;
  try {
    context = new AudioContext();
    await context.resume();
    source = context.createMediaStreamSource(stream);
    analyser = context.createScriptProcessor(2048, 1, 1);
    mutedOutput = context.createGain();
    mutedOutput.gain.value = 0;
    analyser.onaudioprocess = (event) => {
      if (!collectLevels) return;
      const samples = event.inputBuffer.getChannelData(0);
      const copy = new Float32Array(samples);
      const level = streamRms(copy);
      if (level > 0.002) voiceLevels.push(level);
    };
    source.connect(analyser);
    analyser.connect(mutedOutput);
    mutedOutput.connect(context.destination);

    const mimeType = MediaRecorder.isTypeSupported("audio/webm;codecs=opus")
      ? "audio/webm;codecs=opus"
      : "audio/webm";
    recorder = new MediaRecorder(stream, { mimeType });
    const audioChunks: Blob[] = [];
    recorder.addEventListener("dataavailable", (event) => {
      if (event.data.size > 0) audioChunks.push(event.data);
    });
    const stopped = new Promise<void>((resolve, reject) => {
      recorder!.addEventListener("stop", () => resolve(), { once: true });
      recorder!.addEventListener("error", () => reject(new Error("Wake phrase recording failed.")), {
        once: true,
      });
    });
    recorder.start(250);
    await new Promise((resolve) => window.setTimeout(resolve, CALIBRATION_MS));
    collectLevels = false;
    recorder.stop();
    await stopped;
    if (!voiceLevels.length) {
      throw new Error("No voice level detected. Check your microphone and try again.");
    }

    const calibrationAudio = new Blob(audioChunks, { type: mimeType });
    const transcript = await transcribeWakeClip(calibrationAudio, "Hey Macha");
    const match = matchWakePhrase(transcript);
    if (!match) {
      throw new Error(`Whisper heard “${transcript}”. Please try again and say “Hey Macha”.`);
    }
    const savedPronunciation = "Hey Macha";
    voiceLevels.sort((a, b) => a - b);
    const typicalSpeechLevel = voiceLevels[Math.floor(voiceLevels.length * 0.2)];
    const threshold = Math.round(Math.max(0.003, Math.min(0.03, typicalSpeechLevel * 0.35)) * 10000) / 10000;
    return { transcript: savedPronunciation, threshold };
  } finally {
    collectLevels = false;
    if (recorder?.state === "recording") recorder.stop();
    if (analyser) analyser.onaudioprocess = null;
    source?.disconnect();
    analyser?.disconnect();
    mutedOutput?.disconnect();
    if (context) {
      await context.close().catch((error: unknown) => {
        console.error("[coucou] could not close pronunciation recording context", error);
      });
    }
    stream.getTracks().forEach((track) => track.stop());
  }
}

export function createWakeWordListener(): WakeWordListener {
  let stream: MediaStream | null = null;
  let context: AudioContext | null = null;
  let processor: ScriptProcessorNode | null = null;
  let source: MediaStreamAudioSourceNode | null = null;
  let mutedOutput: GainNode | null = null;
  let speechFrames: Float32Array[] = [];
  let preRollFrames: Float32Array[] = [];
  let pendingSpeech: Float32Array[] = [];
  let preRollSamples = 0;
  let speechStartedAt = 0;
  let speechSilenceMs = 0;
  let noiseFloor = 0.0015;
  let active = false;
  let requestRunning = false;
  let generation = 0;
  let activeRequest: AbortController | null = null;
  let threshold = DEFAULT_SPEECH_THRESHOLD;
  let pronunciation = "Hey Macha";
  let useWakeHint = true;
  const pendingAudio: Array<{ audio: Blob; run: number }> = [];
  let onWakeActivity: (() => void) | null = null;
  let onUtterance: ((text: string) => void) | null = null;
  let onError: ((message: string) => void) | null = null;
  let wakeActivityReported = false;

  async function transcribePendingAudio() {
    if (requestRunning) return;
    const pending = pendingAudio.shift();
    if (!pending) return;
    const { audio, run } = pending;
    if (!active || generation !== run || audio.size === 0) {
      void transcribePendingAudio();
      return;
    }
    requestRunning = true;
    const controller = new AbortController();
    activeRequest = controller;
    const timeout = window.setTimeout(() => controller.abort(), STT_TIMEOUT_MS);
    try {
      const form = new FormData();
      form.append("audio", audio, "wake-utterance.wav");
      form.append("mode", useWakeHint ? "wake" : "voice");
      if (useWakeHint) form.append("wake_hint", pronunciation);
      const response = await fetch(STT_ENDPOINT, {
        method: "POST",
        body: form,
        signal: controller.signal,
      });
      if (!response.ok) {
        const body: unknown = await response.json().catch(() => null);
        const message = typeof body === "object" && body !== null && "error" in body
          && typeof body.error === "string"
          ? body.error
          : `Wake-word transcription failed (HTTP ${response.status}).`;
        throw new Error(message);
      }
      const result = readSpeechTranscript(await response.json());
      const accepted = result && (useWakeHint
        ? isAcceptedWakeTranscript(result)
        : isAcceptedVoiceTranscript(result));
      if (result && accepted && active && generation === run) {
        onUtterance?.(result.text);
      }
    } catch (error) {
      if (active && generation === run) {
        const timedOut = error instanceof DOMException && error.name === "AbortError";
        onError?.(timedOut
          ? "Local Whisper wake-word transcription timed out."
          : error instanceof Error
            ? error.message
            : "Wake-word transcription failed.");
      }
    } finally {
      window.clearTimeout(timeout);
      if (activeRequest === controller) {
        activeRequest = null;
        requestRunning = false;
      }
      void transcribePendingAudio();
    }
  }

  function resetUtterance() {
    speechFrames = [];
    pendingSpeech = [];
    speechSilenceMs = 0;
    speechStartedAt = 0;
    wakeActivityReported = false;
  }

  function enqueueUtterance(run: number, sampleRate: number, frames: Float32Array[], durationMs: number) {
    if (!active || generation !== run || durationMs < WAKE_MIN_SPEECH_MS || !frames.length) return;
    pendingAudio.push({ audio: encodeWav(frames, sampleRate), run });
    while (pendingAudio.length > 2) pendingAudio.shift();
    void transcribePendingAudio();
  }

  function finishUtterance(run: number, sampleRate: number) {
    if (!speechStartedAt) {
      resetUtterance();
      return;
    }
    const durationMs = performance.now() - speechStartedAt;
    const utterance = speechFrames;
    resetUtterance();
    enqueueUtterance(run, sampleRate, utterance, durationMs);
  }

  function finishWakeWindow(run: number, sampleRate: number) {
    if (!speechStartedAt) return;
    const durationMs = performance.now() - speechStartedAt;
    const utterance = speechFrames.slice();
    resetUtterance();
    if (durationMs < WAKE_MIN_SPEECH_MS || !utterance.length) return;
    enqueueUtterance(run, sampleRate, utterance, durationMs);
  }

  function stop() {
    generation += 1;
    active = false;
    pendingAudio.length = 0;
    const request = activeRequest;
    activeRequest = null;
    requestRunning = false;
    request?.abort();
    onWakeActivity = null;
    onUtterance = null;
    onError = null;
    if (processor) processor.onaudioprocess = null;
    processor?.disconnect();
    source?.disconnect();
    mutedOutput?.disconnect();
    void context?.close().catch((error: unknown) => {
      console.error("[coucou] could not close wake-word audio context", error);
    });
    stream?.getTracks().forEach((track) => track.stop());
    processor = null;
    source = null;
    mutedOutput = null;
    context = null;
    stream = null;
    resetUtterance();
    preRollFrames = [];
    preRollSamples = 0;
  }

  return {
    async start(onActivity, onText, onFailure, wakePronunciation, speechThreshold, withWakeHint = true) {
      if (active) return;
      if (!navigator.mediaDevices?.getUserMedia) {
        throw new Error("Always-listening microphone is not available in this window.");
      }
      const run = ++generation;
      const failCurrentRun = (message: string) => {
        if (!active || generation !== run) return;
        const reportError = onError;
        stop();
        reportError?.(message);
      };
      onWakeActivity = onActivity;
      onUtterance = onText;
      onError = onFailure;
      pronunciation = wakePronunciation || "Hey Macha";
      useWakeHint = withWakeHint;
      noiseFloor = 0.0015;
      threshold = Number.isFinite(speechThreshold)
        ? Math.max(0.002, Math.min(0.02, speechThreshold * 0.65))
        : DEFAULT_SPEECH_THRESHOLD;
      try {
        await waitForSttReady(() => generation === run);
        if (generation !== run) return;
        stream = await navigator.mediaDevices.getUserMedia({
          audio: {
            channelCount: 1,
            echoCancellation: true,
            noiseSuppression: true,
            autoGainControl: true,
          },
        });
        if (generation !== run) {
          stream.getTracks().forEach((track) => track.stop());
          stream = null;
          return;
        }
        context = new AudioContext();
        await context.resume();
        if (generation !== run) {
          await context.close();
          stream?.getTracks().forEach((track) => track.stop());
          stream = null;
          context = null;
          return;
        }
        source = context.createMediaStreamSource(stream);
        processor = context.createScriptProcessor(2048, 1, 1);
        mutedOutput = context.createGain();
        mutedOutput.gain.value = 0;
        context.addEventListener("statechange", () => {
          if (generation !== run || !active || context?.state === "running") return;
          console.warn(`[coucou] Hey Macha audio context ${context?.state}; reconnecting`);
          failCurrentRun("Microphone audio was interrupted.");
        });
        for (const track of stream.getAudioTracks()) {
          track.addEventListener("ended", () => {
            console.warn("[coucou] Hey Macha microphone track ended; reconnecting");
            failCurrentRun("The microphone disconnected.");
          }, { once: true });
        }
        const preRollLimit = Math.round(context.sampleRate * PRE_ROLL_MS / 1000);
        const sampleRate = context.sampleRate;
        active = true;
        processor.onaudioprocess = (event) => {
          if (!active || generation !== run) return;
          const samples = event.inputBuffer.getChannelData(0);
          const copy = new Float32Array(samples);
          const frameMs = copy.length * 1000 / sampleRate;

          const level = streamRms(copy);
          const speechThreshold = Math.max(threshold, Math.min(0.05, noiseFloor * 3.5));
          const speaking = level >= speechThreshold;
          if (speaking && !speechStartedAt) {
            pendingSpeech.push(copy);
            if (pendingSpeech.length >= WAKE_TRIGGER_FRAMES) {
              speechStartedAt = performance.now() - frameMs * (pendingSpeech.length - 1);
              speechFrames = preRollFrames.concat(pendingSpeech);
              preRollFrames = [];
              preRollSamples = 0;
              pendingSpeech = [];
              speechSilenceMs = 0;
              if (!wakeActivityReported) {
                wakeActivityReported = true;
                console.info("[coucou] Hey Macha listener heard speech", {
                  level: Number(level.toFixed(4)),
                });
                onWakeActivity?.();
              }
            }
          } else if (speaking) {
            speechFrames.push(copy);
            speechSilenceMs = 0;
          } else if (speechStartedAt) {
            pendingSpeech = [];
            speechFrames.push(copy);
            speechSilenceMs += frameMs;
            if (speechSilenceMs >= (useWakeHint ? WAKE_SILENCE_MS : SILENCE_MS)
              || performance.now() - speechStartedAt >= MAX_SPEECH_MS) {
              if (useWakeHint) finishWakeWindow(run, sampleRate);
              else finishUtterance(run, sampleRate);
            }
          } else {
            pendingSpeech = [];
            noiseFloor = noiseFloor * 0.96 + level * 0.04;
            preRollFrames.push(copy);
            preRollSamples += copy.length;
            while (preRollSamples > preRollLimit && preRollFrames.length) {
              preRollSamples -= preRollFrames.shift()!.length;
            }
          }
          if (speechStartedAt && performance.now() - speechStartedAt >= MAX_SPEECH_MS) {
            if (useWakeHint) finishWakeWindow(run, sampleRate);
            else finishUtterance(run, sampleRate);
          }
        };
        source.connect(processor);
        processor.connect(mutedOutput);
        mutedOutput.connect(context.destination);
        console.info("[coucou] Hey Macha wake listener started", {
          sampleRate,
          threshold: Number(threshold.toFixed(4)),
        });
      } catch (error) {
        stop();
        throw error;
      }
    },
    stop,
    isActive: () => active,
  };
}
