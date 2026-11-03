import { Bridge, IS_TAURI } from "../core/bridge";

const STT_ENDPOINT = "http://127.0.0.1:5005/transcribe";
const PARAKEET_SAMPLE_RATE = 16_000;
const MAX_AUDIO_BYTES = 20 * 1024 * 1024;

export async function transcribeAudio(audio: Blob, vocabulary: string[] = []): Promise<string> {
  let parakeetError: unknown;
  if (IS_TAURI) {
    try {
      const models = await Bridge.speechModelsStatus();
      if (models.parakeet.state === "ready") {
        const transcript = await Bridge.parakeetTranscribe(await audioToWavBase64(audio), vocabulary);
        if (transcript.trim()) return transcript.trim();
        parakeetError = new Error("Parakeet did not return any speech.");
      }
    } catch (error) {
      parakeetError = error;
      console.warn("[nixi] Parakeet transcription failed; trying the local voice service", error);
    }
  }

  try {
    return await transcribeWithVoiceService(audio, vocabulary);
  } catch (error) {
    if (parakeetError) {
      const primary = parakeetError instanceof Error ? parakeetError.message : String(parakeetError);
      const fallback = error instanceof Error ? error.message : String(error);
      throw new Error(`On-device transcription failed (${primary}); the local speech service also failed (${fallback}).`);
    }
    throw error;
  }
}

async function transcribeWithVoiceService(audio: Blob, vocabulary: string[]): Promise<string> {
  const form = new FormData();
  const extension = audio.type.toLowerCase().includes("mp4") ? "mp4"
    : audio.type.toLowerCase().includes("ogg") ? "ogg"
      : "webm";
  form.append("audio", audio, `nixi.${extension}`);
  form.append("mode", "voice");
  if (vocabulary.length > 0) {
    form.append("vocabulary", JSON.stringify(vocabulary));
  }

  let response: Response;
  try {
    response = await fetch(STT_ENDPOINT, { method: "POST", body: form });
  } catch (error) {
    const reason = error instanceof Error ? error.message : String(error);
    throw new Error(`Could not reach the local speech service at 127.0.0.1:5005 (${reason}).`);
  }

  if (!response.ok) {
    const body: unknown = await response.json().catch(() => null);
    const reason = typeof body === "object" && body !== null && "error" in body
      && typeof body.error === "string"
      ? body.error
      : `HTTP ${response.status}`;
    throw new Error(`Local speech service failed: ${reason}`);
  }

  const data: unknown = await response.json();
  if (
    typeof data === "object" &&
    data !== null &&
    "text" in data &&
    typeof data.text === "string"
  ) {
    return data.text.trim();
  }
  throw new Error("Voice server returned an invalid transcription response.");
}

async function audioToWavBase64(audio: Blob): Promise<string> {
  if (audio.size === 0 || audio.size > MAX_AUDIO_BYTES) {
    throw new Error("The audio recording is empty or too large to transcribe.");
  }

  const decoder = new AudioContext();
  let decoded: AudioBuffer;
  try {
    decoded = await decoder.decodeAudioData(await audio.arrayBuffer());
  } catch (error) {
    throw new Error(`Could not decode the recorded audio: ${String(error)}`);
  } finally {
    await decoder.close();
  }

  const outputLength = Math.ceil(decoded.duration * PARAKEET_SAMPLE_RATE);
  const pcmSize = outputLength * 2;
  if (!Number.isSafeInteger(outputLength) || outputLength === 0 || pcmSize > MAX_AUDIO_BYTES) {
    throw new Error("The audio recording is too long to transcribe with Parakeet.");
  }
  const output = new OfflineAudioContext(1, outputLength, PARAKEET_SAMPLE_RATE);
  const source = output.createBufferSource();
  source.buffer = decoded;
  source.connect(output.destination);
  source.start();
  const resampled = await output.startRendering();
  const samples = resampled.getChannelData(0);
  const wav = new ArrayBuffer(44 + samples.length * 2);
  const view = new DataView(wav);
  const writeText = (offset: number, text: string) => {
    for (let index = 0; index < text.length; index += 1) {
      view.setUint8(offset + index, text.charCodeAt(index));
    }
  };
  writeText(0, "RIFF");
  view.setUint32(4, wav.byteLength - 8, true);
  writeText(8, "WAVE");
  writeText(12, "fmt ");
  view.setUint32(16, 16, true);
  view.setUint16(20, 1, true);
  view.setUint16(22, 1, true);
  view.setUint32(24, PARAKEET_SAMPLE_RATE, true);
  view.setUint32(28, PARAKEET_SAMPLE_RATE * 2, true);
  view.setUint16(32, 2, true);
  view.setUint16(34, 16, true);
  writeText(36, "data");
  view.setUint32(40, samples.length * 2, true);
  for (let index = 0; index < samples.length; index += 1) {
    const sample = Math.max(-1, Math.min(1, samples[index]));
    view.setInt16(44 + index * 2, sample < 0 ? sample * 0x8000 : sample * 0x7fff, true);
  }

  const bytes = new Uint8Array(wav);
  let binary = "";
  for (let offset = 0; offset < bytes.length; offset += 0x8000) {
    binary += String.fromCharCode(...bytes.subarray(offset, offset + 0x8000));
  }
  return btoa(binary);
}
