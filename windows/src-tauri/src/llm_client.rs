use reqwest::{Client, StatusCode};
use serde::Serialize;
use serde_json::{json, Value};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};
use tokio::sync::watch;

use crate::secrets;

const OLLAMA_BASE: &str = "http://127.0.0.1:11434";
const OPENROUTER_BASE: &str = "https://openrouter.ai/api/v1";
const OPENROUTER_MAX_TOKENS: u32 = 1024;
const VOICE_OLLAMA_MAX_TOKENS: u32 = 256;
const VOICE_OPENROUTER_MAX_TOKENS: u32 = 256;
const CONNECT_TIMEOUT: Duration = Duration::from_secs(3);
const VOICE_OPENROUTER_TIMEOUT: Duration = Duration::from_secs(8);
const VOICE_OLLAMA_TIMEOUT: Duration = Duration::from_secs(45);
const HEALTH_TIMEOUT: Duration = Duration::from_millis(1500);
const HEALTH_CACHE_AGE: Duration = Duration::from_secs(15);
const OLLAMA_START_WAIT: Duration = Duration::from_secs(5);

static HEALTH_CACHE: OnceLock<Mutex<Option<(Instant, OllamaStatus)>>> = OnceLock::new();
static OLLAMA_START_ATTEMPTED: AtomicBool = AtomicBool::new(false);
static RECOVERY_MONITOR_RUNNING: AtomicBool = AtomicBool::new(false);
static ACTIVE_OPENROUTER_ACCOUNT: OnceLock<Mutex<Option<String>>> = OnceLock::new();

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OllamaStatus {
    pub reachable: bool,
    pub models: Vec<String>,
    pub error: Option<String>,
}

