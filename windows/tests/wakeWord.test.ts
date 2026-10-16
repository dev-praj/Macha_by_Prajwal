import assert from "node:assert/strict";
import { describe, it } from "node:test";
import {
  extractWakeCommand,
  isPartialWakePhrase,
  isWakeLeadOnly,
  matchWakePhrase,
} from "../src/core/wakeWord.ts";

describe("wake-word response timing", () => {
  it("treats the exact wake phrase as an immediate signal", () => {
    assert.equal(isWakeLeadOnly("Hey Macha"), false);
    assert.equal(extractWakeCommand("Hey Macha"), "");
  });

  it("extracts a command after the wake phrase", () => {
    assert.equal(extractWakeCommand("Hey Macha, tell me the weather"), "tell me the weather");
  });

  it("accepts Hay Macha and a wake phrase transcribed as one word", () => {
    assert.deepEqual(matchWakePhrase("Hay Macha, tell me the weather"), {
      command: "tell me the weather",
    });
    assert.equal(extractWakeCommand("HeyMacha, tell me the weather"), "tell me the weather");
  });

  it("responds when called by name without the optional hey", () => {
    assert.equal(extractWakeCommand("Macha"), "");
    assert.equal(extractWakeCommand("Macha, tell me the weather"), "tell me the weather");
  });

  it("recognizes partial wake phrases so separated words can be joined", () => {
    assert.equal(isPartialWakePhrase("Hey"), true);
    assert.equal(isPartialWakePhrase("Hey Ma"), true);
    assert.equal(extractWakeCommand("Hey Macha, tell me the weather"), "tell me the weather");
  });

  it("accepts Whisper splitting the name into two words", () => {
    assert.equal(extractWakeCommand("Hey Ma Cha, tell me the weather"), "tell me the weather");
    assert.equal(extractWakeCommand("Ma Cha"), "");
  });

  it("accepts common wake-phrase transcription variants", () => {
    assert.equal(extractWakeCommand("A Macha, tell me the weather"), "tell me the weather");
    assert.equal(extractWakeCommand("Hey Maccha, tell me the weather"), "tell me the weather");
    assert.equal(extractWakeCommand("Hey Maacha, tell me the weather"), "tell me the weather");
    assert.equal(extractWakeCommand("Hey Mochi, tell me the weather"), "tell me the weather");
    assert.equal(extractWakeCommand("HEY MOCHA!"), "");
  });

  it("normalizes accents and punctuation and tolerates a one-edit mishearing", () => {
    assert.equal(extractWakeCommand("Uh, Héy! Mácha — set a timer."), "set a timer.");
    assert.equal(extractWakeCommand("Hey Mach, tell me the weather"), "tell me the weather");
  });

  it("does not match unrelated background speech", () => {
    assert.equal(matchWakePhrase("Music is playing in the background."), null);
    assert.equal(matchWakePhrase("A machine is making noise."), null);
  });
});
