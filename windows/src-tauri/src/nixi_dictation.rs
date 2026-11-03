// Nixi Dictation — text post-processing and text insertion command.

use crate::settings::{DictationReplacement, DictationSnippet};
use regex::Regex;

/// Post-processes transcribed dictation text applying rules in order:
/// 1. Trim leading/trailing whitespace.
/// 2. Remove filler words at word boundaries: \b(um|uh|er|ah|hmm)\b (case-insensitive).
/// 3. Spoken punctuation:
///    - "new line" / "newline" -> \n
///    - "new paragraph" / "newparagraph" -> \n\n
///    - "bullet point" -> •
///    - "period" or "full stop" -> .
///    - "comma" -> ,
///    - "question mark" -> ?
///    - "exclamation mark" -> !
/// 4. Self-correction: if text matches (.+)\s+(?:actually|wait),?\s+make that\s+(.+) -> replace with second capture group.
/// 5. Capitalise first letter of result.
/// 6. Collapse multiple spaces into one.
pub fn postprocess(text: &str) -> String {
    // 1. Trim leading/trailing whitespace
    let mut result = text.trim().to_string();
    if result.is_empty() {
        return String::new();
    }

    // 2. Remove filler words at word boundaries
    if let Ok(filler_re) = Regex::new(r"(?i)\b(?:um|uh|er|ah|hmm)\b") {
        result = filler_re.replace_all(&result, "").to_string();
    }

    // 3. Spoken punctuation (order matters: multi-word phrases first)
    if let Ok(para_re) = Regex::new(r"(?i)\b(?:new\s*paragraph)\b") {
        result = para_re.replace_all(&result, "\n\n").to_string();
    }
    if let Ok(nl_re) = Regex::new(r"(?i)\b(?:new\s*line)\b") {
        result = nl_re.replace_all(&result, "\n").to_string();
    }
    if let Ok(bullet_re) = Regex::new(r"(?i)\b(?:bullet\s+point)\b") {
        result = bullet_re.replace_all(&result, "• ").to_string();
    }
    if let Ok(period_re) = Regex::new(r"(?i)\b(?:period|full\s+stop)\b") {
        result = period_re.replace_all(&result, ".").to_string();
    }
    if let Ok(comma_re) = Regex::new(r"(?i)\bcomma\b") {
        result = comma_re.replace_all(&result, ",").to_string();
    }
    if let Ok(question_re) = Regex::new(r"(?i)\bquestion\s+mark\b") {
        result = question_re.replace_all(&result, "?").to_string();
    }
    if let Ok(exclamation_re) = Regex::new(r"(?i)\bexclamation\s+mark\b") {
        result = exclamation_re.replace_all(&result, "!").to_string();
    }

    // Clean up extra whitespace directly before punctuation marks (. , ? !)
    if let Ok(punct_ws_re) = Regex::new(r"[ \t]+([.,?!])") {
        result = punct_ws_re.replace_all(&result, "$1").to_string();
    }

    // Clean up spaces around newlines
    if let Ok(nl_ws_re) = Regex::new(r"[ \t]*\n[ \t]*") {
        result = nl_ws_re.replace_all(&result, "\n").to_string();
    }

    // 4. Self-correction: if text matches (.+)\s+(?:actually|wait),?\s+make that\s+(.+)
    if let Ok(self_correct_re) = Regex::new(r"(?is)^(.+)\s+(?:actually|wait),?\s+make that\s+(.+)$")
    {
        if let Some(caps) = self_correct_re.captures(&result) {
            if let Some(replacement) = caps.get(2) {
                result = replacement.as_str().trim().to_string();
            }
        }
    }

    // 5. Capitalise first letter and words following sentence-ending punctuation (. ? !)
    result = capitalize_sentences(&result);

    // 6. Collapse multiple spaces into one (horizontal spaces only so newlines remain)
    if let Ok(spaces_re) = Regex::new(r"[ \t]{2,}") {
        result = spaces_re.replace_all(&result, " ").to_string();
    }

    result.trim().to_string()
}

