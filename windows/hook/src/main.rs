//! macha-hook — the relay Claude Code runs on every hook event.
//!
//! Reads the hook JSON on stdin, adds a little terminal context, and hands it to
//! Macha over the named pipe `\\.\pipe\macha-<sid>`.
//!
//! Hard rule: never block Claude Code.
//! * If the pipe does not exist — Macha is closed — we exit 0 immediately with
//!   nothing on stdout, and the session carries on untouched.
//! * Every step runs under a deadline enforced by the main thread, so a pipe that
//!   accepts the connection and then stops reading cannot wedge the session
//!   either: we abandon the worker and exit.
//! * Only `PermissionRequest` waits for an answer, because approving from the
//!   island is the whole point. No answer means empty stdout, and Claude Code
//!   asks in the terminal exactly as if Macha were not installed.
//!
//! Usage: `macha-hook <EventName>` (the name is also read from the JSON).

use std::io::{Read, Write};
use std::sync::mpsc;
use std::time::Duration;

/// Budget for getting a pipe connection. Beyond this Claude Code wins, always.
const CONNECT_TIMEOUT: Duration = Duration::from_millis(300);
/// Whole-run budget for an event nobody waits on: connect and write, no more.
const FIRE_AND_FORGET_BUDGET: Duration = Duration::from_secs(2);
/// How long a permission prompt may stay on screen before the terminal takes over.
const DECISION_BUDGET: Duration = Duration::from_secs(110);

/// Fields that are pointless to forward and can be enormous (a whole file read,
/// a full command output). The island never shows them.
const DROPPED_FIELDS: &[&str] = &["tool_response", "transcript_path"];
/// Longest string forwarded for any single field; the island truncates to far
/// less than this anyway.
const MAX_FIELD_LEN: usize = 2_000;
const MAX_DIFF_INPUT_LEN: usize = 256 * 1024;

#[cfg(windows)]
mod win;
#[cfg(windows)]
use win::connect;

#[cfg(target_os = "linux")]
mod unix;
#[cfg(target_os = "linux")]
use unix::connect;

fn main() {
    let Some((payload, event)) = read_event() else { std::process::exit(0) };

    let waits_for_answer = event == "PermissionRequest";
    let budget = if waits_for_answer { DECISION_BUDGET } else { FIRE_AND_FORGET_BUDGET };

    // The worker owns every blocking call. If it overruns the budget we simply
    // stop listening and exit: the process dying takes the pipe handle with it.
    // (No catch_unwind here — the release profile is panic = "abort", so it would
    // be dead code. `talk` is written to have nothing to panic on instead.)
    let (tx, rx) = mpsc::channel::<Option<String>>();
    std::thread::spawn(move || {
        let _ = tx.send(talk(&payload, waits_for_answer));
    });

    if let Ok(Some(decision)) = rx.recv_timeout(budget) {
        if let Some(json) = decision_json(&decision) {
            let mut out = std::io::stdout();
            let _ = writeln!(out, "{json}");
            let _ = out.flush();
        }
    }
    // Nothing printed: Claude Code asks in the terminal, as if we were not here.
    std::process::exit(0);
}

/// The documented PermissionRequest output. Anything we do not recognise prints
/// nothing at all rather than guessing — silence is the safe answer.
/// See https://code.claude.com/docs/en/hooks
fn decision_json(decision: &str) -> Option<String> {
    let behavior = match decision.trim() {
        // "always" still answers a plain allow; remembering it is the island's
        // business, not Claude Code's.
        "allow" | "always" => r#"{"behavior":"allow"}"#.to_string(),
        "deny" => r#"{"behavior":"deny","message":"Denied from Macha"}"#.to_string(),
        _ => return None,
    };
    Some(format!(
        r#"{{"hookSpecificOutput":{{"hookEventName":"PermissionRequest","decision":{behavior}}}}}"#
    ))
}

