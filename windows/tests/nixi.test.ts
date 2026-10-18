import assert from "node:assert/strict";
import { describe, it } from "node:test";
import { State } from "../src/core/state.ts";

describe("Nixi State", () => {
  it("initializes Nixi hotkey states to false", () => {
    assert.equal(State.nixiPttActive, false);
    assert.equal(State.nixiDictateActive, false);
  });

  it("updates hotkey flags in AppState", () => {
    State.nixiPttActive = true;
    assert.equal(State.nixiPttActive, true);
    State.nixiPttActive = false;
    assert.equal(State.nixiPttActive, false);

    State.nixiDictateActive = true;
    assert.equal(State.nixiDictateActive, true);
    State.nixiDictateActive = false;
    assert.equal(State.nixiDictateActive, false);
  });
});
