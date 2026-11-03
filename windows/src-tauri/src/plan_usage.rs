use std::path::Path;
use std::time::Duration;

use reqwest::header::{ACCEPT, AUTHORIZATION, HeaderValue};
use reqwest::{Client, StatusCode};
use serde::Serialize;
use serde_json::Value;
use time::format_description::well_known::Rfc3339;
use time::OffsetDateTime;
use tauri::{command, Manager};

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PlanWindow {
    pub pct: u8,
    pub resets_at_ms: u64,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Plan {
    pub five_hour: Option<PlanWindow>,
    pub seven_day: Option<PlanWindow>,
    pub reset_credits: Option<u32>,
    pub plan: Option<String>,
    pub updated_at_ms: u64,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderUsage {
    pub usage: Option<Plan>,
    pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UsageReport {
    pub claude: ProviderUsage,
    pub codex: ProviderUsage,
}

#[command]
pub async fn agent_plan_usage(app: tauri::AppHandle) -> Result<UsageReport, String> {
    let report = fetch_report().await?;
    publish_phone_usage(&app, &report);
    Ok(report)
}

/// Refresh the phone's opted-in plan usage without polling when that capability
/// is disabled.
pub fn start(app: tauri::AppHandle) {
    tauri::async_runtime::spawn(async move {
        let mut interval = tokio::time::interval(Duration::from_secs(15 * 60));
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            interval.tick().await;
            if !app.state::<crate::Shared>().settings.lock().unwrap().phone_usage {
                continue;
            }
            match fetch_report().await {
                Ok(report) => publish_phone_usage(&app, &report),
                Err(error) => crate::log::line(format!("plan usage refresh failed: {error}")),
            }
        }
    });
}

pub fn refresh_phone_usage(app: tauri::AppHandle) {
    tauri::async_runtime::spawn(async move {
        if !app.state::<crate::Shared>().settings.lock().unwrap().phone_usage {
            return;
        }
        match fetch_report().await {
            Ok(report) => publish_phone_usage(&app, &report),
            Err(error) => crate::log::line(format!("plan usage refresh failed: {error}")),
        }
    });
}

async fn fetch_report() -> Result<UsageReport, String> {
    let home = crate::platform::home_dir();
    let client = Client::builder()
        .timeout(Duration::from_secs(10))
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .map_err(|error| format!("Could not prepare the usage request: {error}"))?;

    let (claude, codex) = tokio::join!(
        fetch_claude(&client, &home),
        fetch_codex(&client, &home),
    );

    Ok(UsageReport {
        claude: provider_result(claude),
        codex: provider_result(codex),
    })
}

fn publish_phone_usage(app: &tauri::AppHandle, report: &UsageReport) {
    crate::phone_link::publish_usage(app, report_for_phone(report));
}

fn report_for_phone(report: &UsageReport) -> crate::phone_link::hub::UsageIn {
    use crate::phone_link::hub::{PlanIn, UsageIn, WindowIn};

    let to_plan = |provider: &ProviderUsage| {
        provider.usage.as_ref().map(|plan| PlanIn {
            five_hour: plan.five_hour.as_ref().map(|window| WindowIn {
                used_pct: f64::from(window.pct),
                resets_at: window.resets_at_ms,
            }),
            seven_day: plan.seven_day.as_ref().map(|window| WindowIn {
                used_pct: f64::from(window.pct),
                resets_at: window.resets_at_ms,
            }),
            reset_credits: plan.reset_credits,
            plan_type: plan.plan.clone(),
            updated_at: plan.updated_at_ms,
        })
    };
    UsageIn { claude: to_plan(&report.claude), codex: to_plan(&report.codex) }
}

fn provider_result(result: Result<Option<Plan>, String>) -> ProviderUsage {
    match result {
        Ok(usage) => ProviderUsage { usage, error: None },
        Err(error) => ProviderUsage { usage: None, error: Some(error) },
    }
}

async fn fetch_claude(client: &Client, home: &Path) -> Result<Option<Plan>, String> {
    let config_dir = std::env::var_os("CLAUDE_CONFIG_DIR")
        .filter(|value| !value.is_empty())
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| home.join(".claude"));
    let path = config_dir.join(".credentials.json");
    let credentials = read_credentials(&path, "Claude Code")?;
    let token = credentials
        .pointer("/claudeAiOauth/accessToken")
        .and_then(Value::as_str)
        .filter(|token| !token.is_empty())
        .ok_or_else(|| "Claude Code sign-in has no access token. Sign in again in Claude Code.".to_string())?;
    let response = client
        .get("https://api.anthropic.com/api/oauth/usage")
        .header(ACCEPT, "application/json")
        .header(AUTHORIZATION, bearer(token)?)
        .header("anthropic-beta", "oauth-2025-04-20")
        .header("user-agent", "claude-code")
        .send()
        .await
        .map_err(|error| format!("Could not reach Claude usage: {error}"))?;
    let body = response_body(response, "Claude").await?;
    let now = now_ms();
    let five_hour = parse_claude_window(body.get("five_hour"), now);
    let seven_day = parse_claude_window(body.get("seven_day"), now);
    if five_hour.is_none() && seven_day.is_none() {
        return Err("Claude returned no recognized five-hour or weekly usage limits.".into());
    }
    let plan = credentials
        .pointer("/claudeAiOauth/subscriptionType")
        .and_then(Value::as_str)
        .map(str::to_owned);
    Ok(Some(Plan { five_hour, seven_day, reset_credits: None, plan, updated_at_ms: now }))
}

async fn fetch_codex(client: &Client, home: &Path) -> Result<Option<Plan>, String> {
    let config_dir = std::env::var_os("CODEX_HOME")
        .filter(|value| !value.is_empty())
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| home.join(".codex"));
    let path = config_dir.join("auth.json");
    let credentials = read_credentials(&path, "Codex")?;
    let token = credentials
        .pointer("/tokens/access_token")
        .and_then(Value::as_str)
        .filter(|token| !token.is_empty())
        .ok_or_else(|| "Codex sign-in has no access token. Sign in again in Codex.".to_string())?;
    let mut request = client
        .get("https://chatgpt.com/backend-api/wham/usage")
        .header(ACCEPT, "application/json")
        .header(AUTHORIZATION, bearer(token)?)
        .header("user-agent", "codex_cli_rs");
    if let Some(account_id) = credentials
        .pointer("/tokens/account_id")
        .and_then(Value::as_str)
        .filter(|account_id| !account_id.is_empty())
    {
        request = request.header("ChatGPT-Account-Id", account_id);
    }
    let response = request
        .send()
        .await
        .map_err(|error| format!("Could not reach Codex usage: {error}"))?;
    let body = response_body(response, "Codex").await?;
    let now = now_ms();
    let rate_limit = body.get("rate_limit").unwrap_or(&body);
    let (five_hour, seven_day) = parse_codex_windows(rate_limit, now);
    if five_hour.is_none() && seven_day.is_none() {
        return Err("Codex returned no recognized five-hour or weekly usage limits.".into());
    }
    let plan = body
        .get("plan_type")
        .or_else(|| body.pointer("/rate_limit/plan_type"))
        .and_then(Value::as_str)
        .map(str::to_owned);
    let reset_credits = body
        .pointer("/rate_limit_reset_credits/available_count")
        .or_else(|| body.pointer("/credits/resets"))
        .and_then(Value::as_u64)
        .and_then(|value| u32::try_from(value).ok());
    Ok(Some(Plan { five_hour, seven_day, reset_credits, plan, updated_at_ms: now }))
}

