// Settings window — the place where anything that writes to disk is confirmed.
// Stage 2 covers the Claude Code hooks and the general preferences; API keys and
// integrations land here too in a later stage.

import "./settings.css";
import {
  Bridge,
  onEvent,
  type HookStatus,
  type MemoryRecord,
  type OllamaStatus,
  type OpenRouterAccount,
  type SavedWorkflow,
} from "../core/bridge";
import { DEFAULT_SETTINGS, type Settings } from "../core/state";
import { State } from "../core/state";
import * as TTS from "../core/tts";
import { recordWakePronunciation } from "../core/wakeWord";
import { h, clear } from "../views/dom";

let settings: Settings = { ...DEFAULT_SETTINGS };
let version = "";

const root = document.getElementById("settings-root")!;

async function save() {
  State.settings = { ...State.settings, ...settings };
  await Bridge.saveSettings(settings);
}

// ── Reusable bits ─────────────────────────────────────────────────────────────

function toggle(on: boolean, onChange: (v: boolean) => void): HTMLElement {
  const el = h("button", { class: on ? "switch on" : "switch", "aria-pressed": on });
  el.addEventListener("click", () => {
    const next = !el.classList.contains("on");
    el.classList.toggle("on", next);
    el.setAttribute("aria-pressed", String(next));
    onChange(next);
  });
  return el;
}

function statusDot(ok: boolean): HTMLElement {
  return h("i", { class: "dot", style: `background:${ok ? "#22c55e" : "#f4505e"}` });
}

function renderDiff(text: string): HTMLElement {
  const box = h("div", { class: "diff" });
  for (const line of text.split("\n")) {
    const cls = line.startsWith("+") ? "add" : line.startsWith("-") ? "del" : "ctx";
    box.append(h("div", { class: cls, text: line }));
  }
  return box;
}

// ── Claude Code section ───────────────────────────────────────────────────────

function claudeSection(status: HookStatus): HTMLElement {
  const body = h("div", { style: "display:flex;flex-direction:column;gap:12px" });
  const section = h(
    "section",
    {},
    h("h2", {}, statusDot(status.installed), h("span", { text: "Claude Code" })),
    body,
  );

  const rebuild = async () => {
    const fresh = await Bridge.hooksStatus();
    if (fresh) Object.assign(status, fresh);
    clear(body);
    draw();
    const head = section.querySelector("h2")!;
    clear(head);
    head.append(statusDot(status.installed), h("span", { text: "Claude Code" }));
  };

  function draw() {
    body.append(
      h("div", {
        class: "hint",
        text: status.installed
          ? "Coucou is hooked into your Claude Code sessions. Tool calls, questions and permission requests show up in the island, and you can answer them there."
          : "Install the hooks to see your Claude Code sessions in the island and approve permissions without leaving what you are doing.",
      }),
      h("div", { class: "row" },
        h("label", { text: "settings.json" }),
        h("span", { class: "path", text: status.settingsPath }),
      ),
      h("div", { class: "row" },
        h("label", { text: "Relay" }),
        h("span", { class: "path", text: status.hookPath }),
        statusDot(status.hookReady),
      ),
    );

    if (!status.hookReady) {
      body.append(h("div", {
        class: "notice warn",
        text: "coucou-hook.exe is not in place yet. Restart Coucou; if it still fails, build it with `cargo build -p coucou-hook`.",
      }));
    }

    const actions = h("div", { class: "row" });
    const install = h("button", {
      class: "primary",
      text: status.installed ? "Reinstall hooks…" : "Install hooks…",
      onclick: () => showPreview(true),
    });
    // Writing hook commands that point at a relay which isn't there would give
    // every Claude Code session a broken hook and nothing to show for it.
    if (!status.hookReady) {
      install.disabled = true;
      install.title = "The relay isn't installed yet.";
    }
    actions.append(install);
    if (status.installed) {
      actions.append(h("button", {
        class: "danger",
        text: "Uninstall hooks…",
        onclick: () => showPreview(false),
      }));
    }
    body.append(actions);
  }

  async function showPreview(install: boolean) {
    let preview;
    try {
      preview = await Bridge.hooksPreview(install);
    } catch (err) {
      // An unreadable or invalid settings.json stops here rather than being
      // treated as empty and written over.
      clear(body);
      body.append(
        h("div", { class: "notice err", text: String(err).replace(/^Error:\s*/, "") }),
        h("div", { class: "row" }, h("button", {
          text: "Back",
          onclick: () => { clear(body); draw(); },
        })),
      );
      return;
    }
    if (!preview) return;
    clear(body);
    body.append(
      h("div", {
        class: "hint",
        text: install
          ? "This is exactly what will change in your settings.json. Your own hooks are left untouched."
          : "This removes Coucou's entries only. Your own hooks are left untouched.",
      }),
      renderDiff(preview.diff),
      h("div", { class: "row" },
        h("span", { class: "path", text: `Backup → ${preview.backup}` }),
      ),
    );
    const confirm = h("button", {
      class: install ? "primary" : "danger",
      text: install ? "Back up and write" : "Back up and remove",
    });
    confirm.addEventListener("click", async () => {
      confirm.disabled = true;
      try {
        const backup = await Bridge.hooksApply(install, preview.fingerprint);
        clear(body);
        body.append(h("div", {
          class: "notice ok",
          text: `Done. Previous settings saved as ${backup}. Open a new Claude Code session to pick the hooks up.`,
        }));
        window.setTimeout(() => void rebuild(), 2600);
      } catch (err) {
        confirm.disabled = false;
        body.append(h("div", { class: "notice err", text: `Could not write: ${String(err)}` }));
      }
    });
    body.append(h("div", { class: "row" }, confirm, h("button", {
      text: "Cancel",
      onclick: () => { clear(body); draw(); },
    })));
  }

  draw();
  return section;
}

// ── Claude API section ────────────────────────────────────────────────────────

