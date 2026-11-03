//! Authenticated calls to Macha's hosted application service.
//!
//! Supabase access tokens are used only by the native Rust process. They are
//! never returned to the webview, included in URLs, or written to logs.

use serde_json::Value;
use serde_json::json;
use futures_util::StreamExt;
use reqwest::Method;
use url::Url;

fn base_url() -> Result<Url, String> {
    let configured = option_env!("MACHA_API_URL").unwrap_or_default().trim();
    if configured.is_empty() {
        return Err("Macha account service is not configured in this build.".into());
    }
    let mut url =
        Url::parse(configured).map_err(|_| "Macha account service URL is invalid.".to_string())?;
    let local = matches!(url.host_str(), Some("localhost" | "127.0.0.1" | "[::1]"));
    if url.scheme() != "https" && !(url.scheme() == "http" && local) {
        return Err("Macha account service must use HTTPS.".into());
    }
    url.set_path(&format!(
        "{}/v1/auth/session",
        url.path().trim_end_matches('/')
    ));
    url.set_query(None);
    url.set_fragment(None);
    Ok(url)
}

pub async fn register_google_user(access_token: &str) -> Result<(), String> {
    if access_token.is_empty() {
        return Err("Google sign-in did not return an access token.".into());
    }
    let url = base_url()?;
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(15))
        .build()
        .map_err(|_| "Could not prepare the Macha account service request.".to_string())?;
    let response = client
        .post(url)
        .bearer_auth(access_token)
        .send()
        .await
        .map_err(|_| {
            "Could not reach the Macha account service. Check your connection and try again."
                .to_string()
        })?;
    if response.status() == reqwest::StatusCode::UNAUTHORIZED {
        return Err(
            "Google sign-in could not be verified by the Macha account service. Sign in again."
                .into(),
        );
    }
    if !response.status().is_success() {
        return Err(
            "The Macha account service could not save this Google account. Try again later.".into(),
        );
    }
    let account = response.json::<Value>().await.map_err(|_| {
        "The Macha account service returned an invalid account response.".to_string()
    })?;
    if account.get("googleId").and_then(Value::as_str).is_none()
        || account.get("email").and_then(Value::as_str).is_none()
    {
        return Err("The Macha account service returned an invalid account response.".into());
    }
    Ok(())
}

pub async fn copilot_start(access_token: &str) -> Result<Value, String> {
    copilot_request(access_token, "/v1/copilot/device/start", Method::POST, json!({})).await
}

pub async fn copilot_poll(access_token: &str, flow_id: &str) -> Result<Value, String> {
    copilot_request(access_token, "/v1/copilot/device/poll", Method::POST, json!({ "flowId": flow_id })).await
}

pub async fn copilot_accounts(access_token: &str) -> Result<Value, String> {
    copilot_request(access_token, "/v1/copilot/accounts", Method::GET, Value::Null).await
}

pub async fn copilot_provider_accounts(access_token: &str) -> Result<Value, String> {
    copilot_request(access_token, "/v1/copilot/provider/accounts", Method::GET, Value::Null).await
}

pub async fn copilot_disconnect(access_token: &str, account_id: &str) -> Result<(), String> {
    copilot_request(access_token, &format!("/v1/copilot/accounts/{account_id}"), Method::DELETE, Value::Null).await?;
    Ok(())
}

pub async fn copilot_provider_completion(access_token: &str, body: &Value) -> Result<Value, String> {
    let url = copilot_url("/v1/copilot/provider/chat")?;
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(70))
        .build()
        .map_err(|_| "Could not prepare the Copilot request.".to_string())?;
    let response = client.post(url).bearer_auth(access_token).json(body).send().await
        .map_err(|_| "GitHub Copilot service is unreachable. Check your connection and try again.".to_string())?;
    parse_copilot_response(response).await
}