fn read_credentials(path: &Path, provider: &str) -> Result<Value, String> {
    let bytes = std::fs::read(path).map_err(|error| match error.kind() {
        std::io::ErrorKind::NotFound => format!(
            "No {provider} sign-in found. Sign in to {provider} on this computer first."
        ),
        _ => format!("Could not read the local {provider} sign-in: {error}"),
    })?;
    serde_json::from_slice(&bytes)
        .map_err(|_| format!("The local {provider} sign-in file is not valid JSON. Sign in again."))
}

fn bearer(token: &str) -> Result<HeaderValue, String> {
    HeaderValue::from_str(&format!("Bearer {token}"))
        .map_err(|_| "The local sign-in token contains invalid characters. Sign in again.".into())
}

async fn response_body(response: reqwest::Response, provider: &str) -> Result<Value, String> {
    let status = response.status();
    if !status.is_success() {
        let message = match status {
            StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN => {
                format!("{provider} sign-in expired or lacks access. Sign in again.")
            }
            StatusCode::TOO_MANY_REQUESTS => {
                format!("{provider} usage is temporarily rate-limited. Try again shortly.")
            }
            _ => format!("{provider} usage request failed (HTTP {}).", status.as_u16()),
        };
        return Err(message);
    }
    response
        .json()
        .await
        .map_err(|_| format!("{provider} returned an unreadable usage response."))
}

fn parse_claude_window(raw: Option<&Value>, now: u64) -> Option<PlanWindow> {
    let raw = raw?;
    let pct = raw.get("utilization")?.as_f64()?;
    let resets_at_ms = parse_timestamp_ms(raw.get("resets_at")?)?;
    valid_window(pct, resets_at_ms, now)
}

fn parse_codex_windows(rate_limit: &Value, now: u64) -> (Option<PlanWindow>, Option<PlanWindow>) {
    let mut five_hour = None;
    let mut seven_day = None;
    for (slot, raw) in [
        (0, rate_limit.get("primary_window")),
        (1, rate_limit.get("secondary_window")),
    ] {
        let Some(raw) = raw else { continue };
        let parsed = parse_codex_window(Some(raw), now);
        let duration = raw.get("limit_window_seconds").and_then(Value::as_u64);
        let target = match duration {
            Some(seconds) if seconds <= 6 * 60 * 60 => &mut five_hour,
            Some(seconds) if seconds <= 10 * 24 * 60 * 60 => &mut seven_day,
            Some(_) => continue,
            None if slot == 0 => &mut five_hour,
            None => &mut seven_day,
        };
        if target.is_none() {
            *target = parsed;
        }
    }
    (five_hour, seven_day)
}