const MODELS: [string, string][] = [
  ["openrouter/free", "Automatic — select a free model for each task"],
  ["stealth/space-bunny-alpha", "Space Bunny Alpha — general, coding, reasoning, vision"],
  ["nvidia/nemotron-3-ultra-550b-a55b:free", "Nemotron 3 Ultra — complex reasoning, planning, coding"],
  ["poolside/laguna-s-2.1:free", "Laguna S 2.1 — software engineering and coding agents"],
  ["nvidia/nemotron-3.5-lightning:free", "Nemotron 3.5 Lightning — fast agentic tasks"],
  ["dots-studio/dots-3-note-preview:free", "Dots 3 Note — reasoning, coding, multimodal, long context"],
  ["inclusionai/ling-3.0-flash-sante:free", "Ling 3.0 Flash Sante — health and evidence-based reasoning"],
  ["nvidia/nemotron-3-super-120b-a12b:free", "Nemotron 3 Super — general reasoning and planning"],
  ["thinkingmachines/inkling:free", "Inkling — reasoning, coding, tools, multilingual, vision/audio"],
  ["thinkingmachines/inkling-small:free", "Inkling Small — fast reasoning, coding, agents, multilingual"],
  ["qwen/qwen3.8-27b:free", "Qwen 3.8 27B — coding, research, agents, vision"],
  ["cohere/north-mini-code:free", "Cohere North Mini Code — agentic software engineering"],
  ["poolside/laguna-xs-2.1:free", "Laguna XS 2.1 — fast coding agent"],
  ["apodex/apodex-1.1-mini:free", "Apodex 1.1 Mini — research, forecasting, files and code"],
];

function apiSection(
  initialAccounts: OpenRouterAccount[],
  initialOllamaStatus: OllamaStatus,
  accountLoadError?: string,
): HTMLElement {
  let accounts = initialAccounts;
  let ollama = initialOllamaStatus;
  const dot = statusDot(accounts.length > 0 || ollama.reachable);
  const state = h("span", { class: "hint" });
  const feedback = h("div", {});
  const accountsPanel = h("div", { class: "openrouter-accounts", style: "display:none" });
  const toggleAccounts = h("button", { text: "View uploaded keys" });
  const addButton = h("button", { class: "primary", text: "+ Add account" });
  const addForm = h("div", { class: "openrouter-add-form", style: "display:none" });
  const nameField = h("input", {
    type: "text",
    placeholder: "Account name",
    maxlength: "80",
    autocomplete: "off",
    style: "flex:1 1 160px;min-width:0",
  }) as HTMLInputElement;
  const keyField = h("input", {
    type: "password",
    placeholder: "sk-or-v1-...",
    autocomplete: "off",
    spellcheck: "false",
    style: "flex:2 1 240px;min-width:0",
  }) as HTMLInputElement;
  const saveAccount = h("button", { text: "Save account" });
  addForm.append(
    h("div", { class: "row" },
      h("label", { text: "Account name" }),
      nameField,
      keyField,
      saveAccount,
    ),
  );

  function updateState() {
    dot.style.background = accounts.length || ollama.reachable ? "#22c55e" : "#f4505e";
    const ollamaText = ollama.reachable
      ? `Ollama ready (${ollama.models.length} model${ollama.models.length === 1 ? "" : "s"}).`
      : "Ollama unavailable.";
    const accountText = accounts.length
      ? `${accounts.length} OpenRouter account${accounts.length === 1 ? "" : "s"} stored securely.`
      : "No OpenRouter fallback key.";
    state.textContent = `${ollamaText} ${accountText}`;
    toggleAccounts.textContent = accountsPanel.style.display === "none"
      ? "View uploaded keys"
      : "Hide uploaded keys";
  }

  function renderAccounts() {
    clear(accountsPanel);
    for (const account of accounts) {
      const keyField = h("input", {
        type: "password",
        placeholder: "••••••••••••",
        readonly: true,
        autocomplete: "off",
        style: "flex:1 1 auto;min-width:0",
        "aria-label": `${account.name} API key`,
      }) as HTMLInputElement;
      const revealButton = h("button", { text: "👁", title: "Reveal API key", "aria-label": `Reveal ${account.name} API key` });
      let revealed = false;
      revealButton.addEventListener("click", async () => {
        clear(feedback);
        if (revealed) {
          keyField.value = "";
          keyField.type = "password";
          revealButton.title = "Reveal API key";
          revealed = false;
          return;
        }
        revealButton.disabled = true;
        try {
          keyField.value = await Bridge.openRouterAccountReveal(account.id);
          keyField.type = "text";
          revealButton.title = "Hide API key";
          revealed = true;
        } catch (err) {
          feedback.append(h("div", { class: "notice err", text: `Could not reveal key: ${String(err)}` }));
        } finally {
          revealButton.disabled = false;
        }
      });
      const removeButton = h("button", { class: "danger", text: "Remove" });
      removeButton.addEventListener("click", async () => {
        if (!confirm(`Remove the OpenRouter key for "${account.name}"?`)) return;
        removeButton.disabled = true;
        clear(feedback);
        try {
          await Bridge.openRouterAccountRemove(account.id);
          accounts = accounts.filter((item) => item.id !== account.id);
          renderAccounts();
          updateState();
          feedback.append(h("div", { class: "notice ok", text: "Account removed." }));
        } catch (err) {
          removeButton.disabled = false;
          feedback.append(h("div", { class: "notice err", text: `Could not remove account: ${String(err)}` }));
        }
      });
      accountsPanel.append(
        h("div", { class: "openrouter-account" },
          h("div", { class: "openrouter-account-name", text: account.name }),
          h("div", { class: "row openrouter-account-key" }, keyField, revealButton, removeButton),
        ),
      );
    }
  }

  toggleAccounts.addEventListener("click", async () => {
    clear(feedback);
    if (accountsPanel.style.display !== "none") {
      accountsPanel.style.display = "none";
      updateState();
      return;
    }
    try {
      accounts = await Bridge.openRouterAccounts();
      renderAccounts();
      accountsPanel.style.display = "";
      updateState();
    } catch (err) {
      feedback.append(h("div", { class: "notice err", text: `Could not load accounts: ${String(err)}` }));
    }
  });

  addButton.addEventListener("click", () => {
    addForm.style.display = addForm.style.display === "none" ? "" : "none";
    if (addForm.style.display !== "none") nameField.focus();
  });

  saveAccount.addEventListener("click", async () => {
    clear(feedback);
    if (!nameField.value.trim() || !keyField.value.trim()) {
      feedback.append(h("div", { class: "notice warn", text: "Enter both an account name and an API key." }));
      return;
    }
    saveAccount.disabled = true;
    try {
      await Bridge.openRouterAccountAdd(nameField.value, keyField.value);
      nameField.value = "";
      keyField.value = "";
      addForm.style.display = "none";
      accounts = await Bridge.openRouterAccounts();
      renderAccounts();
      if (accountsPanel.style.display !== "none") accountsPanel.style.display = "";
      updateState();
      feedback.append(h("div", { class: "notice ok", text: "Account saved securely in the OS credential manager." }));
    } catch (err) {
      feedback.append(h("div", { class: "notice err", text: `Could not save account: ${String(err)}` }));
    } finally {
      saveAccount.disabled = false;
    }
  });

  const model = h("select", {}) as HTMLSelectElement;
  for (const [id, label] of MODELS) model.append(h("option", { value: id, text: label }));
  if (!MODELS.some(([id]) => id === settings.model)) {
    model.append(h("option", { value: settings.model, text: settings.model }));
  }
  model.value = settings.model;
  model.addEventListener("change", () => {
    settings.model = model.value;
    void save();
  });

  const providerMode = h("select", {}) as HTMLSelectElement;
  providerMode.append(
    h("option", { value: "auto", text: "Auto — Ollama first, OpenRouter fallback" }),
    h("option", { value: "ollamaOnly", text: "Ollama only" }),
    h("option", { value: "openRouterOnly", text: "OpenRouter only" }),
  );
  providerMode.value = settings.providerMode;
  providerMode.addEventListener("change", () => {
    settings.providerMode = providerMode.value as Settings["providerMode"];
    void save();
  });

  const ollamaModel = h("select", {}) as HTMLSelectElement;
  const ollamaWarning = h("div", {});
  function renderOllamaModels() {
    const selected = settings.ollamaModel;
    const available = new Set(["qwen2.5:7b", "gpt-oss:20b", ...ollama.models]);
    if (!available.has(selected)) available.add(selected);
    clear(ollamaModel);
    for (const name of available) {
      ollamaModel.append(h("option", { value: name, text: name }));
    }
    ollamaModel.value = selected;
    clear(ollamaWarning);
    if (ollama.reachable && !ollama.models.includes(selected)) {
      ollamaWarning.append(h("div", {
        class: "notice warn",
        text: `Ollama model ${selected} is not installed. Run: ollama pull ${selected}`,
      }));
    } else if (!ollama.reachable) {
      ollamaWarning.append(h("div", {
        class: "notice warn",
        text: `Ollama is unavailable: ${ollama.error ?? "could not connect"}. Auto mode will use OpenRouter if configured.`,
      }));
    }
  }
  ollamaModel.addEventListener("change", () => {
    settings.ollamaModel = ollamaModel.value;
    renderOllamaModels();
    void save();
  });
  const refreshOllama = h("button", { text: "Check Ollama" });
  refreshOllama.addEventListener("click", async () => {
    refreshOllama.disabled = true;
    clear(feedback);
    try {
      ollama = await Bridge.ollamaStatus(true);
      renderOllamaModels();
      updateState();
    } catch (error) {
      feedback.append(h("div", {
        class: "notice err",
        text: `Could not check Ollama: ${String(error).replace(/^Error:\s*/, "")}`,
      }));
    } finally {
      refreshOllama.disabled = false;
    }
  });
  renderOllamaModels();

  renderAccounts();
  updateState();
  if (accountLoadError) {
    feedback.append(h("div", { class: "notice err", text: `Could not load accounts: ${accountLoadError}` }));
  }

  return h(
    "section",
    {},
    h("h2", {}, dot, h("span", { text: "LLM providers" })),
    h("div", { class: "row" }, h("label", { text: "Provider mode" }), providerMode),
    h("div", { class: "row" },
      h("label", { text: "Ollama model" }), ollamaModel, refreshOllama,
    ),
    ollamaWarning,
    h("p", {
      class: "hint",
      text: "Auto tries local Ollama first and uses OpenRouter only if Ollama is unavailable. Ollama-only never contacts OpenRouter.",
    }),
    h("h3", { text: "OpenRouter fallback" }),
    state,
    h("div", { class: "row" }, addButton, toggleAccounts),
    addForm,
    accountsPanel,
    h("div", { class: "row" }, h("label", { text: "Model" }), model),
    h("p", { class: "hint", text: "Automatic selects a suitable free model for each fallback prompt. You can choose any listed model to pin it instead. Some providers may retain prompts; avoid sending sensitive data to models you do not trust." }),
    feedback,
  );
}

