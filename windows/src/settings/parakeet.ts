import { Bridge, IS_TAURI, onEvent, type SpeechModelsStatus } from "../core/bridge";
import { clear, h } from "../views/dom";

export function parakeetSection(): HTMLElement {
  const status = h("div", { class: "hint", text: "Checking for the Parakeet speech model…" });
  const actions = h("div", { class: "row" });
  let current: SpeechModelsStatus | null = null;
  let busy = false;
  let actionError: string | null = null;

  const draw = () => {
    clear(actions);
    if (!IS_TAURI) {
      status.textContent = "Parakeet model management is available in the Macha desktop app.";
      return;
    }
    if (busy) {
      status.textContent = "Starting Parakeet model action…";
      return;
    }
    if (!current) return;

    const model = current.parakeet;
    if (actionError) {
      status.textContent = actionError;
      actions.append(downloadButton());
      return;
    }
    if (model.state === "ready") {
      status.textContent = current.sharedWithCoucou
        ? "Ready — using the Parakeet model already installed by Coucou."
        : "Ready — Parakeet is installed locally and works offline.";
      if (!current.sharedWithCoucou) {
        actions.append(h("button", {
          class: "danger",
          type: "button",
          text: "Remove model",
          onclick: () => {
            if (!window.confirm("Remove Macha's locally installed Parakeet model?")) return;
            void runAction(Bridge.speechModelsRemove(), "Could not remove Parakeet");
          },
        }));
      }
    } else if (model.state === "downloading") {
      const percent = Math.round(Math.max(0, Math.min(1, model.fraction)) * 100);
      status.textContent = `Downloading Parakeet… ${percent}% of about ${current.downloadMb} MB`;
    } else if (model.state === "failed") {
      status.textContent = `Parakeet download failed: ${model.error}`;
      actions.append(downloadButton());
    } else {
      status.textContent = `Parakeet provides offline speech recognition in 25 languages. Download about ${current.downloadMb} MB once to enable it.`;
      actions.append(downloadButton());
    }
  };

  const refresh = async () => {
    if (!IS_TAURI) {
      draw();
      return;
    }
    try {
      current = await Bridge.speechModelsStatus();
      draw();
    } catch (error) {
      status.textContent = `Could not check Parakeet model status: ${String(error)}`;
    }
  };

  async function runAction(action: Promise<void>, message: string) {
    busy = true;
    actionError = null;
    draw();
    try {
      await action;
      await refresh();
    } catch (error) {
      actionError = `${message}: ${String(error)}`;
    } finally {
      busy = false;
      draw();
    }
  }

  function downloadButton(): HTMLButtonElement {
    return h("button", {
      class: "primary",
      type: "button",
      text: "Download Parakeet",
      onclick: () => void runAction(Bridge.speechModelsDownload(), "Could not start Parakeet download"),
    });
  }

  void onEvent<SpeechModelsStatus>("speech-models", (next) => {
    current = next;
    draw();
  }).catch((error: unknown) => {
    console.error("[macha] could not subscribe to Parakeet model status", error);
    status.textContent = `Could not listen for Parakeet model updates: ${String(error)}`;
  });
  void refresh();

  return h(
    "section",
    {},
    h("h2", {}, h("span", { text: "Parakeet speech recognition" })),
    h("div", {
      class: "hint",
      text: "Macha uses Parakeet for local dictation when the model is installed. Otherwise it keeps using the existing local Whisper service.",
    }),
    status,
    actions,
  );
}
