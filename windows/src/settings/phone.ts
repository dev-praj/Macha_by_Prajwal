import { Bridge, type PhoneLinkPairing, type PhoneLinkStatus } from "../core/bridge";
import { h, clear } from "../views/dom";

type Toggle = (on: boolean, onChange: (value: boolean) => void) => HTMLElement;

export function phoneSection(initial: boolean, detailsEnabled: boolean, makeToggle: Toggle): HTMLElement {
  const feedback = h("div", { class: "hint" });
  const detail = h("div", {});
  const state = h("div", { class: "hint" });
  const clients = h("div", { class: "hint" });
  let latest: PhoneLinkStatus | null = null;

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
  return section;
}
