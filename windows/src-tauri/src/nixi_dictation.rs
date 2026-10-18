// Nixi Dictation — text post-processing and text insertion command.

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
    if let Ok(self_correct_re) =
        Regex::new(r"(?is)^(.+)\s+(?:actually|wait),?\s+make that\s+(.+)$")
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

#[cfg(windows)]
fn insert_via_uia(text: &str) -> Result<(), String> {
    use windows::core::{Interface, BSTR};
    use windows::Win32::System::Com::{
        CoCreateInstance, CoInitializeEx, CoUninitialize, CLSCTX_INPROC_SERVER,
        COINIT_APARTMENTTHREADED,
    };
    use windows::Win32::UI::Accessibility::{
        CUIAutomation, IUIAutomation, IUIAutomationElement, IUIAutomationValuePattern,
        UIA_ValuePatternId,
    };

    unsafe {
        let coinit = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
        let initialized = coinit.is_ok();

        let run = || -> windows::core::Result<()> {
            let automation: IUIAutomation =
                CoCreateInstance(&CUIAutomation, None, CLSCTX_INPROC_SERVER)?;
            let element: IUIAutomationElement = automation.GetFocusedElement()?;
            let unk = element.GetCurrentPattern(UIA_ValuePatternId)?;
            let value_pattern: IUIAutomationValuePattern = unk.cast()?;
            let bstr = BSTR::from(text);
            value_pattern.SetValue(&bstr)?;
            Ok(())
        };

        let result = run();
        if initialized {
            CoUninitialize();
        }

        result.map_err(|e| format!("UIA error: {e}"))
    }
}

#[cfg(windows)]
fn insert_via_sendinput(text: &str) -> Result<(), String> {
    use windows::Win32::UI::Input::KeyboardAndMouse::{
        SendInput, INPUT, INPUT_0, INPUT_KEYBOARD, KEYBDINPUT, KEYEVENTF_KEYUP,
        KEYEVENTF_UNICODE, VIRTUAL_KEY,
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

    let sent = unsafe {
        SendInput(&inputs, std::mem::size_of::<INPUT>() as i32)
    };

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
    use windows::Win32::Foundation::{HANDLE, GlobalFree};
    use windows::Win32::System::DataExchange::{EmptyClipboard, SetClipboardData};
    use windows::Win32::System::Memory::{
        GlobalAlloc, GlobalLock, GlobalUnlock, GMEM_MOVEABLE,
    };

    const CF_UNICODETEXT: u32 = 13;

    let _guard = open_clipboard_with_retry()?;
    unsafe {
        EmptyClipboard().map_err(|e| format!("EmptyClipboard failed: {e}"))?;
        let utf16: Vec<u16> = text.encode_utf16().chain(std::iter::once(0)).collect();
        let bytes = utf16.len() * std::mem::size_of::<u16>();
        let hmem = GlobalAlloc(GMEM_MOVEABLE, bytes)
            .map_err(|e| format!("GlobalAlloc failed: {e}"))?;
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
        SendInput, INPUT, INPUT_0, INPUT_KEYBOARD, KEYBDINPUT, KEYBD_EVENT_FLAGS,
        KEYEVENTF_KEYUP, VK_CONTROL, VIRTUAL_KEY,
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

    let sent = unsafe {
        SendInput(&ctrl_v, std::mem::size_of::<INPUT>() as i32)
    };

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
    // a. Save current clipboard content (text only for now; binary clipboard = skip save)
    let original_text = get_clipboard_text();

    // b. Set clipboard to text
    set_clipboard_text(text)?;

    // c. Send Ctrl+V via SendInput
    send_ctrl_v()?;

    // d. Wait 120ms
    tokio::time::sleep(std::time::Duration::from_millis(120)).await;

    // e. Restore original clipboard (or clear if there was none)
    if let Some(ref original) = original_text {
        let _ = set_clipboard_text(original);
    } else {
        clear_clipboard();
    }

    Ok(())
}

#[tauri::command]
pub async fn nixi_dictate_insert(text: String) -> Result<String, String> {
    let processed = postprocess(&text);
    if processed.is_empty() {
        crate::log::line("nixi_dictate_insert: empty text after postprocessing");
        return Ok("sendinput".to_string());
    }

    crate::log::line(format!(
        "nixi_dictate_insert: attempting insertion (length {})",
        processed.len()
    ));

    #[cfg(windows)]
    {
        // 1. UIAutomation fallback
        crate::log::line("nixi_dictate_insert: attempting UIAutomation");
        let to_insert_uia = processed.clone();
        let uia_result = tokio::task::spawn_blocking(move || {
            insert_via_uia(&to_insert_uia)
        })
        .await;

        match uia_result {
            Ok(Ok(())) => {
                crate::log::line("nixi_dictate_insert: UIAutomation succeeded");
                return Ok("uia".to_string());
            }
            Ok(Err(err)) => {
                crate::log::line(format!("nixi_dictate_insert: UIAutomation failed: {err}"));
            }
            Err(join_err) => {
                crate::log::line(format!(
                    "nixi_dictate_insert: UIAutomation thread join failed: {join_err}"
                ));
            }
        }

        // 2. SendInput Unicode fallback
        crate::log::line("nixi_dictate_insert: attempting SendInput Unicode");
        match insert_via_sendinput(&processed) {
            Ok(()) => {
                crate::log::line("nixi_dictate_insert: SendInput succeeded");
                return Ok("sendinput".to_string());
            }
            Err(err) => {
                crate::log::line(format!("nixi_dictate_insert: SendInput failed: {err}"));
            }
        }

        // 3. Clipboard paste fallback
        crate::log::line("nixi_dictate_insert: attempting Clipboard paste fallback");
        match insert_via_clipboard(&processed).await {
            Ok(()) => {
                crate::log::line("nixi_dictate_insert: Clipboard paste succeeded");
                return Ok("clipboard".to_string());
            }
            Err(err) => {
                crate::log::line(format!(
                    "nixi_dictate_insert: Clipboard paste fallback failed: {err}"
                ));
                return Err(format!("could not insert text: {err}"));
            }
        }
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
        assert_eq!(postprocess("line one newline line two"), "Line one\nline two");
        assert_eq!(
            postprocess("section one new paragraph section two"),
            "Section one\n\nsection two"
        );
        assert_eq!(postprocess("bullet point first item"), "• First item");
    }

    #[test]
    fn test_self_correction() {
        assert_eq!(
            postprocess("I want tea wait make that coffee"),
            "Coffee"
        );
        assert_eq!(
            postprocess("Send to John actually, make that Sarah"),
            "Sarah"
        );
    }

    #[test]
    fn test_multiple_spaces_collapsed() {
        assert_eq!(postprocess("hello    world   today"), "Hello world today");
    }
}