// ── Integrations section ──────────────────────────────────────────────────────

interface IntegrationDef {
  id: string;
  name: string;
  color: string;
  /** Credential Manager keys, in the order they are shown. */
  fields: { key: string; label: string; placeholder: string; secret: boolean }[];
}

const INTEGRATIONS: IntegrationDef[] = [
  { id: "integration_messages", name: "Messages", color: "#22C55E", fields: [] },
  { id: "integration_spotify", name: "Spotify", color: "#1DB954",
    fields: [{ key: "spotify-client-id", label: "Client ID", placeholder: "Spotify app Client ID", secret: false }] },
  { id: "integration_stripe", name: "Stripe", color: "#0570DE",
    fields: [{ key: "stripe-api-key", label: "Secret key", placeholder: "sk_live_…", secret: true }] },
  { id: "integration_github", name: "GitHub", color: "#F4505E",
    fields: [{ key: "github-token", label: "Token", placeholder: "ghp_…", secret: true }] },
  { id: "integration_vercel", name: "Vercel", color: "#7C5CFF",
    fields: [{ key: "vercel-token", label: "Token", placeholder: "…", secret: true }] },
  { id: "integration_notion", name: "Notion", color: "#8C8C8C",
    fields: [{ key: "notion-api-key", label: "Integration token", placeholder: "ntn_…", secret: true }] },
  { id: "integration_calcom", name: "Cal.com", color: "#C9956A",
    fields: [{ key: "calcom-api-key", label: "API key", placeholder: "cal_…", secret: true }] },
];

const MAX_ACTIVE = 4;

