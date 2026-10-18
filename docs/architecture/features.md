# Feature explanations

This guide describes the current Windows working tree. “WebView” means the
embedded browser window that draws the app. The TypeScript frontend calls Rust
through Tauri commands (a typed request/response bridge) and receives events
back. The local Python services use loopback HTTP/WebSocket, so their network
traffic is on the same machine unless otherwise stated.

## Island, mascot, and navigation

### Island window and view navigation

- **What it does:** A small top-of-screen island expands to show the active
  session, questions, approvals, chat, or file-drop flow. The Mochi mascot
  changes position/state with the island mode.
- **User flow:** The app opens at the top of the selected display. Hovering the
  hidden strip reveals the compact island; click expands it. Header tabs switch
  between Overview, Ask, Drop, and in-island Settings. The separate Settings
  window is opened from that view.
- **Under the hood:** `windows/src/core/layout.ts` defines island modes, view
  names, and geometry. `windows/src/island/fsm.ts` manages hidden/compact/
  expanded transitions and timers. `windows/src/island/island.ts` syncs state
  to rendering and window geometry. `windows/src/views/views.ts` creates the
  views; `buildHeader` and `buildViews` connect navigation. `windows/src/mochi/`
  contains mascot rendering/animation.
- **Data:** Current view, mode, task pills, and mascot state are held in
  frontend memory (`windows/src/core/state.ts`). No backend record is created
  by opening or navigating a view.
- **Edge cases:** An expanded alert can be pinned so auto-collapse does not
  hide it. The hidden Windows state is a zero-height window at screen top, not
  a hardware notch. `mail`, `searching`, and `result` are placeholder views;
  `greeting` has layout state but no dedicated `buildViews` entry.
- **Dependencies:** Tauri window commands/events, cursor polling, frontend
  state, and the view registry.

### Tray, hotkey, positioning, and app lifecycle

- **What it does:** The tray can reopen the island, open settings, pause
  activity, or toggle auto-hide. The global `Ctrl+Alt+Space` shortcut opens
  voice input. The island tracks the selected monitor and cursor.
- **User flow:** Use the tray or hotkey while the app is running; close/hide
  and pause behavior depends on the selected setting. A second app launch
  signals the existing instance rather than opening another island.
- **Under the hood:** `windows/src-tauri/src/lib.rs::run` registers
  single-instance, tray, autostart, global-shortcut, and dialog plugins. Its
  setup starts the helpers and creates the Settings window before the island.
  `windows/src-tauri/src/island.rs::spawn_cursor_poll` reads Windows cursor and
  display state. `windows/src/main.ts` receives tray/hotkey events and routes
  them into the island or chat.
- **Data:** Autostart and auto-hide preferences are in `settings.json`; current
  pause/pin/window state is in memory.
- **Edge cases:** Display changes trigger repositioning. Polling is paused or
  reduced when the island need not be interactive. Registration failures are
  logged; the app can still launch without the hotkey.
- **Dependencies:** Windows APIs, Tauri plugins, and per-user settings.

### Activity cards and integration pills

- **What it does:** The overview shows live session steps and up to four
  selected integration pills. New external activity can reveal the island and
  play a sound.
- **User flow:** Open Overview to inspect task pills. Select a pill to focus
  its details. A new integration event may temporarily badge the pill.
- **Under the hood:** `windows/src/core/state.ts` holds `AgentTask` and
  `IntegrationInfo`; `windows/src/views/views.ts::buildOverview` renders cards
  and pills. `windows/src/island/hooks.ts` maps Claude hook events into steps;
  `windows/src/island/integrations.ts` maps integration events into state,
  sound, and island reveal.
- **Data:** Task steps, active selection, integration results, and recent
  notification objects are in memory. They are not a durable event history.
- **Edge cases:** Finished/error badges clear after 60 seconds. The user can
  enable at most four integration pills. Paused state suppresses surfaced
  integration notifications.
- **Dependencies:** Hook relay, integration pollers, frontend state, and sound
  assets.

## Claude Code and the assistant

### Claude Code hook installation and session activity

- **What it does:** A packaged helper receives Claude Code lifecycle/tool
  events and relays them into the island so the user can see session activity.
- **User flow:** In Settings, inspect a preview of the changes to
  `%USERPROFILE%\\.claude\\settings.json`, then explicitly apply or remove the
  hooks. Claude Code events update the matching project/session pill.
