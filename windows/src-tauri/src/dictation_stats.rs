use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::{Mutex, PoisonError};

use serde::{Deserialize, Serialize};
use tauri::State;

const WORDS_PER_TYPING_MINUTE: f64 = 45.0;
const CHART_DAYS: u32 = 30;

#[derive(Clone, Default, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", default)]
struct Day {
    words: u64,
    seconds: f64,
    dictations: u64,
}

#[derive(Default, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", default)]
struct History {
    days: BTreeMap<String, Day>,
    apps: BTreeMap<String, u64>,
}

#[derive(Default)]
pub struct Store(Mutex<History>);

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct Totals {
    words: u64,
    seconds: f64,
    dictations: u64,
    words_per_minute: u64,
    minutes_saved: f64,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Snapshot {
    week: Totals,
    month: Totals,
    all: Totals,
    daily: Vec<(String, u64)>,
    apps: BTreeMap<String, u64>,
}

impl Store {
    pub fn load() -> Self {
        Self(Mutex::new(read_history()))
    }

    fn with_history<R>(&self, f: impl FnOnce(&mut History) -> R) -> R {
        let mut history = self.0.lock().unwrap_or_else(PoisonError::into_inner);
        f(&mut history)
    }

    fn record(&self, text: &str, app: Option<&str>, seconds: f64) -> Result<(), String> {
        if text.len() > 256_000 {
            return Err("Dictation is too large to record in statistics.".into());
        }
        if !seconds.is_finite() || !(0.0..=86_400.0).contains(&seconds) {
            return Err("Dictation duration is invalid.".into());
        }
        let words = word_count(text);
        if words == 0 {
            return Ok(());
        }
        let app = app.map(str::trim).filter(|name| !name.is_empty());
        if app.is_some_and(|name| {
            name.len() > 260
                || !name.chars().all(|character| {
                    character.is_ascii_alphanumeric() || "._- ".contains(character)
                })
        }) {
            return Err("Dictation application name is invalid.".into());
        }

        self.with_history(|history| -> Result<(), String> {
            let day = history.days.entry(today_key()).or_default();
            day.words = day.words.saturating_add(words);
            day.seconds += seconds;
            day.dictations = day.dictations.saturating_add(1);
            if let Some(app) = app {
                let app_words = history.apps.entry(app.to_ascii_lowercase()).or_default();
                *app_words = app_words.saturating_add(words);
            }
            save_history(history)
        })
    }

    fn snapshot(&self) -> Snapshot {
        self.with_history(|history| snapshot(history, &today_key()))
    }
}

fn stats_path() -> PathBuf {
    crate::platform::local_dir().join("dictation-stats.json")
}

fn read_history() -> History {
    let path = stats_path();
    match std::fs::read(&path) {
        Ok(bytes) => match serde_json::from_slice::<History>(&bytes) {
            Ok(history) => history,
            Err(error) => {
                crate::log::line(format!("could not read dictation statistics: {error}"));
                History::default()
            }
        },
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => History::default(),
        Err(error) => {
            crate::log::line(format!("could not read dictation statistics: {error}"));
            History::default()
        }
    }
}

fn save_history(history: &History) -> Result<(), String> {
    let path = stats_path();
    if let Some(parent) = path.parent() {
        crate::platform::ensure_private_dir(parent)
            .map_err(|error| format!("Could not create dictation statistics folder: {error}"))?;
    }
    let bytes = serde_json::to_vec(history)
        .map_err(|error| format!("Could not serialize dictation statistics: {error}"))?;
    let temp = path.with_extension(format!("json.{}.tmp", std::process::id()));
    if let Err(error) = std::fs::write(&temp, bytes).and_then(|()| std::fs::rename(&temp, &path)) {
        let _ = std::fs::remove_file(&temp);
        return Err(format!("Could not save dictation statistics: {error}"));
    }
    Ok(())
}

fn word_count(text: &str) -> u64 {
    text.split_whitespace()
        .filter(|word| word.chars().any(char::is_alphanumeric))
        .count()
        .min(u64::MAX as usize) as u64
}

fn today_key() -> String {
    let today = crate::platform::local_time();
    format!("{:04}-{:02}-{:02}", today.year, today.month, today.day)
}

fn days_in_month(year: u32, month: u32) -> u32 {
    match month {
        2 if year % 4 == 0 && (year % 100 != 0 || year % 400 == 0) => 29,
        2 => 28,
        4 | 6 | 9 | 11 => 30,
        _ => 31,
    }
}

fn date_before(date: &str, days: u32) -> String {
    let (year, month, day) = date
        .split_once('-')
        .and_then(|(year, rest)| {
            let (month, day) = rest.split_once('-')?;
            Some((
                year.parse::<u32>().ok()?,
                month.parse::<u32>().ok()?,
                day.parse::<u32>().ok()?,
            ))
        })
        .unwrap_or((1970, 1, 1));
    let (mut year, mut month, mut day) = (year, month, day);
    for _ in 0..days {
        if day > 1 {
            day -= 1;
        } else if month > 1 {
            month -= 1;
            day = days_in_month(year, month);
        } else {
            year = year.saturating_sub(1);
            month = 12;
            day = 31;
        }
    }
    format!("{year:04}-{month:02}-{day:02}")
}

fn totals(history: &History, from: Option<&str>) -> Totals {
    let mut words = 0u64;
    let mut seconds = 0.0;
    let mut dictations = 0u64;
    for (date, day) in &history.days {
        if from.is_none_or(|start| date.as_str() >= start) {
            words = words.saturating_add(day.words);
            seconds += day.seconds;
            dictations = dictations.saturating_add(day.dictations);
        }
    }
    Totals {
        words,
        seconds,
        dictations,
        words_per_minute: if seconds < 1.0 || words == 0 {
            0
        } else {
            (words as f64 / (seconds / 60.0)).round() as u64
        },
        minutes_saved: (words as f64 / WORDS_PER_TYPING_MINUTE - seconds / 60.0).max(0.0),
    }
}

fn snapshot(history: &History, today: &str) -> Snapshot {
    let week_start = date_before(today, 6);
    let month_start = date_before(today, 29);
    let daily = (0..CHART_DAYS)
        .rev()
        .map(|offset| {
            let date = date_before(today, offset);
            let words = history.days.get(&date).map_or(0, |day| day.words);
            (date, words)
        })
        .collect();
    Snapshot {
        week: totals(history, Some(&week_start)),
        month: totals(history, Some(&month_start)),
        all: totals(history, None),
        daily,
        apps: history.apps.clone(),
    }
}

#[tauri::command]
pub fn nixi_dictation_stats_snapshot(store: State<Store>) -> Snapshot {
    store.snapshot()
}

#[tauri::command]
pub fn nixi_dictation_stats_record(
    store: State<Store>,
    text: String,
    application: Option<String>,
    seconds: f64,
) -> Result<(), String> {
    store.record(&text, application.as_deref(), seconds)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn counts_words_without_retaining_transcript_content() {
        let mut history = History::default();
        let words = word_count("Hello, Macha! 42");
        history.days.insert(
            "2026-10-24".into(),
            Day {
                words,
                seconds: 20.0,
                dictations: 1,
            },
        );
        let view = snapshot(&history, "2026-10-24");
        assert_eq!(view.all.words, 3);
        assert_eq!(view.all.dictations, 1);
        assert_eq!(view.all.words_per_minute, 9);
        assert_eq!(view.all.minutes_saved, 0.0);
        let serialized = serde_json::to_string(&history).unwrap();
        assert!(!serialized.contains("Hello"));
        assert!(!serialized.contains("Macha"));
    }

    #[test]
    fn totals_use_inclusive_local_calendar_windows_and_daily_zeroes() {
        let mut history = History::default();
        history.days.insert(
            "2026-10-18".into(),
            Day {
                words: 12,
                seconds: 30.0,
                dictations: 2,
            },
        );
        history.days.insert(
            "2026-10-17".into(),
            Day {
                words: 100,
                seconds: 60.0,
                dictations: 4,
            },
        );
        history.apps.insert("code.exe".into(), 12);

        let view = snapshot(&history, "2026-10-24");
        assert_eq!(view.week.words, 12);
        assert_eq!(view.month.words, 112);
        assert_eq!(view.all.dictations, 6);
        assert_eq!(view.daily.len(), 30);
        assert_eq!(view.daily.first().unwrap(), &("2026-09-25".into(), 0));
        assert_eq!(view.daily.last().unwrap(), &("2026-10-24".into(), 0));
        assert_eq!(date_before("2024-03-01", 1), "2024-02-29");
        assert_eq!(date_before("2026-01-01", 1), "2025-12-31");
    }
}