fn capitalize_sentences(s: &str) -> String {
    let mut result = String::with_capacity(s.len());
    let mut capitalize_next = true;

    for ch in s.chars() {
        if capitalize_next && ch.is_alphabetic() {
            result.extend(ch.to_uppercase());
            capitalize_next = false;
        } else {
            result.push(ch);
            if ch == '.' || ch == '?' || ch == '!' {
                capitalize_next = true;
            }
        }
    }
    result
}

fn apply_replacements(text: &str, replacements: &[DictationReplacement]) -> Result<String, String> {
    if replacements.len() > 100 {
        return Err("At most 100 dictation replacements are allowed.".into());
    }

    let mut result = text.to_string();
    for entry in replacements {
        let spoken = entry.spoken.trim();
        if spoken.is_empty()
            || spoken.len() > 100
            || entry.replacement.len() > 500
            || spoken.contains('\0')
            || entry.replacement.contains('\0')
        {
            return Err("A dictation replacement is invalid or exceeds its length limit.".into());
        }

        let phrase = spoken
            .split_whitespace()
            .map(regex::escape)
            .collect::<Vec<_>>()
            .join(r"\s+");
        let pattern = format!(r"(?iu)(^|[^\p{{L}}\p{{N}}_])({phrase})($|[^\p{{L}}\p{{N}}_])");
        let regex = Regex::new(&pattern)
            .map_err(|error| format!("Could not compile dictation replacement: {error}"))?;
        let mut replaced = String::with_capacity(result.len());
        let mut cursor = 0;
        for captures in regex.captures_iter(&result) {
            let whole = captures.get(0).expect("capture group 0 always exists");
            replaced.push_str(&result[cursor..whole.start()]);
            if let Some(left) = captures.get(1) {
                replaced.push_str(left.as_str());
            }
            replaced.push_str(&entry.replacement);
            if let Some(right) = captures.get(3) {
                replaced.push_str(right.as_str());
            }
            cursor = whole.end();
        }
        replaced.push_str(&result[cursor..]);
        result = replaced;
    }
    Ok(result)
}

fn prepare_snippets(
    text: &str,
    snippets: &[DictationSnippet],
) -> Result<(String, Vec<(String, String)>), String> {
    const MAX_TEXT_BYTES: usize = 1_000_000;
    if snippets.len() > 100 {
        return Err("At most 100 dictation snippets are allowed.".into());
    }
    if text.len() > MAX_TEXT_BYTES {
        return Err("Dictation text is too large to expand snippets.".into());
    }

    let mut markers = Vec::with_capacity(snippets.len());
    let mut reserved = Vec::with_capacity(snippets.len() * 2 + 1);
    reserved.push(text);
    for snippet in snippets {
        reserved.push(&snippet.trigger);
        reserved.push(&snippet.text);
    }
    for candidate in (0xE000..=0xF8FF).filter_map(char::from_u32) {
        let marker = candidate.to_string();
        if reserved.iter().all(|value| !value.contains(&marker)) {
            markers.push(marker);
            if markers.len() == snippets.len() {
                break;
            }
        }
    }
    if markers.len() != snippets.len() {
        return Err("Could not safely prepare dictation snippets.".into());
    }

    let mut result = text.to_string();
    let mut protected = Vec::with_capacity(snippets.len());
    for (snippet, marker) in snippets.iter().zip(markers) {
        let trigger = snippet.trigger.trim();
        if trigger.is_empty()
            || trigger.len() > 100
            || snippet.text.trim().is_empty()
            || snippet.text.len() > 10_000
            || trigger.contains('\0')
            || snippet.text.contains('\0')
        {
            return Err("A dictation snippet is invalid or exceeds its length limit.".into());
        }

        let phrase = trigger
            .split_whitespace()
            .map(regex::escape)
            .collect::<Vec<_>>()
            .join(r"\s+");
        let pattern = format!(r"(?iu)(^|[^\p{{L}}\p{{N}}_])({phrase})($|[^\p{{L}}\p{{N}}_])");
        let regex = Regex::new(&pattern)
            .map_err(|error| format!("Could not compile dictation snippet trigger: {error}"))?;
        let mut replaced = String::with_capacity(result.len());
        let mut cursor = 0;
        while cursor < result.len() {
            let Some(captures) = regex.captures(&result[cursor..]) else {
                break;
            };
            let phrase = captures.get(2).expect("capture group 2 always exists");
            let start = cursor + phrase.start();
            let end = cursor + phrase.end();
            push_snippet_text(&mut replaced, &result[cursor..start], MAX_TEXT_BYTES)?;
            push_snippet_text(&mut replaced, &marker, MAX_TEXT_BYTES)?;
            cursor = end;
        }
        push_snippet_text(&mut replaced, &result[cursor..], MAX_TEXT_BYTES)?;
        result = replaced;
        protected.push((marker, snippet.text.clone()));
    }
    Ok((result, protected))
}

