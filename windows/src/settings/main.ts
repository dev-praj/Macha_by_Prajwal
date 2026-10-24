// Settings window — the place where anything that writes to disk is confirmed.
// Stage 2 covers the Claude Code hooks and the general preferences; API keys and
// integrations land here too in a later stage.

import "./settings.css";
import {
  Bridge,
  IS_TAURI,
  onEvent,
  type HookStatus,
  type MemoryRecord,
  type OllamaStatus,
  type OpenRouterAccount,
  type SavedWorkflow,
} from "../core/bridge";
import { DEFAULT_SETTINGS, type Settings } from "../core/state";
import { State } from "../core/state";
import { BotEngine } from "../macha/engine";
import { avatarLabel, mountAvatar, setAvatarSelection } from "../macha/avatar";
import * as TTS from "../core/tts";
import { recordWakePronunciation } from "../core/wakeWord";
import { insertTextAtCursor } from "../nixi/setupDictation";
import { registerSetupDictationTest } from "../nixi/setupDictationTest";
import { h, clear } from "../views/dom";
import { LANGUAGES, resolveLanguage, setLanguage, systemLanguages } from "../i18n/i18n";
import { agentsSection } from "./agents";
import { mountSettingsHub, weeklyRecapSection } from "./hub";
import { phoneSection } from "./phone";
import { notetakerSection } from "./notetaker";

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
          ? "Macha is hooked into your Claude Code sessions. Tool calls, questions and permission requests show up in the island, and you can answer them there."
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
        text: "macha-hook.exe is not in place yet. Restart Macha; if it still fails, build it with `cargo build -p macha-hook`.",
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
          : "This removes Macha's entries only. Your own hooks are left untouched.",
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
  providerKeysPresent: Record<string, boolean>,
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
  const directProviderKeys = h("div", {
    style: "display:flex;flex-direction:column;gap:8px",
  });
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
    h("option", { value: "grokOnly", text: "Grok only" }),
    h("option", { value: "claudeOnly", text: "Claude only" }),
    h("option", { value: "openAiOnly", text: "OpenAI only" }),
    h("option", { value: "geminiOnly", text: "Gemini only" }),
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

  for (const { key, provider, placeholder } of [
    { key: "grok-api-key", provider: "Grok", placeholder: "xai-…" },
    { key: "anthropic-api-key", provider: "Claude", placeholder: "sk-ant-…" },
    { key: "openai-api-key", provider: "OpenAI", placeholder: "sk-…" },
    { key: "gemini-api-key", provider: "Gemini", placeholder: "AIza…" },
  ]) {
    const input = h("input", {
      type: "password",
      placeholder: providerKeysPresent[key] ? "••••••••  (stored)" : placeholder,
      autocomplete: "off",
      spellcheck: "false",
      style: "flex:1 1 auto;min-width:0",
      "aria-label": `${provider} API key`,
    }) as HTMLInputElement;
    const dotEl = statusDot(providerKeysPresent[key] ?? false);
    const saveButton = h("button", { class: "primary", text: "Save key" });
    saveButton.addEventListener("click", async () => {
      saveButton.disabled = true;
      clear(feedback);
      const value = input.value.trim();
      try {
        await Bridge.secretSet(key, value);
        providerKeysPresent[key] = value.length > 0;
        input.value = "";
        input.placeholder = providerKeysPresent[key] ? "••••••••  (stored)" : placeholder;
        dotEl.style.background = providerKeysPresent[key] ? "#22c55e" : "#f4505e";
        feedback.append(h("div", {
          class: "notice ok",
          text: providerKeysPresent[key]
            ? `${provider} API key saved securely in the OS credential manager.`
            : `${provider} API key removed from the OS credential manager.`,
        }));
      } catch (err) {
        feedback.append(h("div", {
          class: "notice err",
          text: `Could not save ${provider} API key: ${String(err).replace(/^Error:\s*/, "")}`,
        }));
      } finally {
        saveButton.disabled = false;
      }
    });
    directProviderKeys.append(h("div", { class: "row" },
      h("label", { text: `${provider} API key` }),
      input,
      saveButton,
      dotEl,
    ));
  }

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
      text: "Auto tries local Ollama first and uses OpenRouter only if Ollama is unavailable. The Claude, OpenAI, Gemini, and Grok options send chat directly to that provider.",
    }),
    directProviderKeys,
    h("p", {
      class: "hint",
      text: "Provider API keys are stored securely in the OS credential manager. Leave a field empty and save to remove its key.",
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
  { id: "integration_spotify", name: "Music", color: "#1DB954", fields: [] },
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
          text: "Macha shows notifications from all apps here. Turn off notification banners for individual apps in Windows Settings to avoid duplicate pop-ups.",
        }),
      );
    }
    if (def.id === "integration_spotify") {
      rows.append(
        h("div", {
          class: "hint",
          text: "Controls the active Windows media session. Play music in Spotify or another media app that supports Windows media controls; no Spotify sign-in or API key is needed.",
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
  let socket: WebSocket | null = null;
  let reconnectTimer: number | undefined;
  let reconnects = 0;
  const status = h("div", { class: "hint", text: "Checking Vision Mode…" });
  const feedback = h("div", { class: "hint" });
  const liveStatus = h("div", { class: "gesture-live-status" });
  const start = h("button", { text: "Start Vision Mode" });
  const stop = h("button", { text: "Stop Vision Mode" });
  const cameraConsentKey = "macha.permission.camera.v1";
  let cameraConsent = false;
  try {
    cameraConsent = window.localStorage.getItem(cameraConsentKey) === "true";
  } catch (error) {
    feedback.textContent = "Could not read camera permission settings.";
    console.error("[macha] could not read camera permission", error);
  }
  const cameraConsentToggle = h("input", {
    type: "checkbox",
    checked: cameraConsent,
    "aria-label": "Allow camera use for gesture controls",
  }) as HTMLInputElement;
  cameraConsentToggle.addEventListener("change", async () => {
    cameraConsent = cameraConsentToggle.checked;
    try {
      if (cameraConsent) window.localStorage.setItem(cameraConsentKey, "true");
      else window.localStorage.removeItem(cameraConsentKey);
    } catch (error) {
      cameraConsent = !cameraConsent;
      cameraConsentToggle.checked = cameraConsent;
      feedback.textContent = "Could not save camera consent. Check browser storage permissions and try again.";
      console.error("[macha] could not save camera consent", error);
      return;
    }
    if (!cameraConsent && running) {
      try {
        await Bridge.gestureMouseStop();
        feedback.textContent = "Camera consent revoked. Vision Mode stopped.";
      } catch (error) {
        feedback.textContent = `Camera consent was revoked, but Vision Mode could not be stopped: ${String(error).replace(/^Error:\s*/, "")}`;
        console.error("[macha] could not stop Vision Mode after camera consent was revoked", error);
      } finally {
        await refresh();
      }
    } else {
      feedback.textContent = cameraConsent
        ? "Camera gesture controls are allowed. Macha requests camera access when you enable Vision Mode."
        : "Camera gesture controls are off.";
    }
  });
  const pinchSlider = h("input", {
    type: "range", min: "0.02", max: "0.15", step: "0.01", value: "0.05",
  }) as HTMLInputElement;
  const smoothingSlider = h("input", {
    type: "range", min: "0.05", max: "0.5", step: "0.05", value: "0.3",
  }) as HTMLInputElement;
  const sensitivitySlider = h("input", {
    type: "range", min: "0.5", max: "2.5", step: "0.1", value: "1.0",
  }) as HTMLInputElement;
  const pinchValue = h("output", { text: "0.05" });
  const smoothingValue = h("output", { text: "0.30" });
  const sensitivityValue = h("output", { text: "1.00" });
  const controls = h(
    "fieldset",
    { class: "gesture-controls", disabled: true },
    h("legend", { text: "Controls" }),
    gestureControl("Pinch Threshold", pinchSlider, pinchValue),
    gestureControl("Smoothing", smoothingSlider, smoothingValue),
    gestureControl("Sensitivity", sensitivitySlider, sensitivityValue),
  ) as HTMLFieldSetElement;

  const gestureRows = [
    ["left", "Left Click", "Thumb + Index"],
    ["right", "Right Click", "Thumb + Middle"],
    ["scroll", "Scroll", "Thumb + Pinky"],
    ["lock", "Cursor Lock", "Open palm · 5 seconds"],
    ["locked-scroll", "Directional Scroll", "Index + middle together"],
    ["rotate", "3D Rotate", "Thumb + index · turn wrist"],
  ] as const;
  const gestureList = h(
    "div",
    { class: "gesture-live-list" },
    ...gestureRows.map(([id, label, key]) =>
      h(
        "div",
        { class: "gesture-live-item", id: `gesture-${id}` },
        h("span", { class: "gesture-live-dot" }),
        h("span", { text: label }),
        h("span", { class: "gesture-live-key", text: key }),
      ),
    ),
  );
  const gestureCount = h("strong", { text: "0" });
  const handStatus = h("strong", { text: "—" });
  const fpsStatus = h("strong", { text: "—" });
  const handMouseStatus = h("strong", { text: "—" });
  const fistProgress = h("progress", { max: "1", value: "0" }) as HTMLProgressElement;
  const fistTimer = h("output", { class: "gesture-fist-timer", text: "1.0s" });

  function resetLiveStatus() {
    for (const [id] of gestureRows) {
      document.getElementById(`gesture-${id}`)?.classList.remove("on");
    }
    gestureCount.textContent = "0";
    handStatus.textContent = "—";
    fpsStatus.textContent = "—";
    handMouseStatus.textContent = "—";
    fistProgress.value = 0;
    fistTimer.textContent = "1.0s";
  }

  function update() {
    start.disabled = busy || running;
    stop.disabled = busy || !running;
    controls.disabled = !running;
    status.textContent = running
      ? "Vision Mode is running. The camera preview remains available while these controls stay in Settings."
      : "Vision Mode is stopped.";
    if (!running) {
      closeSocket();
      resetLiveStatus();
      liveStatus.textContent = "Live status is available while Vision Mode is running.";
    } else {
      connectSocket();
    }
  }

  function closeSocket() {
    if (reconnectTimer !== undefined) {
      window.clearTimeout(reconnectTimer);
      reconnectTimer = undefined;
    }
    const current = socket;
    socket = null;
    current?.close();
  }

  function updateControlValues(values: {
    pinch_threshold: number;
    smoothing: number;
    sensitivity: number;
  }) {
    pinchSlider.value = String(values.pinch_threshold);
    smoothingSlider.value = String(values.smoothing);
    sensitivitySlider.value = String(values.sensitivity);
    pinchValue.textContent = Number(pinchSlider.value).toFixed(2);
    smoothingValue.textContent = Number(smoothingSlider.value).toFixed(2);
    sensitivityValue.textContent = Number(sensitivitySlider.value).toFixed(2);
  }

  function sendSettings() {
    if (socket?.readyState !== WebSocket.OPEN) return;
    socket.send(JSON.stringify({
      type: "settings",
      pinch_threshold: Number(pinchSlider.value),
      smoothing: Number(smoothingSlider.value),
      sensitivity: Number(sensitivitySlider.value),
    }));
  }

  function connectSocket() {
    if (!running || socket) return;
    const nextSocket = new WebSocket("ws://127.0.0.1:8765/ws");
    socket = nextSocket;
    nextSocket.addEventListener("open", () => {
      if (socket !== nextSocket) return;
      reconnects = 0;
      liveStatus.textContent = "Connected to live gesture status.";
    });
    nextSocket.addEventListener("message", (event: MessageEvent<string>) => {
      if (socket !== nextSocket) return;
      try {
        const data = JSON.parse(event.data) as Record<string, unknown>;
        if (data.type === "settings") {
          const pinch = Number(data.pinch_threshold);
          const smoothing = Number(data.smoothing);
          const sensitivity = Number(data.sensitivity);
          if ([pinch, smoothing, sensitivity].every(Number.isFinite)) {
            updateControlValues({
              pinch_threshold: pinch,
              smoothing,
              sensitivity,
            });
          }
        } else if (data.type === "status") {
          const activeGestures: Record<string, unknown> = {
            left: data.left,
            right: data.right,
            scroll: data.scroll,
            lock: data.cursor_locked,
            "locked-scroll": data.locked_scroll,
            rotate: data.rotate,
          };
          for (const [id, active] of Object.entries(activeGestures)) {
            document.getElementById(`gesture-${id}`)?.classList.toggle("on", active === true);
          }
          gestureCount.textContent = String(data.gesture_count ?? 0);
          handStatus.textContent = data.hand === true ? "Detected" : "Not detected";
          fpsStatus.textContent = String(data.fps ?? "—");
          handMouseStatus.textContent = data.hand_mouse_active === true ? "Active" : "Paused";
          fistProgress.value = Number(data.fist_hold_progress) || 0;
          if (data.fist_toggle_armed === false) {
            fistTimer.textContent = "Release fist";
          } else {
            const remaining = Number(data.fist_hold_remaining);
            fistTimer.textContent = Number.isFinite(remaining)
              ? `${Math.max(0, remaining).toFixed(1)}s`
              : "1.0s";
          }
        }
      } catch (error) {
        console.error("[macha] invalid Vision Mode live update", error);
      }
    });
    nextSocket.addEventListener("close", () => {
      if (socket !== nextSocket) return;
      socket = null;
      if (running) {
        liveStatus.textContent = "Live controls disconnected. Reconnecting…";
        const delay = Math.min(1000 * (2 ** Math.min(reconnects, 5)), 30000);
        reconnects += 1;
        reconnectTimer = window.setTimeout(connectSocket, delay);
      }
    });
    nextSocket.addEventListener("error", () => {
      if (socket === nextSocket) {
        liveStatus.textContent = "Could not connect to live gesture controls.";
      }
    });
  }

  async function refresh() {
    try {
      const wasRunning = running;
      running = await Bridge.gestureMouseIsRunning();
      if (wasRunning && !running && !stopRequested) {
        feedback.textContent = "Vision Mode exited unexpectedly. Check Macha's log for details.";
      }
      if (!running) stopRequested = false;
      update();
    } catch (error) {
      status.textContent = `Could not check Vision Mode: ${String(error).replace(/^Error:\s*/, "")}`;
    }
  }

  start.addEventListener("click", async () => {
    if (!cameraConsent) {
      feedback.textContent = "Allow camera use for gesture controls before starting Vision Mode.";
      return;
    }
    busy = true;
    stopRequested = false;
    clear(feedback);
    update();
    let startedHere = false;
    try {
      await Bridge.gestureMouseStart();
      startedHere = true;
      if (window.localStorage.getItem(cameraConsentKey) !== "true") {
        await Bridge.gestureMouseStop();
        startedHere = false;
        throw new Error("Camera consent was revoked before Vision Mode could start.");
      }
      feedback.textContent = "Starting camera preview and live controls…";
    } catch (error) {
      let stopError: unknown;
      if (startedHere) {
        try {
          await Bridge.gestureMouseStop();
        } catch (errorWhileStopping) {
          stopError = errorWhileStopping;
          console.error("[macha] could not stop Vision Mode after a camera consent error", errorWhileStopping);
        }
      }
      feedback.textContent = `Could not start Vision Mode: ${String(error).replace(/^Error:\s*/, "")}`;
      if (stopError !== undefined) {
        feedback.textContent += ` Also could not stop it: ${String(stopError).replace(/^Error:\s*/, "")}`;
      }
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
  for (const slider of [pinchSlider, smoothingSlider, sensitivitySlider]) {
    slider.addEventListener("input", () => {
      updateControlValues({
        pinch_threshold: Number(pinchSlider.value),
        smoothing: Number(smoothingSlider.value),
        sensitivity: Number(sensitivitySlider.value),
      });
      sendSettings();
    });
  }
  return h(
    "section",
    {},
    h("h2", {}, h("span", { text: "Vision Mode" })),
    h("div", {
      class: "hint",
      text: "Control the Windows cursor with your webcam: move your wrist to move, pinch thumb + index to click and hold, thumb + middle to right-click, and thumb + pinky to scroll. Hold an open palm steady for five seconds to lock or unlock the cursor; the camera preview shows a progress ring and confirms when it is done. To rotate a 3D view that supports mouse dragging, place the pointer over the model before locking, then hold thumb and index together or extend them, curl the other fingers, and turn your wrist clockwise or anticlockwise. While locked, move joined index and middle fingers to scroll in any direction. Start Vision Mode to open the camera preview and use its live controls here in Settings. You can also say “activate vision mode” while Hey Macha is listening.",
    }),
    h("div", { class: "row" },
      h("label", { text: "Allow camera use for gesture controls" }),
      cameraConsentToggle,
    ),
    h("p", {
      class: "hint",
      text: "Camera access is used only while Vision Mode is running. Turn this off to stop Vision Mode and revoke Macha’s app-level consent.",
    }),
    status,
    h("div", { class: "row" }, start, stop),
    feedback,
    liveStatus,
    h("div", { class: "gesture-live-section" },
      h("h3", { text: "Active Gestures" }),
      gestureList,
    ),
    h("div", { class: "gesture-live-stats" },
      liveStat("Gestures", gestureCount),
      liveStat("Hand", handStatus),
      liveStat("Frames / sec", fpsStatus),
      liveStat("Hand Mouse", handMouseStatus),
    ),
    h("div", { class: "gesture-fist-progress" },
      h("label", { text: "Fist hold to pause or resume" }),
      fistTimer,
      fistProgress,
    ),
    controls,
  );
}

function gestureControl(
  label: string,
  input: HTMLInputElement,
  value: HTMLOutputElement,
): HTMLElement {
  return h(
    "label",
    { class: "gesture-control" },
    h("span", { text: label }),
    value,
    input,
  );
}

function liveStat(label: string, value: HTMLElement): HTMLElement {
  return h(
    "div",
    { class: "gesture-live-stat" },
    value,
    h("span", { text: label }),
  );
}

// ── General section ───────────────────────────────────────────────────────────

function avatarPreview(
  choice: Settings["avatar"],
  displays?: ReturnType<typeof mountAvatar>[],
): HTMLElement {
  const preview = h("div", { class: "avatar-preview" });
  if (choice === "macha") {
    const canvas = h("canvas", {}) as HTMLCanvasElement;
    canvas.width = 88;
    canvas.height = 88;
    const ctx = canvas.getContext("2d");
    if (ctx) {
      const engine = new BotEngine();
      engine.isMini = true;
      engine.setState("idle", true);
      engine.draw(ctx, 88, 88);
    }
    preview.append(canvas);
  } else {
    const display = mountAvatar(preview, choice);
    displays?.push(display);
  }
  return preview;
}

function avatarSection(): HTMLElement {
  const feedback = h("p", { class: "hint", "aria-live": "polite" });
  const choiceIds = ["macha", "cubee", "onee"] as const;
  const choices: HTMLButtonElement[] = [];

  for (const choice of choiceIds) {
    const button = h("button", {
      class: settings.avatar === choice ? "avatar-choice selected" : "avatar-choice",
      "aria-pressed": settings.avatar === choice,
    }) as HTMLButtonElement;
    button.append(
      avatarPreview(choice),
      h("span", { text: avatarLabel(choice) }),
    );
    button.addEventListener("click", async () => {
      if (settings.avatar === choice) return;
      const previous = settings.avatar;
      settings.avatar = choice;
      for (const [index, option] of choices.entries()) {
        const selected = choiceIds[index] === choice;
        option.classList.toggle("selected", selected);
        option.setAttribute("aria-pressed", String(selected));
      }
      try {
        await Bridge.saveSettingsStrict(settings);
        State.settings = { ...State.settings, ...settings };
        feedback.textContent = `${avatarLabel(choice)} is now your avatar.`;
      } catch (error) {
        settings.avatar = previous;
        for (const [index, option] of choices.entries()) {
          const selected = choiceIds[index] === previous;
          option.classList.toggle("selected", selected);
          option.setAttribute("aria-pressed", String(selected));
        }
        feedback.textContent = `Could not save your avatar: ${
          error instanceof Error ? error.message : String(error)
        }`;
        console.error("[macha] could not save avatar selection", error);
      }
    });
    choices.push(button);
  }

  return h(
    "section",
    {},
    h("h2", {}, h("span", { text: "Avatar" })),
    h("p", {
      class: "hint",
      text: "Choose the character shown in the island, agent bar, greeting, and file-drop animation.",
    }),
    h("div", { class: "avatar-choices" }, ...choices),
    feedback,
  );
}

function generalSection(): HTMLElement {
  const language = h("select", {}) as HTMLSelectElement;
  language.append(h("option", { value: "", text: "System language" }));
  for (const item of LANGUAGES) {
    language.append(h("option", { value: item.code, text: item.name }));
  }
  language.value = settings.language;
  language.addEventListener("change", () => {
    settings.language = language.value;
    setLanguage(resolveLanguage(settings.language, systemLanguages()));
    void save();
  });

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
      console.error("[macha] could not save Hey Macha voice activation", error);
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
      console.error("[macha] wake phrase calibration failed", error);
    } finally {
      try {
        await Bridge.setVoiceActive(false);
        await Bridge.setWakeCalibration(false);
      } catch (error) {
        console.error("[macha] could not restore the wake listener after calibration", error);
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
    console.error("[macha] could not list local speech voices", error);
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
    void TTS.speak("Hi, I'm macha.");
  });

  return h(
    "section",
    {},
    h("h2", {}, h("span", { text: "General" })),
    h("div", { class: "row" },
      h("label", { text: "Language" }),
      language,
    ),
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
        text: "When enabled, Macha listens on this device and sends speech only to its local Whisper service. Allow microphone access in Windows privacy settings when prompted. The Voice button remains available when this is off.",
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

function dictationSection(): HTMLElement {
  const feedback = h("p", { class: "hint", "aria-live": "polite" });
  const vocabularyInput = h("input", {
    type: "text",
    maxlength: "64",
    placeholder: "A name or technical term",
    "aria-label": "Custom dictation spelling",
  }) as HTMLInputElement;
  const vocabularyList = h("div", { style: "display:flex;flex-direction:column;gap:8px" });
  const replacementsList = h("div", { style: "display:flex;flex-direction:column;gap:8px" });
  const appStylesList = h("div", { style: "display:flex;flex-direction:column;gap:8px" });
  let saving = false;

  async function saveDictationChange(patch: Partial<Settings>): Promise<boolean> {
    if (saving) return false;
    saving = true;
    const updated = { ...settings, ...patch };
    try {
      await Bridge.saveSettingsStrict(updated);
      settings = updated;
      State.settings = { ...State.settings, ...updated };
      feedback.textContent = "Dictation preferences saved.";
      return true;
    } catch (error) {
      feedback.textContent = `Could not save dictation preferences: ${
        error instanceof Error ? error.message : String(error)
      }`;
      console.error("[macha] could not save dictation preferences", error);
      return false;
    } finally {
      saving = false;
      render();
    }
  }

  function render() {
    clear(vocabularyList);
    if (settings.dictationVocabulary.length === 0) {
      vocabularyList.append(h("div", { class: "hint", text: "No custom spellings yet." }));
    }
    for (const [index, word] of settings.dictationVocabulary.entries()) {
      const remove = h("button", { text: "Remove", "aria-label": `Remove ${word}` });
      remove.disabled = saving;
      remove.addEventListener("click", () => {
        void saveDictationChange({
          dictationVocabulary: settings.dictationVocabulary.filter((_, i) => i !== index),
        });
      });
      vocabularyList.append(h("div", { class: "row" },
        h("span", { style: "flex:1", text: word }),
        remove,
      ));
    }

    clear(replacementsList);
    if (settings.dictationReplacements.length === 0) {
      replacementsList.append(h("div", { class: "hint", text: "No phrase replacements yet." }));
    }
    for (const [index, entry] of settings.dictationReplacements.entries()) {
      const spoken = h("input", {
        type: "text",
        maxlength: "100",
        value: entry.spoken,
        placeholder: "Phrase you say",
        "aria-label": "Spoken phrase",
        style: "flex:1;min-width:120px",
      }) as HTMLInputElement;
      const replacement = h("input", {
        type: "text",
        maxlength: "500",
        value: entry.replacement,
        placeholder: "Text to insert",
        "aria-label": "Replacement text",
        style: "flex:1;min-width:120px",
      }) as HTMLInputElement;
      const save = h("button", { text: "Save" });
      save.disabled = saving;
      save.addEventListener("click", () => {
        const phrase = spoken.value.trim();
        if (!phrase) {
          feedback.textContent = "Enter a phrase to replace.";
          spoken.focus();
          return;
        }
        const updated = [...settings.dictationReplacements];
        updated[index] = { spoken: phrase, replacement: replacement.value };
        void saveDictationChange({ dictationReplacements: updated });
      });
      const remove = h("button", {
        text: "Remove",
        "aria-label": `Remove replacement ${entry.spoken}`,
      });
      remove.disabled = saving;
      remove.addEventListener("click", () => {
        void saveDictationChange({
          dictationReplacements: settings.dictationReplacements.filter((_, i) => i !== index),
        });
      });
      replacementsList.append(h("div", { class: "row" }, spoken, replacement, save, remove));
    }

    clear(appStylesList);
    if (settings.dictationAppStyles.length === 0) {
      appStylesList.append(h("div", { class: "hint", text: "No app-specific styles yet." }));
    }
    for (const [index, entry] of settings.dictationAppStyles.entries()) {
      const application = h("input", {
        type: "text",
        maxlength: "128",
        value: entry.application,
        placeholder: "application.exe",
        "aria-label": "Application executable",
        style: "flex:1;min-width:120px",
      }) as HTMLInputElement;
      const style = h("select", {
        "aria-label": "Dictation style",
      }) as HTMLSelectElement;
      style.append(
        h("option", { value: "standard", text: "Standard cleanup" }),
        h("option", { value: "verbatim", text: "Verbatim" }),
      );
      style.value = entry.style;
      const save = h("button", { text: "Save" });
      save.disabled = saving;
      save.addEventListener("click", () => {
        const applicationName = application.value.trim().toLowerCase();
        if (!/^[a-z0-9._-]+\.exe$/.test(applicationName) || applicationName.length > 128) {
          feedback.textContent = "Enter an executable name such as code.exe.";
          application.focus();
          return;
        }
        if (settings.dictationAppStyles.some((saved, i) =>
          i !== index && saved.application.toLowerCase() === applicationName
        )) {
          feedback.textContent = "That application already has a dictation style.";
          application.focus();
          return;
        }
        const updated = [...settings.dictationAppStyles];
        updated[index] = {
          application: applicationName,
          style: style.value as "standard" | "verbatim",
        };
        void saveDictationChange({ dictationAppStyles: updated });
      });
      const remove = h("button", {
        text: "Remove",
        "aria-label": `Remove ${entry.application} style`,
      });
      remove.disabled = saving;
      remove.addEventListener("click", () => {
        void saveDictationChange({
          dictationAppStyles: settings.dictationAppStyles.filter((_, i) => i !== index),
        });
      });
      appStylesList.append(h("div", { class: "row" }, application, style, save, remove));
    }
  }

  const addWord = h("button", {
    text: "Add spelling",
    onclick: () => {
      const word = vocabularyInput.value.trim();
      if (!word) {
        feedback.textContent = "Enter a word or name to add.";
        vocabularyInput.focus();
        return;
      }
      if (settings.dictationVocabulary.length >= 50) {
        feedback.textContent = "You can add up to 50 custom spellings.";
        return;
      }
      if (settings.dictationVocabulary.some((saved) => saved.toLowerCase() === word.toLowerCase())) {
        feedback.textContent = "That spelling is already in your list.";
        return;
      }
      void saveDictationChange({
        dictationVocabulary: [...settings.dictationVocabulary, word],
      }).then((saved) => {
        if (saved) vocabularyInput.value = "";
      });
    },
  });
  const spokenInput = h("input", {
    type: "text",
    maxlength: "100",
    placeholder: "Phrase you say",
    "aria-label": "New spoken phrase",
  }) as HTMLInputElement;
  const replacementInput = h("input", {
    type: "text",
    maxlength: "500",
    placeholder: "Text to insert",
    "aria-label": "New replacement text",
  }) as HTMLInputElement;
  const addReplacement = h("button", {
    text: "Add replacement",
    onclick: () => {
      const spoken = spokenInput.value.trim();
      if (!spoken) {
        feedback.textContent = "Enter a phrase to replace.";
        spokenInput.focus();
        return;
      }
      if (settings.dictationReplacements.length >= 100) {
        feedback.textContent = "You can add up to 100 phrase replacements.";
        return;
      }
      void saveDictationChange({
        dictationReplacements: [
          ...settings.dictationReplacements,
          { spoken, replacement: replacementInput.value },
        ],
      }).then((saved) => {
        if (saved) {
          spokenInput.value = "";
          replacementInput.value = "";
        }
      });
    },
  });
  const applicationInput = h("input", {
    type: "text",
    maxlength: "128",
    placeholder: "application.exe",
    "aria-label": "Application executable",
  }) as HTMLInputElement;
  const applicationStyle = h("select", {
    "aria-label": "New application dictation style",
  }) as HTMLSelectElement;
  applicationStyle.append(
    h("option", { value: "standard", text: "Standard cleanup" }),
    h("option", { value: "verbatim", text: "Verbatim" }),
  );
  const addAppStyle = h("button", {
    text: "Add app style",
    onclick: () => {
      const application = applicationInput.value.trim().toLowerCase();
      if (!/^[a-z0-9._-]+\.exe$/.test(application) || application.length > 128) {
        feedback.textContent = "Enter an executable name such as code.exe.";
        applicationInput.focus();
        return;
      }
      if (settings.dictationAppStyles.length >= 50) {
        feedback.textContent = "You can add up to 50 app-specific styles.";
        return;
      }
      if (settings.dictationAppStyles.some(
        (saved) => saved.application.toLowerCase() === application,
      )) {
        feedback.textContent = "That application already has a dictation style.";
        return;
      }
      void saveDictationChange({
        dictationAppStyles: [
          ...settings.dictationAppStyles,
          { application, style: applicationStyle.value as "standard" | "verbatim" },
        ],
      }).then((saved) => {
        if (saved) applicationInput.value = "";
      });
    },
  });

  render();
  return h("section", {},
    h("h2", {}, h("span", { text: "Voice dictation" })),
    h("div", {
      class: "hint",
      text: "Add names and technical terms as spelling hints for the local speech recognizer. Phrase replacements are applied on your device before text is inserted.",
    }),
    h("div", { class: "row" }, vocabularyInput, addWord),
    vocabularyList,
    h("h3", { text: "Phrase replacements" }),
    h("div", { class: "row" }, spokenInput, replacementInput, addReplacement),
    replacementsList,
    h("h3", { text: "Styles by application" }),
    h("div", {
      class: "hint",
      text: "Add an app’s executable name (for example, code.exe). Macha detects the focused app locally. Standard cleanup removes fillers and formats spoken punctuation; Verbatim inserts the recognizer’s text with only surrounding whitespace trimmed.",
    }),
    h("div", { class: "row" }, applicationInput, applicationStyle, addAppStyle),
    appStylesList,
    feedback,
  );
}

function voiceSetupSection(
  providerKeysPresent: Record<string, boolean>,
  initialAccounts: OpenRouterAccount[],
): HTMLElement {
  const setupStorageKey = "macha.voiceSetup.v8";
  const visionTestStorageKey = "macha.voiceSetup.visionTested.v1";
  const screenRegionConsentKey = "macha.permission.screenRegion.v1";
  const cameraConsentKey = "macha.permission.camera.v1";
  let stepIndex = 0;
  let microphoneStream: MediaStream | null = null;
  let microphoneContext: AudioContext | null = null;
  let animationFrame = 0;
  let stopTimer = 0;
  let microphoneAttempt = 0;
  let mascotFrame = 0;
  let mascotEngine: BotEngine | null = null;
  let mascotCanvas: HTMLCanvasElement | null = null;
  let mascotDisplay: ReturnType<typeof mountAvatar> | null = null;
  let setupAvatarPreviews: ReturnType<typeof mountAvatar>[] = [];
  let mascotFrameTime = 0;
  let dictationPollTimer = 0;
  let dictationText = "";
  let setupDictationInput: HTMLTextAreaElement | null = null;
  let setupDictationStatus: HTMLElement | null = null;
  let stopSetupDictation: (() => void) | null = null;
  let microphoneReady = false;
  let microphonePermissionReady = false;
  let cameraPermissionReady = false;
  let dictationReady = false;
  let screenRegionConsent = false;
  let cameraConsent = false;
  let visionTested = false;
  let speechReady = false;
  let speechPending = false;
  let setupCompleted = false;
  let standaloneSetupWindow = false;
  let transitioning = false;

  function setStandaloneSetupMode(enabled: boolean): void {
    standaloneSetupWindow = enabled;
    document.body.classList.toggle("voice-setup-only", enabled);
    document.documentElement.classList.toggle("voice-setup-only", enabled);
  }

  try {
    standaloneSetupWindow = IS_TAURI
      && window.localStorage.getItem(setupStorageKey) === null;
    cameraConsent = window.localStorage.getItem(cameraConsentKey) === "true";
    if (standaloneSetupWindow) setStandaloneSetupMode(true);
  } catch (error) {
    console.error("[macha] could not read first-run setup mode", error);
  }

  const setupLabel = h("span", {
    class: "voice-setup-label",
    text: "Ready when you are",
    hidden: IS_TAURI,
  });
  const openButton = h("button", { class: "primary", text: "Start Macha Setup" });
  const dialog = h("dialog", {
    class: "voice-setup-dialog",
    "aria-labelledby": "voice-setup-title",
    "aria-describedby": "voice-setup-description",
  }) as HTMLDialogElement;
  const progress = h("div", {
    class: "voice-setup-progress-fill",
    role: "progressbar",
    "aria-label": "Setup progress",
    "aria-valuemin": "0",
    "aria-valuemax": "8",
    "aria-valuenow": "0",
  });
  const progressTrack = h("div", { class: "voice-setup-progress" }, progress);
  const stepLabel = h("span", { class: "voice-setup-step-label" });
  const stage = h("div", { class: "voice-setup-stage", "aria-live": "polite" });
  const backButton = h("button", { text: "Back" });
  const skipButton = h("button", { class: "voice-setup-skip", text: "Set up later" });
  const nextButton = h("button", { class: "primary", text: "Continue" });
  const controls = h("div", { class: "voice-setup-controls" },
    backButton,
    h("div", { class: "voice-setup-control-end" }, skipButton, nextButton),
  );
  const shell = h("div", { class: "voice-setup-shell" },
    h("div", { class: "voice-setup-topline" },
      h("span", { text: "MACHA SETUP" }),
      stepLabel,
    ),
    progressTrack,
    stage,
    controls,
  );
  dialog.append(shell);

  function persistSetup(value: "completed" | "later"): void {
    try {
      window.localStorage.setItem(setupStorageKey, value);
    } catch (error) {
      console.error("[macha] could not save voice setup progress", error);
    }
  }

  try {
    visionTested = window.localStorage.getItem(visionTestStorageKey) === "true";
    screenRegionConsent = window.localStorage.getItem(screenRegionConsentKey) === "true";
  } catch (error) {
    console.error("[macha] could not read setup consent state", error);
  }

  void onEvent<{ transcript?: string; error?: string }>("dictation-setup-test-result", ({ transcript, error }) => {
    if (!dialog.open || stepIndex !== 3 || !setupDictationInput || !setupDictationStatus) return;
    if (transcript) {
      insertTextAtCursor(setupDictationInput, transcript);
    } else {
      setupDictationStatus.textContent = error
        ? `Dictation test failed: ${error}`
        : "No speech was transcribed. Try again.";
    }
  }).catch((error: unknown) => {
    console.error("[macha] could not listen for setup dictation results", error);
  });

  void onEvent<{ success: boolean; message?: string }>("vision-onboarding-result", ({ success, message }) => {
    visionTested = success;
    try {
      if (success) window.localStorage.setItem(visionTestStorageKey, "true");
      else window.localStorage.removeItem(visionTestStorageKey);
    } catch (error) {
      console.error("[macha] could not update Vision Mode setup progress", error);
    }
    if (dialog.open && stepIndex === 4) {
      const status = stage.querySelector<HTMLElement>("[data-vision-status]");
      if (status) {
        status.textContent = success
          ? "Vision Mode worked! Macha answered a question using the screen region you selected."
          : `Test not completed: ${message ?? "Macha did not receive a selected screen region."}`;
        status.classList.toggle("is-ready", success);
      }
      const button = stage.querySelector<HTMLButtonElement>("[data-vision-test-button]");
      if (button) {
        button.disabled = false;
        button.textContent = success ? "Try Vision Mode again" : "Activate Vision Mode";
      }
    }
  }).catch((error: unknown) => {
    console.error("[macha] could not listen for Vision Mode setup result", error);
  });

  void onEvent<null>("first-run-setup-start", () => {
    setStandaloneSetupMode(true);
    if (!dialog.open) openSetup();
  }).catch((error: unknown) => {
    console.error("[macha] could not listen for first-run setup window state", error);
  });

  function stopMicrophoneTest(message?: string) {
    microphoneAttempt += 1;
    window.cancelAnimationFrame(animationFrame);
    window.clearTimeout(stopTimer);
    animationFrame = 0;
    stopTimer = 0;
    for (const track of microphoneStream?.getTracks() ?? []) track.stop();
    microphoneStream = null;
    if (mascotEngine) mascotEngine.setState("idle");
    const meter = stage.querySelector<HTMLElement>(".voice-setup-meter-fill");
    meter?.style.setProperty("--meter-level", "0");
    stage.querySelector(".voice-setup-visual")?.classList.remove("is-listening");
    const context = microphoneContext;
    microphoneContext = null;
    if (context && context.state !== "closed") {
      void context.close().catch((error: unknown) => {
        console.error("[macha] could not close microphone test audio context", error);
      });
    }
    if (message) {
      const status = stage.querySelector<HTMLElement>("[data-microphone-status]");
      if (status) status.textContent = message;
    }
  }

  function stopSetupMascot() {
    window.cancelAnimationFrame(mascotFrame);
    mascotFrame = 0;
    mascotEngine = null;
    mascotCanvas = null;
    mascotDisplay?.destroy();
    mascotDisplay = null;
    for (const display of setupAvatarPreviews) display.destroy();
    setupAvatarPreviews = [];
  }

  function animateSetupMascot(now: number) {
    const canvas = mascotCanvas;
    const engine = mascotEngine;
    if (!canvas || !engine || !canvas.isConnected) {
      stopSetupMascot();
      return;
    }
    const context = canvas.getContext("2d");
    if (!context) {
      console.error("[macha] could not create the setup mascot canvas context");
      stopSetupMascot();
      return;
    }
    const width = 184;
    const height = 148;
    const dpr = Math.min(2, window.devicePixelRatio || 1);
    if (canvas.width !== width * dpr || canvas.height !== height * dpr) {
      canvas.width = width * dpr;
      canvas.height = height * dpr;
    }
    const elapsed = mascotFrameTime ? Math.min(0.05, (now - mascotFrameTime) / 1000) : 1 / 60;
    mascotFrameTime = now;
    engine.update(elapsed);
    context.setTransform(dpr, 0, 0, dpr, 0, 0);
    context.clearRect(0, 0, width, height);
    engine.draw(context, width, height);
    mascotFrame = window.requestAnimationFrame(animateSetupMascot);
  }

  function followSetupPointer(event: PointerEvent) {
    const lookTarget = settings.avatar === "macha"
      ? mascotCanvas
      : stage.querySelector<HTMLCanvasElement>(".voice-setup-avatar-host canvas");
    if (!lookTarget) return;
    const bounds = lookTarget.getBoundingClientRect();
    if (!bounds.width || !bounds.height) return;
    const x = Math.max(-1, Math.min(1, ((event.clientX - bounds.left) / bounds.width - 0.5) * 2));
    const y = Math.max(-1, Math.min(1, ((event.clientY - bounds.top) / bounds.height - 0.5) * 2));
    if (mascotEngine) {
      mascotEngine.lookX = x;
      mascotEngine.lookY = -y;
    }
    mascotDisplay?.setLook(x, -y);
  }

  dialog.addEventListener("pointermove", followSetupPointer);

  function animateLevel(analyser: AnalyserNode, samples: Uint8Array<ArrayBuffer>) {
    if (!microphoneStream) return;
    analyser.getByteTimeDomainData(samples);
    let sumSquares = 0;
    for (const sample of samples) {
      const value = (sample - 128) / 128;
      sumSquares += value * value;
    }
    const level = Math.min(1, Math.sqrt(sumSquares / samples.length) * 5);
    const meter = stage.querySelector<HTMLElement>(".voice-setup-meter-fill");
    meter?.style.setProperty("--meter-level", String(Math.max(0.025, level)));
    animationFrame = window.requestAnimationFrame(() => animateLevel(analyser, samples));
  }

  async function closeSetup(value: "completed" | "later"): Promise<void> {
    void Bridge.setDictationSetupTestActive(false).catch((error: unknown) => {
      console.error("[macha] could not stop setup dictation mode", error);
    });
    stopMicrophoneTest();
    stopSetupMascot();
    stopSetupDictation?.();
    stopSetupDictation = null;
    window.clearInterval(dictationPollTimer);
    dictationPollTimer = 0;
    if (speechPending) {
      speechPending = false;
      void TTS.stop().catch((error: unknown) => {
        console.error("[macha] could not stop setup voice sample", error);
      });
    }
    setupCompleted ||= value === "completed";
    persistSetup(setupCompleted ? "completed" : value);
    setupLabel.textContent = setupCompleted
      ? "Macha Setup complete"
      : "Setup can be started any time";
    openButton.textContent = setupCompleted ? "Review Macha Setup" : "Start Macha Setup";
    if (dialog.open) dialog.close(value);
    if (standaloneSetupWindow) {
      try {
        await Bridge.finishFirstRunSetupWindow();
        setStandaloneSetupMode(false);
      } catch (error) {
        console.error("[macha] could not restore the Settings window after setup", error);
        window.alert(
          `Setup is saved, but the normal Settings window could not be restored: ${
            error instanceof Error ? error.message : String(error)
          }`,
        );
      }
    }
  }

  function renderStep(): void {
    void Bridge.setDictationSetupTestActive(stepIndex === 3).catch((error: unknown) => {
      console.error("[macha] could not update setup dictation mode", error);
    });
    stopSetupDictation?.();
    stopSetupDictation = null;
    setupDictationInput = null;
    setupDictationStatus = null;
    if (speechPending) {
      speechPending = false;
      void TTS.stop().catch((error: unknown) => {
        console.error("[macha] could not stop setup voice sample", error);
      });
    }
    stopMicrophoneTest();
    stopSetupMascot();
    window.clearInterval(dictationPollTimer);
    dictationPollTimer = 0;
    clear(stage);
    stage.classList.remove("voice-setup-stage-welcome");
    transitioning = false;
    const isWelcome = stepIndex === 0;
    const isPermissions = stepIndex === 1;
    const isMicrophone = stepIndex === 2;
    const isDictation = stepIndex === 3;
    const isVision = stepIndex === 4;
    const isWakePhrase = stepIndex === 5;
    const isSpeech = stepIndex === 6;
    const isProviderSetup = stepIndex === 7;
    const isAvatar = stepIndex === 8;
    const isComplete = isAvatar;
    shell.classList.toggle("voice-setup-shell-vision", isVision);
    shell.classList.toggle("voice-setup-shell-permissions", isPermissions);
    stage.classList.toggle("voice-setup-stage-vision", isVision);
    stage.classList.toggle("voice-setup-stage-permissions", isPermissions);
    stage.classList.toggle("voice-setup-stage-provider", isProviderSetup);
    stage.classList.toggle("voice-setup-stage-avatar", isComplete);
    const titles = [
      "Now let’s set up Macha",
      "Privacy & Permissions",
      "Check Your Microphone",
      "Try Voice Dictation",
      "Vision Mode",
      "Choose How to Talk",
      "Make Sure Macha Can Speak",
      "Connect Your AI Provider",
      "Choose Your Companion",
    ];
    const descriptions = [
      "Your personal AI companion for Windows. In a few quick calibrations, we’ll get your microphone, voice dictation, and hand gesture vision mode ready.",
      "You stay in full control. Macha only accesses what you explicitly grant, processing dictation and gestures locally on this device.",
      "Say a few words to test your audio input. Audio level is measured in real time on your machine — nothing is recorded or sent.",
      "Dictate anywhere with zero lag. Hold Right Alt while speaking, then release to transcribe and insert words directly.",
      "Control your Windows cursor with hand gestures using your webcam. Move your hand to guide the cursor, pinch fingers to click, and scroll without touching the mouse.",
      "With Hey Macha enabled, say the wake phrase to open voice chat. Prefer a button? Push-to-talk remains available either way.",
      "Play a short sample using your selected local voice. You can tune speech speed, voice, and volume anytime in Settings.",
      "Add a provider API key for cloud AI, or skip this step and use a local Ollama model if you have one installed.",
      "Choose the companion you want by your side. Your choice is saved and can be changed anytime in Settings.",
    ];
    stepLabel.textContent = isComplete ? "FINAL STEP" : `STEP ${stepIndex + 1} OF 8`;
    progress.style.width = `${(Math.min(stepIndex, 8) / 8) * 100}%`;
    progress.setAttribute("aria-valuenow", String(Math.min(stepIndex, 8)));
    backButton.hidden = isWelcome;
    skipButton.hidden = isComplete;
    nextButton.textContent = isComplete ? "Finish setup" : "Continue";
    nextButton.disabled = false;

    const visual = h("div", {
      class: `voice-setup-visual ${isMicrophone && microphoneStream ? "is-listening" : ""}`,
      "aria-hidden": "true",
    },
      h("div", { class: "voice-setup-visual-ring voice-setup-visual-ring-outer" }),
      h("div", { class: "voice-setup-visual-ring voice-setup-visual-ring-inner" }),
      h("div", { class: "voice-setup-mascot-glow" }),
      h("canvas", { class: "voice-setup-mascot", width: "368", height: "296" }),
      h("span", { class: "voice-setup-avatar-host" }),
      h("span", { class: "voice-setup-mascot-caption", text: "M A C H A" }),
    );

    stage.append(
      visual,
      h("h2", {
        class: "voice-setup-title",
        id: "voice-setup-title",
        tabindex: "-1",
        text: titles[stepIndex],
      }),
      h("p", {
        class: "voice-setup-description",
        id: "voice-setup-description",
        text: descriptions[stepIndex],
      }),
    );
    mascotCanvas = visual.querySelector<HTMLCanvasElement>(".voice-setup-mascot");
    const avatarHost = visual.querySelector<HTMLElement>(".voice-setup-avatar-host")!;
    const mascotCaption = visual.querySelector<HTMLElement>(".voice-setup-mascot-caption")!;
    if (isWelcome) {
      visual.append(
        h("div", { class: "voice-setup-speech-bubble" },
          h("span", { class: "voice-setup-speech-bubble-wave", text: "👋" }),
          h("span", { class: "voice-setup-speech-bubble-text", text: "Hello!" }),
        ),
      );
    }
    visual.dataset.avatarChoice = settings.avatar;
    mascotCaption.textContent = avatarLabel(settings.avatar).toUpperCase().split("").join(" ");
    setAvatarSelection(settings.avatar);
    mascotDisplay = mountAvatar(avatarHost);
    mascotDisplay.setState(isWelcome ? "finished" : isVision ? "question" : isSpeech || isComplete ? "finished" : "idle");
    mascotEngine = new BotEngine();
    mascotEngine.setState(
      isMicrophone && microphoneStream
        ? "working"
        : isWelcome
          ? "finished"
          : isPermissions
            ? "idle"
            : isDictation
              ? "working"
              : isVision
                ? "question"
                : isWakePhrase
                  ? "thinking"
                  : isSpeech || isComplete
                    ? "finished"
                    : "idle",
    );
    mascotFrameTime = 0;
    mascotFrame = window.requestAnimationFrame(animateSetupMascot);

    if (isWelcome) {
      stage.classList.add("voice-setup-stage-intro");
      const letsGoButton = h("button", {
        type: "button",
        class: "voice-setup-lets-go-btn",
      },
        h("span", { text: "Let’s Go!" }),
        h("span", { class: "voice-setup-lets-go-arrow", text: "→" }),
      );
      letsGoButton.addEventListener("click", () => moveToStep(1));

      const introPills = h("div", { class: "voice-setup-intro-pills" },
        h("span", { text: "🎙️ Natural Voice" }),
        h("span", { text: "⚡ Instant Dictation" }),
        h("span", { text: "🖐️ Gesture Vision" }),
        h("span", { text: "🤖 AI Intelligence" }),
      );

      stage.append(
        introPills,
        letsGoButton,
      );
    }

    if (isProviderSetup) {
      const providerKeys: Array<{ key: string; name: string; placeholder: string }> = [
        { key: "grok-api-key", name: "Grok", placeholder: "xai-…" },
        { key: "anthropic-api-key", name: "Claude", placeholder: "sk-ant-…" },
        { key: "openai-api-key", name: "OpenAI", placeholder: "sk-…" },
        { key: "gemini-api-key", name: "Gemini", placeholder: "AIza…" },
      ];
      let setupAccounts = [...initialAccounts];
      const feedback = h("p", {
        class: "voice-setup-provider-feedback",
        "aria-live": "polite",
        text: "Keys are stored in the Windows credential manager and never included in Macha settings.",
      });
      const providerMode = h("select", {
        class: "voice-setup-provider-mode",
        "aria-label": "AI provider mode",
      }) as HTMLSelectElement;
      providerMode.append(
        h("option", { value: "auto", text: "Automatic — local Ollama first, OpenRouter fallback" }),
        h("option", { value: "ollamaOnly", text: "Local Ollama only — no API key" }),
        h("option", { value: "openRouterOnly", text: "OpenRouter" }),
        h("option", { value: "grokOnly", text: "Grok" }),
        h("option", { value: "claudeOnly", text: "Claude" }),
        h("option", { value: "openAiOnly", text: "OpenAI" }),
        h("option", { value: "geminiOnly", text: "Gemini" }),
      );
      providerMode.value = settings.providerMode;
      const saveMode = h("button", { class: "voice-setup-secondary", text: "Save AI preference" });
      saveMode.addEventListener("click", async () => {
        const selectedMode = providerMode.value as Settings["providerMode"];
        const requiredKey = ({
          openRouterOnly: setupAccounts.length > 0,
          grokOnly: providerKeysPresent["grok-api-key"] ?? false,
          claudeOnly: providerKeysPresent["anthropic-api-key"] ?? false,
          openAiOnly: providerKeysPresent["openai-api-key"] ?? false,
          geminiOnly: providerKeysPresent["gemini-api-key"] ?? false,
        } as Partial<Record<Settings["providerMode"], boolean>>)[selectedMode];
        if (requiredKey === false) {
          feedback.textContent = "Add a key for the selected provider before saving this preference.";
          feedback.classList.add("is-error");
          return;
        }
        const previousMode = settings.providerMode;
        settings.providerMode = selectedMode;
        saveMode.disabled = true;
        feedback.textContent = "Saving your AI preference…";
        feedback.classList.remove("is-error");
        try {
          await Bridge.saveSettingsStrict(settings);
          State.settings = { ...State.settings, ...settings };
          feedback.textContent = "AI preference saved. You can change it any time in Settings.";
        } catch (error) {
          settings.providerMode = previousMode;
          providerMode.value = previousMode;
          feedback.textContent = `Could not save the AI preference: ${
            error instanceof Error ? error.message : String(error)
          }`;
          feedback.classList.add("is-error");
        } finally {
          saveMode.disabled = false;
        }
      });

      const keyCards = providerKeys.map(({ key, name, placeholder }) => {
        const input = h("input", {
          type: "password",
          autocomplete: "off",
          spellcheck: "false",
          placeholder: providerKeysPresent[key] ? "Key stored securely" : placeholder,
          "aria-label": `${name} API key`,
        }) as HTMLInputElement;
        const saveKey = h("button", { text: "Save" });
        const status = h("span", {
          class: "voice-setup-provider-key-status",
          text: providerKeysPresent[key] ? "Connected" : "Optional",
        });
        saveKey.addEventListener("click", async () => {
          const value = input.value.trim();
          if (!value) {
            feedback.textContent = `Enter a ${name} API key to save it.`;
            feedback.classList.add("is-error");
            input.focus();
            return;
          }
          saveKey.disabled = true;
          feedback.textContent = `Saving ${name} key securely…`;
          feedback.classList.remove("is-error");
          try {
            await Bridge.secretSet(key, value);
            providerKeysPresent[key] = true;
            input.value = "";
            input.placeholder = "Key stored securely";
            status.textContent = "Connected";
            feedback.textContent = `${name} key saved in the Windows credential manager.`;
          } catch (error) {
            feedback.textContent = `Could not save the ${name} key: ${
              error instanceof Error ? error.message : String(error)
            }`;
            feedback.classList.add("is-error");
          } finally {
            saveKey.disabled = false;
          }
        });
        return h("label", { class: "voice-setup-provider-key" },
          h("span", { text: name }),
          h("span", { class: "voice-setup-provider-key-input" }, input, saveKey),
          status,
        );
      });

      const accountName = h("input", {
        type: "text",
        maxlength: "80",
        autocomplete: "off",
        placeholder: "Account name",
        "aria-label": "OpenRouter account name",
      }) as HTMLInputElement;
      const accountKey = h("input", {
        type: "password",
        autocomplete: "off",
        spellcheck: "false",
        placeholder: "sk-or-v1-…",
        "aria-label": "OpenRouter API key",
      }) as HTMLInputElement;
      const accountStatus = h("span", {
        class: "voice-setup-provider-key-status",
        text: setupAccounts.length ? `${setupAccounts.length} account${setupAccounts.length === 1 ? "" : "s"} connected` : "Optional",
      });
      const saveAccount = h("button", { text: "Save account" });
      saveAccount.addEventListener("click", async () => {
        const name = accountName.value.trim();
        const key = accountKey.value.trim();
        if (!name || !key) {
          feedback.textContent = "Enter both an OpenRouter account name and API key.";
          feedback.classList.add("is-error");
          return;
        }
        saveAccount.disabled = true;
        feedback.textContent = "Saving OpenRouter account securely…";
        feedback.classList.remove("is-error");
        try {
          await Bridge.openRouterAccountAdd(name, key);
          accountName.value = "";
          accountKey.value = "";
          setupAccounts = await Bridge.openRouterAccounts();
          accountStatus.textContent = `${setupAccounts.length} account${setupAccounts.length === 1 ? "" : "s"} connected`;
          feedback.textContent = "OpenRouter account saved in the Windows credential manager.";
        } catch (error) {
          feedback.textContent = `Could not save the OpenRouter account: ${
            error instanceof Error ? error.message : String(error)
          }`;
          feedback.classList.add("is-error");
        } finally {
          saveAccount.disabled = false;
        }
      });

      stage.append(
        h("div", { class: "voice-setup-provider-mode-row" }, providerMode, saveMode),
        h("p", {
          class: "voice-setup-provider-privacy",
          text: "Direct-provider keys are used when their matching mode is selected. Automatic mode uses Ollama first, then OpenRouter.",
        }),
        h("div", { class: "voice-setup-provider-keys" }, ...keyCards),
        h("div", { class: "voice-setup-provider-openrouter" },
          h("span", { class: "voice-setup-provider-openrouter-title", text: "OpenRouter account" }),
          accountName,
          accountKey,
          saveAccount,
          accountStatus,
        ),
        h("p", {
          class: "voice-setup-provider-privacy",
          text: "Cloud providers process the prompts and any screen region you choose to send. No key is required for local Ollama. You can skip this step and configure providers later in Settings.",
        }),
        feedback,
      );
    }

    if (isComplete) {
      const avatarChoices: Settings["avatar"][] = ["macha", "cubee", "onee"];
      const avatarFeedback = h("p", {
        class: "voice-setup-avatar-feedback",
        "aria-live": "polite",
        text: `Selected: ${avatarLabel(settings.avatar)}`,
      });
      const avatarButtons = avatarChoices.map((choice) => {
        const button = h("button", {
          class: settings.avatar === choice
            ? "voice-setup-avatar-choice selected"
            : "voice-setup-avatar-choice",
          "aria-pressed": String(settings.avatar === choice),
          "aria-label": `${avatarLabel(choice)} avatar`,
        }) as HTMLButtonElement;
        button.append(
          avatarPreview(choice, setupAvatarPreviews),
          h("span", { text: avatarLabel(choice) }),
          h("span", {
            class: "voice-setup-avatar-selected-mark",
            "aria-hidden": "true",
            text: "✓",
          }),
        );
        button.addEventListener("click", async () => {
          if (settings.avatar === choice) return;
          const previous = settings.avatar;
          for (const option of avatarButtons) option.disabled = true;
          settings.avatar = choice;
          State.settings = { ...State.settings, ...settings };
          setAvatarSelection(choice);
          visual.dataset.avatarChoice = choice;
          mascotCaption.textContent = avatarLabel(choice).toUpperCase().split("").join(" ");
          for (const [index, option] of avatarButtons.entries()) {
            const selected = avatarChoices[index] === choice;
            option.classList.toggle("selected", selected);
            option.setAttribute("aria-pressed", String(selected));
          }
          avatarFeedback.textContent = `Selected: ${avatarLabel(choice)}. Saving…`;
          try {
            if (IS_TAURI) await Bridge.saveSettingsStrict(settings);
            avatarFeedback.textContent = `${avatarLabel(choice)} is now your avatar.`;
          } catch (error) {
            settings.avatar = previous;
            State.settings = { ...State.settings, ...settings };
            setAvatarSelection(previous);
            visual.dataset.avatarChoice = previous;
            mascotCaption.textContent = avatarLabel(previous).toUpperCase().split("").join(" ");
            for (const [index, option] of avatarButtons.entries()) {
              const selected = avatarChoices[index] === previous;
              option.classList.toggle("selected", selected);
              option.setAttribute("aria-pressed", String(selected));
            }
            avatarFeedback.textContent = `Could not save your avatar: ${
              error instanceof Error ? error.message : String(error)
            }`;
            console.error("[macha] could not save setup avatar selection", error);
          } finally {
            for (const option of avatarButtons) option.disabled = false;
          }
        });
        return button;
      });
      stage.append(
        h("div", { class: "voice-setup-avatar-picker" },
          h("span", { class: "voice-setup-avatar-picker-label", text: "PICK YOUR COMPANION" }),
          h("div", { class: "voice-setup-avatar-options" }, ...avatarButtons),
          avatarFeedback,
        ),
        h("div", { class: "voice-setup-summary" },
          h("span", { class: "voice-setup-summary-mark", text: "✓", "aria-hidden": "true" }),
          h("span", {
            text: settings.wakeWordEnabled ? "Hey Macha is ready" : "Push-to-talk is ready",
          }),
          h("span", {
            text: microphoneReady ? "Microphone checked" : "Microphone not checked",
          }),
          h("span", {
            text: dictationReady ? "Dictation tested" : "Right Alt dictation ready",
          }),
          h("span", {
            text: visionTested ? "Vision Mode tested" : "Vision Mode not tested",
          }),
          h("span", {
            text: speechReady ? "Spoken replies tested" : "Voice sample skipped",
          }),
        ),
      );
    }

    if (isPermissions) {
      const microphoneStatus = h("p", {
        class: "voice-setup-permission-status",
        "aria-live": "polite",
        "data-permission-microphone-status": "",
        text: microphonePermissionReady
          ? "Microphone access was granted. The permission check is finished; the microphone is off."
          : "Not granted yet. You can allow it now or continue without voice features.",
      });
      const microphoneButton = h("button", {
        class: "voice-setup-secondary",
        text: microphonePermissionReady ? "Check microphone permission again" : "Allow microphone",
      });
      microphoneButton.addEventListener("click", async () => {
        microphoneButton.disabled = true;
        microphoneStatus.textContent = "Waiting for Windows/WebView microphone permission…";
        try {
          if (!navigator.mediaDevices?.getUserMedia) {
            throw new Error("Microphone access is unavailable in this app environment.");
          }
          const stream = await navigator.mediaDevices.getUserMedia({ audio: true });
          for (const track of stream.getTracks()) track.stop();
          microphonePermissionReady = true;
          microphoneStatus.textContent = "Microphone access was granted. The permission check is finished; the microphone is off.";
          microphoneButton.textContent = "Check microphone permission again";
        } catch (error) {
          microphoneStatus.textContent = error instanceof DOMException && error.name === "NotAllowedError"
            ? "Microphone access was denied. You can change it later in Windows microphone privacy settings."
            : `Could not request microphone access: ${error instanceof Error ? error.message : String(error)}`;
          console.error("[macha] microphone permission request failed", error);
        } finally {
          microphoneButton.disabled = false;
        }
      });

      const cameraConsentToggle = h("input", {
        type: "checkbox",
        checked: cameraConsent,
        "aria-label": "Allow camera use for gesture controls",
      }) as HTMLInputElement;
      const cameraStatus = h("p", {
        class: "voice-setup-permission-status",
        "aria-live": "polite",
        "data-permission-camera-status": "",
        text: cameraPermissionReady
          ? "Camera permission was granted. The camera is off now."
          : cameraConsent
            ? "App consent is on. Request camera access to use gesture controls."
            : "Off. Camera gesture controls remain disabled until you allow them.",
      });
      const cameraButton = h("button", {
        class: "voice-setup-secondary",
        text: cameraPermissionReady ? "Check camera permission again" : "Allow camera",
      });
      cameraConsentToggle.addEventListener("change", () => {
        cameraConsent = cameraConsentToggle.checked;
        try {
          if (cameraConsent) window.localStorage.setItem(cameraConsentKey, "true");
          else window.localStorage.removeItem(cameraConsentKey);
          cameraStatus.textContent = cameraConsent
            ? "App consent is on. Request camera access to use gesture controls."
            : "Off. Camera gesture controls remain disabled until you allow them.";
        } catch (error) {
          cameraConsent = !cameraConsent;
          cameraConsentToggle.checked = cameraConsent;
          cameraStatus.textContent = "Could not save this choice. Check browser storage permissions and try again.";
          console.error("[macha] could not save camera consent", error);
        }
      });
      cameraButton.addEventListener("click", async () => {
        cameraButton.disabled = true;
        cameraStatus.textContent = "Waiting for Windows/WebView camera permission…";
        try {
          if (!navigator.mediaDevices?.getUserMedia) {
            throw new Error("Camera access is unavailable in this app environment.");
          }
          const stream = await navigator.mediaDevices.getUserMedia({ video: true });
          for (const track of stream.getTracks()) track.stop();
          cameraPermissionReady = true;
          cameraConsent = true;
          cameraConsentToggle.checked = true;
          window.localStorage.setItem(cameraConsentKey, "true");
          cameraStatus.textContent = "Camera access was granted. The permission check is finished; the camera is off.";
          cameraButton.textContent = "Check camera permission again";
        } catch (error) {
          if (cameraConsent) {
            try {
              window.localStorage.removeItem(cameraConsentKey);
            } catch (storageError) {
              console.error("[macha] could not clear camera consent after permission failure", storageError);
            }
          }
          cameraConsent = false;
          cameraConsentToggle.checked = false;
          cameraStatus.textContent = error instanceof DOMException && error.name === "NotAllowedError"
            ? "Camera access was denied. Gesture controls stay off; you can change access in Windows camera privacy settings."
            : `Could not request camera access: ${error instanceof Error ? error.message : String(error)}`;
          console.error("[macha] camera permission request failed", error);
        } finally {
          cameraButton.disabled = false;
        }
      });

      const screenConsent = h("input", {
        type: "checkbox",
        checked: screenRegionConsent,
        "aria-label": "Allow selected screen-region capture",
      }) as HTMLInputElement;
      const screenStatus = h("p", {
        class: "voice-setup-permission-status",
        "aria-live": "polite",
        text: screenRegionConsent
          ? "Allowed. Macha captures a crop only after you deliberately select it."
          : "Off. Vision Mode will not capture screen regions until you enable this.",
      });
      screenConsent.addEventListener("change", () => {
        screenRegionConsent = screenConsent.checked;
        try {
          if (screenRegionConsent) window.localStorage.setItem(screenRegionConsentKey, "true");
          else window.localStorage.removeItem(screenRegionConsentKey);
          screenStatus.textContent = screenRegionConsent
            ? "Allowed. Macha captures a crop only after you deliberately select it."
            : "Off. Vision Mode will not capture screen regions until you enable this.";
        } catch (error) {
          screenConsent.checked = !screenRegionConsent;
          screenRegionConsent = screenConsent.checked;
          screenStatus.textContent = "Could not save this choice. Check browser storage permissions and try again.";
          console.error("[macha] could not save screen-region permission choice", error);
        }
      });

      stage.append(
        h("div", { class: "voice-setup-permission-card voice-setup-permission-card-microphone" },
          h("div", { class: "voice-setup-permission-heading" },
            h("h3", { text: "Microphone" }),
            h("span", { text: microphonePermissionReady ? "Granted" : "Optional" }),
          ),
          h("p", { text: "Used for voice chat, wake-word listening when enabled, and Right Alt dictation. Macha transcribes dictation on this device." }),
          microphoneStatus,
          microphoneButton,
        ),
        h("div", { class: "voice-setup-permission-card voice-setup-permission-card-camera" },
          h("label", { class: "voice-setup-permission-heading" },
            h("h3", { text: "Camera" }),
            cameraConsentToggle,
          ),
          h("p", { text: "Optional hand-gesture controls use the webcam while Vision Mode runs. Camera frames are processed locally for hand tracking." }),
          cameraStatus,
          cameraButton,
        ),
        h("div", { class: "voice-setup-permission-card voice-setup-permission-card-screen" },
          h("label", { class: "voice-setup-permission-heading" },
            h("h3", { text: "Selected screen regions" }),
            screenConsent,
          ),
          h("p", { text: "Vision Mode can send only the rectangle you deliberately drag-select while holding left Ctrl + left Alt. Macha does not capture the whole screen in this feature." }),
          screenStatus,
        ),
        h("div", { class: "voice-setup-permission-note" },
          h("strong", { text: "Other access is limited and explained when used." }),
          h("p", { text: "Files and desktop actions: accessed for tasks you start; workflow actions may run automatically as configured. Clipboard: may be used briefly for dictation insertion, then restored. Macha detects Right Alt and left Ctrl + left Alt while running. Windows notifications and autostart are separate Settings options." }),
        ),
      );
    }

    if (isMicrophone) {
      const meterFill = h("div", { class: "voice-setup-meter-fill" });
      const microphoneStatus = h("p", {
        class: "voice-setup-status",
        "data-microphone-status": "",
        "aria-live": "polite",
        text: microphoneReady
          ? "Microphone check passed. You can test it again or continue."
          : "The microphone stays active only while this check is running.",
      });
      const microphoneButton = h("button", {
        class: "voice-setup-secondary",
        text: microphoneStream ? "Stop microphone check" : microphoneReady ? "Test again" : "Allow microphone & test",
      });
      const meter = h("div", { class: "voice-setup-meter", "aria-label": "Microphone input level" }, meterFill);
      microphoneButton.addEventListener("click", async () => {
        if (microphoneStream) {
          stopMicrophoneTest("Microphone check stopped. No audio was recorded or sent.");
          microphoneButton.textContent = "Test again";
          return;
        }
        const attempt = ++microphoneAttempt;
        microphoneButton.disabled = true;
        microphoneStatus.textContent = "Requesting microphone access…";
        try {
          if (!navigator.mediaDevices?.getUserMedia) {
            throw new Error("Microphone access is unavailable in this app environment.");
          }
          const stream = await navigator.mediaDevices.getUserMedia({ audio: true });
          if (attempt !== microphoneAttempt || !dialog.open || stepIndex !== 2) {
            for (const track of stream.getTracks()) track.stop();
            return;
          }
          microphoneStream = stream;
          mascotEngine?.setState("working");
          microphoneContext = new AudioContext();
          const analyser = microphoneContext.createAnalyser();
          analyser.fftSize = 256;
          microphoneContext.createMediaStreamSource(microphoneStream).connect(analyser);
          microphoneReady = true;
          microphoneButton.textContent = "Stop microphone check";
          microphoneStatus.textContent = "Microphone ready. Speak to see your level.";
          visual.classList.add("is-listening");
          animateLevel(analyser, new Uint8Array(new ArrayBuffer(analyser.fftSize)));
          stopTimer = window.setTimeout(() => {
            stopMicrophoneTest("Microphone check complete. Access was granted; no audio was recorded or sent.");
            microphoneButton.textContent = "Test again";
            visual.classList.remove("is-listening");
          }, 12000);
        } catch (error) {
          if (attempt !== microphoneAttempt || !dialog.open || stepIndex !== 2) return;
          stopMicrophoneTest();
          microphoneStatus.textContent =
            error instanceof Error && error.name === "NotAllowedError"
              ? "Microphone access was denied. Allow it in Windows privacy settings, then try again."
              : `Microphone check failed: ${error instanceof Error ? error.message : String(error)}`;
          console.error("[macha] microphone setup check failed", error);
        } finally {
          microphoneButton.disabled = false;
        }
      });
      stage.append(meter, microphoneStatus, microphoneButton);
    }

    if (isWakePhrase) {
      const wakeStatus = h("p", {
        class: "voice-setup-status",
        "aria-live": "polite",
        text: settings.wakeWordEnabled
          ? "Hey Macha is enabled and ready."
          : "Hey Macha is off. You can turn it on now or use push-to-talk.",
      });
      const wakeButton = h("button", {
        class: "voice-setup-secondary",
        text: settings.wakeWordEnabled ? "Turn off Hey Macha" : "Enable Hey Macha",
      });
      const wakeCheck = h("span", {
        class: "voice-setup-check",
        text: settings.wakeWordEnabled ? "Ready" : "Optional",
      });
      wakeButton.addEventListener("click", async () => {
        const previous = settings.wakeWordEnabled;
        settings.wakeWordEnabled = !previous;
        wakeButton.disabled = true;
        wakeStatus.textContent = settings.wakeWordEnabled ? "Enabling Hey Macha…" : "Disabling Hey Macha…";
        try {
          await Bridge.saveSettingsStrict(settings);
          State.settings = { ...State.settings, ...settings };
          wakeButton.textContent = settings.wakeWordEnabled ? "Turn off Hey Macha" : "Enable Hey Macha";
          wakeCheck.textContent = settings.wakeWordEnabled ? "Ready" : "Optional";
          wakeStatus.textContent = settings.wakeWordEnabled
            ? "Hey Macha is enabled and ready."
            : "Hey Macha is off. Push-to-talk remains available.";
        } catch (error) {
          settings.wakeWordEnabled = previous;
          wakeStatus.textContent = `Could not update Hey Macha: ${
            error instanceof Error ? error.message : String(error)
          }`;
          console.error("[macha] could not update wake phrase during setup", error);
        } finally {
          wakeButton.disabled = false;
        }
      });
      stage.append(h("div", { class: "voice-setup-choice" }, wakeCheck, wakeStatus), wakeButton);
    }

    if (isDictation) {
      const dictationStatus = h("p", {
        class: "voice-setup-status",
        "aria-live": "polite",
        text: dictationReady
          ? "Dictation inserted successfully. You can try another phrase."
          : "Click the field, hold Right Alt while speaking, and release. The test transcription appears here.",
      });
      const dictationInput = h("textarea", {
        class: "voice-setup-dictation-input",
        rows: "2",
        maxlength: "500",
        placeholder: "Click here, then hold Right Alt and speak…",
        "aria-label": "Dictation test field",
      }) as HTMLTextAreaElement;
      dictationInput.value = dictationText;
      setupDictationInput = dictationInput;
      setupDictationStatus = dictationStatus;
      const updateDictationStatus = () => {
        dictationText = dictationInput.value;
        dictationReady = dictationText.trim().length > 0;
        if (dictationReady) {
          dictationStatus.textContent = "Dictation inserted successfully. You can try another phrase.";
          return;
        }
        const current = dictationStatus.textContent ?? "";
        if (
          current.startsWith("Listening")
          || current === "Transcribing…"
          || current.startsWith("Microphone")
          || current.startsWith("No dictation")
          || current.startsWith("No speech")
          || current.startsWith("Dictation test failed")
        ) {
          return;
        }
        dictationStatus.textContent =
          "Click the field, hold Right Alt while speaking, and release. The test transcription appears here.";
      };
      dictationInput.addEventListener("input", updateDictationStatus);
      dictationPollTimer = window.setInterval(updateDictationStatus, 200);
      const clearDictation = h("button", {
        class: "voice-setup-skip",
        text: "Clear",
        onclick: () => {
          dictationInput.value = "";
          dictationText = "";
          dictationInput.focus();
          dictationReady = false;
          dictationStatus.textContent =
            "Click the field, hold Right Alt while speaking, and release. The test transcription appears here.";
        },
      });
      stage.append(
        h("div", { class: "voice-setup-shortcut" },
          h("span", { text: "HOLD" }),
          h("kbd", { text: "Right Alt" }),
          h("span", { text: "SPEAK, THEN RELEASE" }),
        ),
        dictationInput,
        h("div", { class: "voice-setup-dictation-footer" }, dictationStatus, clearDictation),
      );
      stopSetupDictation = registerSetupDictationTest({
        input: dictationInput,
        status: dictationStatus,
        visual: stage.querySelector<HTMLElement>(".voice-setup-visual"),
      });
      dictationInput.focus();
    }

    if (isVision) {
      let isRunning = false;
      const visionStatus = h("p", {
        class: `voice-setup-status ${visionTested ? "is-ready" : ""}`,
        "aria-live": "polite",
        "data-vision-status": "",
        text: "Checking Vision Mode status…",
      });
      const visionSteps = h("ol", { class: "voice-setup-vision-steps" },
        h("li", {},
          h("span", { text: "1" }),
          h("span", {},
            h("strong", { text: "Move Cursor: " }),
            h("span", { text: "Move your hand or wrist in front of your camera." }),
          ),
        ),
        h("li", {},
          h("span", { text: "2" }),
          h("span", {},
            h("strong", { text: "Left Click: " }),
            h("span", { text: "Pinch your thumb and index finger together." }),
          ),
        ),
        h("li", {},
          h("span", { text: "3" }),
          h("span", {},
            h("strong", { text: "Right Click & Scroll: " }),
            h("span", { text: "Pinch thumb + middle to right-click, or thumb + pinky to scroll." }),
          ),
        ),
      );
      const visionActiveNote = h("p", {
        class: "voice-setup-vision-return",
        hidden: true,
        text: "Vision Mode is live! Move your hand in front of the camera to guide the cursor.",
      });
      const visionButton = h("button", {
        class: "voice-setup-secondary",
        text: "Activate Vision Mode",
        "data-vision-test-button": "",
      });

      const updateVisionState = (running: boolean) => {
        isRunning = running;
        visionButton.textContent = running ? "Stop Vision Mode" : "Activate Vision Mode";
        visionStatus.textContent = running
          ? "Vision Mode is running! Wave your hand in front of the camera to test."
          : "Click “Activate Vision Mode” to turn on webcam hand gestures.";
        visionStatus.classList.toggle("is-ready", running);
        visionActiveNote.hidden = !running;
        visual.classList.toggle("is-listening", running);
        if (running) {
          visionTested = true;
          try {
            window.localStorage.setItem(visionTestStorageKey, "true");
          } catch {}
        }
      };

      // Poll current running state
      void (async () => {
        try {
          const running = await Bridge.gestureMouseIsRunning();
          updateVisionState(running);
        } catch {
          updateVisionState(false);
        }
      })();

      visionButton.addEventListener("click", async () => {
        visionButton.disabled = true;
        try {
          if (isRunning) {
            visionStatus.textContent = "Stopping Vision Mode…";
            await Bridge.gestureMouseStop();
            updateVisionState(false);
            visionStatus.textContent = "Vision Mode stopped. Click Activate to start again.";
          } else {
            // Grant camera consent and remember it
            cameraConsent = true;
            cameraPermissionReady = true;
            try {
              window.localStorage.setItem(cameraConsentKey, "true");
            } catch (err) {
              console.error("[macha] could not save camera consent", err);
            }
            visionStatus.textContent = "Starting camera and gesture engine…";
            await Bridge.gestureMouseStart();
            updateVisionState(true);
          }
        } catch (error) {
          visionStatus.textContent = `Could not ${isRunning ? "stop" : "start"} Vision Mode: ${
            error instanceof Error ? error.message : String(error)
          }`;
          visionStatus.classList.remove("is-ready");
          console.error("[macha] could not toggle Vision Mode during setup", error);
        } finally {
          visionButton.disabled = false;
        }
      });

      stage.append(
        visionSteps,
        h("p", {
          class: "voice-setup-privacy",
          text: "Camera frames are processed entirely on this device for hand tracking. Video is never saved, recorded, or sent to any server.",
        }),
        visionButton,
        visionStatus,
        visionActiveNote,
      );
    }

    if (isSpeech) {
      const speechStatus = h("p", {
        class: "voice-setup-status",
        "aria-live": "polite",
        text: speechReady
          ? "Voice sample played successfully."
          : "Play a sample to check your local speech voice.",
      });
      const speechButton = h("button", {
        class: "voice-setup-secondary",
        text: speechReady ? "Play sample again" : "Play voice sample",
      });
      speechButton.addEventListener("click", async () => {
        speechButton.disabled = true;
        speechPending = true;
        const testedStep = stepIndex;
        speechStatus.textContent = "Playing a sample…";
        let speechError: string | undefined;
        let unsubscribeSpeech: (() => void) | undefined;
        try {
          State.settings = { ...State.settings, ...settings };
          if (IS_TAURI) await Bridge.saveSettingsStrict(settings);
          unsubscribeSpeech = TTS.onStateChange((speaking, error) => {
            if (error) speechError = error;
            else if (speaking) speechStatus.textContent = "Speaking…";
          });
          await TTS.speak("Your Macha voice is ready.");
          if (!dialog.open || stepIndex !== testedStep) return;
          if (speechError) throw new Error(speechError);
          speechReady = true;
          speechStatus.textContent = "Voice sample played successfully.";
          speechButton.textContent = "Play sample again";
        } catch (error) {
          speechStatus.textContent = `Could not play the voice sample: ${
            error instanceof Error ? error.message : String(error)
          }`;
          console.error("[macha] voice setup sample failed", error);
        } finally {
          unsubscribeSpeech?.();
          speechPending = false;
          speechButton.disabled = false;
        }
      });
      stage.append(speechStatus, speechButton);
    }

    stage.classList.remove("voice-setup-stage-exit", "voice-setup-stage-enter");
    void stage.offsetWidth;
    stage.classList.add("voice-setup-stage-enter");
  }

  function moveToStep(nextStep: number): void {
    if (transitioning) return;
    transitioning = true;
    stage.classList.add("voice-setup-stage-exit");
    window.setTimeout(() => {
      stepIndex = Math.max(0, Math.min(8, nextStep));
      renderStep();
      const title = stage.querySelector<HTMLElement>(".voice-setup-title");
      title?.focus({ preventScroll: true });
    }, 160);
  }

  function openSetup(): void {
    if (!dialog.open) {
      stepIndex = 0;
      renderStep();
      dialog.showModal();
    }
  }

  function startFirstRunSetup(): void {
    window.setTimeout(() => {
      if (!IS_TAURI) {
        openSetup();
        return;
      }

      setStandaloneSetupMode(true);
      void Bridge.openFirstRunSetupWindow()
        .then(openSetup)
        .catch((error: unknown) => {
          console.error("[macha] could not open the first-run setup window", error);
          openSetup();
        });
    }, 350);
  }

  openButton.addEventListener("click", async () => {
    if (!IS_TAURI || standaloneSetupWindow) {
      openSetup();
      return;
    }

    openButton.disabled = true;
    setupLabel.textContent = "Opening the setup guide…";
    try {
      await Bridge.openFirstRunSetupWindow();
    } catch (error) {
      setupLabel.textContent = `Could not open setup: ${
        error instanceof Error ? error.message : String(error)
      }`;
      console.error("[macha] could not open the setup guide window", error);
    } finally {
      openButton.disabled = false;
    }
  });
  backButton.addEventListener("click", () => moveToStep(stepIndex - 1));
  skipButton.addEventListener("click", () => void closeSetup("later"));
  nextButton.addEventListener("click", () => {
    if (stepIndex >= 8) {
      void closeSetup("completed");
    } else {
      moveToStep(stepIndex + 1);
    }
  });
  dialog.addEventListener("cancel", (event) => {
    event.preventDefault();
    void closeSetup("later");
  });
  window.addEventListener("pagehide", () => {
    void Bridge.setDictationSetupTestActive(false).catch((error: unknown) => {
      console.error("[macha] could not stop setup dictation mode during close", error);
    });
    stopMicrophoneTest();
    stopSetupMascot();
    stopSetupDictation?.();
    stopSetupDictation = null;
    if (speechPending) {
      speechPending = false;
      void TTS.stop().catch((error: unknown) => {
        console.error("[macha] could not stop setup speech during close", error);
      });
    }
  }, { once: true });

  try {
    const saved = window.localStorage.getItem(setupStorageKey);
    setupCompleted = saved === "completed";
    setupLabel.textContent = saved === "completed"
      ? "Macha Setup complete"
      : saved === "later" ? "Setup can be started any time" : "A few quick steps to get started";
    openButton.textContent = setupCompleted ? "Review Macha Setup" : "Start Macha Setup";
    if (!saved) {
      startFirstRunSetup();
    }
  } catch (error) {
    console.error("[macha] could not read voice setup progress", error);
    startFirstRunSetup();
  }

  return h(
    "section",
    { class: `voice-setup-launcher${IS_TAURI ? " voice-setup-launcher-desktop" : ""}` },
    h("div", { class: "voice-setup-launcher-copy" },
      h("div", { class: "voice-setup-orb", "aria-hidden": "true" }),
      h("div", {},
        h("h2", {}, h("span", { text: "Macha Setup" })),
        setupLabel,
      ),
    ),
    openButton,
    dialog,
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
  setLanguage(resolveLanguage(settings.language, systemLanguages()));
  const status = (await Bridge.hooksStatus()) ?? {
    installed: false, settingsPath: "", hookPath: "", hookReady: false,
  };
  let agents: Awaited<ReturnType<typeof Bridge.agentHooksList>> | null = null;
  let agentError = "";
  try {
    agents = await Bridge.agentHooksList();
  } catch (err) {
    agentError = String(err).replace(/^Error:\s*/, "");
  }

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
    "notion-api-key", "calcom-api-key", "elevenlabs-api-key",
    "grok-api-key", "anthropic-api-key", "openai-api-key", "gemini-api-key",
  ];
  const present: Record<string, boolean> = {};
  for (const k of keys) present[k] = (await Bridge.secretPresent(k)) ?? false;

  const voiceSetup = voiceSetupSection(present, openRouterAccounts);
  const dictation = dictationSection();
  const claude = claudeSection(status);
  const externalAgents = agentsSection(agents, agentError);
  const avatar = avatarSection();
  const chat = apiSection(openRouterAccounts, ollamaStatus, present, openRouterError);
  const memory = memorySection();
  const workflows = workflowsSection();
  const automation = automationSection();
  const vision = gestureMouseSection();
  const integrations = integrationsSection(present);
  const general = generalSection();
  const phone = phoneSection(settings.phoneLink ?? false, settings.phoneDetails ?? false, toggle);
  const featureNotice = (title: string, message: string) => h(
    "section",
    { class: "hub-unavailable" },
    h("h2", {}, h("span", { text: title })),
    h("p", { class: "hint", text: message }),
  );

  mountSettingsHub(root, {
    dictation: { sections: [voiceSetup, dictation] },
    notetaker: { sections: [notetakerSection()] },
    statistics: {
      sections: [
        featureNotice(
          "Dictation statistics",
          "Macha's dictation totals and usage charts have not been ported. Macha's local weekly recap is shown below.",
        ),
        weeklyRecapSection(),
      ],
    },
    dictionary: {
      sections: [
        featureNotice("Dictionary", "Macha's existing Nixi dictation settings include custom vocabulary and spoken-word replacements."),
        dictation,
      ],
    },
    snippets: {
      unavailable: "Macha's standalone text-snippet manager has not been ported to Macha yet.",
    },
    styles: {
      sections: [
        featureNotice("Styles", "Macha's existing Nixi dictation settings include per-application writing styles."),
        dictation,
      ],
    },
    "flow-settings": { sections: [general] },
    general: { sections: [avatar, general] },
    "active-pills": {
      sections: [featureNotice(
        "Active pills",
        "Macha uses its own island and integration controls rather than Macha's pill manager.",
      )],
    },
    agents: {
      sections: [
        claude,
        externalAgents,
        featureNotice("Agent plan usage", "Codex and Claude plan-usage tracking has not been ported yet."),
      ],
    },
    chat: { sections: [chat] },
    voice: { sections: [voiceSetup] },
    integrations: { sections: [integrations] },
    phone: { sections: [phone] },
    shortcuts: { sections: [dictation] },
    memory: { sections: [memory] },
    workflows: { sections: [workflows] },
    automation: { sections: [automation] },
    vision: { sections: [vision] },
  }, version);

  void onEvent<Settings>("settings-changed", (s) => {
    settings = { ...settings, ...s };
    State.settings = { ...State.settings, ...s };
  });
}

void main();
