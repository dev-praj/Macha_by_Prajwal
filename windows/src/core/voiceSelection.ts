const VOICE_PREFERENCE = [
  "Microsoft Zira",
  "Microsoft Aria Natural",
  "Microsoft Jenny",
  "Microsoft David",
];

export function isEnglishVoice(voice: SpeechSynthesisVoice): boolean {
  return /^en(?:-|$)/i.test(voice.lang.trim());
}

export function preferredVoice(
  voices: SpeechSynthesisVoice[],
  selected: string,
): SpeechSynthesisVoice | undefined {
  if (selected) {
    const match = voices.find((voice) => voice.voiceURI === selected && isEnglishVoice(voice));
    if (match) return match;
  }
  for (const preferred of VOICE_PREFERENCE) {
    const match = voices.find((voice) =>
      isEnglishVoice(voice) && voice.name.toLowerCase().includes(preferred.toLowerCase()));
    if (match) return match;
  }
  return voices.find((voice) => /^en-us\b/i.test(voice.lang))
    ?? voices.find((voice) => /^en-in\b/i.test(voice.lang))
    ?? voices.find(isEnglishVoice);
}