fn finish_snippets(text: String, snippets: &[(String, String)]) -> Result<String, String> {
    const MAX_TEXT_BYTES: usize = 1_000_000;
    let mut expanded = String::with_capacity(text.len());
    let mut cursor = 0;
    while cursor < text.len() {
        let next = snippets
            .iter()
            .filter_map(|(marker, snippet)| {
                text[cursor..]
                    .find(marker)
                    .map(|offset| (cursor + offset, marker, snippet))
            })
            .min_by_key(|(start, _, _)| *start);
        let Some((start, marker, snippet)) = next else {
            break;
        };
        push_snippet_text(&mut expanded, &text[cursor..start], MAX_TEXT_BYTES)?;
        push_snippet_text(&mut expanded, snippet, MAX_TEXT_BYTES)?;
        cursor = start + marker.len();
    }
    push_snippet_text(&mut expanded, &text[cursor..], MAX_TEXT_BYTES)?;
    Ok(expanded)
}

#[cfg(test)]
fn apply_snippets(text: &str, snippets: &[DictationSnippet]) -> Result<String, String> {
    let (prepared, protected) = prepare_snippets(text, snippets)?;
    finish_snippets(prepared, &protected)
}

fn process_dictation_text(
    text: &str,
    style: &str,
    replacements: &[DictationReplacement],
    snippets: &[DictationSnippet],
) -> Result<String, String> {
    let (prepared, protected) = prepare_snippets(text, snippets)?;
    finish_snippets(
        apply_replacements(&apply_style(&prepared, style)?, replacements)?,
        &protected,
    )
}

fn push_snippet_text(target: &mut String, value: &str, limit: usize) -> Result<(), String> {
    if target.len().saturating_add(value.len()) > limit {
        return Err("Expanded dictation snippets exceed the 1 MB limit.".into());
    }
    target.push_str(value);
    Ok(())
}

fn apply_style(text: &str, style: &str) -> Result<String, String> {
    match style {
        "standard" => Ok(postprocess(text)),
        "verbatim" => Ok(text.trim().to_string()),
        _ => Err("Unsupported dictation writing style.".into()),
    }
}

#[tauri::command]
pub fn nixi_foreground_application() -> Result<Option<String>, String> {
    #[cfg(windows)]
    {
        use windows::core::PWSTR;
        use windows::Win32::Foundation::CloseHandle;
        use windows::Win32::System::Threading::{
            OpenProcess, QueryFullProcessImageNameW, PROCESS_NAME_FORMAT,
            PROCESS_QUERY_LIMITED_INFORMATION,
        };
        use windows::Win32::UI::WindowsAndMessaging::{
            GetForegroundWindow, GetWindowThreadProcessId,
        };

        let foreground = unsafe { GetForegroundWindow() };
        if foreground.0.is_null() {
            return Ok(None);
        }
        let mut process_id = 0;
        unsafe {
            GetWindowThreadProcessId(foreground, Some(&mut process_id));
        }
        if process_id == 0 {
            return Ok(None);
        }
        let process = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, process_id) }
            .map_err(|error| {
                format!("Could not identify the dictation target application: {error}")
            })?;

        let mut path = vec![0u16; 32_768];
        let mut length = path.len() as u32;
        let query = unsafe {
            QueryFullProcessImageNameW(
                process,
                PROCESS_NAME_FORMAT(0),
                PWSTR(path.as_mut_ptr()),
                &mut length,
            )
        };
        let _ = unsafe { CloseHandle(process) };
        query
            .map_err(|error| format!("Could not read the dictation target application: {error}"))?;

        let path = String::from_utf16_lossy(&path[..length as usize]);
        Ok(std::path::Path::new(&path)
            .file_name()
            .map(|name| name.to_string_lossy().to_ascii_lowercase()))
    }
    #[cfg(not(windows))]
    {
        Ok(None)
    }
}

