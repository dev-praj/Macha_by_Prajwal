import assert from "node:assert/strict";
import { describe, it } from "node:test";
import {
  canStartHoldToTalk,
  isPartialVisionModeActivation,
  isVisionModeActivation,
} from "../src/core/voiceCommands.ts";

describe("Vision Mode voice commands", () => {
  it("recognizes activation requests in a full wake-word transcript", () => {
    assert.equal(isVisionModeActivation("Hay Macha, activate vision mode"), true);
    assert.equal(isVisionModeActivation("Hey Macha, and tell it to activate the vision mode"), true);
  });

  describe("Ctrl+Alt hold-to-talk availability", () => {
    it("can interrupt the idle wake-word listener", () => {
      assert.equal(canStartHoldToTalk("wake", false), true);
    });

    it("starts from idle but does not interrupt active recording or workflows", () => {
      assert.equal(canStartHoldToTalk("idle", false), true);
      assert.equal(canStartHoldToTalk("listening", false), false);
      assert.equal(canStartHoldToTalk("transcribing", false), false);
      assert.equal(canStartHoldToTalk("speaking", false), false);
      assert.equal(canStartHoldToTalk("wake", true), false);
    });
  });

  it("recognizes common activation verbs and ignores unrelated mentions", () => {
    assert.equal(isVisionModeActivation("Please enable Vision Mode"), true);
    assert.equal(isVisionModeActivation("I use vision mode for work"), false);
  });

  it("holds partial wake commands for the next speech clip", () => {
    assert.equal(isPartialVisionModeActivation("and tell it to activate"), true);
    assert.equal(isPartialVisionModeActivation("activate vision"), true);
    assert.equal(isPartialVisionModeActivation("and tell it to"), true);
    assert.equal(isPartialVisionModeActivation("tell me a joke"), false);
  });
});