/// Reads stdin and returns the payload to forward plus the event name.
fn read_event() -> Option<(String, String)> {
    let mut raw = Vec::new();
    if std::io::stdin().read_to_end(&mut raw).is_err() || raw.is_empty() {
        return None;
    }
    // Some shells hand us a UTF-8 BOM; serde_json would choke on it.
    if raw.starts_with(&[0xEF, 0xBB, 0xBF]) {
        raw.drain(..3);
    }

    let mut payload = serde_json::from_slice::<serde_json::Value>(&raw).ok()?;
    let map = payload.as_object_mut()?;

    // Parse argv: "macha-hook.exe [--agent <name>] [<EventName>]"
    // --agent tags the payload with macha_agent so the app routes to the right pill.
    // Absent or invalid names are validated and discarded by the app, not here.
    let mut agent = String::new();
    let mut arg_event = String::new();
    {
        let mut it = std::env::args().skip(1);
        while let Some(arg) = it.next() {
            if arg == "--agent" {
                agent = it.next().unwrap_or_default();
            } else if arg_event.is_empty() {
                arg_event = arg;
            }
        }
    }
    // Which agent this hook was installed for. Absent means Claude Code,
    // so existing hook commands keep working unchanged.
    if !agent.is_empty() {
        map.insert("macha_agent".into(), serde_json::Value::String(agent));
    }
    let event = map
        .get("hook_event_name")
        .and_then(|v| v.as_str())
        .map(str::to_string)
        .filter(|s| !s.is_empty())
        .unwrap_or(arg_event);
    map.insert("hook_event_name".into(), serde_json::Value::String(event.clone()));

    for field in DROPPED_FIELDS {
        map.remove(*field);
    }

    let cwd_missing = map
        .get("cwd")
        .and_then(|v| v.as_str())
        .map(str::is_empty)
        .unwrap_or(true);
    if cwd_missing {
        if let Ok(cwd) = std::env::current_dir() {
            map.insert(
                "cwd".into(),
                serde_json::Value::String(cwd.to_string_lossy().to_string()),
            );
        }
    }

    // Which terminal the session runs in. Unlike macOS, Macha here accepts
    // events from every terminal, so this is context only — never a filter.
    for (key, var) in [
        ("term_program", "TERM_PROGRAM"),
        ("wt_session", "WT_SESSION"),
        ("term_session_id", "TERM_SESSION_ID"),
        ("vscode_pid", "VSCODE_PID"),
        ("session_pid", "CLAUDE_CODE_SSE_PORT"),
    ] {
        if !map.contains_key(key) {
            let value = std::env::var(var).unwrap_or_default();
            map.insert(key.into(), serde_json::Value::String(value));
        }
    }

    truncate_event(&mut payload);

    let mut line = payload.to_string();
    line.push('\n');
    Some((line, event))
}

fn truncate_event(payload: &mut serde_json::Value) {
    let tool = payload
        .get("tool_name")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("");
    let is_file_edit = payload
        .get("hook_event_name")
        .and_then(serde_json::Value::as_str)
        == Some("PostToolUse")
        && matches!(tool, "Edit" | "MultiEdit" | "Write");

    if is_file_edit {
        let input = payload
            .as_object_mut()
            .and_then(|map| map.remove("tool_input"));
        if let Some(mut input) = input {
            let input_size = input.to_string().len();
            if input_size <= MAX_DIFF_INPUT_LEN {
                truncate_strings(payload);
                truncate_strings_with_limit(&mut input, MAX_DIFF_INPUT_LEN);
                if let Some(map) = payload.as_object_mut() {
                    map.insert("tool_input".into(), input);
                }
                return;
            }
            if let Some(map) = payload.as_object_mut() {
                map.insert(
                    "macha_diff_truncated".into(),
                    serde_json::Value::Bool(true),
                );
            }
        }
    }

    truncate_strings(payload);
}

/// Caps every string in the payload. A single Write can carry a whole file.
fn truncate_strings(value: &mut serde_json::Value) {
    truncate_strings_with_limit(value, MAX_FIELD_LEN);
}

fn truncate_strings_with_limit(value: &mut serde_json::Value, max_len: usize) {
    match value {
        serde_json::Value::String(s) => {
            if s.len() > max_len {
                // Cut on a char boundary; a lone byte index can split UTF-8.
                let mut end = max_len;
                while end > 0 && !s.is_char_boundary(end) {
                    end -= 1;
                }
                s.truncate(end);
                s.push('…');
            }
        }
        serde_json::Value::Array(items) => {
            items.iter_mut().for_each(|item| truncate_strings_with_limit(item, max_len));
        }
        serde_json::Value::Object(map) => {
            map.values_mut().for_each(|item| truncate_strings_with_limit(item, max_len));
        }
        _ => {}
    }
}