#[cfg(windows)]
fn insert_via_sendinput(text: &str) -> Result<(), String> {
    use windows::Win32::UI::Input::KeyboardAndMouse::{
        SendInput, INPUT, INPUT_0, INPUT_KEYBOARD, KEYBDINPUT, KEYEVENTF_KEYUP, KEYEVENTF_UNICODE,
        VIRTUAL_KEY,
    };

    let mut inputs = Vec::new();
    for unit in text.encode_utf16() {
        inputs.push(INPUT {
            r#type: INPUT_KEYBOARD,
            Anonymous: INPUT_0 {
                ki: KEYBDINPUT {
                    wVk: VIRTUAL_KEY(0),
                    wScan: unit,
                    dwFlags: KEYEVENTF_UNICODE,
                    time: 0,
                    dwExtraInfo: 0,
                },
            },
        });
        inputs.push(INPUT {
            r#type: INPUT_KEYBOARD,
            Anonymous: INPUT_0 {
                ki: KEYBDINPUT {
                    wVk: VIRTUAL_KEY(0),
                    wScan: unit,
                    dwFlags: KEYEVENTF_UNICODE | KEYEVENTF_KEYUP,
                    time: 0,
                    dwExtraInfo: 0,
                },
            },
        });
    }

    if inputs.is_empty() {
        return Ok(());
    }

    let sent = unsafe { SendInput(&inputs, std::mem::size_of::<INPUT>() as i32) };

    if sent == inputs.len() as u32 {
        Ok(())
    } else {
        Err(format!(
            "SendInput only processed {sent} of {} events",
            inputs.len()
        ))
    }
}

#[cfg(windows)]
struct ClipboardGuard;

#[cfg(windows)]
impl Drop for ClipboardGuard {
    fn drop(&mut self) {
        unsafe {
            let _ = windows::Win32::System::DataExchange::CloseClipboard();
        }
    }
}