function integrationsSection(present: Record<string, boolean>): HTMLElement {
  const note = h("div", { class: "hint" });
  const list = h("div", { style: "display:flex;flex-direction:column;gap:14px" });

  function updateNote() {
    const used = settings.activeIntegrations.length;
    note.textContent = `Pick up to ${MAX_ACTIVE} pills to show next to Macha — ${used}/${MAX_ACTIVE} in use. Keys are stored in the Windows Credential Manager, never on disk.`;
  }

  const elevenLabsKey = "elevenlabs-api-key";
  const elevenLabsInput = h("input", {
    type: "password",
    placeholder: present[elevenLabsKey] ? "••••••••  (stored)" : "API key",
    autocomplete: "off",
    spellcheck: "false",
    style: "flex:1 1 auto;min-width:0",
  }) as HTMLInputElement;
  const elevenLabsDot = statusDot(present[elevenLabsKey] ?? false);
  const elevenLabsFeedback = h("div", { class: "hint" });
  const saveElevenLabsKey = h("button", { text: "Save" });
  saveElevenLabsKey.addEventListener("click", async () => {
    saveElevenLabsKey.disabled = true;
    clear(elevenLabsFeedback);
    const value = elevenLabsInput.value.trim();
    try {
      await Bridge.secretSet(elevenLabsKey, value);
      present[elevenLabsKey] = value.length > 0;
      elevenLabsInput.value = "";
      elevenLabsInput.placeholder = value ? "••••••••  (stored)" : "API key";
      elevenLabsDot.style.background = value ? "#22c55e" : "#f4505e";
      elevenLabsFeedback.textContent = "Saved securely in the OS credential manager.";
    } catch (error) {
      elevenLabsFeedback.textContent = `Could not save key: ${String(error).replace(/^Error:\s*/, "")}`;
    } finally {
      saveElevenLabsKey.disabled = false;
    }
  });
  list.append(
    h("div", { style: "display:flex;gap:12px;align-items:flex-start" },
      h("div", { style: "display:flex;align-items:center;gap:8px;min-width:132px;padding-top:4px" },
        h("i", { class: "dot", style: "background:#F4C15D" }),
        h("span", { style: "font-size:12.5px", text: "ElevenLabs" }),
      ),
      h("div", { style: "display:flex;flex-direction:column;gap:6px;flex:1 1 auto;min-width:0" },
        h("div", { class: "row" },
          h("label", { style: "min-width:104px", text: "API key" }),
          elevenLabsInput, saveElevenLabsKey, elevenLabsDot,
        ),
        elevenLabsFeedback,
      ),
    ),
  );

  for (const def of INTEGRATIONS) {
    const active = settings.activeIntegrations.includes(def.id);
    const sw = h("button", { class: active ? "switch on" : "switch" });
    sw.addEventListener("click", () => {
      const on = settings.activeIntegrations.includes(def.id);
      if (on) {
        settings.activeIntegrations = settings.activeIntegrations.filter((x) => x !== def.id);
      } else {
        if (settings.activeIntegrations.length >= MAX_ACTIVE) return;
        settings.activeIntegrations = [...settings.activeIntegrations, def.id];
      }
      sw.classList.toggle("on", !on);
      updateNote();
      void save();
    });

    const rows = h("div", { style: "display:flex;flex-direction:column;gap:6px;flex:1 1 auto;min-width:0" });
    if (def.id === "integration_messages") {
      const enable = h("button", { text: "Allow Windows notifications" });
      enable.addEventListener("click", async () => {
        try {
          await Bridge.requestMessageAccess();
          enable.textContent = (await Bridge.messageAccessStatus())
            ? "Notification access enabled"
            : "Windows did not grant notification access";
        } catch (error) {
          enable.textContent = String(error).replace(/^Error:\s*/, "");
        }
      });
      rows.append(
        enable,
        h("div", {
          class: "hint",
          text: "Coucou shows notifications from all apps here. Turn off notification banners for individual apps in Windows Settings to avoid duplicate pop-ups.",
        }),
      );
    }
    if (def.id === "integration_spotify") {
      const connect = h("button", { text: "Connect Spotify" });
      connect.addEventListener("click", async () => {
        try {
          await Bridge.openUrlStrict(await Bridge.spotifyConnect());
          connect.textContent = "Complete sign-in in your browser";
        } catch (error) {
          connect.textContent = String(error).replace(/^Error:\s*/, "");
        }
      });
      const disconnect = h("button", {
        text: "Disconnect Spotify",
        onclick: async () => {
          try {
            await Bridge.spotifyDisconnect();
            disconnect.textContent = "Spotify disconnected";
          } catch (error) {
            disconnect.textContent = String(error).replace(/^Error:\s*/, "");
          }
        },
      });
      rows.append(
        connect,
        disconnect,
        h("div", {
          class: "hint",
          text: "In your Spotify Developer app, add this redirect URI exactly: http://127.0.0.1:43821/callback. Playback control may require Spotify Premium.",
        }),
      );
    }
    for (const field of def.fields) {
      const input = h("input", {
        type: field.secret ? "password" : "text",
        placeholder: present[field.key] ? "••••••••  (stored)" : field.placeholder,
        autocomplete: "off",
        spellcheck: "false",
        style: "flex:1 1 auto;min-width:0",
      }) as HTMLInputElement;
      const saveBtn = h("button", { text: "Save" });
      const dotEl = statusDot(present[field.key] ?? false);
      saveBtn.addEventListener("click", async () => {
        const value = input.value.trim();
        try {
          await Bridge.secretSet(field.key, value);
          present[field.key] = value.length > 0;
          input.value = "";
          input.placeholder = value ? "••••••••  (stored)" : field.placeholder;
          dotEl.style.background = value ? "#22c55e" : "#f4505e";
        } catch {
          dotEl.style.background = "#f5a524";
        }
      });
      rows.append(
        h("div", { class: "row" },
          h("label", { style: "min-width:104px", text: field.label }),
          input, saveBtn, dotEl,
        ),
      );
    }

    list.append(
      h("div", { style: "display:flex;gap:12px;align-items:flex-start" },
        h("div", { style: "display:flex;align-items:center;gap:8px;min-width:132px;padding-top:4px" },
          sw,
          h("i", { class: "dot", style: `background:${def.color}` }),
          h("span", { style: "font-size:12.5px", text: def.name }),
        ),
        rows,
      ),
    );
  }

  updateNote();
  return h("section", {}, h("h2", {}, h("span", { text: "Integrations" })), note, list);
}

// ── Vision Mode ───────────────────────────────────────────────────────────────

