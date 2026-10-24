import { Bridge, type PhoneChatStatus, type PhoneLinkPairing, type PhoneLinkStatus } from "../core/bridge";
import { h, clear } from "../views/dom";

type Toggle = (on: boolean, onChange: (value: boolean) => void) => HTMLElement;

export function phoneSection(initial: boolean, detailsEnabled: boolean, makeToggle: Toggle): HTMLElement {
  const feedback = h("div", { class: "hint" });
  const detail = h("div", {});
  const state = h("div", { class: "hint" });
  const clients = h("div", { class: "hint" });
  const chatSettings = h("div", {});
  const chatFeedback = h("div", { class: "hint" });
  let latest: PhoneLinkStatus | null = null;
  let latestChat: PhoneChatStatus = { enabled: false, models: [] };

  const showError = (message: string) => {
    feedback.textContent = message;
  };

  const pairingBox = h("div", {});
  const pairingRows = (pairing: PhoneLinkPairing): HTMLElement[] => {
    const qr = h("img", {
      class: "phone-pairing-qr",
      alt: "Android phone pairing QR code",
      src: `data:image/svg+xml;base64,${btoa(pairing.qrSvg)}`,
    }) as HTMLImageElement;
    const link = h("textarea", {
      class: "phone-pairing-link",
      readonly: true,
      spellcheck: "false",
      "aria-label": "Phone pairing link",
    }, pairing.link) as HTMLTextAreaElement;
    const copy = h("button", { type: "button", text: "Copy pairing link" });
    copy.addEventListener("click", () => {
      void navigator.clipboard.writeText(pairing.link).then(
        () => showError("Pairing link copied."),
        (error: unknown) => showError(`Could not copy pairing link: ${String(error)}`),
      );
    });
    const again = h("button", { type: "button", class: "danger", text: "Pair again…" });
    again.addEventListener("click", () => {
      if (!window.confirm("Disconnect paired phones and create a new pairing code?")) return;
      void Bridge.phoneLinkNewPairing().then((fresh) => {
        clear(pairingBox);
        pairingBox.append(...pairingRows(fresh));
        showError("A new pairing code is ready. Previously paired phones must pair again.");
      }).catch((error: unknown) => showError(`Could not create a new pairing code: ${String(error)}`));
    });
    return [
      h("div", { class: "phone-pairing" },
        qr,
        h("div", { class: "phone-pairing-copy" },
          h("div", { class: "hint", text: `Scan with Macha for Android, or paste the pairing link. Computer: ${pairing.name} (${pairing.host}:${pairing.port}).` }),
          link,
          h("div", { class: "row" }, copy, again),
        ),
      ),
    ];
  };

  const showPairing = async () => {
    try {
      const pairing = await Bridge.phoneLinkPairing();
      clear(pairingBox);
      pairingBox.append(...pairingRows(pairing));
      showError("");
    } catch (error) {
      showError(`Could not show pairing code: ${String(error)}`);
    }
  };

  const detailToggle = makeToggle(detailsEnabled, (enabled) => {
    void Bridge.phoneDetailsSetEnabled(enabled).then((result) => {
      detailsControl.classList.toggle("on", result.enabled);
      detailsControl.setAttribute("aria-pressed", String(result.enabled));
    }).catch((error: unknown) => {
      detailsControl.classList.toggle("on", !enabled);
      detailsControl.setAttribute("aria-pressed", String(!enabled));
      showError(`Could not update phone session details: ${String(error)}`);
    });
  });
  const detailsControl = detailToggle as HTMLButtonElement;

  const renderChatSettings = (status: PhoneChatStatus) => {
    latestChat = status;
    chatFeedback.textContent = "";
    clear(chatSettings);
    const chatToggle = makeToggle(status.enabled, (enabled) => {
      void Bridge.phoneChatSetEnabled(enabled).then(renderChatSettings).catch((error: unknown) => {
        renderChatSettings(latestChat);
        chatFeedback.textContent = `Could not update phone chat: ${String(error)}`;
      });
    });
    const modelInput = h("input", {
      type: "text",
      placeholder: "provider/model (for example openai/gpt-4o)",
      maxlength: "210",
      autocomplete: "off",
      spellcheck: "false",
      "aria-label": "Provider/model ID allowed for phone chat",
    }) as HTMLInputElement;
    const updateModels = (models: string[]) => {
      chatFeedback.textContent = "";
      void Bridge.phoneChatSetModels(models).then(renderChatSettings).catch((error: unknown) => {
        chatFeedback.textContent = `Could not update phone chat models: ${String(error)}`;
      });
    };
    const addModel = h("button", { type: "button", text: "Add model" });
    const add = () => {
      const model = modelInput.value.trim();
      if (!model) {
        chatFeedback.textContent = "Enter a provider/model ID first.";
        return;
      }
      if (latestChat.models.includes(model)) {
        chatFeedback.textContent = "That model is already allowed.";
        return;
      }
      updateModels([...latestChat.models, model]);
    };
    addModel.addEventListener("click", add);
    modelInput.addEventListener("keydown", (event: KeyboardEvent) => {
      if (event.key === "Enter") add();
    });

    const allowedModels = h("div", {});
    for (const model of status.models) {
      const remove = h("button", { type: "button", text: "Remove" });
      remove.addEventListener("click", () => updateModels(status.models.filter((item) => item !== model)));
      allowedModels.append(h("div", { class: "row" }, h("span", { text: model }), remove));
    }
    chatSettings.append(
      h("div", { class: "row" }, h("label", { text: "Let the phone chat with my AI providers" }), chatToggle),
      h("div", { class: "hint", text: "Off by default. The computer uses API keys saved in Settings → Chat; keys stay on this computer. Add at least one provider/model ID, such as anthropic/claude-sonnet-4-5, openai/gpt-4o, or ollama/qwen2.5:7b." }),
      h("div", { class: "row" }, modelInput, addModel),
      allowedModels,
      chatFeedback,
    );
  };

  const draw = (status: PhoneLinkStatus | null) => {
    latest = status;
    clear(detail);
    clear(pairingBox);
    feedback.textContent = status?.error ? `Phone link error: ${status.error}` : "";
    state.textContent = status?.running
      ? `Listening on ${status.host || "local network"}:${status.port}`
      : status?.enabled
        ? "Phone link is enabled but not running."
        : "";
    clients.textContent = status?.running ? `Connected phones: ${status.clients}` : "";
    if (!status?.running) return;

    const pairing = h("button", { type: "button", text: "Show pairing QR code" });
    pairing.addEventListener("click", () => void showPairing());
    detail.append(
      h("div", { class: "row" }, pairing),
      pairingBox,
      clients,
      h("div", { class: "hint", text: "Keep Macha and the phone on the same private Wi-Fi. If Windows Firewall asks, allow Macha on private networks only." }),
      h("div", { class: "hint", text: "The link is off by default. Its TLS certificate and pairing token stay in Windows Credential Manager; the Android app pins that certificate." }),
      h("div", { class: "row" },
        h("label", { text: "Share session details with the paired phone" }),
        detailsControl,
      ),
    );
  };

  const phoneToggle = makeToggle(initial, (enabled) => {
    feedback.textContent = "";
    void Bridge.phoneLinkSetEnabled(enabled).then(draw).catch((error: unknown) => {
      phoneSwitch.classList.toggle("on", !enabled);
      phoneSwitch.setAttribute("aria-pressed", String(!enabled));
      draw(latest);
      showError(`Could not ${enabled ? "start" : "stop"} phone link: ${String(error)}`);
    });
  });
  const phoneSwitch = phoneToggle as HTMLButtonElement;

  const section = h(
    "section",
    {},
    h("h2", {}, h("span", { text: "Android phone" })),
    h("div", { class: "hint", text: "Pair Macha for Android to view agent sessions and handle Claude Code permission requests. The local-network server is opt-in and stays off until enabled." }),
    h("div", { class: "hint", text: "The Android app requires device authentication before it can allow a request; deny remains available without authentication." }),
    h("div", { class: "row" }, h("label", { text: "Phone link" }), phoneSwitch),
    feedback,
    state,
    detail,
    chatSettings,
  );

  const watch = window.setInterval(() => {
    if (!section.isConnected) {
      window.clearInterval(watch);
      return;
    }
    if (document.visibilityState !== "visible") return;
    void Bridge.phoneLinkStatus().then((status) => {
      if (status?.running) {
        latest = status;
        clients.textContent = `Connected phones: ${status.clients}`;
      }
    });
  }, 4000);

  void Bridge.phoneLinkStatus().then(draw);
  void Bridge.phoneChatStatus().then(renderChatSettings).catch((error: unknown) => {
    chatFeedback.textContent = `Could not load phone chat settings: ${String(error)}`;
  });
  return section;
}
