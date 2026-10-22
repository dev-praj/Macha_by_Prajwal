// Nixi hotkey handlers: Push-To-Talk (Right Ctrl) and Dictation (Right Alt).

import {
  Bridge,
  IS_TAURI,
  onEvent,
  onNixiDictateStart,
  onNixiDictateStop,
  onNixiPttStart,
  onNixiPttStop,
} from "../core/bridge";
import { State } from "../core/state";
import { Sound } from "../core/sound";
import type { Island } from "../island/island";
import { NixiAudioRecorder } from "./audioRecorder";
import { isDictationSetupTestPayload } from "./setupDictation";
import { transcribeAudio } from "./stt";
import type { NixiStatusBar } from "./status";

export function registerNixiHotkeyHandlers(
  island: Island,
  statusBar: NixiStatusBar,
): () => void {
  const recorder = new NixiAudioRecorder();
  let recordingGeneration = 0;
  let dictationApplication: Promise<string | null> = Promise.resolve(null);
  let dictationApplicationError: string | null = null;
  let dictationSetupTestActive = false;
  let dictationSetupTestRecording = false;

  const unlistenSetupTest = onEvent<boolean>("dictation-setup-test-active", (active) => {
    dictationSetupTestActive = active;
  }).catch((error: unknown) => {
    console.error("[nixi] could not listen for setup dictation mode", error);
    return () => {};
  });
  void Bridge.getDictationSetupTestActive().then((active) => {
    dictationSetupTestActive = active;
  }).catch((error: unknown) => {
    console.error("[nixi] could not read setup dictation mode", error);
  });
  const reportSetupDictation = (transcript: string, error?: string) => {
    void Bridge.completeDictationSetupTest(transcript, error).catch((reportError: unknown) => {
      console.error("[nixi] could not report setup dictation result", reportError);
    });
  };

  const onPttStart = () => {
    const session = ++recordingGeneration;
    island.setNixiPtt(true);
    statusBar.setStatus("Listening…");
    Sound.play("peek");
    void recorder.start("ptt").then((started) => {
      if (started || session !== recordingGeneration) return;
      island.setNixiPtt(false);
      statusBar.setStatus("Microphone unavailable", "Allow microphone access, then try push-to-talk again.");
    });
  };

  const onPttStop = () => {
    const session = ++recordingGeneration;
    island.setNixiPtt(false);
    statusBar.setStatus("Thinking…");
    void (async () => {
      const result = await recorder.stop();
      if (session !== recordingGeneration) return;
      if (!result || result.blob.size === 0) {
        statusBar.setStatus("No audio captured", "Hold Right Ctrl while speaking, then release.");
        return;
      }
      try {
        const transcript = await transcribeAudio(result.blob);
        if (session !== recordingGeneration) return;
        if (!transcript) {
          statusBar.setStatus("No speech heard");
          return;
        }
        statusBar.setStatus("Executing…", transcript);
        const cmdRes = await Bridge.nixiRunCommand(transcript);
        if (session !== recordingGeneration) return;
        if (!cmdRes.success) {
          statusBar.setStatus(cmdRes.summary, cmdRes.detail);
        }
      } catch (err) {
        if (session !== recordingGeneration) return;
        console.error("[nixi] PTT transcription failed", err);
        statusBar.setStatus(
          "Speech service unavailable",
          err instanceof Error ? err.message : String(err),
        );
      }
    })();
  };

  const onDictateStart = () => {
    if (dictationSetupTestActive) return;
    const session = ++recordingGeneration;
    dictationSetupTestRecording = dictationSetupTestActive;
    dictationApplicationError = null;
    dictationApplication = IS_TAURI && !dictationSetupTestRecording
      ? Bridge.nixiForegroundApplication().catch((error: unknown) => {
        dictationApplicationError = error instanceof Error ? error.message : String(error);
        console.error("[nixi] could not identify the dictation target application", error);
        return null;
      })
      : Promise.resolve(null);
    island.setNixiDictate(true);
    statusBar.setStatus("Dictating…");
    Sound.play("peek");
    void recorder.start("dictate").then((started) => {
      if (started || session !== recordingGeneration) return;
      island.setNixiDictate(false);
      statusBar.setStatus("Microphone unavailable", "Allow microphone access, then hold Right Alt to dictate.");
      if (dictationSetupTestRecording) {
        dictationSetupTestRecording = false;
        reportSetupDictation("", "Microphone unavailable. Allow microphone access, then try again.");
      }
    });
  };

  const onDictateStop = () => {
    if (dictationSetupTestActive && !dictationSetupTestRecording) return;
    const session = ++recordingGeneration;
    const isSetupTest = dictationSetupTestRecording;
    dictationSetupTestRecording = false;
    island.setNixiDictate(false);
    statusBar.setStatus("Transcribing…");
    void (async () => {
      const result = await recorder.stop();
      if (session !== recordingGeneration) return;
      if (!result || result.blob.size === 0) {
        statusBar.setStatus("No dictation captured", "Hold Right Alt while speaking, then release.");
        if (isSetupTest) {
          reportSetupDictation("", "No dictation audio was captured. Hold Right Alt while speaking, then release.");
        }
        return;
      }
      try {
        const transcript = await transcribeAudio(result.blob, State.settings.dictationVocabulary);
        if (session !== recordingGeneration) return;
        if (!transcript) {
          statusBar.setStatus("No speech heard");
          if (isSetupTest) {
            reportSetupDictation("", "No speech was detected. Try speaking a little longer.");
          }
          return;
        }
        if (isSetupTest) {
          reportSetupDictation(transcript);
          statusBar.setStatus("Setup dictation transcribed");
          return;
        }
        const targetApplication = await dictationApplication;
        if (session !== recordingGeneration) return;
        const appStyle = targetApplication
          ? State.settings.dictationAppStyles.find(
            (entry) => entry.application.toLowerCase() === targetApplication.toLowerCase(),
          )
          : undefined;
        await Bridge.nixiDictateInsert(
          transcript,
          State.settings.dictationReplacements,
          appStyle?.style ?? "standard",
        );
        if (session !== recordingGeneration) return;
        const detail = appStyle
          ? `${appStyle.style === "verbatim" ? "Verbatim" : "Standard"} style for ${targetApplication}.`
          : dictationApplicationError
            ? `Could not identify the target app; standard formatting was used. ${dictationApplicationError}`
            : targetApplication && State.settings.dictationAppStyles.length > 0
              ? `No style is configured for ${targetApplication}; standard formatting was used.`
              : undefined;
        statusBar.setStatus("Dictation inserted", detail);
      } catch (err) {
        if (session !== recordingGeneration) return;
        console.error("[nixi] Dictation transcription failed", err);
        statusBar.setStatus(
          "Dictation failed",
          err instanceof Error ? err.message : String(err),
        );
        if (isSetupTest) {
          reportSetupDictation(
            "",
            err instanceof Error ? err.message : "Dictation transcription failed.",
          );
        }
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
      void onNixiDictateStart((payload) => {
        try {
          if (isDictationSetupTestPayload(payload)) {
            dictationSetupTestActive = true;
            return;
          }
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
      void onNixiDictateStop((payload) => {
        try {
          if (isDictationSetupTestPayload(payload)) {
            dictationSetupTestActive = true;
            return;
          }
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
    void unlistenSetupTest.then((unlisten) => unlisten());
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