function gestureMouseSection(): HTMLElement {
  let running = false;
  let busy = false;
  let stopRequested = false;
  const status = h("div", { class: "hint", text: "Checking Vision Mode…" });
  const feedback = h("div", { class: "hint" });
  const start = h("button", { text: "Start Vision Mode" });
  const stop = h("button", { text: "Stop Vision Mode" });
  const dashboard = h("iframe", {
    class: "gesture-dashboard",
    title: "Vision Mode live dashboard",
    loading: "lazy",
    referrerpolicy: "no-referrer",
    style: "display:none",
  }) as HTMLIFrameElement;

  function showDashboard(on: boolean) {
    dashboard.style.display = on ? "block" : "none";
    if (on && dashboard.src !== "http://127.0.0.1:8765/") {
      dashboard.src = "http://127.0.0.1:8765/";
    } else if (!on && dashboard.hasAttribute("src")) {
      dashboard.removeAttribute("src");
    }
  }

  function update() {
    start.disabled = busy || running;
    stop.disabled = busy || !running;
    showDashboard(running);
    status.textContent = running
      ? "Vision Mode is running. Close the camera preview or stop it here."
      : "Vision Mode is stopped.";
  }

  async function refresh() {
    try {
      const wasRunning = running;
      running = await Bridge.gestureMouseIsRunning();
      if (wasRunning && !running && !stopRequested) {
        feedback.textContent = "Vision Mode exited unexpectedly. Check Coucou's log for details.";
      }
      if (!running) stopRequested = false;
      update();
    } catch (error) {
      status.textContent = `Could not check Vision Mode: ${String(error).replace(/^Error:\s*/, "")}`;
    }
  }

  start.addEventListener("click", async () => {
    busy = true;
    stopRequested = false;
    clear(feedback);
    update();
    try {
      await Bridge.gestureMouseStart();
      feedback.textContent = "Starting camera preview and embedded dashboard…";
    } catch (error) {
      feedback.textContent = `Could not start Vision Mode: ${String(error).replace(/^Error:\s*/, "")}`;
    } finally {
      busy = false;
      await refresh();
    }
  });

  stop.addEventListener("click", async () => {
    busy = true;
    stopRequested = true;
    clear(feedback);
    update();
    try {
      await Bridge.gestureMouseStop();
      feedback.textContent = "Vision Mode stopped.";
    } catch (error) {
      stopRequested = false;
      feedback.textContent = `Could not stop Vision Mode: ${String(error).replace(/^Error:\s*/, "")}`;
    } finally {
      busy = false;
      await refresh();
    }
  });

  void refresh();
  window.setInterval(() => void refresh(), 1500);
  return h(
    "section",
    {},
    h("h2", {}, h("span", { text: "Vision Mode" })),
    h("div", {
      class: "hint",
      text: "Control the Windows cursor with your webcam: move your wrist to move, pinch thumb + index to click and hold, thumb + middle to right-click, and thumb + pinky to scroll. Hold an open palm steady for five seconds to lock or unlock the cursor; the camera preview shows a progress ring and confirms when it is done. To rotate a 3D view that supports mouse dragging, place the pointer over the model before locking, then hold thumb and index together or extend them, curl the other fingers, and turn your wrist clockwise or anticlockwise. While locked, move joined index and middle fingers to scroll in any direction. Start Vision Mode to open the camera preview and live dashboard here. You can also say “activate vision mode” while Hey Macha is listening.",
    }),
    status,
    h("div", { class: "row" }, start, stop),
    feedback,
    dashboard,
  );
}

// ── General section ───────────────────────────────────────────────────────────