#[derive(Debug)]
pub struct ChatReply {
    pub response: Value,
    pub provider: String,
    pub model: String,
    pub fallback_notice: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ProviderMode {
    Auto,
    VoiceAuto,
    OllamaOnly,
    OpenRouterOnly,
}

impl ProviderMode {
    fn parse(value: &str) -> Self {
        match value {
            "voiceAuto" => Self::VoiceAuto,
            "ollamaOnly" => Self::OllamaOnly,
            "openRouterOnly" => Self::OpenRouterOnly,
            _ => Self::Auto,
        }
    }
}

#[derive(Debug)]
struct ProviderError {
    message: String,
    retry_openrouter: bool,
    status: Option<StatusCode>,
}

impl ProviderError {
    fn local(message: impl Into<String>, retry_openrouter: bool) -> Self {
        Self {
            message: message.into(),
            retry_openrouter,
            status: None,
        }
    }
}

pub async fn ollama_status(refresh: bool) -> OllamaStatus {
    if refresh {
        *HEALTH_CACHE
            .get_or_init(|| Mutex::new(None))
            .lock()
            .unwrap() = None;
    }
    ensure_ollama().await
}

pub async fn chat(
    messages: Vec<Value>,
    tools: Option<Value>,
    mode: &str,
    ollama_model: &str,
    openrouter_model: &str,
    cancel: &mut watch::Receiver<bool>,
) -> Result<ChatReply, String> {
    let mode = ProviderMode::parse(mode);
    if mode == ProviderMode::VoiceAuto {
        let local_result = ollama_completion(
            messages.clone(),
            tools.clone(),
            ollama_model,
            false,
            cancel,
            Some(VOICE_OLLAMA_TIMEOUT),
            Some(VOICE_OLLAMA_MAX_TOKENS),
        )
        .await;
        match local_result {
            Ok(response) => {
                crate::log::line(format!("voice chat provider=ollama model={ollama_model}"));
                return Ok(ChatReply {
                    response,
                    provider: "ollama".into(),
                    model: ollama_model.into(),
                    fallback_notice: None,
                });
            }
            Err(error) if error.message == "Request cancelled." => return Err(error.message),
            Err(local_error) => {
                crate::log::line(format!(
                    "voice chat Ollama unavailable; trying OpenRouter: {}",
                    local_error.message
                ));
            }
        }
        return match openrouter_completion_with_timeout(
            messages,
            tools,
            openrouter_model,
            cancel,
            VOICE_OPENROUTER_TIMEOUT,
            VOICE_OPENROUTER_MAX_TOKENS,
        )
        .await
        {
            Ok((response, actual_model)) => {
                crate::log::line(format!(
                    "voice chat provider=openrouter model={actual_model} fallback=true"
                ));
                Ok(ChatReply {
                    response,
                    provider: "openrouter".into(),
                    model: actual_model,
                    fallback_notice: Some("Using OpenRouter (Ollama unavailable)".into()),
                })
            }
            Err(cloud_error) => Err(cloud_error),
        };
    }

    if mode != ProviderMode::OpenRouterOnly {
        let ollama_result = ollama_completion(
            messages.clone(),
            tools.clone(),
            ollama_model,
            mode == ProviderMode::OllamaOnly,
            cancel,
            None,
            None,
        )
        .await;
        match ollama_result {
            Ok(response) => {
                crate::log::line(format!("chat provider=ollama model={ollama_model}"));
                return Ok(ChatReply {
                    response,
                    provider: "ollama".into(),
                    model: ollama_model.into(),
                    fallback_notice: None,
                });
            }
            Err(error) if error.message == "Request cancelled." => return Err(error.message),
            Err(error) if mode == ProviderMode::OllamaOnly => return Err(error.message),
            Err(ollama_error) if !ollama_error.retry_openrouter => return Err(ollama_error.message),
            Err(ollama_error) => {
                let openrouter_result =
                    openrouter_completion(messages, tools, openrouter_model, cancel).await;
                return match openrouter_result {
                    Ok((response, actual_model)) => {
                        crate::log::line(format!(
                            "chat provider=openrouter model={actual_model} fallback=true"
                        ));
                        Ok(ChatReply {
                            response,
                            provider: "openrouter".into(),
                            model: actual_model,
                            fallback_notice: Some("Using OpenRouter (Ollama unavailable)".into()),
                        })
                    }
                    Err(openrouter_error) => Err(format!(
                        "Ollama failed: {}. OpenRouter failed: {}",
                        ollama_error.message, openrouter_error
                    )),
                };
            }
        }
    }

    let (response, actual_model) =
        openrouter_completion(messages, tools, openrouter_model, cancel).await?;
    crate::log::line(format!("chat provider=openrouter model={actual_model}"));
    Ok(ChatReply {
        response,
        provider: "openrouter".into(),
        model: actual_model,
        fallback_notice: None,
    })
}

pub async fn chat_stream(
    messages: Vec<Value>,
    tools: Option<Value>,
    mode: &str,
    ollama_model: &str,
    openrouter_model: &str,
    cancel: &mut watch::Receiver<bool>,
    deltas: &tokio::sync::mpsc::UnboundedSender<String>,
) -> Result<ChatReply, String> {
    let mode = ProviderMode::parse(mode);
    let mut emitted_text = false;
    if mode == ProviderMode::VoiceAuto {
        let local_result = ollama_completion_stream(
            messages.clone(),
            tools.clone(),
            ollama_model,
            false,
            cancel,
            Some(VOICE_OLLAMA_TIMEOUT),
            Some(VOICE_OLLAMA_MAX_TOKENS),
            deltas,
            &mut emitted_text,
        )
        .await;
        match local_result {
            Ok(response) => {
                crate::log::line(format!("voice chat provider=ollama model={ollama_model}"));
                return Ok(ChatReply {
                    response,
                    provider: "ollama".into(),
                    model: ollama_model.into(),
                    fallback_notice: None,
                });
            }
            Err(error) if error.message == "Request cancelled." => return Err(error.message),
            Err(error) if emitted_text => return Err(error.message),
            Err(error) => {
                crate::log::line(format!(
                    "voice chat Ollama unavailable; trying OpenRouter: {}",
                    error.message
                ));
            }
        }
        return match openrouter_completion_stream(
            messages,
            tools,
            openrouter_model,
            cancel,
            VOICE_OPENROUTER_TIMEOUT,
            VOICE_OPENROUTER_MAX_TOKENS,
            deltas,
            &mut emitted_text,
        )
        .await
        {
            Ok((response, actual_model)) => Ok(ChatReply {
                response,
                provider: "openrouter".into(),
                model: actual_model,
                fallback_notice: Some("Using OpenRouter (Ollama unavailable)".into()),
            }),
            Err(error) => Err(error),
        };
    }

    if mode != ProviderMode::OpenRouterOnly {
        match ollama_completion_stream(
            messages.clone(),
            tools.clone(),
            ollama_model,
            mode == ProviderMode::OllamaOnly,
            cancel,
            None,
            None,
            deltas,
            &mut emitted_text,
        )
        .await
        {
            Ok(response) => {
                crate::log::line(format!("chat provider=ollama model={ollama_model}"));
                return Ok(ChatReply {
                    response,
                    provider: "ollama".into(),
                    model: ollama_model.into(),
                    fallback_notice: None,
                });
            }
            Err(error) if error.message == "Request cancelled." => return Err(error.message),
            Err(error) if emitted_text => return Err(error.message),
            Err(error) if mode == ProviderMode::OllamaOnly => return Err(error.message),
            Err(error) if !error.retry_openrouter => return Err(error.message),
            Err(ollama_error) => {
                return match openrouter_completion_stream(
                    messages,
                    tools,
                    openrouter_model,
                    cancel,
                    Duration::from_secs(60),
                    OPENROUTER_MAX_TOKENS,
                    deltas,
                    &mut emitted_text,
                )
                .await
                {
                    Ok((response, actual_model)) => Ok(ChatReply {
                        response,
                        provider: "openrouter".into(),
                        model: actual_model,
                        fallback_notice: Some("Using OpenRouter (Ollama unavailable)".into()),
                    }),
                    Err(openrouter_error) => Err(format!(
                        "Ollama failed: {}. OpenRouter failed: {}",
                        ollama_error.message, openrouter_error
                    )),
                };
            }
        }
    }

    let (response, actual_model) = openrouter_completion_stream(
        messages,
        tools,
        openrouter_model,
        cancel,
        Duration::from_secs(60),
        OPENROUTER_MAX_TOKENS,
        deltas,
        &mut emitted_text,
    )
    .await?;
    crate::log::line(format!("chat provider=openrouter model={actual_model}"));
    Ok(ChatReply {
        response,
        provider: "openrouter".into(),
        model: actual_model,
        fallback_notice: None,
    })
}

async fn ollama_completion(
    messages: Vec<Value>,
    tools: Option<Value>,
    model: &str,
    wait_for_start: bool,
    cancel: &mut watch::Receiver<bool>,
    response_timeout: Option<Duration>,
    max_tokens: Option<u32>,
) -> Result<Value, ProviderError> {
    let status = if wait_for_start {
        ensure_ollama().await
    } else {
        ensure_ollama_for_auto_chat().await
    };
    if !status.reachable {
        return Err(ProviderError::local(
            status
                .error
                .unwrap_or_else(|| "Ollama is not reachable at http://localhost:11434.".into()),
            true,
        ));
    }
    if !status.models.iter().any(|installed| installed == model) {
        return Err(ProviderError::local(
            format!("Ollama model {model} is not installed. Run: ollama pull {model}"),
            true,
        ));
    }
    let timeout = response_timeout.unwrap_or_else(|| {
        if model == "qwen2.5:7b" {
            Duration::from_secs(60)
        } else {
            Duration::from_secs(120)
        }
    });
    let client = client(timeout).map_err(|error| ProviderError::local(error, true))?;
    let body = request_body(model, messages, tools, max_tokens, true);
    send_json(
        &client,
        format!("{OLLAMA_BASE}/v1/chat/completions"),
        "ollama",
        "ollama",
        body,
        cancel,
    )
    .await
}

async fn ollama_completion_stream(
    messages: Vec<Value>,
    tools: Option<Value>,
    model: &str,
    wait_for_start: bool,
    cancel: &mut watch::Receiver<bool>,
    response_timeout: Option<Duration>,
    max_tokens: Option<u32>,
    deltas: &tokio::sync::mpsc::UnboundedSender<String>,
    emitted_text: &mut bool,
) -> Result<Value, ProviderError> {
    let status = if wait_for_start {
        ensure_ollama().await
    } else {
        ensure_ollama_for_auto_chat().await
    };
    if !status.reachable {
        return Err(ProviderError::local(
            status
                .error
                .unwrap_or_else(|| "Ollama is not reachable at http://localhost:11434.".into()),
            true,
        ));
    }
    if !status.models.iter().any(|installed| installed == model) {
        return Err(ProviderError::local(
            format!("Ollama model {model} is not installed. Run: ollama pull {model}"),
            true,
        ));
    }
    let timeout = response_timeout.unwrap_or_else(|| {
        if model == "qwen2.5:7b" {
            Duration::from_secs(60)
        } else {
            Duration::from_secs(120)
        }
    });
    let client = client(timeout).map_err(|error| ProviderError::local(error, true))?;
    let mut body = request_body(model, messages, tools, max_tokens, true);
    body["stream"] = json!(true);
    send_json_stream(
        &client,
        format!("{OLLAMA_BASE}/v1/chat/completions"),
        "ollama",
        "ollama",
        body,
        cancel,
        deltas,
        emitted_text,
    )
    .await
}

async fn openrouter_completion(
    messages: Vec<Value>,
    tools: Option<Value>,
    model: &str,
    cancel: &mut watch::Receiver<bool>,
) -> Result<(Value, String), String> {
    openrouter_completion_with_timeout(
        messages,
        tools,
        model,
        cancel,
        Duration::from_secs(60),
        OPENROUTER_MAX_TOKENS,
    )
    .await
}

async fn openrouter_completion_with_timeout(
    messages: Vec<Value>,
    tools: Option<Value>,
    model: &str,
    cancel: &mut watch::Receiver<bool>,
    response_timeout: Duration,
    max_tokens: u32,
) -> Result<(Value, String), String> {
    let mut accounts = secrets::openrouter_accounts()?;
    if accounts.is_empty() {
        return Err("No OpenRouter API key is configured in the OS credential manager.".into());
    }
    let active = ACTIVE_OPENROUTER_ACCOUNT.get_or_init(|| Mutex::new(None));
    if let Some(id) = active.lock().unwrap().clone() {
        if let Some(index) = accounts.iter().position(|account| account.id == id) {
            accounts.rotate_left(index);
        }
    }
    let client = client(response_timeout).map_err(|error| format!("OpenRouter: {error}"))?;
    let mut errors = Vec::new();
    for account in accounts {
        let key = secrets::openrouter_key_for_account(&account.id)
            .map_err(|_| "OpenRouter could not read the configured credential.".to_string())?;
        let body = request_body(
            &openrouter_model(model),
            messages.clone(),
            tools.clone(),
            Some(max_tokens),
            false,
        );
        match send_openrouter_with_retry(&client, &key, body, cancel).await {
            Ok((response, used_model)) => {
                *active.lock().unwrap() = Some(account.id);
                return Ok((response, used_model));
            }
            Err(error) if error == "Request cancelled." => return Err(error),
            Err(error) => errors.push(error),
        }
    }
    if errors
        .iter()
        .any(|error| error.contains("OpenRouter credits exhausted"))
    {
        return Err("OpenRouter credits exhausted.".into());
    }
    Err(errors
        .first()
        .cloned()
        .unwrap_or_else(|| "OpenRouter request failed.".into()))
}

async fn openrouter_completion_stream(
    messages: Vec<Value>,
    tools: Option<Value>,
    model: &str,
    cancel: &mut watch::Receiver<bool>,
    response_timeout: Duration,
    max_tokens: u32,
    deltas: &tokio::sync::mpsc::UnboundedSender<String>,
    emitted_text: &mut bool,
) -> Result<(Value, String), String> {
    let mut accounts = secrets::openrouter_accounts()?;
    if accounts.is_empty() {
        return Err("No OpenRouter API key is configured in the OS credential manager.".into());
    }
    let active = ACTIVE_OPENROUTER_ACCOUNT.get_or_init(|| Mutex::new(None));
    if let Some(id) = active.lock().unwrap().clone() {
        if let Some(index) = accounts.iter().position(|account| account.id == id) {
            accounts.rotate_left(index);
        }
    }
    let client = client(response_timeout).map_err(|error| format!("OpenRouter: {error}"))?;
    let mut errors = Vec::new();
    for account in accounts {
        let key = secrets::openrouter_key_for_account(&account.id)
            .map_err(|_| "OpenRouter could not read the configured credential.".to_string())?;
        let mut body = request_body(
            &openrouter_model(model),
            messages.clone(),
            tools.clone(),
            Some(max_tokens),
            false,
        );
        body["stream"] = json!(true);
        let mut completed = None;
        for attempt in 0..=1 {
            match send_json_stream(
                &client,
                format!("{OPENROUTER_BASE}/chat/completions"),
                &key,
                "openrouter",
                body.clone(),
                cancel,
                deltas,
                emitted_text,
            )
            .await
            {
                Ok(response) => {
                    completed = Some(response);
                    break;
                }
                Err(error) if error.message == "Request cancelled." => return Err(error.message),
                Err(error) if *emitted_text => return Err(error.message),
                Err(error)
                    if attempt == 0
                        && error.status == Some(StatusCode::PAYMENT_REQUIRED)
                        && error.message.to_ascii_lowercase().contains("max_tokens") =>
                {
                    let current = body["max_tokens"]
                        .as_u64()
                        .and_then(|value| u32::try_from(value).ok())
                        .unwrap_or(max_tokens);
                    body["max_tokens"] = json!(reduced_max_tokens(&error.message, current));
                }
                Err(error) if error.status == Some(StatusCode::PAYMENT_REQUIRED) => {
                    errors.push("OpenRouter credits exhausted.".to_string());
                    break;
                }
                Err(error) => {
                    errors.push(error.message);
                    break;
                }
            }
        }
        if let Some(response) = completed {
            let actual_model = response_model(&response, &openrouter_model(model));
            *active.lock().unwrap() = Some(account.id);
            return Ok((response, actual_model));
        }
    }
    if errors
        .iter()
        .any(|error| error.contains("OpenRouter credits exhausted"))
    {
        return Err("OpenRouter credits exhausted.".into());
    }
    Err(errors
        .first()
        .cloned()
        .unwrap_or_else(|| "OpenRouter request failed.".into()))
}

async fn send_openrouter_with_retry(
    client: &Client,
    key: &str,
    body: Value,
    cancel: &mut watch::Receiver<bool>,
) -> Result<(Value, String), String> {
    send_openrouter_with_retry_at(
        client,
        format!("{OPENROUTER_BASE}/chat/completions"),
        key,
        body,
        cancel,
    )
    .await
}

async fn send_openrouter_with_retry_at(
    client: &Client,
    endpoint: String,
    key: &str,
    mut body: Value,
    cancel: &mut watch::Receiver<bool>,
) -> Result<(Value, String), String> {
    let model = body["model"].as_str().unwrap_or_default().to_string();
    for attempt in 0..=1 {
        match send_json(
            client,
            endpoint.clone(),
            key,
            "openrouter",
            body.clone(),
            cancel,
        )
        .await
        {
            Ok(response) => {
                let actual_model = response_model(&response, &model);
                return Ok((response, actual_model));
            }
            Err(error)
                if attempt == 0
                    && error.status == Some(StatusCode::PAYMENT_REQUIRED)
                    && error.message.to_ascii_lowercase().contains("max_tokens") =>
            {
                let current = body["max_tokens"]
                    .as_u64()
                    .and_then(|value| u32::try_from(value).ok())
                    .unwrap_or(OPENROUTER_MAX_TOKENS);
                let lower = reduced_max_tokens(&error.message, current);
                body["max_tokens"] = json!(lower);
            }
            Err(error) if error.status == Some(StatusCode::PAYMENT_REQUIRED) => {
                return Err("OpenRouter credits exhausted.".into())
            }
            Err(error) => return Err(error.message),
        }
    }
    Err("OpenRouter credits exhausted.".into())
}

fn request_body(
    model: &str,
    messages: Vec<Value>,
    tools: Option<Value>,
    max_tokens: Option<u32>,
    ollama: bool,
) -> Value {
    let mut body = json!({
        "model": model,
        "messages": messages,
        "max_tokens": max_tokens.unwrap_or(2048),
    });
    if let Some(tools) = tools {
        body["tools"] = tools;
        body["parallel_tool_calls"] = json!(false);
    }
    if ollama {
        body["keep_alive"] = json!("10m");
    } else {
        body["plugins"] = json!([{ "id": "web" }]);
    }
    body
}

async fn send_json(
    client: &Client,
    endpoint: String,
    credential: &str,
    provider: &str,
    body: Value,
    cancel: &mut watch::Receiver<bool>,
) -> Result<Value, ProviderError> {
    let mut request = client
        .post(endpoint)
        .header("content-type", "application/json")
        .json(&body);
    if provider == "ollama" {
        request = request.bearer_auth("ollama");
    } else {
        request = request
            .bearer_auth(credential)
            .header("HTTP-Referer", "https://github.com/Louis-CFM/coucou")
            .header("X-Title", "Coucou");
    }
    let response = tokio::select! {
        response = request.send() => response.map_err(classify_network_error)?,
        _ = cancelled(cancel) => return Err(ProviderError::local("Request cancelled.", false)),
    };
    let status = response.status();
    let text = tokio::select! {
        text = response.text() => text.map_err(classify_network_error)?,
        _ = cancelled(cancel) => return Err(ProviderError::local("Request cancelled.", false)),
    };
    if !status.is_success() {
        let parsed = serde_json::from_str::<Value>(&text).ok();
        let detail = parsed
            .as_ref()
            .and_then(|value| value.get("error"))
            .and_then(|error| error.get("message").or(Some(error)))
            .and_then(Value::as_str)
            .map(str::to_string)
            .unwrap_or_else(|| text.chars().take(240).collect());
        let lower_detail = detail.to_ascii_lowercase();
        let model_missing = lower_detail.contains("model not found")
            || (status == StatusCode::NOT_FOUND && lower_detail.contains("model"));
        let retry_openrouter = if provider == "ollama" {
            ollama_failure_allows_fallback(status, &detail)
        } else {
            false
        };
        return Err(ProviderError {
            message: if provider == "ollama" && model_missing {
                format!("Ollama model is unavailable: {detail}")
            } else {
                format!("HTTP {status}: {detail}")
            },
            retry_openrouter,
            status: Some(status),
        });
    }
    serde_json::from_str(&text).map_err(|error| {
        ProviderError::local(
            format!(
                "Invalid response from {}: {error}",
                if provider == "ollama" {
                    "Ollama"
                } else {
                    "OpenRouter"
                }
            ),
            false,
        )
    })
}

async fn send_json_stream(
    client: &Client,
    endpoint: String,
    credential: &str,
    provider: &str,
    body: Value,
    cancel: &mut watch::Receiver<bool>,
    deltas: &tokio::sync::mpsc::UnboundedSender<String>,
    emitted_text: &mut bool,
) -> Result<Value, ProviderError> {
    let mut request = client
        .post(endpoint)
        .header("content-type", "application/json")
        .json(&body);
    if provider == "ollama" {
        request = request.bearer_auth("ollama");
    } else {
        request = request
            .bearer_auth(credential)
            .header("HTTP-Referer", "https://github.com/Louis-CFM/coucou")
            .header("X-Title", "Coucou");
    }
    let mut response = tokio::select! {
        response = request.send() => response.map_err(classify_network_error)?,
        _ = cancelled(cancel) => return Err(ProviderError::local("Request cancelled.", false)),
    };
    let status = response.status();
    if !status.is_success() {
        let text = tokio::select! {
            text = response.text() => text.map_err(classify_network_error)?,
            _ = cancelled(cancel) => return Err(ProviderError::local("Request cancelled.", false)),
        };
        let parsed = serde_json::from_str::<Value>(&text).ok();
        let detail = parsed
            .as_ref()
            .and_then(|value| value.get("error"))
            .and_then(|error| error.get("message").or(Some(error)))
            .and_then(Value::as_str)
            .map(str::to_string)
            .unwrap_or_else(|| text.chars().take(240).collect());
        let lower_detail = detail.to_ascii_lowercase();
        let model_missing = lower_detail.contains("model not found")
            || (status == StatusCode::NOT_FOUND && lower_detail.contains("model"));
        return Err(ProviderError {
            message: if provider == "ollama" && model_missing {
                format!("Ollama model is unavailable: {detail}")
            } else {
                format!("HTTP {status}: {detail}")
            },
            retry_openrouter: provider == "ollama"
                && ollama_failure_allows_fallback(status, &detail),
            status: Some(status),
        });
    }

    let mut response_body = json!({
        "choices": [{
            "message": { "role": "assistant", "content": "", "tool_calls": [] },
            "finish_reason": null
        }]
    });
    let mut line_buffer = Vec::new();
    loop {
        let chunk = tokio::select! {
            chunk = response.chunk() => chunk.map_err(classify_network_error)?,
            _ = cancelled(cancel) => return Err(ProviderError::local("Request cancelled.", false)),
        };
        let Some(chunk) = chunk else { break };
        for byte in chunk {
            if byte == b'\n' {
                let line = String::from_utf8_lossy(&line_buffer);
                let line = line.trim();
                if let Some(data) = line.strip_prefix("data:") {
                    let data = data.trim();
                    if data == "[DONE]" {
                        return Ok(response_body);
                    }
                    if !data.is_empty() {
                        let event = serde_json::from_str::<Value>(data).map_err(|error| {
                            ProviderError::local(
                                format!("Invalid streaming response from {provider}: {error}"),
                                provider == "ollama",
                            )
                        })?;
                        append_stream_event(&mut response_body, &event, deltas, emitted_text)?;
                    }
                }
                line_buffer.clear();
            } else {
                line_buffer.push(byte);
            }
        }
    }
    if !line_buffer.is_empty() {
        let line = String::from_utf8_lossy(&line_buffer);
        if let Some(data) = line.trim().strip_prefix("data:") {
            let data = data.trim();
            if data != "[DONE]" && !data.is_empty() {
                let event = serde_json::from_str::<Value>(data).map_err(|error| {
                    ProviderError::local(
                        format!("Invalid streaming response from {provider}: {error}"),
                        provider == "ollama",
                    )
                })?;
                append_stream_event(&mut response_body, &event, deltas, emitted_text)?;
            }
        }
    }
    Ok(response_body)
}

fn append_stream_event(
    response: &mut Value,
    event: &Value,
    deltas: &tokio::sync::mpsc::UnboundedSender<String>,
    emitted_text: &mut bool,
) -> Result<(), ProviderError> {
    if let Some(error) = event.get("error") {
        let message = error
            .get("message")
            .and_then(Value::as_str)
            .or_else(|| error.as_str())
            .unwrap_or("The provider returned an error while streaming.");
        return Err(ProviderError::local(
            format!("Streaming response error: {message}"),
            false,
        ));
    }
    if let Some(model) = event.get("model") {
        response["model"] = model.clone();
    }
    let Some(choice) = event
        .get("choices")
        .and_then(Value::as_array)
        .and_then(|choices| choices.first())
    else {
        return Ok(());
    };
    if let Some(reason) = choice.get("finish_reason") {
        response["choices"][0]["finish_reason"] = reason.clone();
    }
    let Some(delta) = choice.get("delta") else {
        return Ok(());
    };
    if let Some(text) = delta.get("content").and_then(Value::as_str) {
        let current = response["choices"][0]["message"]["content"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        response["choices"][0]["message"]["content"] = json!(format!("{current}{text}"));
        *emitted_text |= !text.is_empty();
        if !deltas.is_closed() && deltas.send(text.to_string()).is_err() {
            crate::log::line("voice response listener closed while streaming a reply");
        }
    }
    if let Some(tool_calls) = delta.get("tool_calls").and_then(Value::as_array) {
        for tool_delta in tool_calls {
            let Some(index) = tool_delta.get("index").and_then(Value::as_u64) else {
                continue;
            };
            let index = index as usize;
            let calls = response["choices"][0]["message"]["tool_calls"]
                .as_array_mut()
                .expect("tool_calls is initialized as an array");
            while calls.len() <= index {
                calls.push(json!({
                    "id": "",
                    "type": "function",
                    "function": { "name": "", "arguments": "" }
                }));
            }
            let call = &mut calls[index];
            if let Some(id) = tool_delta.get("id").and_then(Value::as_str) {
                call["id"] = json!(id);
            }
            if let Some(kind) = tool_delta.get("type").and_then(Value::as_str) {
                call["type"] = json!(kind);
            }
            if let Some(name) = tool_delta["function"].get("name").and_then(Value::as_str) {
                let current = call["function"]["name"].as_str().unwrap_or_default();
                call["function"]["name"] = json!(format!("{current}{name}"));
            }
            if let Some(arguments) = tool_delta["function"]["arguments"].as_str() {
                let current = call["function"]["arguments"].as_str().unwrap_or_default();
                call["function"]["arguments"] = json!(format!("{current}{arguments}"));
            }
        }
    }
    Ok(())
}

fn classify_network_error(error: reqwest::Error) -> ProviderError {
    let retry_openrouter = error.is_timeout() || error.is_connect();
    ProviderError::local(
        if error.is_timeout() {
            "request timed out".to_string()
        } else if error.is_connect() {
            "connection failed".to_string()
        } else {
            "network request failed".to_string()
        },
        retry_openrouter,
    )
}

async fn cancelled(cancel: &mut watch::Receiver<bool>) {
    loop {
        if *cancel.borrow() {
            return;
        }
        if cancel.changed().await.is_err() {
            return;
        }
    }
}

fn client(response_timeout: Duration) -> Result<Client, String> {
    Client::builder()
        .connect_timeout(CONNECT_TIMEOUT)
        .timeout(response_timeout)
        .build()
        .map_err(|error| format!("Could not create HTTP client: {error}"))
}

async fn ensure_ollama() -> OllamaStatus {
    let cache = HEALTH_CACHE.get_or_init(|| Mutex::new(None));
    if let Some((checked, status)) = cache.lock().unwrap().as_ref() {
        if checked.elapsed() < HEALTH_CACHE_AGE {
            return status.clone();
        }
    }

    let mut status = fetch_ollama_models(HEALTH_TIMEOUT).await;
    if !status.reachable && !OLLAMA_START_ATTEMPTED.swap(true, Ordering::SeqCst) {
        let start_error = start_ollama();
        let deadline = Instant::now() + OLLAMA_START_WAIT;
        while Instant::now() < deadline {
            tokio::time::sleep(Duration::from_millis(250)).await;
            status = fetch_ollama_models(Duration::from_millis(500)).await;
            if status.reachable {
                break;
            }
        }
        if !status.reachable {
            status.error = Some(match start_error {
                Ok(()) => {
                    "Ollama did not become reachable after attempting to start `ollama serve`."
                        .into()
                }
                Err(error) => format!("Ollama is unavailable and could not be started: {error}"),
            });
        }
    }
    *cache.lock().unwrap() = Some((Instant::now(), status.clone()));
    if !status.reachable {
        monitor_ollama_recovery();
    }
    status
}

async fn ensure_ollama_for_auto_chat() -> OllamaStatus {
    let cache = HEALTH_CACHE.get_or_init(|| Mutex::new(None));
    if let Some((checked, status)) = cache.lock().unwrap().as_ref() {
        if checked.elapsed() < HEALTH_CACHE_AGE {
            return status.clone();
        }
    }

    let status = fetch_ollama_models(Duration::from_millis(500)).await;
    *cache.lock().unwrap() = Some((Instant::now(), status.clone()));
    if !status.reachable {
        if !OLLAMA_START_ATTEMPTED.swap(true, Ordering::SeqCst) {
            match start_ollama() {
                Ok(()) => crate::log::line(
                    "Ollama is starting in the background while Auto mode uses its fallback.",
                ),
                Err(error) => crate::log::line(format!(
                    "Ollama background startup failed; Auto mode will continue with fallback: {error}"
                )),
            }
        }
        monitor_ollama_recovery();
    }
    status
}

fn monitor_ollama_recovery() {
    if RECOVERY_MONITOR_RUNNING.swap(true, Ordering::SeqCst) {
        return;
    }
    tokio::spawn(async {
        loop {
            tokio::time::sleep(Duration::from_secs(5)).await;
            let status = fetch_ollama_models(HEALTH_TIMEOUT).await;
            *HEALTH_CACHE
                .get_or_init(|| Mutex::new(None))
                .lock()
                .unwrap() = Some((Instant::now(), status.clone()));
            if status.reachable {
                break;
            }
        }
        RECOVERY_MONITOR_RUNNING.store(false, Ordering::SeqCst);
    });
}

async fn fetch_ollama_models(timeout: Duration) -> OllamaStatus {
    let result = async {
        let client = Client::builder()
            .connect_timeout(Duration::from_millis(500))
            .timeout(timeout)
            .build()
            .map_err(|error| error.to_string())?;
        let response = client
            .get(format!("{OLLAMA_BASE}/api/tags"))
            .send()
            .await
            .map_err(|error| {
                if error.is_timeout() {
                    "Ollama health check timed out".to_string()
                } else {
                    "Could not connect to Ollama at http://localhost:11434".to_string()
                }
            })?;
        if !response.status().is_success() {
            return Err(format!(
                "Ollama health check returned HTTP {}",
                response.status()
            ));
        }
        let body: Value = response
            .json()
            .await
            .map_err(|error| format!("Invalid Ollama model list: {error}"))?;
        let models = body
            .get("models")
            .and_then(Value::as_array)
            .ok_or_else(|| "Ollama did not return a model list.".to_string())?
            .iter()
            .filter_map(|model| {
                model
                    .get("name")
                    .and_then(Value::as_str)
                    .map(str::to_string)
            })
            .collect();
        Ok::<_, String>(models)
    }
    .await;
    match result {
        Ok(models) => OllamaStatus {
            reachable: true,
            models,
            error: None,
        },
        Err(error) => OllamaStatus {
            reachable: false,
            models: Vec::new(),
            error: Some(error),
        },
    }
}

#[cfg(target_os = "windows")]
fn start_ollama() -> Result<(), String> {
    use std::os::windows::process::CommandExt;
    std::process::Command::new("ollama")
        .arg("serve")
        .creation_flags(0x08000000)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .map(|_| ())
        .map_err(|error| format!("Could not start Ollama: {error}"))
}

#[cfg(target_os = "linux")]
fn start_ollama() -> Result<(), String> {
    std::process::Command::new("ollama")
        .arg("serve")
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .map(|_| ())
        .map_err(|error| format!("Could not start Ollama: {error}"))
}

#[cfg(not(any(target_os = "windows", target_os = "linux")))]
fn start_ollama() -> Result<(), String> {
    Err("Automatic Ollama startup is not supported on this platform.".into())
}

fn response_model(response: &Value, requested_model: &str) -> String {
    response
        .get("model")
        .and_then(Value::as_str)
        .filter(|model| !model.is_empty())
        .unwrap_or(requested_model)
        .to_string()
}

fn openrouter_model(model: &str) -> String {
    let slug = match model {
        "claude-haiku-4-5" => "claude-haiku-4.5",
        other => other,
    };
    if slug.contains('/') {
        slug.to_string()
    } else {
        format!("anthropic/{slug}")
    }
}

fn ollama_failure_allows_fallback(status: StatusCode, detail: &str) -> bool {
    let detail = detail.to_ascii_lowercase();
    status.is_server_error()
        || detail.contains("model not found")
        || (status == StatusCode::NOT_FOUND && detail.contains("model"))
        || detail.contains("multimodal")
        || detail.contains("does not support image")
        || detail.contains("image input")
}

fn reduced_max_tokens(message: &str, current: u32) -> u32 {
    let upper_bound = current.saturating_sub(1).max(1);
    affordable_tokens(message)
        .map(|affordable| affordable.saturating_sub(64).max(1))
        .unwrap_or(512)
        .min(upper_bound)
}

fn affordable_tokens(message: &str) -> Option<u32> {
    if let Some((_, affordable)) = message.to_ascii_lowercase().split_once("can only afford") {
        return affordable
            .split(|character: char| !character.is_ascii_digit())
            .find_map(|part| part.parse::<u32>().ok());
    }
    message
        .split(|character: char| !character.is_ascii_digit())
        .filter_map(|part| part.parse::<u32>().ok())
        .last()
}

#[cfg(test)]
mod tests {
    use super::{
        affordable_tokens, append_stream_event, ollama_failure_allows_fallback, openrouter_model,
        reduced_max_tokens, request_body, send_json, send_openrouter_with_retry_at, ProviderMode,
    };
    use reqwest::{Client, StatusCode};
    use serde_json::Value;
    use std::time::Duration;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;
    use tokio::sync::watch;

    #[test]
    fn provider_modes_parse_unknown_values_as_auto() {
        assert_eq!(ProviderMode::parse("auto"), ProviderMode::Auto);
        assert_eq!(ProviderMode::parse("ollamaOnly"), ProviderMode::OllamaOnly);
        assert_eq!(
            ProviderMode::parse("openRouterOnly"),
            ProviderMode::OpenRouterOnly
        );
        assert_eq!(ProviderMode::parse("voiceAuto"), ProviderMode::VoiceAuto);
        assert_eq!(ProviderMode::parse("invalid"), ProviderMode::Auto);
    }

    #[test]
    fn streaming_response_accumulates_text_and_tool_call_fragments() {
        let (deltas, mut received) = tokio::sync::mpsc::unbounded_channel();
        let mut emitted_text = false;
        let mut response = serde_json::json!({
            "choices": [{
                "message": { "role": "assistant", "content": "", "tool_calls": [] },
                "finish_reason": null
            }]
        });
        let events = [
            serde_json::json!({
                "model": "local-model",
                "choices": [{
                    "delta": {
                        "content": "Hello. ",
                        "tool_calls": [{
                            "index": 0,
                            "id": "call-1",
                            "type": "function",
                            "function": { "name": "open_", "arguments": "{\"url\":" }
                        }]
                    },
                    "finish_reason": null
                }]
            }),
            serde_json::json!({
                "choices": [{
                    "delta": {
                        "content": "Opening now.",
                        "tool_calls": [{
                            "index": 0,
                            "function": { "name": "website", "arguments": "\"https://example.com\"}" }
                        }]
                    },
                    "finish_reason": "tool_calls"
                }]
            }),
        ];
        for event in events {
            append_stream_event(&mut response, &event, &deltas, &mut emitted_text).unwrap();
        }

        assert_eq!(received.try_recv().unwrap(), "Hello. ");
        assert_eq!(received.try_recv().unwrap(), "Opening now.");
        assert_eq!(
            response["choices"][0]["message"]["content"],
            "Hello. Opening now."
        );
        assert_eq!(
            response["choices"][0]["message"]["tool_calls"][0]["function"]["name"],
            "open_website"
        );
        assert_eq!(
            response["choices"][0]["message"]["tool_calls"][0]["function"]["arguments"],
            "{\"url\":\"https://example.com\"}"
        );
        assert_eq!(response["choices"][0]["finish_reason"], "tool_calls");
        assert_eq!(response["model"], "local-model");
    }

    #[test]
    fn ollama_falls_back_only_for_server_errors_and_missing_models() {
        assert!(ollama_failure_allows_fallback(
            StatusCode::SERVICE_UNAVAILABLE,
            "service unavailable"
        ));
        assert!(ollama_failure_allows_fallback(
            StatusCode::NOT_FOUND,
            "model 'missing:1b' not found"
        ));
        assert!(!ollama_failure_allows_fallback(
            StatusCode::BAD_REQUEST,
            "invalid request"
        ));
        assert!(!ollama_failure_allows_fallback(
            StatusCode::NOT_FOUND,
            "route not found"
        ));
    }

    #[test]
    fn openrouter_model_preserves_existing_slugs() {
        assert_eq!(openrouter_model("openrouter/free"), "openrouter/free");
        assert_eq!(openrouter_model("qwen/qwen3:free"), "qwen/qwen3:free");
        assert_eq!(
            openrouter_model("claude-haiku-4-5"),
            "anthropic/claude-haiku-4.5"
        );
    }

    #[test]
    fn payment_retry_uses_affordable_output_with_a_margin_and_a_single_lower_limit() {
        let error = "requires fewer max_tokens (requested 4096, can only afford 1600)";
        assert_eq!(affordable_tokens(error), Some(1600));
        assert_eq!(reduced_max_tokens(error, 1024), 1023);
        assert_eq!(reduced_max_tokens("max_tokens not affordable", 1024), 512);
        assert_eq!(reduced_max_tokens("can only afford 32", 1024), 1);
    }

    #[tokio::test]
    async fn openrouter_402_retries_once_with_fewer_tokens_then_reports_exhaustion() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let endpoint = format!("http://{}/chat/completions", listener.local_addr().unwrap());
        let server = tokio::spawn(async move {
            let mut requests = Vec::new();
            for _ in 0..2 {
                let (mut stream, _) = listener.accept().await.unwrap();
                let mut bytes = Vec::new();
                let mut chunk = [0; 1024];
                let (body_offset, content_length) = loop {
                    let count = stream.read(&mut chunk).await.unwrap();
                    assert!(count > 0, "client closed before sending a complete request");
                    bytes.extend_from_slice(&chunk[..count]);
                    if let Some(offset) = bytes.windows(4).position(|part| part == b"\r\n\r\n") {
                        let headers = String::from_utf8_lossy(&bytes[..offset]);
                        let content_length = headers
                            .lines()
                            .find_map(|line| {
                                let (name, value) = line.split_once(':')?;
                                name.eq_ignore_ascii_case("content-length")
                                    .then(|| value.trim().parse::<usize>().unwrap())
                            })
                            .unwrap();
                        let body_offset = offset + 4;
                        if bytes.len() >= body_offset + content_length {
                            break (body_offset, content_length);
                        }
                    }
                };
                requests.push(
                    serde_json::from_slice::<Value>(
                        &bytes[body_offset..body_offset + content_length],
                    )
                    .unwrap(),
                );
                let payload = r#"{"error":{"message":"requires fewer max_tokens (requested 4096, can only afford 1600)"}}"#;
                let response = format!(
                    "HTTP/1.1 402 Payment Required\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                    payload.len(),
                    payload
                );
                stream.write_all(response.as_bytes()).await.unwrap();
            }
            requests
        });

        let client = Client::builder().build().unwrap();
        let body = request_body("openrouter/free", vec![], None, Some(1024), false);
        let (sender, mut cancel) = watch::channel(false);
        let result =
            send_openrouter_with_retry_at(&client, endpoint, "test-key", body, &mut cancel).await;
        drop(sender);
        assert_eq!(result.unwrap_err(), "OpenRouter credits exhausted.");

        let requests = server.await.unwrap();
        assert_eq!(requests.len(), 2);
        assert_eq!(requests[0]["max_tokens"], 1024);
        assert_eq!(requests[1]["max_tokens"], 1023);
    }

    #[tokio::test]
    async fn cancelling_an_in_flight_http_request_returns_without_fallback() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let endpoint = format!("http://{}/chat/completions", listener.local_addr().unwrap());
        let server = tokio::spawn(async move {
            let _connection = listener.accept().await.unwrap();
            tokio::time::sleep(Duration::from_secs(2)).await;
        });

        let client = Client::builder().build().unwrap();
        let body = request_body("qwen2.5:7b", vec![], None, None, true);
        let (sender, mut cancel) = watch::channel(false);
        let cancel_task = tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(50)).await;
            let _ = sender.send(true);
        });
        let result = send_json(&client, endpoint, "ollama", "ollama", body, &mut cancel).await;
        cancel_task.await.unwrap();
        server.abort();
        assert_eq!(result.unwrap_err().message, "Request cancelled.");
    }
}