pub async fn copilot_provider_stream(
    access_token: &str,
    body: &Value,
    cancel: &mut tokio::sync::watch::Receiver<bool>,
    deltas: &tokio::sync::mpsc::UnboundedSender<String>,
    emitted_text: &mut bool,
) -> Result<Value, String> {
    let url = copilot_url("/v1/copilot/provider/stream")?;
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(70))
        .build()
        .map_err(|_| "Could not prepare the Copilot request.".to_string())?;
    let response = client.post(url).bearer_auth(access_token).json(body).send().await
        .map_err(|_| "GitHub Copilot service is unreachable. Check your connection and try again.".to_string())?;
    if !response.status().is_success() { return parse_copilot_response(response).await; }
    let mut stream = response.bytes_stream();
    let mut pending = Vec::new();
    let mut completion = None;
    loop {
        tokio::select! {
            changed = cancel.changed() => {
                if changed.is_err() || *cancel.borrow() { return Err("Request cancelled.".into()); }
            }
            item = stream.next() => {
                let Some(item) = item else { break; };
                let chunk = item.map_err(|_| "The Copilot response stream ended unexpectedly.".to_string())?;
                pending.extend_from_slice(&chunk);
                while let Some(boundary) = pending.windows(2).position(|window| window == b"\n\n") {
                    let frame = pending.drain(..boundary + 2).collect::<Vec<_>>();
                    let text = String::from_utf8_lossy(&frame);
                    let Some(data) = text.lines().find_map(|line| line.strip_prefix("data: ")) else { continue; };
                    let event: Value = serde_json::from_str(data).map_err(|_| "The Copilot service returned an invalid stream event.".to_string())?;
                    match event.get("type").and_then(Value::as_str) {
                        Some("delta") => {
                            if let Some(delta) = event.get("delta").and_then(Value::as_str).filter(|value| !value.is_empty()) {
                                *emitted_text = true;
                                let _ = deltas.send(delta.to_string());
                            }
                        }
                        Some("result") => completion = event.get("response").cloned(),
                        Some("error") => return Err(copilot_error_message(event.get("error").and_then(Value::as_str).unwrap_or_default()).into()),
                        _ => {}
                    }
                }
            }
        }
    }
    completion.ok_or_else(|| "The Copilot response stream ended before completion.".into())
}

async fn parse_copilot_response(response: reqwest::Response) -> Result<Value, String> {
    let status = response.status();
    let body = response.json::<Value>().await.unwrap_or(Value::Null);
    if status.is_success() { return Ok(body); }
    let code = body.get("error").and_then(Value::as_str).unwrap_or_default();
    Err(copilot_error_message(code).into())
}

fn copilot_error_message(code: &str) -> &'static str {
    match code {
        "copilot_needs_reconnect" => "Your GitHub Copilot account needs reconnecting. Reconnect it in Settings.",
        "copilot_subscription_inactive" => "This GitHub account has no active Copilot subscription.",
        "copilot_subscription_unverified" => "The GitHub Copilot subscription could not be verified.",
        "copilot_account_not_found" => "Select a connected GitHub Copilot account in Settings.",
        "copilot_rate_limited" => "GitHub Copilot is rate-limited. Wait a moment and try again.",
        "copilot_request_timeout" => "GitHub Copilot took too long to respond. Try again.",
        "unauthorized" => "Sign in with Google to use GitHub Copilot.",
        _ => "GitHub Copilot service is unavailable. Check your connection and try again.",
    }
}

fn copilot_url(path: &str) -> Result<Url, String> {
    let mut url = base_url()?;
    let prefix = url.path().trim_end_matches("/v1/auth/session").trim_end_matches('/').to_string();
    url.set_path(&format!("{prefix}{path}"));
    Ok(url)
}

async fn copilot_request(access_token: &str, path: &str, method: Method, body: Value) -> Result<Value, String> {
    if access_token.is_empty() { return Err("Sign in with Google to connect GitHub Copilot.".into()); }
    let mut url = base_url()?;
    let prefix = url.path().trim_end_matches("/v1/auth/session").trim_end_matches('/').to_string();
    url.set_path(&format!("{prefix}{path}"));
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(20))
        .build()
        .map_err(|_| "Could not prepare the Copilot connection request.".to_string())?;
    let mut request = client.request(method, url).bearer_auth(access_token);
    if !body.is_null() { request = request.json(&body); }
    let response = request.send().await.map_err(|_| "Could not reach the Macha account service. Check your connection and try again.".to_string())?;
    let status = response.status();
    if status == reqwest::StatusCode::UNAUTHORIZED {
        return Err("Your Google sign-in expired. Sign in with Google again.".into());
    }
    if status == reqwest::StatusCode::NO_CONTENT { return Ok(Value::Null); }
    let value: Value = response.json().await.unwrap_or(Value::Null);
    if !status.is_success() {
        return Err(match value.get("error").and_then(Value::as_str) {
            Some("copilot_service_unavailable") => "The Copilot connection service is unavailable. Try again later.",
            Some("unauthorized") => "Sign in with Google to connect GitHub Copilot.",
            _ => "The Copilot connection request could not be completed. Try again later.",
        }.into());
    }
    Ok(value)
}