fn parse_codex_window(raw: Option<&Value>, now: u64) -> Option<PlanWindow> {
    let raw = raw?;
    let pct = raw.get("used_percent")?.as_f64()?;
    let resets_at_ms = raw
        .get("reset_at")
        .and_then(parse_timestamp_ms)
        .or_else(|| {
            raw.get("reset_after_seconds")
                .and_then(Value::as_u64)
                .and_then(|seconds| seconds.checked_mul(1000))
                .and_then(|delay| now.checked_add(delay))
        })?;
    valid_window(pct, resets_at_ms, now)
}

fn valid_window(pct: f64, resets_at_ms: u64, now: u64) -> Option<PlanWindow> {
    if !pct.is_finite() || !(0.0..=200.0).contains(&pct) {
        return None;
    }
    let max_reset = now.saturating_add(400 * 24 * 60 * 60 * 1000);
    if resets_at_ms == 0 || resets_at_ms > max_reset {
        return None;
    }
    Some(PlanWindow { pct: pct.round().clamp(0.0, 100.0) as u8, resets_at_ms })
}

fn parse_timestamp_ms(raw: &Value) -> Option<u64> {
    if let Some(seconds) = raw.as_u64() {
        return seconds.checked_mul(1000);
    }
    let text = raw.as_str()?;
    let timestamp = OffsetDateTime::parse(text, &Rfc3339).ok()?;
    u64::try_from(timestamp.unix_timestamp_nanos() / 1_000_000).ok()
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_millis() as u64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn parses_claude_percentages_and_rfc3339_reset_times() {
        let now = 1_700_000_000_000;
        let window = parse_claude_window(
            Some(&json!({ "utilization": 68.4, "resets_at": "2023-11-15T00:00:00Z" })),
            now,
        )
        .unwrap();
        assert_eq!(window.pct, 68);
        assert_eq!(window.resets_at_ms, 1_700_006_400_000);
    }

    #[test]
    fn parses_codex_epoch_reset_and_clamps_usage_at_one_hundred() {
        let now = 1_700_000_000_000;
        let window = parse_codex_window(
            Some(&json!({ "used_percent": 120, "reset_at": 1_700_006_400 })),
            now,
        )
        .unwrap();
        assert_eq!(window.pct, 100);
        assert_eq!(window.resets_at_ms, 1_700_006_400_000);
    }

    #[test]
    fn classifies_codex_windows_by_duration_instead_of_slot_name() {
        let now = 1_700_000_000_000;
        let raw = json!({
            "primary_window": {
                "used_percent": 31,
                "limit_window_seconds": 604_800,
                "reset_at": 1_700_006_400
            },
            "secondary_window": {
                "used_percent": 12,
                "limit_window_seconds": 18_000,
                "reset_at": 1_700_000_300
            }
        });
        let (five_hour, seven_day) = parse_codex_windows(&raw, now);
        assert_eq!(five_hour.unwrap().pct, 12);
        assert_eq!(seven_day.unwrap().pct, 31);

        let weekly_only = json!({
            "primary_window": {
                "used_percent": 45,
                "limit_window_seconds": 604_800,
                "reset_at": 1_700_006_400
            },
            "secondary_window": null
        });
        let (five_hour, seven_day) = parse_codex_windows(&weekly_only, now);
        assert!(five_hour.is_none());
        assert_eq!(seven_day.unwrap().pct, 45);
    }

    #[test]
    fn phone_snapshot_contains_only_available_provider_windows() {
        let now = 1_700_000_000_000;
        let report = UsageReport {
            claude: ProviderUsage {
                usage: Some(Plan {
                    five_hour: Some(PlanWindow { pct: 68, resets_at_ms: now + 60_000 }),
                    seven_day: None,
                    reset_credits: None,
                    plan: Some("max".into()),
                    updated_at_ms: now,
                }),
                error: None,
            },
            codex: ProviderUsage { usage: None, error: Some("not signed in".into()) },
        };
        let phone = report_for_phone(&report);
        let claude = phone.claude.unwrap();
        let five_hour = claude.five_hour.unwrap();
        assert_eq!(five_hour.used_pct, 68.0);
        assert_eq!(five_hour.resets_at, now + 60_000);
        assert!(phone.codex.is_none());
    }

    #[test]
    fn rejects_invalid_percentages_and_implausible_reset_times() {
        let now = 1_700_000_000_000;
        assert!(parse_codex_window(
            Some(&json!({ "used_percent": -1, "reset_at": 1_700_006_400 })),
            now,
        ).is_none());
        assert!(parse_codex_window(
            Some(&json!({ "used_percent": 60, "reset_at": 2_000_000_000_000i64 })),
            now,
        ).is_none());
        assert!(parse_claude_window(
            Some(&json!({ "utilization": "unknown", "resets_at": "not a date" })),
            now,
        ).is_none());
    }
}