#[cfg(windows)]
fn open_clipboard_with_retry() -> Result<ClipboardGuard, String> {
    use windows::Win32::System::DataExchange::OpenClipboard;

    for _ in 0..10 {
        if unsafe { OpenClipboard(None).is_ok() } {
            return Ok(ClipboardGuard);
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    Err("OpenClipboard timed out".to_string())
}

#[cfg(windows)]
fn get_clipboard_text() -> Option<String> {
    use windows::Win32::Foundation::HGLOBAL;
    use windows::Win32::System::DataExchange::{GetClipboardData, IsClipboardFormatAvailable};
    use windows::Win32::System::Memory::{GlobalLock, GlobalUnlock};

    const CF_UNICODETEXT: u32 = 13;

    let _guard = open_clipboard_with_retry().ok()?;
    if unsafe { IsClipboardFormatAvailable(CF_UNICODETEXT).is_err() } {
        return None;
    }

    let handle = unsafe { GetClipboardData(CF_UNICODETEXT).ok()? };
    if handle.0.is_null() {
        return None;
    }

    let hglobal = HGLOBAL(handle.0);
    let ptr = unsafe { GlobalLock(hglobal) };
    if ptr.is_null() {
        return None;
    }

    let wide_ptr = ptr as *const u16;
    let mut len = 0;
    unsafe {
        while *wide_ptr.add(len) != 0 {
            len += 1;
        }
    }

    let slice = unsafe { std::slice::from_raw_parts(wide_ptr, len) };
    let text = String::from_utf16_lossy(slice);
    unsafe {
        let _ = GlobalUnlock(hglobal);
    }
    Some(text)
}

#[cfg(windows)]
fn set_clipboard_text(text: &str) -> Result<(), String> {
    use windows::Win32::Foundation::{GlobalFree, HANDLE};
    use windows::Win32::System::DataExchange::{EmptyClipboard, SetClipboardData};
    use windows::Win32::System::Memory::{GlobalAlloc, GlobalLock, GlobalUnlock, GMEM_MOVEABLE};

    const CF_UNICODETEXT: u32 = 13;

    let _guard = open_clipboard_with_retry()?;
    unsafe {
        EmptyClipboard().map_err(|e| format!("EmptyClipboard failed: {e}"))?;
        let utf16: Vec<u16> = text.encode_utf16().chain(std::iter::once(0)).collect();
        let bytes = utf16.len() * std::mem::size_of::<u16>();
        let hmem =
            GlobalAlloc(GMEM_MOVEABLE, bytes).map_err(|e| format!("GlobalAlloc failed: {e}"))?;
        let ptr = GlobalLock(hmem);
        if ptr.is_null() {
            let _ = GlobalFree(Some(hmem));
            return Err("GlobalLock returned null".to_string());
        }
        std::ptr::copy_nonoverlapping(utf16.as_ptr(), ptr as *mut u16, utf16.len());
        let _ = GlobalUnlock(hmem);

        if let Err(e) = SetClipboardData(CF_UNICODETEXT, Some(HANDLE(hmem.0))) {
            let _ = GlobalFree(Some(hmem));
            return Err(format!("SetClipboardData failed: {e}"));
        }
        Ok(())
    }
}

#[cfg(windows)]
fn clear_clipboard() {
    use windows::Win32::System::DataExchange::EmptyClipboard;
    if let Ok(_guard) = open_clipboard_with_retry() {
        unsafe {
            let _ = EmptyClipboard();
        }
    }
}

#[cfg(windows)]
fn send_ctrl_v() -> Result<(), String> {
    use windows::Win32::UI::Input::KeyboardAndMouse::{
        SendInput, INPUT, INPUT_0, INPUT_KEYBOARD, KEYBDINPUT, KEYBD_EVENT_FLAGS, KEYEVENTF_KEYUP,
        VIRTUAL_KEY, VK_CONTROL,
    };

    let ctrl_v = [
        INPUT {
            r#type: INPUT_KEYBOARD,
            Anonymous: INPUT_0 {
                ki: KEYBDINPUT {
                    wVk: VK_CONTROL,
                    wScan: 0,
                    dwFlags: KEYBD_EVENT_FLAGS(0),
                    time: 0,
                    dwExtraInfo: 0,
                },
            },
        },
        INPUT {
            r#type: INPUT_KEYBOARD,
            Anonymous: INPUT_0 {
                ki: KEYBDINPUT {
                    wVk: VIRTUAL_KEY(0x56), // 'V'
                    wScan: 0,
                    dwFlags: KEYBD_EVENT_FLAGS(0),
                    time: 0,
                    dwExtraInfo: 0,
                },
            },
        },
        INPUT {
            r#type: INPUT_KEYBOARD,
            Anonymous: INPUT_0 {
                ki: KEYBDINPUT {
                    wVk: VIRTUAL_KEY(0x56),
                    wScan: 0,
                    dwFlags: KEYEVENTF_KEYUP,
                    time: 0,
                    dwExtraInfo: 0,
                },
            },
        },
        INPUT {
            r#type: INPUT_KEYBOARD,
            Anonymous: INPUT_0 {
                ki: KEYBDINPUT {
                    wVk: VK_CONTROL,
                    wScan: 0,
                    dwFlags: KEYEVENTF_KEYUP,
                    time: 0,
                    dwExtraInfo: 0,
                },
            },
        },
    ];

    let sent = unsafe { SendInput(&ctrl_v, std::mem::size_of::<INPUT>() as i32) };

    if sent == ctrl_v.len() as u32 {
        Ok(())
    } else {
        Err(format!(
            "SendInput Ctrl+V processed {sent} of {} events",
            ctrl_v.len()
        ))
    }
}

#[cfg(windows)]
async fn insert_via_clipboard(text: &str) -> Result<(), String> {
    // Save text before replacing the clipboard. Non-text clipboard formats cannot be
    // reconstructed here, so those are cleared after paste rather than left stale.
    let original_text = get_clipboard_text();

    // Always restore/clear the clipboard, including when SendInput fails. Previously
    // an early `?` here left the dictated text in the user's clipboard indefinitely.
    let paste_result = async {
        set_clipboard_text(text)?;
        send_ctrl_v()?;
        // Let the target application finish handling WM_PASTE before restoring it.
        tokio::time::sleep(std::time::Duration::from_millis(750)).await;
        Ok::<(), String>(())
    }
    .await;

    if let Some(ref original) = original_text {
        if let Err(error) = set_clipboard_text(original) {
            crate::log::line(format!("nixi_dictate_insert: could not restore clipboard text: {error}"));
        }
    } else {
        clear_clipboard();
    }

    paste_result
}

/// Use VoiceOS's native injector when it is installed. Match VoiceOS's own focused
/// text path: send a `paste` request over its JSON IPC interface and wait for the
/// matching `{ id, ok }` response instead of assuming a spawned process typed text.
/// Macha's clipboard and Unicode paths remain the fallback.
#[cfg(windows)]
async fn insert_via_voiceos_ipc(binary: &std::path::Path, text: &str) -> Result<(), String> {
    use std::process::Stdio;
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
    use tokio::process::Command;
    use tokio::time::{timeout, Duration};

    let mut command = Command::new(binary);
    command
        .arg("--ipc")
        .creation_flags(0x0800_0000) // CREATE_NO_WINDOW
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true);

    let mut child = command
        .spawn()
        .map_err(|error| format!("Could not start VoiceOS text injector IPC: {error}"))?;
    let result = async {
        let stdin = child
            .stdin
            .as_mut()
            .ok_or_else(|| "VoiceOS text injector IPC has no input pipe.".to_string())?;
        let request = serde_json::json!({
            "id": 1,
            "command": "paste",
            "delayMs": 120,
            "text": text,
        });
        let mut request_line = serde_json::to_vec(&request)
            .map_err(|error| format!("Could not encode the VoiceOS paste request: {error}"))?;
        request_line.push(b'\n');
        stdin
            .write_all(&request_line)
            .await
            .map_err(|error| format!("Could not send text to VoiceOS injector: {error}"))?;
        stdin
            .flush()
            .await
            .map_err(|error| format!("Could not flush the VoiceOS paste request: {error}"))?;

        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| "VoiceOS text injector IPC has no output pipe.".to_string())?;
        let mut reader = BufReader::new(stdout);
        let read_response = async {
            let mut line = String::new();
            loop {
                line.clear();
                let bytes = reader
                    .read_line(&mut line)
                    .await
                    .map_err(|error| format!("Could not read the VoiceOS injector response: {error}"))?;
                if bytes == 0 {
                    return Err("VoiceOS text injector exited without confirming the paste.".to_string());
                }
                let Ok(response) = serde_json::from_str::<serde_json::Value>(line.trim()) else {
                    continue;
                };
                if response.get("id").and_then(serde_json::Value::as_u64) != Some(1)
                    || !response.get("ok").is_some_and(serde_json::Value::is_boolean)
                {
                    continue;
                }
                if response["ok"].as_bool() == Some(true) {
                    // Recent VoiceOS helpers report whether the target application
                    // acknowledged the paste separately from whether the IPC request
                    // itself was accepted. Do not treat an unconfirmed paste as typed.
                    if response
                        .get("pasteConfirmed")
                        .and_then(serde_json::Value::as_bool)
                        == Some(false)
                    {
                        return Err("VoiceOS accepted the request but did not confirm the paste.".into());
                    }
                    return Ok(());
                }
                let detail = response
                    .get("error")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or("VoiceOS rejected the paste request.");
                return Err(detail.to_string());
            }
        };

        timeout(Duration::from_secs(10), read_response)
            .await
            .map_err(|_| "VoiceOS text injector timed out before confirming the paste.".to_string())?
    }
    .await;

    if result.is_err() {
        let _ = child.kill().await;
    } else {
        // This process is intentionally one-request-per-dictation; stop it cleanly
        // after the positive response so repeated dictations cannot leak helpers.
        let _ = child.kill().await;
    }
    result
}