function generalSection(): HTMLElement {
  const volume = h("input", {
    type: "range", min: "0", max: "0.2", step: "0.005",
    value: String(settings.soundVolume),
  }) as HTMLInputElement;
  volume.addEventListener("input", () => {
    settings.soundVolume = Number(volume.value);
    void save();
  });

  const autoClose = h("input", {
    type: "number", min: "5", max: "120", step: "1",
    value: String(Math.round(settings.autoCloseInterval)),
    style: "width:72px",
  }) as HTMLInputElement;
  autoClose.addEventListener("change", () => {
    settings.autoCloseInterval = Math.max(5, Math.min(120, Number(autoClose.value) || 15));
    autoClose.value = String(settings.autoCloseInterval);
    void save();
  });

  const screen = h("select", {}) as HTMLSelectElement;
  screen.append(
    h("option", { value: "primary", text: "Main display" }),
    h("option", { value: "cursor", text: "Display under the cursor" }),
  );
  screen.value = settings.screen;
  screen.addEventListener("change", () => {
    settings.screen = screen.value as Settings["screen"];
    void save();
  });

  const speakMode = h("select", {}) as HTMLSelectElement;
  speakMode.append(
    h("option", { value: "off", text: "Off" }),
    h("option", { value: "voiceOnly", text: "Voice messages only" }),
    h("option", { value: "always", text: "Always" }),
  );
  speakMode.value = settings.speakRepliesMode;
  speakMode.addEventListener("change", () => {
    settings.speakRepliesMode = speakMode.value as Settings["speakRepliesMode"];
    void save();
  });

  const wakePhraseHint = h("span", {
    class: "hint",
    text: `Saved phrase: “${settings.wakeWordPronunciation}”.`,
  });
  const voiceActivationStatus = h("span", {
    class: "hint",
    "aria-live": "polite",
    text: settings.wakeWordEnabled ? "Hey Macha is enabled." : "Hey Macha is disabled.",
  });
  const voiceActivationToggle = toggle(settings.wakeWordEnabled, (enabled) => {
    const previous = settings.wakeWordEnabled;
    settings.wakeWordEnabled = enabled;
    voiceActivationToggle.disabled = true;
    voiceActivationStatus.textContent = enabled
      ? "Enabling Hey Macha…"
      : "Disabling Hey Macha…";
    void Bridge.saveSettingsStrict(settings).then(() => {
      State.settings = { ...State.settings, ...settings };
      voiceActivationStatus.textContent = enabled ? "Hey Macha is enabled." : "Hey Macha is disabled.";
    }).catch((error: unknown) => {
      settings.wakeWordEnabled = previous;
      voiceActivationToggle.classList.toggle("on", previous);
      voiceActivationToggle.setAttribute("aria-pressed", String(previous));
      voiceActivationStatus.textContent = `Could not save the voice activation setting: ${
        error instanceof Error ? error.message : String(error)
      }`;
      console.error("[coucou] could not save Hey Macha voice activation", error);
    }).finally(() => {
      voiceActivationToggle.disabled = false;
    });
  }) as HTMLButtonElement;
  voiceActivationToggle.setAttribute("aria-label", "Hey Macha voice activation");
  const calibrateWakePhrase = h("button", { text: "Record “Hey Macha”" });
  calibrateWakePhrase.addEventListener("click", async () => {
    calibrateWakePhrase.disabled = true;
    wakePhraseHint.textContent = "Recording for 5 seconds — say “Hey Macha” a few times.";
    try {
      await Bridge.setWakeCalibration(true);
      await Bridge.setVoiceActive(true);
      const calibration = await recordWakePronunciation();
      settings.wakeWordPronunciation = calibration.transcript;
      settings.wakeWordThreshold = calibration.threshold;
      State.settings = { ...State.settings, ...settings };
      await Bridge.saveSettingsStrict(settings);
      wakePhraseHint.textContent = `Saved “${calibration.transcript}” and tuned microphone sensitivity. The recording was discarded.`;
    } catch (error) {
      wakePhraseHint.textContent = error instanceof Error
        ? error.message
        : "Could not calibrate the wake phrase.";
      console.error("[coucou] wake phrase calibration failed", error);
    } finally {
      try {
        await Bridge.setVoiceActive(false);
        await Bridge.setWakeCalibration(false);
      } catch (error) {
        console.error("[coucou] could not restore the wake listener after calibration", error);
      }
      calibrateWakePhrase.disabled = false;
    }
  });

  const engine = h("select", {}) as HTMLSelectElement;
  engine.append(
    h("option", { value: "auto", text: "Automatic (recommended)" }),
    h("option", { value: "webSpeech", text: "Web Speech (local voices)" }),
    h("option", { value: "sapi", text: "Windows SAPI" }),
  );
  engine.value = settings.ttsEngine;
  engine.addEventListener("change", () => {
    settings.ttsEngine = engine.value as Settings["ttsEngine"];
    void save();
  });

  const voice = h("select", {}) as HTMLSelectElement;
  voice.append(h("option", { value: "", text: "Automatic — preferred local voice" }));
  voice.value = settings.ttsVoice;
  voice.addEventListener("change", () => {
    settings.ttsVoice = voice.value;
    void save();
  });
  void TTS.availableVoices().then((voices) => {
    if (settings.ttsVoice && !voices.some((item) => item.voiceURI === settings.ttsVoice)) {
      voice.append(h("option", {
        value: settings.ttsVoice,
        text: `${settings.ttsVoice} (unavailable or not English)`,
      }));
    }
    for (const item of voices) {
      voice.append(h("option", { value: item.voiceURI, text: `${item.name} (${item.lang})` }));
    }
    voice.value = settings.ttsVoice;
  }).catch((error: unknown) => {
    console.error("[coucou] could not list local speech voices", error);
  });

  const rateLabel = h("span", { text: `${settings.ttsRate.toFixed(1)}×` });
  const rate = h("input", {
    type: "range", min: "0.8", max: "1.4", step: "0.1", value: String(settings.ttsRate),
  }) as HTMLInputElement;
  rate.addEventListener("input", () => {
    settings.ttsRate = Number(rate.value);
    rateLabel.textContent = `${settings.ttsRate.toFixed(1)}×`;
  });
  rate.addEventListener("change", () => void save());

  const ttsVolumeLabel = h("span", { text: `${Math.round(settings.ttsVolume * 100)}%` });
  const ttsVolume = h("input", {
    type: "range", min: "0", max: "1", step: "0.05", value: String(settings.ttsVolume),
  }) as HTMLInputElement;
  ttsVolume.addEventListener("input", () => {
    settings.ttsVolume = Number(ttsVolume.value);
    ttsVolumeLabel.textContent = `${Math.round(settings.ttsVolume * 100)}%`;
  });
  ttsVolume.addEventListener("change", () => void save());

  const testVoice = h("button", { text: "Test voice" });
  const voiceFeedback = h("span", { class: "hint" });
  TTS.onStateChange((speaking, error) => {
    if (error) voiceFeedback.textContent = error;
    else if (speaking) voiceFeedback.textContent = "Speaking…";
    else voiceFeedback.textContent = "";
  });
  testVoice.addEventListener("click", () => {
    void save();
    void TTS.speak("Hi, I'm coucou.");
  });

  return h(
    "section",
    {},
    h("h2", {}, h("span", { text: "General" })),
    h("div", { class: "row" },
      h("label", { text: "Sound" }),
      toggle(settings.soundEnabled, (v) => { settings.soundEnabled = v; void save(); }),
      volume,
    ),
    h("div", { class: "row" },
      h("label", { text: "Auto-close" }),
      autoClose,
      h("span", { class: "hint", text: "seconds after you leave the island" }),
    ),
    h("div", { class: "row" },
      h("label", { text: "Island lives on" }),
      screen,
    ),
    h("div", { class: "row" },
      h("label", { text: "Launch at startup" }),
      toggle(settings.autostart, (v) => { settings.autostart = v; void save(); }),
    ),
    h("div", { class: "row" },
      h("label", { text: "Speak replies" }),
      speakMode,
    ),
    h("div", { class: "row" },
      h("label", { text: "Voice activation" }),
      voiceActivationToggle,
      voiceActivationStatus,
      h("span", {
        class: "hint",
        text: "When enabled, Coucou listens on this device and sends speech only to its local Whisper service. Allow microphone access in Windows privacy settings when prompted. The Voice button remains available when this is off.",
      }),
    ),
    h("div", { class: "row" },
      h("label", { text: "Your pronunciation" }),
      calibrateWakePhrase,
      wakePhraseHint,
    ),
    h("div", { class: "row" },
      h("label", { text: "Speech engine" }),
      engine,
    ),
    h("div", { class: "row" },
      h("label", { text: "Local voice" }),
      voice,
    ),
    h("div", { class: "row" },
      h("label", { text: "Speech rate" }),
      rate,
      rateLabel,
    ),
    h("div", { class: "row" },
      h("label", { text: "Speech volume" }),
      ttsVolume,
      ttsVolumeLabel,
    ),
    h("div", { class: "row" },
      h("label", { text: "Test" }),
      testVoice,
      voiceFeedback,
    ),
  );
}

