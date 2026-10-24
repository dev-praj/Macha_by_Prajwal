use std::path::PathBuf;
use std::sync::{Mutex, PoisonError};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use serde_json::Value;
use tauri::{AppHandle, Manager, State};

const DAY_SECONDS: i64 = 86_400;
const WEEK_SECONDS: i64 = 7 * DAY_SECONDS;
const MAX_DAYS: usize = 14;

#[derive(Clone, Default, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", default)]
struct Day {
    date: i64,
    sessions: u32,
    files_changed: u32,
    lines_added: u32,
    lines_removed: u32,
    commands_run: u32,
    permissions_allowed: u32,
    permissions_denied: u32,
}

#[derive(Default, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", default)]
struct History {
    enabled: bool,
    days: Vec<Day>,
}

#[derive(Default)]
pub struct Store(Mutex<History>);

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Snapshot {
    enabled: bool,
    sessions: u32,
    files_changed: u32,
    lines_added: u32,
    lines_removed: u32,
    commands_run: u32,
    permissions_allowed: u32,
    permissions_denied: u32,
}

impl Store {
    pub fn load() -> Self {
        let history = read_history();
        Self(Mutex::new(history))
    }

    fn with_history<R>(&self, f: impl FnOnce(&mut History) -> R) -> R {
        let mut history = self.0.lock().unwrap_or_else(PoisonError::into_inner);
        f(&mut history)
    }
}

fn history_path() -> PathBuf {
    crate::platform::local_dir().join("weekly-recap.json")
}

fn read_history() -> History {
    let path = history_path();
    let Ok(bytes) = std::fs::read(&path) else {
        return History { enabled: true, days: Vec::new() };
    };
    match serde_json::from_slice::<History>(&bytes) {
        Ok(mut history) => {
            if history.days.len() > MAX_DAYS {
                history.days.sort_by_key(|day| day.date);
                history.days.drain(..history.days.len() - MAX_DAYS);
            }
            history
        }
        Err(error) => {
            crate::log::line(format!("could not read weekly recap history: {error}"));
            History { enabled: true, days: Vec::new() }
        }
    }
}

fn save(history: &History) {
    let path = history_path();
    if let Some(parent) = path.parent() {
        if let Err(error) = crate::platform::ensure_private_dir(parent) {
            crate::log::line(format!("could not create weekly recap folder: {error}"));
            return;
        }
    }
    let bytes = match serde_json::to_vec(history) {
        Ok(bytes) => bytes,
        Err(error) => {
            crate::log::line(format!("could not serialize weekly recap: {error}"));
            return;
        }
    };
    let temp = path.with_extension(format!("json.{}.tmp", std::process::id()));
    let result = std::fs::write(&temp, bytes).and_then(|()| std::fs::rename(&temp, &path));
    if let Err(error) = result {
        let _ = std::fs::remove_file(&temp);
        crate::log::line(format!("could not save weekly recap: {error}"));
    }
}

fn today() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs() as i64 / DAY_SECONDS)
        .unwrap_or(0)
}

fn day_mut(history: &mut History, date: i64) -> &mut Day {
    if !history.days.iter().any(|day| day.date == date) {
        history.days.push(Day { date, ..Day::default() });
    }
    history.days.sort_by_key(|day| day.date);
    if history.days.len() > MAX_DAYS {
        let excess = history.days.len() - MAX_DAYS;
        history.days.drain(..excess);
    }
    history.days.iter_mut().find(|day| day.date == date).expect("day was inserted")
}

fn line_diff(old: &str, new: &str) -> (u32, u32) {
    const MAX_LINES: usize = 4_000;
    const MAX_CELLS: usize = 1_000_000;
    let old_lines: Vec<&str> = old.lines().collect();
    let new_lines: Vec<&str> = new.lines().collect();
    let (m, n) = (old_lines.len(), new_lines.len());
    if m > MAX_LINES || n > MAX_LINES || m.saturating_mul(n) > MAX_CELLS {
        let old_unique: std::collections::HashSet<&str> = old_lines.iter().copied().collect();
        let new_unique: std::collections::HashSet<&str> = new_lines.iter().copied().collect();
        let removed = old_lines.iter().filter(|line| !new_unique.contains(**line)).count();
        let added = new_lines.iter().filter(|line| !old_unique.contains(**line)).count();
        return (added.min(u32::MAX as usize) as u32, removed.min(u32::MAX as usize) as u32);
    }
    let width = n + 1;
    let mut lcs = vec![0u32; (m + 1) * width];
    for i in 1..=m {
        for j in 1..=n {
            let index = i * width + j;
            lcs[index] = if old_lines[i - 1] == new_lines[j - 1] {
                lcs[(i - 1) * width + j - 1] + 1
            } else {
                lcs[(i - 1) * width + j].max(lcs[i * width + j - 1])
            };
        }
    }
    let common = lcs[m * width + n] as usize;
    (
        n.saturating_sub(common).min(u32::MAX as usize) as u32,
        m.saturating_sub(common).min(u32::MAX as usize) as u32,
    )
}