/// Connect, send, and — for a permission request — wait for the island's word.
fn talk(payload: &str, waits_for_answer: bool) -> Option<String> {
    let mut pipe = connect()?;

    if pipe.write_all(payload.as_bytes()).is_err() {
        return None;
    }
    let _ = pipe.flush();

    if !waits_for_answer {
        return None;
    }

    let mut buf = Vec::new();
    let mut chunk = [0u8; 1024];
    loop {
        match pipe.read(&mut chunk) {
            Ok(0) => break,
            Ok(n) => {
                buf.extend_from_slice(&chunk[..n]);
                if buf.contains(&b'\n') {
                    break;
                }
            }
            Err(_) => break,
        }
    }
    let answer = String::from_utf8_lossy(&buf).trim().to_string();
    (!answer.is_empty()).then_some(answer)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preserves_bounded_file_edit_content_for_diffing() {
        let mut payload = serde_json::json!({
            "hook_event_name": "PostToolUse",
            "tool_name": "Write",
            "tool_input": { "content": "x".repeat(MAX_FIELD_LEN + 1) }
        });
        truncate_event(&mut payload);
        assert_eq!(
            payload["tool_input"]["content"].as_str().unwrap().len(),
            MAX_FIELD_LEN + 1
        );
        assert_eq!(payload.get("macha_diff_truncated"), None);
    }

    #[test]
    fn drops_oversized_file_edit_content_and_marks_the_event() {
        let mut payload = serde_json::json!({
            "hook_event_name": "PostToolUse",
            "tool_name": "Write",
            "tool_input": { "content": "x".repeat(MAX_DIFF_INPUT_LEN + 1) }
        });
        truncate_event(&mut payload);
        assert_eq!(payload["macha_diff_truncated"], true);
        assert_eq!(payload.get("tool_input"), None);
    }

    #[test]
    fn continues_to_cap_unrelated_hook_fields() {
        let mut payload = serde_json::json!({
            "hook_event_name": "PostToolUse",
            "tool_name": "Bash",
            "tool_input": { "command": "x".repeat(MAX_FIELD_LEN + 1) }
        });
        truncate_event(&mut payload);
        assert_eq!(payload["tool_input"]["command"].as_str().unwrap().len(), MAX_FIELD_LEN + 3);
        assert_eq!(payload.get("macha_diff_truncated"), None);
    }

    #[test]
    fn truncates_unrelated_events_as_before() {
        let mut payload = serde_json::json!({
            "hook_event_name": "PreToolUse",
            "tool_name": "Write",
            "tool_input": { "content": "x".repeat(MAX_FIELD_LEN + 1) }
        });
        truncate_event(&mut payload);
        assert_eq!(payload["tool_input"]["content"].as_str().unwrap().len(), MAX_FIELD_LEN + 3);
    }

    #[test]
    fn does_not_mark_unrelated_tools_as_truncated_diffs() {
        let mut payload = serde_json::json!({
            "hook_event_name": "PostToolUse",
            "tool_name": "Bash",
            "tool_input": { "command": "echo hello" }
        });
        truncate_event(&mut payload);
        assert_eq!(payload.get("macha_diff_truncated"), None);
    }

    #[test]
    fn truncate_strings_still_respects_utf8_boundaries() {
        let mut payload = serde_json::json!({
            "command": "é".repeat(MAX_FIELD_LEN / 2 + 1)
        });
        truncate_event(&mut payload);
        assert!(payload["command"].as_str().unwrap().ends_with('…'));
    }

    #[test]
    fn unrelated_fields_on_file_edit_events_remain_bounded() {
        let mut payload = serde_json::json!({
            "hook_event_name": "PostToolUse",
            "tool_name": "Write",
            "message": "x".repeat(MAX_FIELD_LEN + 1),
            "tool_input": { "content": "short" }
        });
        truncate_event(&mut payload);
        assert_eq!(payload["message"].as_str().unwrap().len(), MAX_FIELD_LEN + 3);
    }
    #[test]
    fn decision_json_matches_the_documented_shape() {
        assert_eq!(
            decision_json("allow").unwrap(),
            r#"{"hookSpecificOutput":{"hookEventName":"PermissionRequest","decision":{"behavior":"allow"}}}"#
        );
        assert_eq!(
            decision_json("deny").unwrap(),
            r#"{"hookSpecificOutput":{"hookEventName":"PermissionRequest","decision":{"behavior":"deny","message":"Denied from Macha"}}}"#
        );
        // "always" is an island concept; Claude Code just gets an allow.
        assert!(decision_json("always").unwrap().contains(r#""behavior":"allow""#));
    }

    #[test]
    fn anything_unrecognised_prints_nothing() {
        assert!(decision_json("").is_none());
        assert!(decision_json("maybe").is_none());
        // The shape the app used to send must not be mistaken for a decision.
        assert!(decision_json(r#"{"permissionDecision":"allow"}"#).is_none());
    }

    #[test]
    fn long_strings_are_cut_on_a_char_boundary() {
        let mut v = serde_json::json!({ "tool_input": { "content": "é".repeat(4000) } });
        truncate_strings(&mut v);
        let s = v["tool_input"]["content"].as_str().unwrap();
        assert!(s.len() <= MAX_FIELD_LEN + 4);
        assert!(s.ends_with('…'));
    }
}