- **Under the hood:** `windows/src/settings/main.ts::claudeSection` calls
  `hooks_status`, `hooks_preview`, and `hooks_apply`.
  `windows/src-tauri/src/hooks.rs::preview` merges or removes Coucou entries,
  prepares a dated backup and a file fingerprint, and writes only after
  confirmation if the file still matches the preview. The hook helper in
  `windows/hook/src/main.rs` reads JSON on stdin, trims large fields, adds
  context, and connects to the Windows named pipe. `windows/src-tauri/src/pipe.rs`
  emits ordinary events to the island; `windows/src/island/hooks.ts::handleHook`
  updates the task pill.
- **Data:** Claude Code settings and backups are local files. The relay
  forwards selected hook payload fields; active task state is in memory.
- **Edge cases:** Malformed or unreadable Claude settings are reported rather
  than overwritten. Hook helper events have short budgets and exit without
  blocking Claude Code if Coucou is unavailable. Empty/invalid hook input
  produces no forwarded event.
- **Dependencies:** Claude Code, the bundled `coucou-hook.exe`, the per-user
  named pipe, and Tauri events. The hook event list includes session, prompt,
  tool, notification, stop, and subagent lifecycle events.

### Permission cards and Claude Code questions

- **What it does:** A `PermissionRequest` can be answered from an island card;
  a normal Claude question is displayed but must still be answered in the
  terminal.
- **User flow:** Inspect the operation/file/URL in the approval card and choose
  Allow or Deny. For a question card, switch to the Claude Code terminal and
  answer there.
- **Under the hood:** `windows/src-tauri/src/pipe.rs::handle` assigns a request
  ID and waits for a UI acknowledgement and decision. The frontend
  `windows/src/island/hooks.ts` routes the payload, and `buildApproval` /
  `buildQuestion` in `windows/src/views/views.ts` render it. `approval_decision`,
  `approval_ack`, and `approval_decline` in `windows/src-tauri/src/lib.rs`
  return the decision to the helper, which formats the Claude Code hook
  response in `windows/hook/src/main.rs`.
- **Data:** The request and pending response channel are in memory; no
  approval database or persistent “always allow” rules were found.
- **Edge cases:** The UI acknowledgement deadline is 800 ms; the relay's
  decision deadline is 108 seconds, just below the helper's 110-second budget.
  If the island is paused, unavailable, or the card is not acknowledged, the
  helper leaves Claude Code to ask in the terminal.
- **Dependencies:** Installed hooks, named pipe, active island event listener,
  and a human decision.

### Chat, model routing, and conversation history

- **What it does:** Chat answers questions, can use local context and memories,
  and may propose local or desktop actions.
- **User flow:** Open Ask, type or dictate a request, review the reply and any
  proposed action, and continue in the conversation. Conversations can be
  selected/deleted; assistant text and code can be copied.
- **Under the hood:** `windows/src/views/chat.ts` builds the chat surface and
  calls `Bridge.chatSend`. `windows/src/core/bridge.ts` invokes Rust. In
  `windows/src-tauri/src/claude.rs::send`, the app adds system instructions,
  up to 20 history messages, memory context, file/window context, and optional
  screen image. `windows/src-tauri/src/llm_client.rs::chat` routes to Ollama
  (`127.0.0.1:11434`) or OpenRouter (`https://openrouter.ai/api/v1`).
- **Data:** Conversation IDs/titles/messages/current file pointer are saved in
  WebView `localStorage` under `coucou.conversations.v1`. Active LLM request
  history and cancellation tokens live in Rust memory. Durable memories are
  loaded separately and inserted into prompt context.
- **Edge cases:** The `auto` provider mode tries local Ollama first and can
  fall back to OpenRouter; `ollamaOnly` and `openRouterOnly` constrain it.
  Malformed conversation storage is logged and ignored. Some generic Tauri
  bridge calls catch errors and return `null`; `callOrThrow` calls instead
  propagate errors to the UI.
- **Dependencies:** User-configured model/provider, optional API credentials,
  network for OpenRouter, or a locally installed/running Ollama model.

### Local desktop and file automation

- **What it does:** The assistant can list/read/create/edit/move files, open
  apps/paths/websites, search the web, and issue supported Windows mouse,
  keyboard, window, and PowerShell actions.
