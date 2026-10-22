const STT_ENDPOINT = "http://127.0.0.1:5005/transcribe";

export async function transcribeAudio(audio: Blob, vocabulary: string[] = []): Promise<string> {
  const form = new FormData();
  form.append("audio", audio, "nixi.webm");
  form.append("mode", "voice");
  if (vocabulary.length > 0) {
    form.append("vocabulary", JSON.stringify(vocabulary));
  }

  const response = await fetch(STT_ENDPOINT, {
    method: "POST",
    body: form,
  });

  if (!response.ok) {
    throw new Error(`Voice server error (HTTP ${response.status})`);
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
  return "";
}