function automationSection(): HTMLElement {
  return h(
    "section",
    {},
    h("h2", {}, h("span", { text: "Local automation" })),
    h("div", {
      class: "hint",
      text: "Macha can access files anywhere on this computer that your account can access. Use an absolute path to specify a location; previously selected folders are still preferred for relative paths. Every action needs your approval. File edits show a preview and create a backup first. Deleting files and running commands are not available. Files you ask Macha to read are sent to the selected chat provider.",
    }),
  );
}

function workflowsSection(): HTMLElement {
  const list = h("div", { class: "workflow-list" });
  const feedback = h("div", {});
  const editorTitle = h("h3", { text: "Create a workflow" });
  const name = h("input", {
    type: "text",
    maxlength: "100",
    placeholder: "Workflow name",
    "aria-label": "Workflow name",
  }) as HTMLInputElement;
  const prompt = h("textarea", {
    maxlength: "10000",
    rows: "4",
    placeholder: "Describe the task Macha should perform when you run this workflow.",
    "aria-label": "Workflow instructions",
  }) as HTMLTextAreaElement;
  const saveButton = h("button", { class: "primary", text: "Save workflow" });
  const cancelButton = h("button", { text: "Cancel" });
  const editor = h("div", { class: "workflow-editor" },
    editorTitle,
    name,
    prompt,
    h("div", { class: "row" }, saveButton, cancelButton),
  );
  editor.hidden = true;
  let workflows: SavedWorkflow[] = [];
  let editingId: string | undefined;

  function showEditor(workflow?: SavedWorkflow) {
    editingId = workflow?.id;
    editor.hidden = false;
    editorTitle.textContent = workflow ? "Edit workflow" : "Create a workflow";
    name.value = workflow?.name ?? "";
    prompt.value = workflow?.prompt ?? "";
    saveButton.textContent = workflow ? "Save changes" : "Save workflow";
    clear(feedback);
    if (!workflow) name.focus();
  }

  function closeEditor() {
    editingId = undefined;
    name.value = "";
    prompt.value = "";
    editor.hidden = true;
  }

  function renderList() {
    clear(list);
    const active = workflows.filter((workflow) => !workflow.archived);
    const archived = workflows.filter((workflow) => workflow.archived);
    if (active.length === 0 && archived.length === 0) {
      list.append(h("div", { class: "hint", text: "No saved workflows yet." }));
    }

    const renderGroup = (title: string, records: SavedWorkflow[], isArchived: boolean) => {
      if (records.length === 0) return;
      list.append(h("h3", { class: "workflow-group-title", text: title }));
      for (const workflow of records) {
        const run = h("button", {
          class: "primary",
          text: "Run automatically",
          disabled: isArchived,
          title: isArchived ? "Unarchive this workflow before running it." : "Run this workflow automatically with your current LLM. Its requested actions will not ask for separate approvals.",
          onclick: async () => {
            run.disabled = true;
            try {
              await Bridge.workflowRun(workflow.id);
              feedback.replaceChildren(h("div", {
                class: "notice ok",
                text: `“${workflow.name}” was started with your current LLM. Its requested desktop and file actions run automatically without separate approvals. PowerShell commands use your Windows account permissions.`,
              }));
            } catch (error) {
              feedback.replaceChildren(h("div", {
                class: "notice err",
                text: `Could not run workflow: ${String(error).replace(/^Error:\s*/, "")}`,
              }));
            } finally {
              run.disabled = isArchived;
            }
          },
        });
        const edit = h("button", {
          text: "Edit",
          onclick: () => showEditor(workflow),
        });
        const duplicate = h("button", {
          text: "Duplicate",
          onclick: async () => {
            duplicate.disabled = true;
            try {
              const copy = await Bridge.workflowSave({
                name: `${workflow.name} copy`.slice(0, 100),
                prompt: workflow.prompt,
              });
              workflows.push(copy);
              clear(feedback);
              renderList();
            } catch (error) {
              feedback.replaceChildren(h("div", {
                class: "notice err",
                text: `Could not duplicate workflow: ${String(error).replace(/^Error:\s*/, "")}`,
              }));
            } finally {
              duplicate.disabled = false;
            }
          },
        });
        const archive = h("button", {
          text: isArchived ? "Restore" : "Archive",
          onclick: async () => {
            archive.disabled = true;
            try {
              await Bridge.workflowSetArchived(workflow.id, !isArchived);
              await reload();
            } catch (error) {
              archive.disabled = false;
              feedback.replaceChildren(h("div", {
                class: "notice err",
                text: `Could not ${isArchived ? "restore" : "archive"} workflow: ${String(error).replace(/^Error:\s*/, "")}`,
              }));
            }
          },
        });
        const remove = h("button", {
          class: "danger",
          text: "Delete",
          onclick: async () => {
            if (!window.confirm(`Delete “${workflow.name}” permanently?`)) return;
            remove.disabled = true;
            try {
              await Bridge.workflowDelete(workflow.id);
              workflows = workflows.filter((entry) => entry.id !== workflow.id);
              clear(feedback);
              renderList();
            } catch (error) {
              remove.disabled = false;
              feedback.replaceChildren(h("div", {
                class: "notice err",
                text: `Could not delete workflow: ${String(error).replace(/^Error:\s*/, "")}`,
              }));
            }
          },
        });
        const updatedAt = new Date(workflow.updatedAt * 1000).toLocaleDateString();
        list.append(h("article", { class: "workflow-card" },
          h("div", { class: "workflow-card-copy" },
            h("strong", { text: workflow.name }),
            h("span", { class: "hint", text: `Updated ${updatedAt}` }),
            h("p", { class: "workflow-prompt-preview", text: workflow.prompt }),
          ),
          h("div", { class: "row workflow-actions" }, run, edit, duplicate, archive, remove),
        ));
      }
    };

    renderGroup("Active", active, false);
    renderGroup("Archived", archived, true);
  }

  async function reload() {
    try {
      workflows = await Bridge.workflowsList();
      renderList();
    } catch (error) {
      list.replaceChildren(h("div", {
        class: "notice err",
        text: `Could not load workflows: ${String(error).replace(/^Error:\s*/, "")}`,
      }));
    }
  }

  saveButton.addEventListener("click", async () => {
    saveButton.disabled = true;
    clear(feedback);
    try {
      const saved = await Bridge.workflowSave({
        ...(editingId ? { id: editingId } : {}),
        name: name.value,
        prompt: prompt.value,
      });
      workflows = editingId
        ? workflows.map((workflow) => workflow.id === saved.id ? saved : workflow)
        : [...workflows, saved];
      closeEditor();
      feedback.replaceChildren(h("div", {
        class: "notice ok",
        text: "Workflow saved on this device.",
      }));
      renderList();
    } catch (error) {
      feedback.replaceChildren(h("div", {
        class: "notice err",
        text: `Could not save workflow: ${String(error).replace(/^Error:\s*/, "")}`,
      }));
    } finally {
      saveButton.disabled = false;
    }
  });
  cancelButton.addEventListener("click", closeEditor);

  const create = h("button", {
    class: "primary",
    text: "New workflow",
    onclick: () => showEditor(),
  });
  void reload();
  return h(
    "section",
    {},
    h("h2", {}, h("span", { text: "Workflows" })),
    h("div", {
      class: "hint",
      text: "Save reusable instructions for Macha. Running a workflow sends them through your existing chat model and automatically performs requested file, app, desktop, and PowerShell actions without separate approvals. Review each workflow before starting it. Workflows are stored locally on this device.",
    }),
    h("div", { class: "row" }, create),
    editor,
    feedback,
    list,
  );
}