- **User flow:** Ask in chat. Ordinary chat proposals show an action card and
  wait for the user to approve or deny it. App launches with ambiguous exact
  matches require a target selection first.
- **Under the hood:** `windows/src-tauri/src/claude.rs` registers tool schemas,
  validates model output, queues actions, and calls
  `windows/src-tauri/src/automation.rs::prepare` / `execute`. The latter checks
  action names, arguments, paths, previews, size limits, and Windows targets.
  `chat_action` resolves the user’s decision.
- **Data:** Read results and action status are fed into the current model
  conversation. File writes affect the user’s filesystem; edits create a
  backup and validate that the file did not change after its preview.
- **Edge cases:** Unsupported actions and invalid arguments return errors.
  Folder access may use configured automation folders. Existing file deletion
  is deliberately not an exposed tool; PowerShell still runs with the current
  Windows user’s permissions and can make broad changes.
- **Dependencies:** Chat/model response, Windows APIs, current-user file
  permissions, and user approval outside a workflow.

### Saved workflows and visible-step verification

- **What it does:** A saved prompt is run as a sequence of single visible
  desktop actions, with screenshots used to check progress.
- **User flow:** Save a workflow in Settings and select Run. The app requests
  screen-sharing access, creates checkpoints from the saved prompt, then takes
  a fresh screenshot before each action and another after the action. A model
  verifies the checkpoint; after retries it reports failure instead of claiming
  success.
- **Under the hood:** `windows/src/settings/main.ts::workflowsSection` uses
  `Bridge.workflowSave` / `workflowRun`. `windows/src-tauri/src/workflows.rs`
  persists prompt/name/IDs and emits `workflow-run`. In
  `windows/src/views/chat.ts::runWorkflow`, the prompt is placed verbatim inside
  a model request wrapper, then `captureScreenSnapshot`, `Bridge.chatSend`
  with plan/action/verify phases, `Bridge.chatAction`, and verification logic
  form the loop. `windows/src-tauri/src/claude.rs::workflow_action_tools`
  limits each action phase to one of app open/click/type/shortcut/scroll.
- **Data:** Workflows are JSON objects in `%APPDATA%\\Coucou\\workflows.json`
  with `id`, `name`, `prompt`, `archived`, `createdAt`, `updatedAt`.
  Checkpoints and screenshots exist only for the running workflow; screenshots
  are kept in memory, not written to a file. Conversation turns are in the
  current chat history.
- **Edge cases:** The app has a 60-iteration cap, two retries after the first
  attempt (three attempts total), a default 1.5-second settle delay, and 5
  seconds after opening apps/websites. `VITE_WORKFLOW_SETTLE_MS` and
  `VITE_WORKFLOW_LAUNCH_SETTLE_MS` can override those delays at build time.
  Only an explicit final `COMPLETE` plus a final screenshot check is accepted;
  workflow stop conditions produce `STOPPED_FOR_USER`, and unresolved steps
  produce `FAILED`. Final verification currently encodes a WhatsApp contact
  requirement in the prompt path; it is not generalized to arbitrary final
  states.
- **Dependencies:** Screen sharing, model provider, automation tools, browser
  or app state, and the user-started workflow. A configured cloud model may
  receive screenshots and prompt text.

### File drop and chat file context

- **What it does:** A file can be dropped onto the island or selected from a
  native picker, copied to an app inbox, and attached to a conversation.
- **User flow:** Drop/select a file, see the upload sequence and file chip, then
  ask a question about it. Directories are not accepted.
- **Under the hood:** `windows/src/views/upload.ts` handles the file flow and
  `windows/src/core/bridge.ts` exposes drop/picker/`ingest_file`. Rust
  `windows/src-tauri/src/files.rs::ingest` copies the file. `claude.rs` reads
  file context for chat and attaches it to the first turn.
- **Data:** Copies go under `%LOCALAPPDATA%\\Coucou\\inbox`; the conversation
  stores the display name and copy path, not a second copy of the bytes.
- **Edge cases:** Folders are rejected. Name collisions get numbered names.
  Inbox copies older than seven days are swept on later ingestion. Unreadable
  files and oversized/unsupported content report an error.
- **Dependencies:** Tauri drag/drop and file-picker permissions, local disk,
  and an LLM provider if the user asks a model to interpret the contents.

## Voice, speech, and Vision Mode

### Push-to-talk voice input

