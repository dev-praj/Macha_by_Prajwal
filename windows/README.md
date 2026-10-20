<div align="center">

<img src="src-tauri/icons/128x128.png" width="96" alt="Macha icon">

# Macha for Windows

**Macha doesn't get a notch on a PC — so it lives at the top of your screen instead.**

Approve Claude Code permissions, watch your session work, drop a file, chat with Claude, keep an eye on your services — without leaving what you're doing.

![Windows 10/11](https://img.shields.io/badge/Windows-10%2F11-0078D4?logo=windows)
![Tauri 2](https://img.shields.io/badge/Tauri-2-FFC131?logo=tauri&logoColor=black)
![Rust](https://img.shields.io/badge/Rust-backend-000?logo=rust)
![License: MIT](https://img.shields.io/badge/license-MIT-green)

</div>

<img src="screenshots/greeting.png" width="640" alt="Macha waving hello at launch">

---

## Install

The downloadable installer is **temporarily unavailable**. Microsoft Defender
wrongly flags the unsigned installer as malware (`Trojan:Win32/Wacatac.H!ml`, a
machine-learning false positive). A report is under review at Microsoft, and the
installer will be published again once it is cleared and code-signed.

Until then, [build it yourself](#build-it-yourself): it takes a few minutes and
installs for the current user only — no admin prompt.

## Using it

<img src="screenshots/compact.png" width="292" alt="The compact island, with the integration pills as mini Machas">
<img src="screenshots/overview.png" width="640" alt="The overview: the focused integration on the left, the other pills on the right">
<img src="screenshots/approval.png" width="640" alt="A Claude Code permission request, with Deny and Allow">
<img src="screenshots/chat.png" width="640" alt="Chatting with Claude from the island">
<img src="screenshots/drop.png" width="640" alt="Macha turned into a box, waiting for a file">

| What you do | What happens |
|---|---|
| Move the mouse to the very top-centre of the screen | Macha peeks out |
| Click the small island | It opens |
| Click Macha | It gets annoyed. Three times in a row and it goes dizzy |
| Rest the pointer on Macha for two seconds | Hearts |
| Choose a file in Drop or drag one onto the island | Macha turns into a box, swallows it, then offers to answer questions about it |
| `Esc` | Closes the island |
| Tray icon | Open, Settings…, Pause, Quit |

Everything else happens on its own: a Claude Code permission request opens the
island with **Deny / Allow**, a finished session shows what it did, and
your integrations sit in the coloured pills next to Macha.

Choose **Macha**, **Cubee**, or **Onee** in **Settings → Avatar**. Your selection
is saved on this device and appears throughout the island, agent pills, greeting,
and file-drop animation.

## Claude Code

<img src="screenshots/settings.png" width="562" alt="The settings window">

Open **Settings… → Claude Code → Install hooks…**. You get the exact diff of what
will change in `%USERPROFILE%\.claude\settings.json`, the path of the dated backup
that will be taken, and nothing is written until you click. Your own hooks are
never touched, and uninstalling removes only Macha's entries.

The relay is a tiny executable, `macha-hook.exe`, copied to
`%LOCALAPPDATA%\Macha\bin\` at launch. It is given 300 ms to reach Macha and
exits cleanly if the app is closed, slow or crashed — **a Claude Code session is
never blocked or slowed down by Macha.** If nobody answers a permission request
in time, Macha stays quiet and Claude Code asks in the terminal as usual.

It works from any terminal — Windows Terminal, PowerShell, VS Code, Git Bash.

## LLM providers

**Settings… → LLM providers** defaults to **Auto**: Macha tries the local
Ollama server first and contacts OpenRouter only if Ollama is unavailable, the
selected model is missing, or the local request fails with a server error or
timeout. It checks Ollama again automatically so it can return to local
inference. Choose **Ollama only** to prevent any OpenRouter calls, or
**OpenRouter only** to bypass Ollama.

The Ollama model picker includes `qwen2.5:7b`, `gpt-oss:20b`, and any other
installed models discovered from Ollama. The default is `qwen2.5:7b`; if a
model is missing, Settings shows the matching `ollama pull <model>` command.
Ollama must be running locally; Macha tries `ollama serve` once if it is not
reachable.

OpenRouter accounts are configured as the fallback in **Settings… → LLM
providers**. API keys stay in the **Windows Credential Manager**; only account
names are saved in the app's configuration. Uploaded keys are masked in
Settings and returned to the interface only after you explicitly click the
reveal button. OpenRouter requests default to 1024 output tokens and have a
single lower-token retry for credit errors.

The default **Automatic** model uses OpenRouter's free-model router to choose a
model for each fallback task. Settings also lists the models in the supplied
model guide for manual selection. OpenRouter's lineup and limits can change.
Macha sends plain-text files, images, and PDFs, and reports when a file cannot
be read or is not supported.

The chat view includes **Past conversations** and **New conversation**. Chat
transcripts are saved locally in the app's WebView storage; attached files stay
in Macha's local inbox. New conversations can also use the text and attachment
contents from previous chats as context when the original attachment is still
available in Macha's inbox; visible conversation transcripts remain separate.

Macha can keep a small set of durable memories between conversations. They are
stored locally in the user's Macha data folder, included in requests to the
selected chat provider, and can be reviewed or removed in **Settings → Memory**.
You can add memories directly in Settings as well. Models without function-call
support still answer normally; use Settings to save memories when automatic
memory capture is unavailable. Do not save credentials or other sensitive
information. **Capture screen** asks
Windows or the desktop environment which screen or window to share, captures a
single still image, and attaches it only to the next chat message; it is not
stored in the transcript. Chat can also open websites or run web searches in
the system browser; ordinary chat asks for approval, while workflows run these
actions automatically after you start them.

### Local automation

In **Settings… → Local automation**, Macha can work with files anywhere your
current account has access, including other mounted drives. Absolute paths can
target any location; previously selected folders remain preferred starting
points for relative paths. Macha can open installed apps from Windows app
registrations or Start menu shortcuts, list and read files, open files or
folders, create items, and move files. It can also replace or clear a file's
contents after showing the proposed change and creating a backup. On Windows,
it can also inspect visible windows, switch or resize windows, click, type,
drag, scroll, press keyboard shortcuts, and run PowerShell commands for a task
you request. In ordinary chat, each GUI operation and PowerShell command is
presented separately for your approval. When you click **Run** on a saved
workflow, its requested actions run automatically without per-action approval;
review the workflow instructions before starting it. PowerShell commands run
as your current Windows user and can access or change anything that account is
permitted to access. They are limited to a 60-second runtime. Macha does not
provide a separate delete-file action, though PowerShell commands can perform
deletions.

When Macha reads a file, its contents and the paths involved are sent to the
selected chat provider (local Ollama or OpenRouter). Request file reads only
when you are comfortable sharing that content with the configured provider.
Models that do not support native tool calls use a compatibility prompt for
local actions where supported. Macha validates every proposed action.
Ordinary chat actions require explicit approval; workflow actions are
pre-authorized when the workflow is started from Settings.

### Saved workflows

In **Settings → Workflows**, create reusable named instructions, edit or
duplicate them, archive and restore them, or delete them. Running a workflow sends its instructions as a chat message through the
currently configured provider and model; it does not switch or replace the
LLM. The workflow automatically carries out requested local file and desktop
actions without asking for approval at each step. Review instructions before
running: PowerShell executes with your account permissions and can make
system-wide changes. Ambiguous app matches ask you to select the intended app,
then continue automatically. Workflow definitions are stored locally in
Macha's configuration folder.

No telemetry. The only network requests Macha makes are to the services you
configure yourself.

## Messages and music controls

**Messages** uses Windows' notification listener and therefore requires the
packaged MSIX build; the regular NSIS installer and development build do not
have the package identity Windows requires. In **Settings → Integrations**,
choose **Allow Windows notifications** and approve access in Windows. Macha
shows notifications from all apps in the island for about three seconds.
Clicking one opens its source app; Windows does not expose a reliable way for
every app to reopen an individual toast. Turn off banners for apps in Windows
notification settings if you want to avoid seeing both Macha and Windows
notifications.

**Music** uses the Windows media-session controls. Start playback in Spotify or
another app that publishes Windows media controls, then Macha can show the
current track and control play/pause, skip, and track position. No Spotify
Developer app, API key, sign-in, or Spotify Premium account is required.

To build an MSIX with the notification-listener capability, install the Windows
10/11 SDK and a signing certificate with its private key in the current user's
certificate store, then run:

```powershell
npm run pack:msix -- -CertificateThumbprint YOUR_CERTIFICATE_THUMBPRINT
```

The signed package is written to `windows/target/msix/`. The publisher in the
package manifest is taken from the certificate, so Windows will only accept
updates signed by a certificate with the same publisher identity. WebView2
Evergreen Runtime must be installed on the machine.

## Voice control

Voice input runs offline with the local `faster-whisper` model `base.en`;
transcription is forced to English and sent only to the local service at `127.0.0.1:5005`, and voice
chat uses Ollama-only mode so transcripts are not sent to OpenRouter. Macha can
also speak replies locally using Web Speech when an installed voice is available,
with Windows SAPI as the automatic fallback. Both engines select English voices
only, preferring en-US and then en-IN; install an English Windows speech voice if
none is available. Configure **Settings… → General → Speak replies** to keep it
off, enable it only for voice messages (the default), or speak every reply. The
voice, engine, rate, and volume are configurable there; speech text is processed
on-device. Spoken replies begin as the configured model streams text, rather
than waiting for the complete answer before playback starts.

The **Wake phrase "Hey Macha"** setting is enabled by default. While enabled,
Macha listens locally for speech and sends completed speech segments only to
the local Whisper service. Say **“Hey Macha, [your question]”** in one go, or
call **“Macha”** and then ask after it acknowledges you. Short pauses between
the wake words are combined automatically. For better recognition on your
microphone, choose **Settings… → General → Record “Hey Macha”** and repeat the
phrase during the five-second recording. Macha validates the local Whisper
transcript and tunes the microphone speech threshold; the recording is discarded
after calibration. The microphone indicator pulses blue while the
conversation stays open until its spoken reply finishes, then normal auto-hide
resumes. The indicator remains blue while the wake listener is armed, and the
listener reconnects when the app returns from sleep or the microphone changes.
Disable the setting to stop background microphone monitoring. Allow Macha
under **Privacy & security → Microphone** when Windows asks for access.

Opening an application, file, or folder requires a separate confirmation:
Macha asks aloud and waits for an English **“yes”** or **“no”** before acting.
It looks up the exact full name in Windows' installed Start-app catalog
(including packaged apps), then falls back to Start Menu shortcuts. If multiple
matching entries are found, Macha asks which one you mean before asking for
final confirmation. The on-screen choice and Yes/No buttons remain available
as alternatives.

Install Python 3.10 or newer and the dependencies from the repository root:

```powershell
python -m venv .venv
.\.venv\Scripts\python.exe -m pip install -r requirements.txt
```

Macha starts the transcription service when it launches. The wake listener
waits for the local model to finish loading and retries automatically if startup
takes longer than expected. If automatic startup is unavailable, run
`python .\stt_server.py` from the repository root. Press
**Ctrl+Alt+Space** or the **Voice** button beside the chat input to start
recording; press it again to stop and send the recognized text through chat.
In automatic provider mode, voice requests try the configured OpenRouter model
first and fall back to Ollama if the network or OpenRouter credits are
unavailable. Explicit Ollama-only or OpenRouter-only settings are respected.
The service loads the Whisper model on startup. To use a different model, edit
`MODEL_NAME` in the root `stt_server.py` (the default is `base.en`).

## Vision Mode

On Windows, **Settings… → Vision Mode → Start Vision Mode** opens the
camera preview and embeds its live dashboard in Settings. Move your wrist to
move the cursor, pinch thumb + index to left-click and drag, thumb + middle to
right-click, and thumb + pinky to scroll. Press **Esc**, close the preview, or
use **Stop Vision Mode** in Settings to stop it. You can also say
**“activate vision mode”** while Hey Macha is listening. Cursor tracking stays a few
pixels away from screen corners to prevent accidental activation of the
mouse-control emergency fail-safe; moving the pointer manually into a corner
still stops Vision Mode.

Hold an open palm steady for five seconds to lock the cursor in place; the
camera preview animates a progress ring and confirms when the cursor locks.
Show the palm again for five seconds to unlock. While locked, move the joined index and middle
fingers to scroll up, down, left, or right. To rotate a 3D view in apps that
support mouse-drag rotation, move the pointer over the model before locking,
then hold the thumb and index finger together or extend them, curl the other
three fingers, and turn your wrist clockwise or anticlockwise. The pointer
returns to its locked position after each rotation drag.

Install Python 3.13 and the gesture packages once from the repository root.
The isolated environment is kept in Macha's local application data, separate
from Macha's other Python tools, and works with both the packaged app and the
development build:

```powershell
py -3.13 -m venv "$env:LOCALAPPDATA\Macha\gesture_mouse\.venv"
& "$env:LOCALAPPDATA\Macha\gesture_mouse\.venv\Scripts\python.exe" -m pip install -r windows\gesture_mouse\requirements.txt
```

### Build the Windows installer with bundled Python helpers

The packaged app includes its speech and Gesture Mouse executables and their
models, so Python is not required on the target PC. Building the installer
requires Python 3.13, Node.js/npm, Rust with the MSVC toolchain, and internet
access to retrieve the pinned helper models. From the repository root, run:

```powershell
powershell -NoProfile -ExecutionPolicy Bypass -File .\windows\scripts\build-python-helpers.ps1
```

This builds both one-file helpers and then runs the existing Tauri installer
build. The Windows WebView2 runtime and optional external services such as
Ollama are not bundled.

When run from source, the MediaPipe hand landmark model is downloaded to
Macha's local application data on first start. The packaged installer bundles
the model with the Gesture Mouse helper. Webcam processing runs locally; the
embedded dashboard only listens on localhost. Allow desktop apps to access the camera in Windows
**Privacy & security → Camera**. The included implementation is derived from
[HandGesture-Mouse](https://github.com/murfidya/HandGesture-Mouse) and is
MIT-licensed; see `windows/gesture_mouse/LICENSE`.

## Build it yourself

You need [Rust](https://rustup.rs), [Node 20+](https://nodejs.org), and the
**MSVC build tools** (Visual Studio Build Tools with "Desktop development with
C++"). WebView2 ships with Windows 10/11.

```powershell
cd windows
npm install
npm run tauri dev      # live-reloading development build
powershell -NoProfile -ExecutionPolicy Bypass -File .\scripts\build-python-helpers.ps1 # bundles helpers, then builds installer
npm run pack:msix -- -CertificateThumbprint YOUR_CERTIFICATE_THUMBPRINT # signed MSIX with notification access
```

`npm run dev` alone serves the front end in an ordinary browser, which is enough
to work on the island's looks. It also serves `dev/upload-preview.html`, which
replays the whole file-drop choreography on a loop — the one part of the UI that
otherwise needs a real drag from Explorer to see. Neither page ships in the app.

`npm run pack` leaves two files in `windows/release/`:

```
Macha-Windows-X.Y.Z-setup.exe    the versioned installer
Macha-Windows-setup.exe          the same file under the rolling name
```

Installing is optional — `target/release/macha.exe` runs on its own. There is no
window in the taskbar and no console: the island at the top of the screen and the
Macha in the notification area are the whole app, and Quit lives in its menu.

The sound files live in `assets/sounds/` and are copied into the app during the
frontend build.

The app icon and the tray icon are drawn in code, like Macha itself:

```powershell
npm run icons          # regenerates src-tauri/icons from scripts/gen-icons.mjs
```

## Start at Windows logon

The Task Scheduler task runs the release app as the current user, 20 seconds
after logon. It launches the app's tray/island UI (no terminal window), waits
for the local speech service at `http://127.0.0.1:5005/health`, and monitors the
app so it can restart it after a crash, at most three times with 10 seconds
between attempts.
Installation disables the app's separate autostart registration so only the
scheduled task launches it.
This is a per-user task; it does not need administrator rights and does not
start the app before sign-in.

Run these commands from the repository root in PowerShell:

```powershell
# Install and enable the logon task.
powershell.exe -NoProfile -ExecutionPolicy Bypass -File .\windows\manage-startup-task.ps1 -Action Install

# Test the launcher directly, then run the scheduled task on demand.
powershell.exe -NoProfile -ExecutionPolicy Bypass -File .\windows\start-app.ps1
Start-ScheduledTask -TaskPath "\Macha\" -TaskName "Macha Auto Start"
Get-ScheduledTaskInfo -TaskPath "\Macha\" -TaskName "Macha Auto Start"

# Manage or remove the task.
powershell.exe -NoProfile -ExecutionPolicy Bypass -File .\windows\manage-startup-task.ps1 -Action Enable
powershell.exe -NoProfile -ExecutionPolicy Bypass -File .\windows\manage-startup-task.ps1 -Action Disable
powershell.exe -NoProfile -ExecutionPolicy Bypass -File .\windows\manage-startup-task.ps1 -Action Remove
```

The launcher is idempotent: it reuses a running Macha process, and reports a
clear error rather than terminating another process if port 5005 is occupied.
Its timestamped log is `%LOCALAPPDATA%\Macha\startup.log`; Macha and speech
service diagnostics are in `%LOCALAPPDATA%\Macha\macha.log`. Check both after
a failure. The GUI executable has no console streams; its timestamped app log
is where runtime errors are recorded.

To verify after a reboot, sign in, wait up to 20 seconds plus the speech-model
startup time, confirm the Macha island/tray icon appears, and run
`curl.exe http://127.0.0.1:5005/health` (expect `{"status":"ok"}`). Review both
logs for the latest timestamp. `-Action Remove` unregisters the task and
restores the app-managed autostart preference/registration that was present
before installation. If you move the repository or rebuild the app, reinstall
the task so its absolute executable and working-directory paths are refreshed.

### Layout

```
windows/
  src/                 island front end (TypeScript, no framework)
    macha/             Macha and the launch greeting, in Canvas 2D
    island/            state machine, hooks, integrations
    views/             every island view
    settings/          the settings window
  src-tauri/           Rust backend: window, named pipe, Claude API, pollers
    msix/              package manifest and notification-listener capability
  hook/                macha-hook.exe, the Claude Code relay
  scripts/             icon generator and MSIX packager
```

### Log

`%LOCALAPPDATA%\Macha\macha.log` — hook events, permission decisions, poller
problems. It stays on your machine.

- What the Windows build leaves out, this one does too: sending a file by
  email, dragging Macha onto a window, and jumping to a specific terminal
  window — "Open terminal" opens the folder in VS Code.
