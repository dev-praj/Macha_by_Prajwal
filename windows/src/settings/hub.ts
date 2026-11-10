import { Bridge } from "../core/bridge";
import { mountAvatarIcon } from "../macha/avatar";
import { h, clear, svg } from "../views/dom";
import { ICONS } from "../views/icons";
import { HUB_GROUPS } from "./hub-pages";

export interface HubPanel {
  sections?: HTMLElement[];
  unavailable?: string;
}

const HUB_ICONS: Record<string, string> = {
  dictation: ICONS.bubble,
  notetaker: ICONS.doc,
  statistics: ICONS.timer,
  dictionary: ICONS.doc,
  snippets: ICONS.stack,
  styles: ICONS.doc,
  "flow-settings": ICONS.gear,
  general: ICONS.gear,
  agents: ICONS.bubble,
  chat: ICONS.bubble,
  voice: ICONS.speakerOn,
  integrations: ICONS.stack,
  phone: ICONS.bubble,
  supabase: ICONS.stack,
  shortcuts: ICONS.gear,
  memory: ICONS.doc,
  workflows: ICONS.stack,
  automation: ICONS.gear,
  vision: ICONS.bubble,
};

const GROUP_DESCRIPTIONS: Record<string, string> = {
  Flow: "Dictation and tools for your everyday work.",
  Notch: "Personalize how Macha listens, responds, and connects.",
  Macha: "Manage Macha's memory and automation.",
};

export function weeklyRecapSection(): HTMLElement {
  const status = h("div", { class: "hint", text: "Loading weekly recap…" });
  const metrics = h("div", { class: "hub-recap-grid" });
  const enabled = h("button", { class: "secondary", text: "Loading…", type: "button", disabled: true });
  const clearHistory = h("button", { class: "danger", text: "Clear history", type: "button", disabled: true });
  const section = h(
    "section",
    {},
    h("h2", {}, h("span", { text: "Weekly recap" })),
    h("div", { class: "hint", text: "A local summary of Macha activity from the last seven days." }),
    status,
    metrics,
    h("div", { class: "row" }, enabled, clearHistory),
  );

  const refresh = async () => {
    status.textContent = "Loading weekly recap…";
    metrics.replaceChildren();
    try {
      const snapshot = await Bridge.recapSnapshot();
      status.textContent = "";
      enabled.textContent = snapshot.enabled ? "Pause recap" : "Enable recap";
      enabled.disabled = false;
      clearHistory.disabled = false;
      for (const [label, value] of [
        ["Sessions", snapshot.sessions],
        ["Files changed", snapshot.filesChanged],
        ["Lines added", snapshot.linesAdded],
        ["Lines removed", snapshot.linesRemoved],
        ["Commands run", snapshot.commandsRun],
        ["Approvals allowed", snapshot.permissionsAllowed],
        ["Approvals denied", snapshot.permissionsDenied],
      ] as const) {
        metrics.append(h("div", { class: "hub-recap-metric" },
          h("strong", { text: String(value) }),
          h("span", { text: label }),
        ));
      }
      enabled.onclick = () => {
        void Bridge.recapSetEnabled(!snapshot.enabled).then(refresh).catch((error: unknown) => {
          status.textContent = `Could not update recap settings: ${String(error)}`;
        });
      };
    } catch (error) {
      status.textContent = `Could not load weekly recap: ${String(error)}`;
      enabled.textContent = "Unavailable";
      enabled.disabled = true;
      clearHistory.disabled = true;
    }
  };

  clearHistory.onclick = () => {
    if (!window.confirm("Clear the locally stored weekly recap history?")) return;
    void Bridge.recapClear().then(refresh).catch((error: unknown) => {
      status.textContent = `Could not clear recap history: ${String(error)}`;
    });
  };
  void refresh();
  return section;
}

export function mountSettingsHub(
  root: HTMLElement,
  panels: Record<string, HubPanel>,
  version: string,
): void {
  document.body.classList.add("hub-window");
  const sidebar = h("nav", { class: "hub-side", "aria-label": "Macha features" });
  const brandMark = h("span", { class: "hub-brand-mark" });
  mountAvatarIcon(brandMark, 34);
  sidebar.append(
    h("div", { class: "hub-brand" }, brandMark, h("span", { text: "Macha" })),
  );
  const content = h("main", { class: "hub-panel", "aria-live": "polite" });
  const pageHeader = h("header", { class: "hub-page-header" },
    h("div", { class: "hub-page-kicker" }),
    h("h1", {}),
    h("p", {}),
  );
  const pageKicker = pageHeader.querySelector<HTMLElement>(".hub-page-kicker")!;
  const pageTitle = pageHeader.querySelector<HTMLHeadingElement>("h1")!;
  const pageDescription = pageHeader.querySelector<HTMLParagraphElement>("p")!;
  const buttons = new Map<string, HTMLButtonElement>();
  let current = "";

  const open = (id: string, title: string) => {
    if (current === id) return;
    current = id;
    for (const [itemId, button] of buttons) {
      const selected = itemId === id;
      button.classList.toggle("on", selected);
      if (selected) button.setAttribute("aria-current", "page");
      else button.removeAttribute("aria-current");
    }
    clear(content);
    const group = HUB_GROUPS.find((candidate) => candidate.items.some((item) => item.id === id));
    pageKicker.textContent = group?.title ?? "Macha";
    pageTitle.textContent = title;
    pageDescription.textContent = GROUP_DESCRIPTIONS[group?.title ?? ""] ?? "Manage your Macha settings.";
    content.append(pageHeader);
    const panel = panels[id];
    if (panel?.sections) {
      content.append(...panel.sections);
      return;
    }
    content.append(h("section", { class: "hub-unavailable" },
      h("h2", {}, h("span", { text: title })),
      h("p", { class: "hint", text: panel?.unavailable ?? "This feature is not available yet." }),
    ));
  };

  for (const group of HUB_GROUPS) {
    sidebar.append(h("div", { class: "hub-group", text: group.title }));
    for (const item of group.items) {
      const button = h("button", {
        class: "hub-item",
        type: "button",
        "aria-label": item.title,
        onclick: () => open(item.id, item.title),
      });
      button.append(svg(HUB_ICONS[item.id] ?? ICONS.gear, 17, { stroke: 1.7 }), h("span", { text: item.title }));
      buttons.set(item.id, button);
      sidebar.append(button);
    }
  }
  sidebar.append(h("div", { class: "spacer" }), h("div", { class: "hub-version", text: `Version ${version}` }));
  clear(root);
  root.append(sidebar, content);
  window.addEventListener("macha-cloud-open-phone", () => open("phone", "Android phone"));
  window.addEventListener("macha-open-connection", () => open("connection", "Connection"));
  open("dictation", "Dictation");
}