- **What it does:** Records a short microphone clip, transcribes it locally,
  and sends the resulting text to chat.
- **User flow:** Click the voice control or press `Ctrl+Alt+Space`, allow
  microphone access, speak, then wait for the transcript and assistant response.
- **Under the hood:** `windows/src/main.ts` receives the hotkey event.
  `windows/src/views/chat.ts::startListening` records with browser
  `getUserMedia`/`MediaRecorder`, monitors silence, and posts to
  `http://127.0.0.1:5005/transcribe`. `windows/src-tauri/src/voice.rs`
  starts/monitors `stt_server.py`; the Python service decodes audio and runs
  `faster-whisper` (`base.en`, English, CPU/int8).
- **Data:** Raw audio is posted to the local Flask process and temporary audio
  files are deleted after transcription. Transcript text enters chat and may
  then be sent to the configured model provider.
- **Edge cases:** Silence, low-confidence text, missing microphone, and
  unavailable service show an error/notice. The recorder stops after silence
  or a 30-second maximum. English is the configured transcription language.
- **Dependencies:** Microphone permission, Python plus Flask/Whisper/audio
  dependencies, and local Whisper model files. The app attempts to start a
  suitable local Python service.

### “Hey Macha” wake phrase

- **What it does:** Continuous local microphone listening recognizes a wake
  phrase and either opens voice capture or submits words spoken after the
  phrase.
- **User flow:** The default-on wake listener starts when the chat view and
  Tauri window are ready. Its status is reflected in the chat voice state.
  Say “Hey Macha”; the app expands/pins the island, plays a blip, speaks a
  local acknowledgement, and starts capturing if no command followed the
  phrase.
- **Under the hood:** `windows/src/core/wakeWord.ts::createWakeWordListener`
  uses Web Audio to detect speech, collects a short clip, and sends it to local
  `/transcribe`. `matchWakePhrase` normalizes punctuation/diacritics and
  accepts phonetic spellings. `chat.ts::reconcileWakeWord` manages listener
  state/retry; `onWakeUtterance` dispatches the matched command; `main.ts`
  expands the island and calls `Island.reactToWakePhrase`.
- **Data:** Wake clips go only to the loopback transcription service. The
  recognized command, if submitted, follows normal chat/provider routing.
  Phrase, enable flag, and speech threshold are stored in Settings.
- **Edge cases:** The listener retries after service/audio errors, reacts to
  ended microphone tracks and audio-context changes, and pauses while
  recording/speaking or handling a pending action. A denied mic permission
  requires the user to enable it in Windows privacy settings. Phrase matching
  is heuristic, not a guaranteed speech-recognition result.
- **Dependencies:** WebView microphone permission, local STT, app settings,
  local audio feedback, and chat.

### Spoken assistant replies

- **What it does:** Reads assistant responses aloud through a local speech
  engine.
- **User flow:** Choose voice reply mode/voice/rate/volume in Settings. For
  qualifying replies, cleaned response text is chunked and queued for speech;
  the user can stop playback.
- **Under the hood:** `windows/src/core/tts.ts` filters local English voices,
  cleans Markdown/URLs, and queues text using browser Speech Synthesis or
  `Bridge.ttsSpeak`. The native command in
  `windows/src-tauri/src/voice.rs::tts_speak` invokes Windows PowerShell
  `System.Speech` (SAPI) and handles stop/status.
- **Data:** Text comes from the current assistant response; engine/voice/rate/
  volume settings are in `settings.json`. No ElevenLabs runtime consumer was
  found; the saved ElevenLabs credential is not used by this path.
- **Edge cases:** If no local English Windows voice is installed, native
  speech reports a clear error. Text is divided into short chunks. Speech is
  stopped before starting a microphone capture.
- **Dependencies:** Windows local voice installation or Web Speech local
  voices, app settings, and assistant response text.

### Vision Mode: hand gestures and palm lock

- **What it does:** An optional Python webcam process maps detected hand
  gestures to mouse movement, clicking, scrolling, and cursor lock.
- **User flow:** Enable/start Vision Mode from Settings, grant camera access,
  and use the live dashboard/camera window. A sustained fully open palm
  toggles cursor lock; while locked, supported hand poses can scroll or rotate.