fn file_change(tool: &str, input: &Value) -> Option<(u32, u32)> {
    match tool {
        "Write" => {
            let content = input.get("content")?.as_str()?;
            Some((content.lines().count().min(u32::MAX as usize) as u32, 0))
        }
        "Edit" => {
            let old = input.get("old_string")?.as_str()?;
            let new = input.get("new_string")?.as_str()?;
            Some(line_diff(old, new))
        }
        "MultiEdit" => {
            let edits = input.get("edits")?.as_array()?;
            let mut total = (0u32, 0u32);
            for edit in edits {
                let old = edit.get("old_string")?.as_str()?;
                let new = edit.get("new_string")?.as_str()?;
                let change = line_diff(old, new);
                total.0 = total.0.saturating_add(change.0);
                total.1 = total.1.saturating_add(change.1);
            }
            Some(total)
        }
        _ => None,
    }
}

pub fn observe(app: &AppHandle, payload: &Value) {
    let Some(store) = app.try_state::<Store>() else {
        return;
    };
    let event = payload.get("hook_event_name").and_then(Value::as_str).unwrap_or_default();
    let tool = payload.get("tool_name").and_then(Value::as_str).unwrap_or_default();
    store.with_history(|history| {
        if !history.enabled {
            return;
        }
        let day = day_mut(history, today());
        match event {
            "SessionStart" => day.sessions = day.sessions.saturating_add(1),
            "PreToolUse" if matches!(tool, "Bash" | "PowerShell" | "Execute" | "run_command") => {
                day.commands_run = day.commands_run.saturating_add(1);
            }
            "PostToolUse" if matches!(tool, "Write" | "Edit" | "MultiEdit") => {
                day.files_changed = day.files_changed.saturating_add(1);
                let input = payload.get("tool_input").unwrap_or(&Value::Null);
                if let Some((added, removed)) = file_change(tool, input) {
                    day.lines_added = day.lines_added.saturating_add(added);
                    day.lines_removed = day.lines_removed.saturating_add(removed);
                }
            }
            _ => return,
        }
        save(history);
    });
}

pub fn record_permission(app: &AppHandle, decision: &str) {
    let Some(store) = app.try_state::<Store>() else {
        return;
    };
    store.with_history(|history| {
        if !history.enabled {
            return;
        }

        let day = day_mut(history, today());
        match decision {
            "allow" | "always" => day.permissions_allowed = day.permissions_allowed.saturating_add(1),
            "deny" => day.permissions_denied = day.permissions_denied.saturating_add(1),
            _ => return,
        }
        save(history);
    });
}

pub fn forget_request(_app: &AppHandle, _request_id: &str) {}

pub fn record_decision(app: &AppHandle, _request_id: &str, decision: &str) {
    record_permission(app, decision);
}

#[tauri::command]
pub fn recap_snapshot(store: State<Store>) -> Snapshot {
    store.with_history(|history| {
        let cutoff = today() - (WEEK_SECONDS / DAY_SECONDS);
        let mut snapshot = Snapshot {
            enabled: history.enabled,
            sessions: 0,
            files_changed: 0,
            lines_added: 0,
            lines_removed: 0,
            commands_run: 0,
            permissions_allowed: 0,
            permissions_denied: 0,
        };
        for day in history.days.iter().filter(|day| day.date >= cutoff) {
            snapshot.sessions = snapshot.sessions.saturating_add(day.sessions);
            snapshot.files_changed = snapshot.files_changed.saturating_add(day.files_changed);
            snapshot.lines_added = snapshot.lines_added.saturating_add(day.lines_added);
            snapshot.lines_removed = snapshot.lines_removed.saturating_add(day.lines_removed);
            snapshot.commands_run = snapshot.commands_run.saturating_add(day.commands_run);
            snapshot.permissions_allowed = snapshot.permissions_allowed.saturating_add(day.permissions_allowed);
            snapshot.permissions_denied = snapshot.permissions_denied.saturating_add(day.permissions_denied);
        }
        snapshot
    })
}

#[tauri::command]
pub fn recap_set_enabled(store: State<Store>, enabled: bool) {
    store.with_history(|history| {
        history.enabled = enabled;
        save(history);
    });
}

#[tauri::command]
pub fn recap_clear(store: State<Store>) {
    store.with_history(|history| {
        history.days.clear();
        save(history);
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn privacy_limited_day_records_only_counts() {
        let mut history = History { enabled: true, days: Vec::new() };
        let date = 12_345;
        let day = day_mut(&mut history, date);
        day.sessions = day.sessions.saturating_add(1);
        day.files_changed = day.files_changed.saturating_add(1);
        assert_eq!(history.days.len(), 1);
        assert_eq!(history.days[0].date, date);
        assert_eq!(history.days[0].sessions, 1);
        assert_eq!(history.days[0].files_changed, 1);
    }

    #[test]
    fn history_retains_only_a_bounded_number_of_days() {
        let mut history = History { enabled: true, days: Vec::new() };
        for date in 0..(MAX_DAYS as i64 + 3) {
            day_mut(&mut history, date);
        }
        assert_eq!(history.days.len(), MAX_DAYS);
        assert_eq!(history.days[0].date, 3);
    }

    #[test]
    fn line_diff_counts_replacements_and_unchanged_context() {
        assert_eq!(line_diff("before\nold\nafter", "before\nnew\nafter"), (1, 1));
        assert_eq!(line_diff("same\n", "same\nadded\n"), (1, 0));
    }
}