function memorySection(): HTMLElement {
  const list = h("div", { style: "display:flex;flex-direction:column;gap:8px" });
  const feedback = h("div", {});
  let memories: MemoryRecord[] = [];
  const category = h("select", { "aria-label": "Memory category" }) as HTMLSelectElement;
  for (const [value, label] of [
    ["identity", "Identity"],
    ["preference", "Preference"],
    ["goal", "Goal"],
    ["project", "Project"],
    ["relationship", "Relationship"],
    ["emotional", "Life event"],
    ["behavior", "Behavior"],
  ] as const) {
    category.append(h("option", { value, text: label }));
  }
  const fact = h("input", {
    type: "text",
    maxlength: 500,
    placeholder: "A detail to remember",
    "aria-label": "A detail to remember",
  }) as HTMLInputElement;
  const add = h("button", {
    class: "primary",
    text: "Remember",
    onclick: async () => {
      if (!fact.value.trim()) {
        feedback.replaceChildren(h("div", {
          class: "notice warn",
          text: "Enter a detail to remember.",
        }));
        return;
      }
      add.disabled = true;
      try {
        await Bridge.memoryRemember(category.value as MemoryRecord["category"], fact.value);
        fact.value = "";
        await reload();
      } catch (error) {
        feedback.replaceChildren(h("div", {
          class: "notice err",
          text: `Could not save this memory: ${String(error).replace(/^Error:\s*/, "")}`,
        }));
      } finally {
        add.disabled = false;
      }
    },
  });

  function render() {
    clear(list);
    if (memories.length === 0) {
      list.append(h("div", { class: "hint", text: "No saved memories yet." }));
      return;
    }
    for (const memory of memories) {
      const remove = h("button", {
        class: "danger",
        text: "Forget",
        "aria-label": `Forget memory: ${memory.text}`,
        onclick: async () => {
          remove.disabled = true;
          try {
            await Bridge.memoryForget(memory.id);
            memories = memories.filter((entry) => entry.id !== memory.id);
            clear(feedback);
            render();
          } catch (error) {
            remove.disabled = false;
            feedback.replaceChildren(h("div", {
              class: "notice err",
              text: `Could not forget this memory: ${String(error).replace(/^Error:\s*/, "")}`,
            }));
          }
        },
      });
      list.append(h("div", { class: "row" },
        h("span", { class: "path", text: `[${memory.category}] ${memory.text}`, style: "flex:1 1 300px" }),
        remove,
      ));
    }
  }

  async function reload() {
    try {
      memories = await Bridge.memoryList();
      clear(feedback);
      render();
    } catch (error) {
      feedback.replaceChildren(h("div", {
        class: "notice err",
        text: `Could not load saved memories: ${String(error).replace(/^Error:\s*/, "")}`,
      }));
    }
  }

  const refresh = h("button", {
    text: "Refresh",
    onclick: () => void reload(),
  });
  void reload();
  return h(
    "section",
    {},
    h("h2", {}, h("span", { text: "Memory" })),
    h("div", {
      class: "hint",
      text: "Macha may save durable details to help in future conversations. You can also add memories yourself. Memories stay on this device and are included when you use chat; review or forget them here at any time.",
    }),
    h("div", { class: "row" }, category, fact, add),
    list,
    h("div", { class: "row" }, refresh),
    feedback,
  );
}

// ── Boot ──────────────────────────────────────────────────────────────────────

async function main() {
  const boot = await Bridge.boot();
  if (boot) {
    settings = { ...settings, ...boot.settings };
    State.settings = { ...State.settings, ...boot.settings };
    version = boot.version;
  }
  const status = (await Bridge.hooksStatus()) ?? {
    installed: false, settingsPath: "", hookPath: "", hookReady: false,
  };

  let openRouterAccounts: OpenRouterAccount[] = [];
  let openRouterError: string | undefined;
  let ollamaStatus: OllamaStatus = { reachable: false, models: [], error: "Ollama status has not been checked." };
  try {
    openRouterAccounts = await Bridge.openRouterAccounts();
  } catch (err) {
    openRouterError = String(err);
  }
  try {
    ollamaStatus = await Bridge.ollamaStatus();
  } catch (err) {
    ollamaStatus = { reachable: false, models: [], error: String(err).replace(/^Error:\s*/, "") };
  }

  const keys = [
    "stripe-api-key", "github-token", "vercel-token",
    "spotify-client-id", "notion-api-key", "calcom-api-key", "elevenlabs-api-key",
  ];
  const present: Record<string, boolean> = {};
  for (const k of keys) present[k] = (await Bridge.secretPresent(k)) ?? false;

  clear(root);
  root.append(
    h("h1", {}, h("span", { text: "Coucou" }), h("span", { class: "version", text: version })),
    claudeSection(status),
    apiSection(openRouterAccounts, ollamaStatus, openRouterError),
    memorySection(),
    workflowsSection(),
    automationSection(),
    gestureMouseSection(),
    integrationsSection(present),
    generalSection(),
    h("div", {
      class: "hint",
      text: "No telemetry. Network requests only go to the services you configure yourself.",
    }),
  );

  void onEvent<Settings>("settings-changed", (s) => {
    settings = { ...settings, ...s };
    State.settings = { ...State.settings, ...s };
  });
}

void main();