- **Under the hood:** `windows/src-tauri/src/gesture_mouse.rs` locates Python,
  checks dependencies, launches `windows/gesture_mouse/app.py`, and waits for
  `GET /health` on `127.0.0.1:8765`. The Python process uses OpenCV and
  MediaPipe hand landmarks, PyAutoGUI for cursor actions, and an aiohttp
  dashboard (`/`, `/health`, WebSocket `/ws`). Its status payload includes
  hand detection, gesture flags, lock state, hold progress, and FPS.
  `windows/src/settings/main.ts::gestureMouseSection` starts/stops the process.
- **Data:** Camera frames and gesture calculations are processed by the local
  Python process; dashboard status is local WebSocket data. No webcam image
  upload path was found.
- **Edge cases:** Python 3.9+ and listed dependencies are required. The hand
  landmark model is downloaded from Google storage on first run if missing.
  The process startup has a 30-second dashboard readiness timeout. PyAutoGUI's
  failsafe is enabled. The current working tree's `is_open_palm` requires
  extended fingers and thumb; a partially closed hand resets the timer.
- **Dependencies:** Webcam, Python/OpenCV/MediaPipe/PyAutoGUI/aiohttp, local
  model file, and Windows cursor APIs. The current palm-lock source differs
  from the recorded repository HEAD because of the preceding user task.

## Integration features

All integration cards are optional. The Settings screen stores API credentials
in Windows Credential Manager and lets the user select up to four active pills.
Rust pollers emit in-memory updates; the frontend renders them. Credentials are
not included in this guide.

### Vercel

- **What it does / user flow:** Shows recent deployments and status in its
  island card; the user configures a token and can refresh/open the dashboard.
- **Under the hood:** `integrations.rs::poll_vercel` calls
  `https://api.vercel.com/v6/deployments?limit=5`; `views/integrations.ts`
  renders deployment details.
- **Data:** Recent deployment response fields are held in memory; secret name
  is `vercel-token`.
- **Edge cases / dependencies:** Poll starts after five seconds and repeats
  every 30 seconds while enabled and unpaused. Missing credentials skip the
  request; HTTP/network errors become card state.

### GitHub

- **What it does / user flow:** Shows repository counts/stars and recent
  activity from the configured account.
- **Under the hood:** `integrations.rs::poll_github` reads the authenticated
  user and owned repositories using `api.github.com`; `views/integrations.ts`
  renders the card.
- **Data:** Counts and card data are in memory; secret name is `github-token`.
- **Edge cases / dependencies:** Poll starts after seven seconds and repeats
  every five minutes while enabled. API errors are surfaced in card state;
  repository fetch failure can leave the star count at zero.

### Stripe

- **What it does / user flow:** Displays account balance and recent charges.
- **Under the hood:** `integrations.rs::poll_stripe` requests the Stripe
  balance and latest charges; the integration view formats the returned values.
- **Data:** Summary values are held in memory; secret name is `stripe-api-key`.
- **Edge cases / dependencies:** Poll starts after six seconds and repeats
  every 30 seconds. Error text distinguishes invalid key/access from network
  failure.

### Notion

- **What it does / user flow:** Displays recently edited pages/databases with
  title, icon, edit time, and link.
- **Under the hood:** `integrations.rs::poll_notion` posts a three-item search
  to `https://api.notion.com/v1/search`; `parse_notion_page` normalizes the
  response; the integration view renders it.
- **Data:** Page summaries live in memory; secret name is `notion-api-key`.
- **Edge cases / dependencies:** Poll starts after nine seconds and repeats
  every five minutes. Missing credential skips the call; API errors populate
  card error state.

### Cal.com

- **What it does / user flow:** Displays upcoming bookings and attendee details.
- **Under the hood:** `integrations.rs::poll_calcom` requests
  `https://api.cal.com/v2/bookings?status=upcoming`; the integration view
  renders the result.
- **Data:** Booking summaries live in memory; secret name is `calcom-api-key`.
- **Edge cases / dependencies:** Poll starts after eight seconds and repeats
  every five minutes. API and network errors populate the card error state.

### Windows notification cards

- **What it does / user flow:** With Windows notification permission, Coucou
  surfaces newly observed notifications as island cards and can open their
  source app.
- **Under the hood:** `messages.rs::request_message_access` requests
  `UserNotificationListener` access. `messages.rs::start` polls at one-second
  intervals, emits new items, and maps the notification ID to the source app.
  `windows/src/island/integrations.ts` queues/renders the cards.
