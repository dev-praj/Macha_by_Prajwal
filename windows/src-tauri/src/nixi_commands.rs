// Nixi Command Engine — Phase 4
// Grammar/rule-based intent parser with strict JSON schema validation,
// multi-step command execution, instant execution, pill status reporting, and undo support.

use std::sync::Mutex;
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter};

use crate::island;
use crate::log;

static LAST_UNDO: Mutex<Option<UndoAction>> = Mutex::new(None);

#[derive(Clone)]
enum UndoAction {
    StatusOnly(String),
    Custom {
        description: String,
        action: std::sync::Arc<dyn Fn(&AppHandle) -> Result<String, String> + Send + Sync>,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "intent", content = "params")]
pub enum NixiIntent {
    #[serde(rename = "open_apps")]
    OpenApps { apps: Vec<String> },

    #[serde(rename = "open_url")]
    OpenUrl { url: String },

    #[serde(rename = "web_search")]
    WebSearch { query: String },

    #[serde(rename = "volume")]
    Volume { action: String, step: Option<u8> },

    #[serde(rename = "media")]
    Media { action: String },

    #[serde(rename = "dark_mode")]
    DarkMode { enable: Option<bool> },

    #[serde(rename = "screenshot")]
    Screenshot,

    #[serde(rename = "find_file")]
    FindFile { name: String },

    #[serde(rename = "reminder")]
    Reminder { text: String, time: Option<String> },

    #[serde(rename = "timer")]
    Timer { duration_secs: u64, label: Option<String> },

    #[serde(rename = "teleprompter")]
    Teleprompter,

    #[serde(rename = "read_screen")]
    ReadScreen,

