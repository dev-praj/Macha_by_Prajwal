# Coucou Windows architecture

## Index

- [Feature inventory](#feature-inventory): source-backed scope for this guide.
- [How to read the diagrams](#how-to-read-the-diagrams)
- [Diagrams](#diagrams): three overview maps and eleven feature-flow diagrams.
- [Feature explanations](./features.md): user flows, implementation paths, data, and current limitations.

## How to read the diagrams

The diagrams distinguish the WebView UI, Rust application logic, local data and
services, Windows facilities, and remote providers. Arrows mean a real
call, event, read/write, or dependency found in the repository; they do not
imply a relational database or a hosted Coucou backend. A dotted or labeled
boundary marks work that is optional, user-configured, or not confirmed as
active. File paths in this guide are repository-relative. “Uncertain” means
the code or wiring does not establish the behavior conclusively.

## Feature inventory

### Application surface and navigation

| Feature | Files | Depends on |
|---|---|---|
| Island window, mascot, views, and navigation | `windows/src/island/island.ts`, `windows/src/island/fsm.ts`, `windows/src/core/layout.ts`, `windows/src/views/views.ts`, `windows/src/views/chat.ts`, `windows/src/views/upload.ts` | Tauri island window, frontend state, hook and integration events |
| Screen/view registry and settings entry point | `windows/src/core/layout.ts`, `windows/src/views/views.ts`, `windows/src/settings/main.ts`, `windows/src-tauri/src/lib.rs` | Tauri bridge and settings window |
| Tray, pause, auto-hide, positioning, and cursor polling | `windows/src/main.ts`, `windows/src-tauri/src/lib.rs`, `windows/src-tauri/src/island.rs`, `windows/src-tauri/src/platform/windows.rs` | Windows window/cursor APIs and Tauri window events |
| Task pills and live activity updates | `windows/src/core/state.ts`, `windows/src/island/hooks.ts`, `windows/src/island/integrations.ts`, `windows/src/views/views.ts` | Claude Code hook events, integration pollers, in-memory task state |

The view registry includes `overview`, `empty`, `approval`, `question`, `error`,
`finished`, `confused`, `upload`, `uploading`, `choose`, `prompt`, `note`,
`settings`, `greeting`, `mail`, `searching`, and `result`. The last three named
views are placeholders in `views.ts`; their presence in the registry is not
evidence that mail sending or a separate search/results screen is implemented.
`greeting` is a layout mode rather than a builder in `views.ts`.

### Claude Code, chat, and automation

| Feature | Files | Depends on |
|---|---|---|
| Claude Code hook preview, install, backup, and removal | `windows/src/settings/main.ts`, `windows/src-tauri/src/hooks.rs`, `windows/src-tauri/src/lib.rs`, `windows/hook/src/main.rs` | User confirmation, `%USERPROFILE%\\.claude\\settings.json`, packaged `coucou-hook.exe` |
| Claude Code session activity and task pills | `windows/hook/src/main.rs`, `windows/src-tauri/src/pipe.rs`, `windows/src/island/hooks.ts`, `windows/src/core/state.ts` | Claude Code hook events, Windows named pipe, island event listener |
| Permission requests and user decisions | `windows/src-tauri/src/pipe.rs`, `windows/src-tauri/src/hooks.rs`, `windows/src-tauri/src/lib.rs`, `windows/src/island/hooks.ts`, `windows/src/views/views.ts` | PermissionRequest hook, visible approval card, Claude Code fallback on timeout/decline |
| Conversational assistant and conversation management | `windows/src/views/chat.ts`, `windows/src/core/bridge.ts`, `windows/src/core/state.ts`, `windows/src-tauri/src/claude.rs`, `windows/src-tauri/src/lib.rs` | Provider configuration, local Rust state, `localStorage`, optional memories/files |
| Provider selection and LLM requests | `windows/src/settings/main.ts`, `windows/src-tauri/src/llm_client.rs`, `windows/src-tauri/src/claude.rs`, `windows/src-tauri/src/secrets.rs` | OpenRouter credentials or local Ollama service |
| Local desktop, filesystem, and web actions | `windows/src-tauri/src/claude.rs`, `windows/src-tauri/src/automation.rs`, `windows/src/views/chat.ts` | Model tool/action response, Windows APIs, filesystem; confirmation policies vary by action |
| Saved workflows and step-by-step execution | `windows/src/settings/main.ts`, `windows/src/views/chat.ts`, `windows/src/core/bridge.ts`, `windows/src-tauri/src/workflows.rs`, `windows/src-tauri/src/claude.rs` | Persisted workflow text, model, screenshot/action automation, current screen |
| File drop, inbox copy, and chat context | `windows/src/views/upload.ts`, `windows/src/views/chat.ts`, `windows/src-tauri/src/files.rs`, `windows/src-tauri/src/claude.rs` | Tauri file-drop/dialog APIs and per-user local inbox |

### Voice and camera

| Feature | Files | Depends on |
|---|---|---|
| Push-to-talk transcription | `windows/src/main.ts`, `windows/src/core/wakeWord.ts`, `windows/src-tauri/src/voice.rs`, `stt_server.py` | Browser microphone permission, local Whisper service at `127.0.0.1:5005` |
| “Hey Macha” wake phrase and command capture | `windows/src/main.ts`, `windows/src/core/wakeWord.ts`, `windows/src-tauri/src/voice.rs`, `stt_server.py` | Microphone permission, local Whisper, wake-phrase settings |
| Local spoken responses | `windows/src-tauri/src/voice.rs`, `windows/src-tauri/src/settings.rs`, `windows/src/core/bridge.ts` | Windows SAPI/TTS process and speech setting |
| Vision Mode and gesture-controlled cursor | `windows/src/settings/main.ts`, `windows/src-tauri/src/gesture_mouse.rs`, `windows/gesture_mouse/app.py` | Opt-in camera access, Python and gesture dependencies, local dashboard at `127.0.0.1:8765` |
| Palm-lock countdown and cursor lock | `windows/gesture_mouse/app.py` | Webcam hand landmarks and Vision Mode state; current Python working-tree edits are noted under [evidence and scope](#evidence-and-scope) |

### Integrations and notifications

| Feature | Files | Depends on |
|---|---|---|
| Vercel deployment status | `windows/src-tauri/src/integrations.rs`, `windows/src/views/integrations.ts`, `windows/src/core/state.ts` | User-supplied Vercel token and Vercel API |
| GitHub activity | `windows/src-tauri/src/integrations.rs`, `windows/src/views/integrations.ts`, `windows/src/core/state.ts` | User-supplied GitHub token and GitHub API |
| Stripe activity | `windows/src-tauri/src/integrations.rs`, `windows/src/views/integrations.ts`, `windows/src/core/state.ts` | User-supplied Stripe API key and Stripe API |
| Notion updates | `windows/src-tauri/src/integrations.rs`, `windows/src/views/integrations.ts`, `windows/src/core/state.ts` | User-supplied Notion API key and Notion API |
| Cal.com bookings | `windows/src-tauri/src/integrations.rs`, `windows/src/views/integrations.ts`, `windows/src/core/state.ts` | User-supplied Cal.com API key and Cal.com API |
| Windows notification cards / Messages integration | `windows/src-tauri/src/messages.rs`, `windows/src/island/integrations.ts`, `windows/src/views/integrations.ts`, `windows/src-tauri/capabilities/default.json` | User-enabled Windows notification access; packaged app identity is required |
| Spotify playback status and controls | `windows/src-tauri/src/spotify.rs`, `windows/src/views/integrations.ts`, `windows/src/settings/main.ts` | Spotify OAuth/client configuration and Spotify Web API |

The runtime scheduler starts the five HTTP integration pollers listed above.
Windows notifications and Spotify use separate background loops. Although
credential names and poll functions for Resend and n8n exist, the inspected
startup scheduler does not start their pollers; they are not listed as active
features here. The `elevenlabs-api-key` credential is exposed in Settings, but
no runtime consumer was found during discovery. These are documented as gaps,
not as working integrations.

### Settings, persistence, and runtime

| Feature | Files | Depends on |
|---|---|---|
| Settings window and user preferences | `windows/src/settings/main.ts`, `windows/src/core/state.ts`, `windows/src-tauri/src/settings.rs`, `windows/src-tauri/src/lib.rs` | Settings WebView, Tauri commands, `%APPDATA%\\Coucou\\settings.json` |
| API credential and OpenRouter account management | `windows/src/settings/main.ts`, `windows/src-tauri/src/secrets.rs`, `windows/src-tauri/src/lib.rs` | Windows Credential Manager; account names/IDs in `%APPDATA%\\Coucou\\openrouter-accounts.json` |
| Durable assistant memories | `windows/src/settings/main.ts`, `windows/src-tauri/src/memory.rs`, `windows/src-tauri/src/claude.rs` | `%LOCALAPPDATA%\\Coucou\\memories.json`; chat context |
| Conversation and task state | `windows/src/core/state.ts`, `windows/src-tauri/src/claude.rs`, `windows/src-tauri/src/integrations.rs` | Browser `localStorage` for conversation history; in-memory Rust/frontend state for active tasks |
| Logging, background service startup, and shutdown | `windows/src-tauri/src/lib.rs`, `windows/src-tauri/src/log.rs`, `windows/src-tauri/src/voice.rs`, `windows/src-tauri/src/integrations.rs`, `windows/src-tauri/src/messages.rs`, `windows/src-tauri/src/spotify.rs` | Per-user local directories, child processes, timers |
| Build, install, and packaging | `windows/package.json`, `windows/vite.config.ts`, `windows/src-tauri/tauri.conf.json`, `windows/src-tauri/Cargo.toml`, `windows/src-tauri/capabilities/default.json`, `windows/scripts/pack.mjs`, `windows/scripts/pack-msix.ps1`, `requirements.txt`, `windows/gesture_mouse/requirements.txt` | Node.js, Rust, WebView2, Python speech/gesture dependencies, Windows packaging tools |

## Data, services, permissions, and configuration inventory

- **Database:** no SQL, Firebase, or hosted Coucou database was found. The app
  uses JSON files, browser `localStorage`, process memory, inbox files, and the
  Windows Credential Manager. See [features](./features.md) for fields and
  ownership.
- **Auth and roles:** no Coucou account/login or application role model was
  found. The application runs in the current Windows user context. Claude Code
  tool approval is an action-level permission decision, not an app role.
- **Local service routes:** STT Flask service exposes `GET /health` and
  `POST|OPTIONS /transcribe` on `127.0.0.1:5005`. Vision Mode exposes `GET
  /health` and a WebSocket at `ws://127.0.0.1:8765/ws`.
- **Configured external services:** Anthropic/OpenRouter and Ollama for model
  requests; GitHub, Vercel, Stripe, Notion, Cal.com for integration data;
  Spotify OAuth/API; Claude Code hook configuration. URLs and data read/write
  behavior are described in [features](./features.md).
- **User-configured keys and URLs (names only):** `openrouter-api-key`,
  `anthropic-api-key`, `elevenlabs-api-key`, `n8n-url`, `n8n-api-key`,
  `vercel-token`, `github-token`, `stripe-api-key`, `resend-api-key`,
  `notion-api-key`, `calcom-api-key`, `spotify-client-id`,
  `spotify-refresh-token`, and `spotify-oauth-token`. Secret values are not
  read or reproduced in this guide.
- **Operating-system/environment inputs:** `APPDATA`, `LOCALAPPDATA`,
  `USERPROFILE`, `PATH`, `PATHEXT`, `USERNAME`, plus hook-context names
  `TERM_PROGRAM`, `WT_SESSION`, `TERM_SESSION_ID`, `VSCODE_PID`, and
  `CLAUDE_CODE_SSE_PORT`. No `.env`-driven application configuration was
  established.
- **Background work:** Claude Code named-pipe relay; local speech-service
  process monitor; Vercel, GitHub, Stripe, Notion, and Cal.com polling;
  Windows notification polling; Spotify polling; and island cursor/position
  polling. Intervals, pause rules, and failure handling are detailed in the
  feature guide.
- **Permissions:** microphone permission is requested by WebView media APIs;
  Vision Mode requires camera access; Windows notification access is explicitly
  requested. Hook writes require an explicit preview/confirmation. Chat actions
  have action-specific approval behavior.
- **Build/deployment:** Tauri 2 + Rust and TypeScript/Vite; NSIS installer and
  a separate MSIX packager. The packaged resources include `stt_server.py`,
  the gesture controller, and the native hook helper.

## How the inventory is scoped

This is a Windows-application guide, not a claim that every registered route,
credential field, or helper function is a finished feature. The implementation
and call paths were inspected; placeholders and apparently unused integrations
are called out explicitly. No endpoint or feature is inferred solely from a
name.

## Diagrams

The inventory contains 30 grouped entries across the application surface,
Claude Code/chat/automation, voice and camera, integrations, and settings/runtime.
The diagrams comprise three architecture maps and eleven request or data-path
sequences. The HTML diagrams are interactive; use the Export menu in each one
for standalone SVG or raster files.

The linked PNGs are Archify-rendered 1440×900 light-theme preview captures,
including the diagram viewer controls. They are image previews rather than
cropped standalone exports. Each diagram also has additional dark-theme and
2048×1320 captures in the same `images/` folder.

| Diagram | Archify JSON | Interactive HTML | PNG preview |
|---|---|---|---|
| System overview | [Source](./system-overview.architecture.json) | [Open](./system-overview.html) | [View](./images/system-overview.visual-check.1440x900.light.png) |
| Feature dependency map | [Source](./feature-map.architecture.json) | [Open](./feature-map.html) | [View](./images/feature-map.visual-check.1440x900.light.png) |
| Data and persistence model | [Source](./data-model.architecture.json) | [Open](./data-model.html) | [View](./images/data-model.visual-check.1440x900.light.png) |
| Launch and island navigation | [Source](./flow-app-shell.sequence.json) | [Open](./flow-app-shell.html) | [View](./images/flow-app-shell.visual-check.1440x900.light.png) |
| Claude Code hook and approval | [Source](./flow-hooks-approval.sequence.json) | [Open](./diagrams/flow-hooks-approval.html) | [View](./images/flow-hooks-approval.visual-check.1440x900.light.png) |
| Chat and local action | [Source](./flow-chat-automation.sequence.json) | [Open](./diagrams/repaired/flow-chat-automation.html) | [View](./images/flow-chat-automation.visual-check.1440x900.light.png) |
| Saved workflow observe / act / verify | [Source](./flow-workflows.sequence.json) | [Open](./diagrams/flow-workflows.html) | [View](./images/flow-workflows.visual-check.1440x900.light.png) |
| File drop and chat context | [Source](./flow-file-drop.sequence.json) | [Open](./diagrams/flow-file-drop.html) | [View](./images/flow-file-drop.visual-check.1440x900.light.png) |
| Push-to-talk and Hey Macha | [Source](./flow-voice.sequence.json) | [Open](./diagrams/repaired/flow-voice.html) | [View](./images/flow-voice.visual-check.1440x900.light.png) |
| Vision Mode gesture loop | [Source](./flow-vision-mode.sequence.json) | [Open](./diagrams/flow-vision-mode.html) | [View](./images/flow-vision-mode.visual-check.1440x900.light.png) |
| External integration pollers | [Source](./flow-integration-pollers.sequence.json) | [Open](./diagrams/flow-integration-pollers.html) | [View](./images/flow-integration-pollers.visual-check.1440x900.light.png) |
| Windows notifications | [Source](./flow-windows-notifications.sequence.json) | [Open](./diagrams/flow-windows-notifications.html) | [View](./images/flow-windows-notifications.visual-check.1440x900.light.png) |
| Spotify connect and playback | [Source](./flow-spotify.sequence.json) | [Open](./diagrams/flow-spotify.html) | [View](./images/flow-spotify.visual-check.1440x900.light.png) |
| Settings, credentials, and memory | [Source](./flow-settings-memory.sequence.json) | [Open](./diagrams/repaired/flow-settings-memory.html) | [View](./images/flow-settings-memory.visual-check.1440x900.light.png) |

All 14 artifacts passed Archify's showcase validation, strict artifact check,
and browser check. Architecture routing warnings remain: the system overview
has 3 reported crossovers; the feature map has 32 crossovers, 15 routes over
the suggested bend count, and 2 over the suggested stretch; the data model has
15 crossovers, 5 routes over the suggested bend count, and 1 over the suggested
stretch. The feature-map capture shows visibly busy routing, so follow its
arrows by label rather than treating line crossings as feature dependencies.
PNG captures were generated for every diagram; manual visual inspection covered
the three architecture maps and the workflow sequence, not every flow capture.

## Evidence and scope

At discovery, repository HEAD was `bd05b4a680a5876221544934fa776d1963bb28cf`.
The preceding palm-lock task left `windows/gesture_mouse/app.py` and a Python
`__pycache__` file modified in the working tree. This read-only architecture
task does not alter them; the palm-lock explanation describes the current
working tree, and all other statements are based on source inspection.