- **Data:** Notification IDs, app IDs, and deduplication state are in process
  memory; notification text is passed to the island.
- **Edge cases / dependencies:** Existing notifications are recorded as the
  initial baseline and are not all replayed on startup. The source map is
  ephemeral, so opening an old item can fail. Access errors are surfaced.
  Packaged MSIX identity is documented as a prerequisite for the Windows
  listener; development builds may not receive it.

### Spotify

- **What it does / user flow:** Shows current playback and offers playback
  controls from the island.
- **Under the hood:** `spotify.rs::spotify_connect` starts a temporary callback
  listener at `127.0.0.1:43821/callback`, creates OAuth PKCE state, and opens
  Spotify sign-in. The access/refresh token is stored with the credential
  helper. `spotify.rs::start` polls every four seconds; control commands target
  Spotify Web API player endpoints.
- **Data:** Current track, queue, and playback status are in memory. OAuth
  tokens are in Windows Credential Manager; client ID uses
  `spotify-client-id`.
- **Edge cases / dependencies:** OAuth callback expires after five minutes;
  playback control may require Spotify Premium and an available device.
  Refresh and authorization errors are emitted to the card.

## Settings and local data

### Settings and provider configuration

- **What it does:** Configures sound, auto-hide, screen, startup, integrations,
  provider/model, local automation folders, workflows, voice, wake phrase,
  Vision Mode, hooks, and memories.
- **User flow:** Open the separate Settings window, change values, and use
  save/refresh controls. The settings page sends `settings-changed` so the
  island can apply updates without restarting.
- **Under the hood:** `windows/src/settings/main.ts` builds each section.
  `settings.rs::Settings` defines serde fields, defaults, and migrations;
  `lib.rs::save_settings` persists and emits the updated settings.
- **Data:** `%APPDATA%\\Coucou\\settings.json`, except secrets which use the OS
  credential store. Settings include active integration IDs and voice/provider
  options.
- **Edge cases:** Older speech/integration values are migrated. The settings
  loader falls back to defaults if JSON parsing or deserialization fails; this
  can hide a malformed-file problem unless separately logged.
- **Dependencies:** Tauri settings window, platform config paths, credential
  store for keys, and integration/service availability.

### Credential management

- **What it does:** Stores service keys and account tokens outside plaintext
  settings files.
- **User flow:** Enter a key in Settings and save it; OpenRouter accounts can be
  named, removed, or explicitly revealed.
- **Under the hood:** `windows/src-tauri/src/secrets.rs` restricts writes to a
  fixed key allowlist and calls the `keyring` crate. OpenRouter account
  name/ID metadata is in `openrouter-accounts.json`; actual key material stays
  in the Windows credential service.
- **Data:** Credential Manager stores values; the metadata file stores only
  account `id` and `name`. Configured key names are listed in README without
  values.
- **Edge cases:** Unknown key names are refused. Invalid/duplicate account
  metadata is reported. OpenRouter reveal is explicit; ordinary
  `secret_present` only returns a boolean.
- **Dependencies:** Windows credential service and the user-configured
  external integrations/model providers.

### Durable memories

- **What it does:** Lets the user add, review, and delete short facts that can
  be included in future assistant prompts.
- **User flow:** Use Settings → Memory to add a category/text or remove a
  saved memory.
- **Under the hood:** `memory.rs::remember`, `list`, and `forget` validate and
  persist entries. `claude.rs` loads them for chat context. The Settings UI
  refreshes and reports errors.
- **Data:** `%LOCALAPPDATA%\\Coucou\\memories.json`; each item has `id`,
  `category`, `text`, `createdAt`, and `updatedAt`. Categories are constrained
  to a known list.
- **Edge cases:** Maximum 200 entries and 500 bytes of text. Credential-like
  strings and control characters are rejected. Duplicate same-category facts
  update their timestamp rather than creating a duplicate.
- **Dependencies:** Local disk and assistant chat. Although stored locally,
  memory text is included in model prompt context and can be sent to whichever
  provider handles that request.

### Persistence overview

