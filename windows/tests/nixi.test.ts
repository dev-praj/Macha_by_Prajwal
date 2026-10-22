import assert from "node:assert/strict";
import { describe, it } from "node:test";
import { State } from "../src/core/state.ts";
import { NixiAudioRecorder } from "../src/nixi/audioRecorder.ts";
import {
  insertTextAtCursor,
  isDictationHoldKey,
  isDictationSetupTestPayload,
} from "../src/nixi/setupDictation.ts";

class FakeMediaRecorder extends EventTarget {
  static isTypeSupported(): boolean {
    return true;
  }

  state: RecordingState = "inactive";
  mimeType = "audio/webm;codecs=opus";

  start(_timeslice?: number): void {
    this.state = "recording";
  }

  stop(): void {
    this.state = "inactive";
    const dataEvent = new Event("dataavailable");
    Object.defineProperty(dataEvent, "data", { value: new Blob(["test dictation"]) });
    this.dispatchEvent(dataEvent);
    this.dispatchEvent(new Event("stop"));
  }
}

function installRecorderMocks(getUserMedia: () => Promise<MediaStream>) {
  const navigatorDescriptor = Object.getOwnPropertyDescriptor(globalThis, "navigator");
  const recorderDescriptor = Object.getOwnPropertyDescriptor(globalThis, "MediaRecorder");
  Object.defineProperty(globalThis, "navigator", {
    configurable: true,
    value: { mediaDevices: { getUserMedia } },
  });
  Object.defineProperty(globalThis, "MediaRecorder", {
    configurable: true,
    value: FakeMediaRecorder,
  });
  return () => {
    if (navigatorDescriptor) {
      Object.defineProperty(globalThis, "navigator", navigatorDescriptor);
    } else {
      Reflect.deleteProperty(globalThis, "navigator");
    }
    if (recorderDescriptor) {
      Object.defineProperty(globalThis, "MediaRecorder", recorderDescriptor);
    } else {
      Reflect.deleteProperty(globalThis, "MediaRecorder");
    }
  };
}

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

describe("Setup dictation test helpers", () => {
  it("treats Right Alt and AltGraph as the dictation hold key", () => {
    assert.equal(isDictationHoldKey({ code: "AltRight", key: "Alt" }), true);
    assert.equal(isDictationHoldKey({ code: "AltLeft", key: "AltGraph" }), true);
    assert.equal(isDictationHoldKey({ code: "AltLeft", key: "Alt" }), false);
  });

  it("recognizes setup-test payloads from the native hotkey", () => {
    assert.equal(isDictationSetupTestPayload({ setupTest: true }), true);
    assert.equal(isDictationSetupTestPayload({ setupTest: false }), false);
    assert.equal(isDictationSetupTestPayload(null), false);
  });

  it("inserts transcribed text at the caret", () => {
    const input = {
      value: "hello ",
      selectionStart: 6,
      selectionEnd: 6,
      setRangeText(text: string, start: number, end: number) {
        this.value = this.value.slice(0, start) + text + this.value.slice(end);
        this.selectionStart = start + text.length;
        this.selectionEnd = this.selectionStart;
      },
      dispatchEvent() {
        return true;
      },
      focus() {},
    };
    insertTextAtCursor(input as unknown as HTMLTextAreaElement, "world");
    assert.equal(input.value, "hello world");
  });
});

describe("Nixi dictation recording", () => {
  it("stops a recording and returns its captured audio", async () => {
    const track = { stop() {} };
    const stream = {
      active: true,
      getTracks: () => [track],
    } as unknown as MediaStream;
    const restore = installRecorderMocks(async () => stream);
    try {
      const recorder = new NixiAudioRecorder();
      assert.equal(await recorder.start("dictate"), true);
      const result = await recorder.stop();
      assert.equal(result?.mode, "dictate");
      assert.equal(await result?.blob.text(), "test dictation");
      recorder.destroy();
    } finally {
      restore();
    }
  });

  it("does not start recording if the hotkey is released while permission is pending", async () => {
    let resolvePermission: ((stream: MediaStream) => void) | undefined;
    let trackStopped = false;
    const stream = {
      active: true,
      getTracks: () => [{ stop: () => { trackStopped = true; } }],
    } as unknown as MediaStream;
    const restore = installRecorderMocks(() => new Promise((resolve) => {
      resolvePermission = resolve;
    }));
    try {
      const recorder = new NixiAudioRecorder();
      const starting = recorder.start("dictate");
      const stopping = recorder.stop();
      assert.ok(resolvePermission);
      resolvePermission(stream);
      assert.equal(await starting, false);
      assert.equal(await stopping, null);
      assert.equal(trackStopped, true);
      recorder.destroy();
    } finally {
      restore();
    }
  });
});
