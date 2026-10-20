// Nixi hotkey handlers: Push-To-Talk (Right Ctrl) and Dictation (Right Alt).

import {
  Bridge,
  IS_TAURI,
  onNixiDictateStart,
  onNixiDictateStop,
  onNixiPttStart,
  onNixiPttStop,
} from "../core/bridge";
import { Sound } from "../core/sound";
import type { Island } from "../island/island";
import type { NixiStatusBar } from "./status";

const STT_ENDPOINT = "http://127.0.0.1:5005/transcribe";

class NixiAudioRecorder {
  private mediaStream: MediaStream | null = null;
  private recorder: MediaRecorder | null = null;
  private audioChunks: Blob[] = [];
  private activeMode: "ptt" | "dictate" | null = null;

  async start(mode: "ptt" | "dictate"): Promise<boolean> {
    this.activeMode = mode;
    this.audioChunks = [];
    try {
      if (!this.mediaStream || !this.mediaStream.active) {
        this.mediaStream = await navigator.mediaDevices.getUserMedia({
          audio: {
            channelCount: 1,
            echoCancellation: true,
            noiseSuppression: true,
            autoGainControl: true,
          },
        });
      }
      const mimeType = MediaRecorder.isTypeSupported("audio/webm;codecs=opus")
        ? "audio/webm;codecs=opus"
        : "audio/webm";
      this.recorder = new MediaRecorder(this.mediaStream, { mimeType });
      this.recorder.addEventListener("dataavailable", (event) => {
        if (event.data.size > 0) this.audioChunks.push(event.data);
      });
      this.recorder.start();
      return true;
    } catch (err) {
      console.error("[nixi] failed to start audio recording", err);
      return false;
    }
  }

  async stop(): Promise<{ blob: Blob; mode: "ptt" | "dictate" } | null> {
    const mode = this.activeMode;
    this.activeMode = null;
    if (!this.recorder || this.recorder.state !== "recording" || !mode) {
      return null;
    }

    return new Promise((resolve) => {
      this.recorder!.addEventListener(
        "stop",
        () => {
          const blob = new Blob(this.audioChunks, {
            type: this.recorder?.mimeType || "audio/webm",
          });
          this.audioChunks = [];
          resolve({ blob, mode });
        },
        { once: true },
      );
      try {
        this.recorder!.stop();
      } catch (err) {
        console.error("[nixi] error stopping recorder", err);
        resolve(null);
      }
    });
  }

  destroy() {
    if (this.recorder && this.recorder.state === "recording") {
      try {
        this.recorder.stop();
      } catch {
        // ignore
      }
    }
    if (this.mediaStream) {
      this.mediaStream.getTracks().forEach((track) => track.stop());
      this.mediaStream = null;
    }
  }
}