#[cfg(windows)]
async fn insert_via_voiceos_injector(text: &str) -> Result<bool, String> {
    let candidates = [
        std::env::var_os("ProgramFiles")
            .map(std::path::PathBuf::from)
            .map(|base| base.join("VoiceOS\\resources\\binaries\\text-injector.exe")),
        std::env::var_os("ProgramFiles(x86)")
            .map(std::path::PathBuf::from)
            .map(|base| base.join("VoiceOS\\resources\\binaries\\text-injector.exe")),
        Some(std::path::PathBuf::from(
            r"C:\Program Files\VoiceOS\resources\binaries\text-injector.exe",
        )),
    ];
    let Some(binary) = candidates
        .into_iter()
        .flatten()
        .find(|path| path.is_file())
    else {
        return Ok(false);
    };

    match insert_via_voiceos_ipc(&binary, text).await {
        Ok(()) => Ok(true),
        Err(ipc_error) if ipc_error.contains("timed out") => Err(ipc_error),
        Err(ipc_error) => Err(ipc_error),
    }
}

#[tauri::command]
pub async fn nixi_dictate_insert(
    text: String,
    replacements: Vec<DictationReplacement>,
    style: String,
    snippets: Vec<DictationSnippet>,
) -> Result<String, String> {
    let processed = process_dictation_text(&text, &style, &replacements, &snippets)?;
    if processed.is_empty() {
        crate::log::line("nixi_dictate_insert: empty text after postprocessing");
        return Ok("sendinput".to_string());
    }

    #[cfg(windows)]
    crate::nixi_hotkeys::restore_dictation_target_window()?;

    // SetForegroundWindow can return before the destination has processed its
    // activation message. Give that app a short moment to restore its caret before
    // keyboard input or either paste fallback runs.
    #[cfg(windows)]
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;

    crate::log::line(format!(
        "nixi_dictate_insert: attempting insertion (length {})",
        processed.len()
    ));

    #[cfg(windows)]
    {
        crate::log::line("nixi_dictate_insert: checking for the VoiceOS native text injector");
        let mut insertion_errors = Vec::new();
        match insert_via_voiceos_injector(&processed).await {
            Ok(true) => {
                crate::log::line("nixi_dictate_insert: VoiceOS native text injector succeeded");
                return Ok("voiceos-injector".to_string());
            }
            Ok(false) => {
                crate::log::line("nixi_dictate_insert: VoiceOS injector is not installed; using Macha fallback");
            }
            Err(error) => {
                crate::log::line(format!(
                    "nixi_dictate_insert: VoiceOS injector failed; using Macha fallback: {error}"
                ));
                insertion_errors.push(format!("VoiceOS: {error}"));
            }
        }

        // If the app rejected simulated text input, try a focused-control paste.
        crate::log::line("nixi_dictate_insert: attempting Clipboard paste");
        match insert_via_clipboard(&processed).await {
            Ok(()) => {
                crate::log::line("nixi_dictate_insert: Clipboard paste succeeded");
                return Ok("clipboard".to_string());
            }
            Err(err) => {
                crate::log::line(format!(
                    "nixi_dictate_insert: Clipboard paste failed: {err}"
                ));
                insertion_errors.push(format!("clipboard paste: {err}"));
            }
        }

        // SendInput can report that Windows queued key events even when a target
        // control ignored them. Keep it as a last resort after the confirmed paste
        // routes so the frontend is less likely to show a false insertion success.
        crate::log::line("nixi_dictate_insert: attempting Unicode keyboard fallback");
        match insert_via_sendinput(&processed) {
            Ok(()) => {
                crate::log::line("nixi_dictate_insert: Unicode keyboard fallback was queued");
                return Ok("sendinput".to_string());
            }
            Err(error) => {
                crate::log::line(format!("nixi_dictate_insert: Unicode keyboard fallback failed: {error}"));
                insertion_errors.push(format!("keyboard input: {error}"));
            }
        }

        Err(format!("could not insert text ({})", insertion_errors.join("; ")))
    }

    #[cfg(not(windows))]
    {
        let _ = processed;
        Err("could not insert text: dictation insertion is only supported on Windows".to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_trim_and_filler_words() {
        assert_eq!(postprocess("  um hello uh world  "), "Hello world");
        assert_eq!(postprocess("er ah hmm test"), "Test");
    }

    #[test]
    fn test_spoken_punctuation() {
        assert_eq!(postprocess("hello comma world period"), "Hello, world.");
        assert_eq!(
            postprocess("is this working question mark yes exclamation mark"),
            "Is this working? Yes!"
        );
        assert_eq!(
            postprocess("line one newline line two"),
            "Line one\nline two"
        );
        assert_eq!(
            postprocess("section one new paragraph section two"),
            "Section one\n\nsection two"
        );
        assert_eq!(postprocess("bullet point first item"), "• First item");
    }

    #[test]
    fn test_self_correction() {
        assert_eq!(postprocess("I want tea wait make that coffee"), "Coffee");
        assert_eq!(
            postprocess("Send to John actually, make that Sarah"),
            "Sarah"
        );
    }

    #[test]
    fn test_multiple_spaces_collapsed() {
        assert_eq!(postprocess("hello    world   today"), "Hello world today");
    }

    #[test]
    fn phrase_replacements_match_case_insensitively_at_word_boundaries() {
        let replacements = vec![
            DictationReplacement {
                spoken: "macha".into(),
                replacement: "Macha AI".into(),
            },
            DictationReplacement {
                spoken: "project name".into(),
                replacement: "Macha".into(),
            },
        ];

        assert_eq!(
            apply_replacements("Macha, open project   name; catalog.", &replacements).unwrap(),
            "Macha AI, open Macha; catalog."
        );
    }

    #[test]
    fn snippets_replace_case_insensitive_phrases_and_preserve_inserted_text() {
        let snippets = vec![DictationSnippet {
            trigger: "my signature".into(),
            text: "Kind regards,\nMacha user.".into(),
        }];
        assert_eq!(
            apply_snippets("please add MY   SIGNATURE, thanks", &snippets).unwrap(),
            "please add Kind regards,\nMacha user., thanks"
        );
        assert_eq!(
            apply_snippets("signature please", &snippets).unwrap(),
            "signature please"
        );
        assert_eq!(
            apply_snippets("my signature my signature", &snippets).unwrap(),
            "Kind regards,\nMacha user. Kind regards,\nMacha user."
        );
    }

    #[test]
    fn snippet_text_is_protected_from_dictation_cleanup_and_replacements() {
        let snippets = vec![
            DictationSnippet {
                trigger: "my signature".into(),
                text: "kind regards, macha user.".into(),
            },
            DictationSnippet {
                trigger: "my address".into(),
                text: "42 example road".into(),
            },
        ];
        let replacements = vec![
            DictationReplacement {
                spoken: "my signature".into(),
                replacement: "replacement must not swallow the trigger".into(),
            },
            DictationReplacement {
                spoken: "macha user".into(),
                replacement: "replacement must not alter inserted snippet text".into(),
            },
            DictationReplacement {
                spoken: "cloud code".into(),
                replacement: "Claude Code".into(),
            },
        ];

        assert_eq!(
            process_dictation_text(
                "my address, my signature and cloud code",
                "verbatim",
                &replacements,
                &snippets,
            )
            .unwrap(),
            "42 example road, kind regards, macha user. and Claude Code"
        );
    }

    #[test]
    fn snippets_reject_invalid_entries() {
        assert!(apply_snippets(
            "trigger",
            &[DictationSnippet {
                trigger: " ".into(),
                text: "replacement".into(),
            }],
        )
        .is_err());
        assert!(apply_snippets(
            "trigger",
            &vec![
                DictationSnippet {
                    trigger: "trigger".into(),
                    text: "x".repeat(10_001),
                };
                1
            ]
        )
        .is_err());
    }

    #[test]
    fn snippets_cannot_expand_dictation_without_a_bound() {
        let snippets = vec![DictationSnippet {
            trigger: "x".into(),
            text: "y".repeat(10_000),
        }];
        assert!(apply_snippets(&"x ".repeat(101), &snippets).is_err());
    }

    #[test]
    fn phrase_replacements_reject_invalid_entries() {
        let replacements = vec![DictationReplacement {
            spoken: "  ".into(),
            replacement: "value".into(),
        }];
        assert!(apply_replacements("text", &replacements).is_err());
    }

    #[test]
    fn dictation_styles_are_local_and_verbatim_skips_cleanup() {
        assert_eq!(
            apply_style("  um hello period  ", "standard").unwrap(),
            "Hello."
        );
        assert_eq!(
            apply_style("  um hello period  ", "verbatim").unwrap(),
            "um hello period"
        );
        assert!(apply_style("text", "unknown").is_err());
    }
}