| Data | Location | Lifetime / relationship |
|---|---|---|
| Preferences | `%APPDATA%\\Coucou\\settings.json` | Durable; controls active settings and integration IDs |
| Saved workflows | `%APPDATA%\\Coucou\\workflows.json` | Durable; independent workflows with prompt text |
| OpenRouter account metadata | `%APPDATA%\\Coucou\\openrouter-accounts.json` | Durable names/IDs only; key value stored separately |
| API keys and OAuth tokens | Windows Credential Manager | Durable per-user credentials |
| Memories | `%LOCALAPPDATA%\\Coucou\\memories.json` | Durable; inserted into model context |
| Dropped file copies | `%LOCALAPPDATA%\\Coucou\\inbox\\` | Local copies; swept after seven days on ingestion |
| Conversation history | WebView `localStorage`, key `coucou.conversations.v1` | Durable per WebView profile; transcript/file path metadata |
| Active chat/provider/task/integration state | Frontend and Rust process memory | Volatile; cleared when app exits |
| Workflow screenshots/checkpoints | Workflow execution memory | Temporary; screenshot is sent with model call and not written as a file |
| Hook settings/backups | `%USERPROFILE%\\.claude\\settings.json` and dated sibling backups | Durable Claude Code configuration |

There is no entity-relational database. The arrows in the data-model diagram
will show only actual read/write/context relationships, not SQL foreign keys.

## Build, runtime dependencies, and deployment

- `windows/package.json`: Vite, TypeScript, Tauri CLI/API, and dialog plugin;
  `build` runs TypeScript checking then Vite.
- `windows/src-tauri/Cargo.toml`: Rust/Tauri application, HTTP/serialization,
  credential, Windows API, and process support.
- `windows/vite.config.ts`: frontend entrypoints and the Windows asset path.
- `windows/src-tauri/tauri.conf.json`: application windows, packaging, and
  bundled resources such as the hook helper, STT script, and gesture program.
- `requirements.txt`: local speech-service dependencies.
- `windows/gesture_mouse/requirements.txt`: camera/gesture dependencies.
- `windows/scripts/pack.mjs` and `windows/scripts/pack-msix.ps1`: distribution
  packaging flows; Tauri config includes NSIS packaging.
- Runtime external service endpoints and environment variable names are listed
  in the README. No environment variable value or credential was accessed for
  this guide.

## Gaps and risks observed (not fixed)

1. **Dormant or unconnected surfaces:** Resend and n8n polling functions are
   defined but not started by the integration scheduler. The ElevenLabs key is
   saved/displayed but has no runtime consumer. `anthropic-api-key` is allowed
   by the secret store but no active use was found. The island's mail/searching/
   result builders are placeholders. `n8n`/`resend` migration aliases remain
   in settings code.
2. **Local-service boundaries:** STT and Vision Mode bind to loopback, but
   their HTTP/WebSocket handlers have no app-specific authentication. STT adds
   `Access-Control-Allow-Origin: *`. This is a local boundary, not a claim of
   externally reachable service.
3. **First-run downloads:** The gesture model is downloaded from Google
   storage if absent. The STT service loads a named Whisper model, but the
   source does not bundle a model asset or establish offline first-run setup.
   Thus “local processing” does not prove “no network on first launch.”
4. **Model privacy:** Chat/workflow text, included memory, file context, and
   workflow screenshots can leave the device when a configured cloud model is
   selected or used as fallback. The app does not upload raw microphone audio
   to that provider; audio is transcribed locally first.
5. **Generic bridge errors:** `Bridge.call` logs and returns `null` on command
   failure. Some UI call sites use defaults, so not all failures are surfaced
   consistently. `callOrThrow` is used where explicit error display is
   required.
6. **Settings recovery:** `settings::load` falls back to defaults when reading
   or parsing settings fails, rather than returning a startup error. A user may
   lose effective settings until the file is repaired.
7. **Integration data quality:** Some poller code treats secondary response
   errors as empty/default results; for example, a failed GitHub repository
   fetch produces a zero star count after the user lookup succeeded.
8. **Windows packaging requirements:** Windows notification access requires
   package identity according to the README. A development build can therefore
   behave differently from an MSIX install.
9. **Python working-tree evidence:** The palm-lock implementation is modified
   relative to the HEAD recorded during discovery; this guide describes the
   current working tree. This task did not edit that source.
10. **Build target residue:** Some Rust modules still contain conditional
    non-Windows branches even though the packaged app and requested inventory
    are Windows-specific. Their presence alone is not treated as a supported
    user-facing platform.

Uncertainty is called out inline where code establishes only part of a path
(for example, local Whisper model provisioning or unused credential fields).