async function transcribeAudio(audio: Blob): Promise<string> {
  const form = new FormData();
  form.append("audio", audio, "nixi.webm");
  form.append("mode", "voice");

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

export function registerNixiHotkeyHandlers(
  island: Island,
  statusBar: NixiStatusBar,
): () => void {
  const recorder = new NixiAudioRecorder();

  const onPttStart = () => {
    island.setNixiPtt(true);
    statusBar.setStatus("Listening…");
    Sound.play("peek");
    void recorder.start("ptt");
  };

  const onPttStop = () => {
    island.setNixiPtt(false);
    statusBar.setStatus("Thinking…");
    void (async () => {
      const result = await recorder.stop();
      if (!result || result.blob.size === 0) {
        statusBar.clear();
        return;
      }
      try {
        const transcript = await transcribeAudio(result.blob);
        if (!transcript) {
          statusBar.setStatus("No speech heard");
          return;
        }
        statusBar.setStatus("Executing…", transcript);
        const cmdRes = await Bridge.nixiRunCommand(transcript);
        if (!cmdRes.success) {
          statusBar.setStatus(cmdRes.summary, cmdRes.detail);
        }
      } catch (err) {
        console.error("[nixi] PTT transcription failed", err);
        statusBar.setStatus("Speech service offline");
      }
    })();
  };

  const onDictateStart = () => {
    island.setNixiDictate(true);
    statusBar.setStatus("Dictating…");
    Sound.play("peek");
    void recorder.start("dictate");
  };

  const onDictateStop = () => {
    island.setNixiDictate(false);
    statusBar.setStatus("Transcribing…");
    void (async () => {
      const result = await recorder.stop();
      if (!result || result.blob.size === 0) {
        statusBar.clear();
        return;
      }
      try {
        const transcript = await transcribeAudio(result.blob);
        if (!transcript) {
          statusBar.setStatus("No speech heard");
          return;
        }
        await Bridge.nixiDictateInsert(transcript);
        statusBar.setStatus("Dictation inserted");
      } catch (err) {
        console.error("[nixi] Dictation transcription failed", err);
        statusBar.setStatus("Dictation failed");
      }
    })();
  };

  window.addEventListener("macha-nixi-ptt-start", onPttStart);
  window.addEventListener("macha-nixi-ptt-stop", onPttStop);
  window.addEventListener("macha-nixi-dictate-start", onDictateStart);
  window.addEventListener("macha-nixi-dictate-stop", onDictateStop);

  const unlistenFns: Array<() => void> = [];

  if (IS_TAURI) {
    try {
      void onNixiPttStart(() => {
        try {
          window.dispatchEvent(new CustomEvent("macha-nixi-ptt-start"));
        } catch (err) {
          void Bridge.log(`[nixi] error dispatching macha-nixi-ptt-start: ${String(err)}`);
        }
      })
        .then((unlisten) => {
          if (typeof unlisten === "function") unlistenFns.push(unlisten);
        })
        .catch((err: unknown) => {
          void Bridge.log(`[nixi] failed to register onNixiPttStart: ${String(err)}`);
        });
    } catch (err) {
      void Bridge.log(`[nixi] onNixiPttStart registration error: ${String(err)}`);
    }

    try {
      void onNixiPttStop(() => {
        try {
          window.dispatchEvent(new CustomEvent("macha-nixi-ptt-stop"));
        } catch (err) {
          void Bridge.log(`[nixi] error dispatching macha-nixi-ptt-stop: ${String(err)}`);
        }
      })
        .then((unlisten) => {
          if (typeof unlisten === "function") unlistenFns.push(unlisten);
        })
        .catch((err: unknown) => {
          void Bridge.log(`[nixi] failed to register onNixiPttStop: ${String(err)}`);
        });
    } catch (err) {
      void Bridge.log(`[nixi] onNixiPttStop registration error: ${String(err)}`);
    }

    try {
      void onNixiDictateStart(() => {
        try {
          window.dispatchEvent(new CustomEvent("macha-nixi-dictate-start"));
        } catch (err) {
          void Bridge.log(`[nixi] error dispatching macha-nixi-dictate-start: ${String(err)}`);
        }
      })
        .then((unlisten) => {
          if (typeof unlisten === "function") unlistenFns.push(unlisten);
        })
        .catch((err: unknown) => {
          void Bridge.log(`[nixi] failed to register onNixiDictateStart: ${String(err)}`);
        });
    } catch (err) {
      void Bridge.log(`[nixi] onNixiDictateStart registration error: ${String(err)}`);
    }

    try {
      void onNixiDictateStop(() => {
        try {
          window.dispatchEvent(new CustomEvent("macha-nixi-dictate-stop"));
        } catch (err) {
          void Bridge.log(`[nixi] error dispatching macha-nixi-dictate-stop: ${String(err)}`);
        }
      })
        .then((unlisten) => {
          if (typeof unlisten === "function") unlistenFns.push(unlisten);
        })
        .catch((err: unknown) => {
          void Bridge.log(`[nixi] failed to register onNixiDictateStop: ${String(err)}`);
        });
    } catch (err) {
      void Bridge.log(`[nixi] onNixiDictateStop registration error: ${String(err)}`);
    }
  }

  return () => {
    recorder.destroy();
    window.removeEventListener("macha-nixi-ptt-start", onPttStart);
    window.removeEventListener("macha-nixi-ptt-stop", onPttStop);
    window.removeEventListener("macha-nixi-dictate-start", onDictateStart);
    window.removeEventListener("macha-nixi-dictate-stop", onDictateStop);
    for (const unlisten of unlistenFns) {
      unlisten();
    }
  };
}
