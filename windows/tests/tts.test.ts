import assert from "node:assert/strict";
import test from "node:test";
import { preferredVoice } from "../src/core/voiceSelection.ts";

function voice(name: string, lang = "en-US"): SpeechSynthesisVoice {
  return { name, lang, voiceURI: name } as SpeechSynthesisVoice;
}

test("prefers the installed Microsoft Zira English voice", () => {
  const voices = [
    voice("Microsoft David - English (United States)"),
    voice("Microsoft Zira - English (United States)"),
  ];

  assert.equal(preferredVoice(voices, "")?.name, voices[1].name);
});

test("falls back to an installed English voice when Zira is unavailable", () => {
  const voices = [
    voice("A non-English voice", "fr-FR"),
    voice("Microsoft Mark - English (United States)"),
  ];

  assert.equal(preferredVoice(voices, "")?.name, voices[1].name);
});