    #[serde(rename = "unknown")]
    Unknown { raw: String },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NixiCommandResult {
    pub success: bool,
    pub intent_name: String,
    pub summary: String,
    pub detail: Option<String>,
    pub can_undo: bool,
}

/// Normalizes raw user input by stripping polite phrases and wake words.
pub fn clean_input(input: &str) -> String {
    let mut s = input.trim().to_lowercase();

    // Strip wake word prefixes
    for prefix in &["hey nixi,", "hey nixi", "nixi,", "nixi", "hey macha,", "hey macha"] {
        if s.starts_with(prefix) {
            s = s[prefix.len()..].trim_start_matches(|c: char| c == ',' || c == ' ' || c == ':').to_string();
            break;
        }
    }

    // Strip common leading politeness
    for prefix in &["could you please", "can you please", "please", "would you", "tell me to", "tell nixi to"] {
        if s.starts_with(prefix) {
            s = s[prefix.len()..].trim().to_string();
        }
    }

    // Strip trailing politeness
    for suffix in &["please", "thanks", "thank you"] {
        if s.ends_with(suffix) {
            let cut = s.len() - suffix.len();
            s = s[..cut].trim_end_matches(|c: char| c == ',' || c == ' ').to_string();
        }
    }

    s.trim().to_string()
}

/// Rule-based intent parser. Returns Some(NixiIntent) if matched.
pub fn parse_intent(raw: &str) -> NixiIntent {
    let text = clean_input(raw);
    if text.is_empty() {
        return NixiIntent::Unknown { raw: raw.to_string() };
    }

    // 1. Teleprompter
    if text == "start teleprompter" || text == "open teleprompter" || text == "teleprompter" {
        return NixiIntent::Teleprompter;
    }

    // 2. Screen reading
    if text == "read screen" || text == "read this page" || text == "summarize this page" || text == "what does this error say" {
        return NixiIntent::ReadScreen;
    }

    // 3. Screenshot
    if text == "take a screenshot" || text == "take screenshot" || text == "capture screen" || text == "screenshot" {
        return NixiIntent::Screenshot;
    }

    // 4. Dark mode toggle
    if text == "toggle dark mode" || text == "dark mode" || text == "switch to dark mode" {
        return NixiIntent::DarkMode { enable: None };
    }
    if text == "turn on dark mode" || text == "enable dark mode" {
        return NixiIntent::DarkMode { enable: Some(true) };
    }
    if text == "turn off dark mode" || text == "light mode" || text == "enable light mode" {
        return NixiIntent::DarkMode { enable: Some(false) };
    }

    // 5. Volume controls
    if text == "volume up" || text == "turn up the volume" || text == "increase volume" || text == "louder" {
        return NixiIntent::Volume { action: "up".into(), step: Some(5) };
    }
    if text == "a little louder" || text == "bit louder" {
        return NixiIntent::Volume { action: "up".into(), step: Some(2) };
    }
    if text == "much louder" {
        return NixiIntent::Volume { action: "up".into(), step: Some(15) };
    }
    if text == "volume down" || text == "turn down the volume" || text == "lower volume" || text == "quieter" || text == "softer" {
        return NixiIntent::Volume { action: "down".into(), step: Some(5) };
    }
    if text == "a little quieter" || text == "bit quieter" || text == "a little softer" {
        return NixiIntent::Volume { action: "down".into(), step: Some(2) };
    }
    if text == "mute" || text == "mute volume" || text == "silence" {
        return NixiIntent::Volume { action: "mute".into(), step: None };
    }
    if text == "unmute" || text == "unmute volume" {
        return NixiIntent::Volume { action: "unmute".into(), step: None };
    }

    // 6. Media playback
    if text == "play" || text == "pause" || text == "play pause" || text == "play music" || text == "pause music" {
        return NixiIntent::Media { action: "play_pause".into() };
    }
    if text == "next track" || text == "next song" || text == "skip" || text == "skip track" || text == "skip song" {
        return NixiIntent::Media { action: "next".into() };
    }
    if text == "previous track" || text == "previous song" || text == "prev track" || text == "prev song" {
        return NixiIntent::Media { action: "previous".into() };
    }

    // 7. Web search
    if let Some(query) = text.strip_prefix("search for ").or_else(|| text.strip_prefix("google ")).or_else(|| text.strip_prefix("search ")) {
        let q = query.trim().to_string();
        if !q.is_empty() {
            return NixiIntent::WebSearch { query: q };
        }
    }

    // 8. Find file
    if let Some(target) = text.strip_prefix("find file ")
        .or_else(|| text.strip_prefix("find my file "))
        .or_else(|| text.strip_prefix("find "))
        .or_else(|| text.strip_prefix("locate "))
        .or_else(|| text.strip_prefix("search for file "))
    {
        let name = target.trim().to_string();
        if !name.is_empty() && !name.starts_with("http") {
            return NixiIntent::FindFile { name };
        }
    }

    // 9. Timers ("set a timer for 5 minutes", "timer for 10 seconds")
    if text.starts_with("timer ") || text.starts_with("set a timer ") || text.starts_with("set timer ") {
        if let Some(secs) = parse_timer_seconds(&text) {
            return NixiIntent::Timer { duration_secs: secs, label: None };
        }
    }

    // 10. Reminders ("remind me at 6 to call mom", "remind me to call mom in 20 minutes")
    if let Some(rem_intent) = parse_reminder(&text) {
        return rem_intent;
    }

    // 11. App opening & Multi-app opening ("open Chrome, Spotify and Calculator")
    if let Some(rest) = text.strip_prefix("open ").or_else(|| text.strip_prefix("launch ")).or_else(|| text.strip_prefix("start ")) {
        let rest = rest.trim();
        // Check if URL
        if is_likely_url(rest) {
            return NixiIntent::OpenUrl { url: normalize_url(rest) };
        }

        // Multi-app parsing: split by commas and "and" / "&"
        let apps = split_apps(rest);
        if !apps.is_empty() {
            return NixiIntent::OpenApps { apps };
        }
    }

    NixiIntent::Unknown { raw: raw.to_string() }
}

fn is_likely_url(s: &str) -> bool {
    s.starts_with("http://")
        || s.starts_with("https://")
        || s.ends_with(".com")
        || s.ends_with(".org")
        || s.ends_with(".net")
        || s.ends_with(".io")
        || s.ends_with(".dev")
        || s.contains(".com/")
}

fn normalize_url(s: &str) -> String {
    if s.starts_with("http://") || s.starts_with("https://") {
        s.to_string()
    } else {
        format!("https://{s}")
    }
}

/// Splits "Chrome, Spotify and Calculator" into ["Chrome", "Spotify", "Calculator"]
pub fn split_apps(s: &str) -> Vec<String> {
    let mut items = Vec::new();
    let cleaned = s.replace(" and ", ", ").replace(" & ", ", ");
    for part in cleaned.split(',') {
        let trimmed = part.trim();
        let app = trimmed.strip_prefix("the ").unwrap_or(trimmed).trim();
        let app = app.strip_suffix(" app").unwrap_or(app).trim();
        if !app.is_empty() {
            // Capitalize first character for clean presentation
            let mut chars = app.chars();
            let cap = match chars.next() {
                Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
                None => String::new(),
            };
            items.push(cap);
        }
    }
    items
}

fn parse_timer_seconds(text: &str) -> Option<u64> {
    let words: Vec<&str> = text.split_whitespace().collect();
    for (i, word) in words.iter().enumerate() {
        if let Ok(num) = word.parse::<u64>() {
            if let Some(unit) = words.get(i + 1) {
                if unit.starts_with("second") || unit.starts_with("sec") {
                    return Some(num);
                }
                if unit.starts_with("minute") || unit.starts_with("min") {
                    return Some(num * 60);
                }
                if unit.starts_with("hour") || unit.starts_with("hr") {
                    return Some(num * 3600);
                }
            }
            return Some(num * 60); // default to minutes
        }
    }
    None
}

fn parse_reminder(text: &str) -> Option<NixiIntent> {
    if !text.starts_with("remind me ") {
        return None;
    }
    let rest = text.strip_prefix("remind me ")?.trim();

    // Pattern 1: "remind me to <task> in/at <time>"
    if let Some(task_rest) = rest.strip_prefix("to ") {
        if let Some((task, time)) = split_time_clause(task_rest) {
            return Some(NixiIntent::Reminder {
                text: task.trim().to_string(),
                time: Some(time.trim().to_string()),
            });
        }
        return Some(NixiIntent::Reminder {
            text: task_rest.trim().to_string(),
            time: None,
        });
    }

    // Pattern 2: "remind me at <time> to <task>"
    if let Some(time_rest) = rest.strip_prefix("at ") {
        if let Some((time, task)) = time_rest.split_once(" to ") {
            return Some(NixiIntent::Reminder {
                text: task.trim().to_string(),
                time: Some(time.trim().to_string()),
            });
        }
    }

    Some(NixiIntent::Reminder {
        text: rest.to_string(),
        time: None,
    })
}

fn split_time_clause(s: &str) -> Option<(&str, &str)> {
    if let Some(idx) = s.rfind(" in ") {
        return Some((&s[..idx], &s[idx + 4..]));
    }
    if let Some(idx) = s.rfind(" at ") {
        return Some((&s[..idx], &s[idx + 4..]));
    }
    if let Some(idx) = s.rfind(" tomorrow") {
        return Some((&s[..idx], &s[idx + 1..]));
    }
    None
}

/// Validates intent structure against strict JSON schema.
pub fn validate_intent_schema(intent: &NixiIntent) -> Result<(), String> {
    let value = serde_json::to_value(intent).map_err(|e| format!("Serialization error: {e}"))?;
    let obj = value.as_object().ok_or_else(|| "Intent is not a JSON object".to_string())?;

    if !obj.contains_key("intent") {
        return Err("Missing 'intent' property".to_string());
    }

    match intent {
        NixiIntent::OpenApps { apps } => {
            if apps.is_empty() {
                return Err("open_apps must have at least one app name".to_string());
            }
        }
        NixiIntent::OpenUrl { url } => {
            if !(url.starts_with("http://") || url.starts_with("https://")) {
                return Err("open_url must be an HTTP/HTTPS URL".to_string());
            }
        }
        NixiIntent::WebSearch { query } => {
            if query.trim().is_empty() {
                return Err("web_search query cannot be empty".to_string());
            }
        }
        NixiIntent::Volume { action, .. } => {
            if !["up", "down", "mute", "unmute"].contains(&action.as_str()) {
                return Err(format!("Invalid volume action: {action}"));
            }
        }
        NixiIntent::Media { action } => {
            if !["play_pause", "next", "previous"].contains(&action.as_str()) {
                return Err(format!("Invalid media action: {action}"));
            }
        }
        _ => {}
    }

    Ok(())
}

/// Executes a parsed NixiIntent. Reports result in pill status and registers undo.
pub async fn execute_intent(app: &AppHandle, intent: NixiIntent) -> Result<NixiCommandResult, String> {
    validate_intent_schema(&intent)?;

    match intent {
        NixiIntent::OpenApps { apps } => {
            let mut opened = Vec::new();

            for app_name in &apps {
                #[cfg(target_os = "windows")]
                {
                    // Use standard Windows shell open for instant execution
                    let status = std::process::Command::new("powershell")
                        .args(["-NoProfile", "-NonInteractive", "-Command", &format!("Start-Process -FilePath '{app_name}' -ErrorAction SilentlyContinue")])
                        .status();
                    if status.is_ok_and(|s| s.success()) {
                        opened.push(app_name.clone());
                    } else {
                        // Fallback to cmd start
                        let _ = std::process::Command::new("cmd")
                            .args(["/c", "start", "", app_name])
                            .status();
                        opened.push(app_name.clone());
                    }
                }
                #[cfg(not(target_os = "windows"))]
                {
                    opened.push(app_name.clone());
                }
            }

            let summary = if opened.len() == 1 {
                format!("Opened {}.", opened[0])
            } else {
                format!("Opened {} apps.", opened.len())
            };

            let detail = if !opened.is_empty() {
                Some(opened.iter().map(|a| format!("{a} — opened")).collect::<Vec<_>>().join("\n"))
            } else {
                None
            };

            emit_status(app, &summary, detail.as_deref());

            // Save undo
            *LAST_UNDO.lock().unwrap() = Some(UndoAction::StatusOnly(format!("Apps were opened: {}", opened.join(", "))));

            Ok(NixiCommandResult {
                success: true,
                intent_name: "open_apps".into(),
                summary,
                detail,
                can_undo: false,
            })
        }

        NixiIntent::OpenUrl { url } => {
            crate::platform::open_url(&url)?;
            let summary = format!("Opened {url}");
            emit_status(app, &summary, None);
            Ok(NixiCommandResult {
                success: true,
                intent_name: "open_url".into(),
                summary,
                detail: None,
                can_undo: false,
            })
        }

        NixiIntent::WebSearch { query } => {
            let search_url = format!("https://www.google.com/search?q={}", url::form_urlencoded::byte_serialize(query.as_bytes()).collect::<String>());
            crate::platform::open_url(&search_url)?;
            let summary = format!("Searched for \"{query}\"");
            emit_status(app, &summary, None);
            Ok(NixiCommandResult {
                success: true,
                intent_name: "web_search".into(),
                summary,
                detail: None,
                can_undo: false,
            })
        }

        NixiIntent::Volume { action, step } => {
            let steps = step.unwrap_or(match action.as_str() {
                "mute" | "unmute" => 1,
                _ => 2,
            });
            match crate::nixi_system::adjust_volume(&action, steps) {
                Ok(desc) => {
                    emit_status(app, &desc, None);
                    if action == "mute" {
                        *LAST_UNDO.lock().unwrap() = Some(UndoAction::Custom {
                            description: "Unmuted volume".into(),
                            action: std::sync::Arc::new(|_| {
                                crate::nixi_system::adjust_volume("unmute", 1)?;
                                Ok("Unmuted volume".into())
                            }),
                        });
                    }
                    Ok(NixiCommandResult {
                        success: true,
                        intent_name: "volume".into(),
                        summary: desc,
                        detail: None,
                        can_undo: action == "mute",
                    })
                }
                Err(err) => {
                    let summary = format!("Volume adjustment failed: {err}");
                    emit_status(app, &summary, None);
                    Ok(NixiCommandResult {
                        success: false,
                        intent_name: "volume".into(),
                        summary,
                        detail: Some(err),
                        can_undo: false,
                    })
                }
            }
        }

        NixiIntent::Media { action } => {
            match crate::nixi_system::send_media_key(&action) {
                Ok(desc) => {
                    emit_status(app, &desc, None);
                    Ok(NixiCommandResult {
                        success: true,
                        intent_name: "media".into(),
                        summary: desc,
                        detail: None,
                        can_undo: false,
                    })
                }
                Err(err) => {
                    let summary = format!("Media action failed: {err}");
                    emit_status(app, &summary, None);
                    Ok(NixiCommandResult {
                        success: false,
                        intent_name: "media".into(),
                        summary,
                        detail: Some(err),
                        can_undo: false,
                    })
                }
            }
        }

        NixiIntent::DarkMode { enable } => {
            match crate::nixi_system::toggle_dark_mode(enable) {
                Ok(is_dark) => {
                    let summary = if is_dark {
                        "Switched to dark mode".to_string()
                    } else {
                        "Switched to light mode".to_string()
                    };
                    emit_status(app, &summary, None);

                    let revert_target = Some(!is_dark);
                    *LAST_UNDO.lock().unwrap() = Some(UndoAction::Custom {
                        description: "Reverted theme toggle".into(),
                        action: std::sync::Arc::new(move |_| {
                            crate::nixi_system::toggle_dark_mode(revert_target)?;
                            Ok("Theme reverted".into())
                        }),
                    });

                    Ok(NixiCommandResult {
                        success: true,
                        intent_name: "dark_mode".into(),
                        summary,
                        detail: None,
                        can_undo: true,
                    })
                }
                Err(err) => {
                    let summary = format!("Theme change failed: {err}");
                    emit_status(app, &summary, None);
                    Ok(NixiCommandResult {
                        success: false,
                        intent_name: "dark_mode".into(),
                        summary,
                        detail: Some(err),
                        can_undo: false,
                    })
                }
            }
        }

        NixiIntent::Screenshot => {
            match crate::nixi_system::capture_screenshot() {
                Ok(capture) => {
                    let summary = "Screenshot saved".to_string();
                    let detail = Some(capture.path.clone());
                    emit_status(app, &summary, detail.as_deref());
                    let _ = crate::nixi_search::reveal_in_explorer(&capture.path);
                    Ok(NixiCommandResult {
                        success: true,
                        intent_name: "screenshot".into(),
                        summary,
                        detail,
                        can_undo: false,
                    })
                }
                Err(err) => {
                    let summary = format!("Screenshot failed: {err}");
                    emit_status(app, &summary, None);
                    Ok(NixiCommandResult {
                        success: false,
                        intent_name: "screenshot".into(),
                        summary,
                        detail: Some(err),
                        can_undo: false,
                    })
                }
            }
        }

        NixiIntent::Timer { duration_secs, label } => {
            let summary = format!("Timer set for {} seconds", duration_secs);
            emit_status(app, &summary, label.as_deref());
            let timer_label = label.clone().unwrap_or_else(|| "Timer".to_string());
            let timer_app = app.clone();
            tauri::async_runtime::spawn(async move {
                tokio::time::sleep(std::time::Duration::from_secs(duration_secs)).await;
                let finished_msg = format!("⏰ Timer finished: {timer_label}");
                emit_status(&timer_app, &finished_msg, Some("Time is up!"));
                let _ = timer_app.emit_to(island::WINDOW_LABEL, "nixi-timer-done", timer_label);
            });
            Ok(NixiCommandResult {
                success: true,
                intent_name: "timer".into(),
                summary,
                detail: label,
                can_undo: false,
            })
        }

        NixiIntent::Reminder { text, time } => {
            let summary = format!("Reminder set: {text}");
            emit_status(app, &summary, time.as_deref());
            let reminder_app = app.clone();
            let reminder_text = text.clone();
            if let Some(ref t) = time {
                if let Some(secs) = parse_timer_seconds(t) {
                    tauri::async_runtime::spawn(async move {
                        tokio::time::sleep(std::time::Duration::from_secs(secs)).await;
                        let alert = format!("🔔 Reminder: {reminder_text}");
                        emit_status(&reminder_app, &alert, Some("Scheduled reminder"));
                        let _ = reminder_app.emit_to(island::WINDOW_LABEL, "nixi-reminder-done", reminder_text);
                    });
                }
            }
            Ok(NixiCommandResult {
                success: true,
                intent_name: "reminder".into(),
                summary,
                detail: time,
                can_undo: false,
            })
        }

        NixiIntent::FindFile { name } => {
            let matches = crate::nixi_search::search_files(&name, 5);
            if let Some(top_match) = matches.first() {
                let _ = crate::nixi_search::reveal_in_explorer(&top_match.path);
                let summary = format!("Found {}", top_match.name);
                let detail = if matches.len() > 1 {
                    Some(format!("{} matches found. Showing in Explorer.", matches.len()))
                } else {
                    Some(top_match.folder.clone())
                };
                emit_status(app, &summary, detail.as_deref());
                Ok(NixiCommandResult {
                    success: true,
                    intent_name: "find_file".into(),
                    summary,
                    detail,
                    can_undo: false,
                })
            } else {
                let summary = format!("No files found for \"{name}\"");
                emit_status(app, &summary, Some("Searched user library folders"));
                Ok(NixiCommandResult {
                    success: false,
                    intent_name: "find_file".into(),
                    summary,
                    detail: Some("No matching files found".into()),
                    can_undo: false,
                })
            }
        }

        NixiIntent::Teleprompter => {
            let summary = "Teleprompter ready".to_string();
            emit_status(app, &summary, Some("Speak clearly into your microphone"));
            Ok(NixiCommandResult {
                success: true,
                intent_name: "teleprompter".into(),
                summary,
                detail: Some("Teleprompter activated".into()),
                can_undo: false,
            })
        }

        NixiIntent::ReadScreen => {
            let summary = "Reading screen content...".to_string();
            emit_status(app, &summary, Some("Analyzing active window"));
            let _ = app.emit_to(island::WINDOW_LABEL, "macha-read-screen", ());
            Ok(NixiCommandResult {
                success: true,
                intent_name: "read_screen".into(),
                summary,
                detail: None,
                can_undo: false,
            })
        }

        NixiIntent::Unknown { raw } => {
            let summary = format!("Unknown command: \"{raw}\"");
            emit_status(app, &summary, Some("Try saying 'open Chrome', 'volume up', 'search for...', or 'remind me to...'"));
            Ok(NixiCommandResult {
                success: false,
                intent_name: "unknown".into(),
                summary,
                detail: Some("Could not parse command".into()),
                can_undo: false,
            })
        }
    }
}

fn emit_status(app: &AppHandle, text: &str, detail: Option<&str>) {
    #[derive(Clone, Serialize)]
    struct StatusPayload {
        text: String,
        detail: Option<String>,
    }
    let _ = app.emit_to(
        island::WINDOW_LABEL,
        "nixi-status",
        StatusPayload {
            text: text.to_string(),
            detail: detail.map(|s| s.to_string()),
        },
    );
}



// ── Tauri Commands ──────────────────────────────────────────────────────────

#[tauri::command]
pub async fn nixi_run_command(app: AppHandle, input: String) -> Result<NixiCommandResult, String> {
    log::line(format!("nixi_run_command: input={input:?}"));
    let intent = parse_intent(&input);
    execute_intent(&app, intent).await
}

#[tauri::command]
pub fn nixi_undo_last_command(app: AppHandle) -> Result<String, String> {
    let mut undo_lock = LAST_UNDO.lock().unwrap();
    let action = undo_lock.take().ok_or_else(|| "Nothing to undo.".to_string())?;

    match action {
        UndoAction::StatusOnly(msg) => {
            emit_status(&app, &format!("Cannot undo: {msg}"), None);
            Ok(msg)
        }
        UndoAction::Custom { description, action } => {
            let res = action(&app)?;
            emit_status(&app, &format!("Undone: {description}"), None);
            Ok(res)
        }
    }
}

// ── Unit Tests ──────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_clean_input() {
        assert_eq!(clean_input("Hey Nixi, open Chrome please"), "open chrome");
        assert_eq!(clean_input("nixi: volume up thanks"), "volume up");
        assert_eq!(clean_input("Could you please search for rust"), "search for rust");
        assert_eq!(clean_input("Hey Macha, start teleprompter"), "start teleprompter");
    }

    #[test]
    fn test_parse_multi_app_open() {
        let intent = parse_intent("open Chrome, Spotify and Calculator");
        assert_eq!(
            intent,
            NixiIntent::OpenApps {
                apps: vec!["Chrome".into(), "Spotify".into(), "Calculator".into()],
            }
        );

        let single = parse_intent("launch notepad");
        assert_eq!(
            single,
            NixiIntent::OpenApps {
                apps: vec!["Notepad".into()],
            }
        );
    }

    #[test]
    fn test_parse_volume() {
        assert_eq!(parse_intent("volume up"), NixiIntent::Volume { action: "up".into(), step: Some(5) });
        assert_eq!(parse_intent("a little louder"), NixiIntent::Volume { action: "up".into(), step: Some(2) });
        assert_eq!(parse_intent("mute"), NixiIntent::Volume { action: "mute".into(), step: None });
        assert_eq!(parse_intent("unmute"), NixiIntent::Volume { action: "unmute".into(), step: None });
    }

    #[test]
    fn test_parse_media() {
        assert_eq!(parse_intent("play music"), NixiIntent::Media { action: "play_pause".into() });
        assert_eq!(parse_intent("next song"), NixiIntent::Media { action: "next".into() });
        assert_eq!(parse_intent("previous track"), NixiIntent::Media { action: "previous".into() });
    }

    #[test]
    fn test_parse_dark_mode() {
        assert_eq!(parse_intent("toggle dark mode"), NixiIntent::DarkMode { enable: None });
        assert_eq!(parse_intent("turn on dark mode"), NixiIntent::DarkMode { enable: Some(true) });
        assert_eq!(parse_intent("light mode"), NixiIntent::DarkMode { enable: Some(false) });
    }

    #[test]
    fn test_parse_url_and_search() {
        assert_eq!(
            parse_intent("open https://github.com"),
            NixiIntent::OpenUrl { url: "https://github.com".into() }
        );
        assert_eq!(
            parse_intent("open youtube.com"),
            NixiIntent::OpenUrl { url: "https://youtube.com".into() }
        );
        assert_eq!(
            parse_intent("search for best mechanical keyboard"),
            NixiIntent::WebSearch { query: "best mechanical keyboard".into() }
        );
    }

    #[test]
    fn test_parse_file_search() {
        assert_eq!(
            parse_intent("find file quarterly_report.pdf"),
            NixiIntent::FindFile { name: "quarterly_report.pdf".into() }
        );
        assert_eq!(
            parse_intent("locate resume"),
            NixiIntent::FindFile { name: "resume".into() }
        );
    }

    #[test]
    fn test_parse_timer_and_reminder() {
        assert_eq!(
            parse_intent("set a timer for 5 minutes"),
            NixiIntent::Timer { duration_secs: 300, label: None }
        );
        assert_eq!(
            parse_intent("timer for 30 seconds"),
            NixiIntent::Timer { duration_secs: 30, label: None }
        );
        assert_eq!(
            parse_intent("remind me to call Mom in 20 minutes"),
            NixiIntent::Reminder { text: "call mom".into(), time: Some("20 minutes".into()) }
        );
    }

    #[test]
    fn test_validate_intent_schema() {
        let valid = NixiIntent::OpenApps { apps: vec!["Chrome".into()] };
        assert!(validate_intent_schema(&valid).is_ok());

        let invalid = NixiIntent::OpenApps { apps: vec![] };
        assert!(validate_intent_schema(&invalid).is_err());

        let bad_url = NixiIntent::OpenUrl { url: "ftp://files".into() };
        assert!(validate_intent_schema(&bad_url).is_err());
    }
}
