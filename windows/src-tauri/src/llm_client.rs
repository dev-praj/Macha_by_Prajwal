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
const ANTHROPIC_BASE: &str = "https://api.anthropic.com/v1";
const OPENAI_BASE: &str = "https://api.openai.com/v1";
const GEMINI_OPENAI_BASE: &str = "https://generativelanguage.googleapis.com/v1beta/openai";
const GROK_BASE: &str = "https://api.x.ai/v1";
const CLAUDE_MODEL: &str = "claude-sonnet-5-5";
const OPENAI_MODEL: &str = "gpt-4.1-mini";
const GEMINI_MODEL: &str = "gemini-3.8-flash";
const GROK_MODEL: &str = "grok-4.7";
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
    GrokOnly,
    ClaudeOnly,
    OpenAiOnly,
    GeminiOnly,
}

impl ProviderMode {
    fn parse(value: &str) -> Self {
        match value {
            "voiceAuto" => Self::VoiceAuto,
            "ollamaOnly" => Self::OllamaOnly,
            "openRouterOnly" => Self::OpenRouterOnly,
            "grokOnly" => Self::GrokOnly,
            "claudeOnly" => Self::ClaudeOnly,
            "openAiOnly" => Self::OpenAiOnly,
            "geminiOnly" => Self::GeminiOnly,
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
    if is_direct_provider(mode) {
        let (provider, response, actual_model) =
            direct_completion(mode, messages, tools, cancel).await?;
        crate::log::line(format!("chat provider={provider} model={actual_model}"));
        return Ok(ChatReply {
            response,
            provider,
            model: actual_model,
            fallback_notice: None,
        });
    }
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
    if is_direct_provider(mode) {
        let (provider, response, actual_model) = direct_completion_stream(
            mode,
            messages,
            tools,
            cancel,
            deltas,
            &mut emitted_text,
        )
        .await?;
        crate::log::line(format!("chat provider={provider} model={actual_model}"));
        return Ok(ChatReply {
            response,
            provider,
            model: actual_model,
            fallback_notice: None,
        });
    }
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

fn is_direct_provider(mode: ProviderMode) -> bool {
    matches!(
        mode,
        ProviderMode::GrokOnly
            | ProviderMode::ClaudeOnly
            | ProviderMode::OpenAiOnly
            | ProviderMode::GeminiOnly
    )
}

async fn direct_completion(
    mode: ProviderMode,
    messages: Vec<Value>,
    tools: Option<Value>,
    cancel: &mut watch::Receiver<bool>,
) -> Result<(String, Value, String), String> {
    let (provider, response, model) = match mode {
        ProviderMode::GrokOnly => {
            let (response, model) = grok_completion(messages, tools, cancel).await?;
            ("grok", response, model)
        }
        ProviderMode::ClaudeOnly => {
            let (response, model) = claude_completion(messages, tools, cancel).await?;
            ("claude", response, model)
        }
        ProviderMode::OpenAiOnly => {
            let (response, model) = compatible_provider_completion(
                "openai",
                "openai-api-key",
                OPENAI_BASE,
                OPENAI_MODEL,
                messages,
                tools,
                cancel,
            )
            .await?;
            ("openai", response, model)
        }
        ProviderMode::GeminiOnly => {
            let (response, model) = compatible_provider_completion(
                "gemini",
                "gemini-api-key",
                GEMINI_OPENAI_BASE,
                GEMINI_MODEL,
                messages,
                tools,
                cancel,
            )
            .await?;
            ("gemini", response, model)
        }
        _ => return Err("The selected provider mode is not a direct provider.".into()),
    };
    Ok((provider.to_string(), response, model))
}

async fn direct_completion_stream(
    mode: ProviderMode,
    messages: Vec<Value>,
    tools: Option<Value>,
    cancel: &mut watch::Receiver<bool>,
    deltas: &tokio::sync::mpsc::UnboundedSender<String>,
    emitted_text: &mut bool,
) -> Result<(String, Value, String), String> {
    if mode == ProviderMode::ClaudeOnly {
        let (response, model) = claude_completion(messages, tools, cancel).await?;
        let text = response["choices"][0]["message"]["content"]
            .as_str()
            .unwrap_or_default();
        if !text.is_empty() {
            *emitted_text = true;
            if !deltas.is_closed() && deltas.send(text.to_string()).is_err() {
                crate::log::line("voice response listener closed while streaming a reply");
            }
        }
        return Ok(("claude".into(), response, model));
    }
    let (provider_name, key_name, endpoint, model) = match mode {
        ProviderMode::GrokOnly => ("grok", "grok-api-key", GROK_BASE, GROK_MODEL),
        ProviderMode::OpenAiOnly => ("openai", "openai-api-key", OPENAI_BASE, OPENAI_MODEL),
        ProviderMode::GeminiOnly => (
            "gemini",
            "gemini-api-key",
            GEMINI_OPENAI_BASE,
            GEMINI_MODEL,
        ),
        _ => return Err("The selected provider mode is not a direct provider.".into()),
    };
    let key = secrets::get(key_name)
        .ok_or_else(|| format!("No {provider_name} API key is configured in the OS credential manager."))?;
    let client = client(Duration::from_secs(60))
        .map_err(|error| format!("{provider_name}: {error}"))?;
    let mut body = compatible_request_body(provider_name, model, messages, tools);
    body["stream"] = json!(true);
    let response = send_json_stream(
        &client,
        format!("{endpoint}/chat/completions"),
        &key,
        provider_name,
        body,
        cancel,
        deltas,
        emitted_text,
    )
    .await
    .map_err(|error| error.message)?;
    let actual_model = response_model(&response, model);
    Ok((provider_name.to_string(), response, actual_model))
}

async fn grok_completion(
    messages: Vec<Value>,
    tools: Option<Value>,
    cancel: &mut watch::Receiver<bool>,
) -> Result<(Value, String), String> {
    compatible_provider_completion(
        "grok",
        "grok-api-key",
        GROK_BASE,
        GROK_MODEL,
        messages,
        tools,
        cancel,
    )
    .await
}

async fn compatible_provider_completion(
    provider: &str,
    key_name: &str,
    endpoint: &str,
    model: &str,
    messages: Vec<Value>,
    tools: Option<Value>,
    cancel: &mut watch::Receiver<bool>,
) -> Result<(Value, String), String> {
    let key = secrets::get(key_name)
        .ok_or_else(|| format!("No {provider} API key is configured in the OS credential manager."))?;
    let client =
        client(Duration::from_secs(60)).map_err(|error| format!("{provider}: {error}"))?;
    let body = compatible_request_body(provider, model, messages, tools);
    let response = send_json(
        &client,
        format!("{endpoint}/chat/completions"),
        &key,
        provider,
        body,
        cancel,
    )
    .await
    .map_err(|error| error.message)?;
    let actual_model = response_model(&response, model);
    Ok((response, actual_model))
}

fn compatible_request_body(
    provider: &str,
    model: &str,
    messages: Vec<Value>,
    tools: Option<Value>,
) -> Value {
    let mut body = request_body(model, messages, tools, Some(OPENROUTER_MAX_TOKENS), false);
    let object = body.as_object_mut().expect("request body is an object");
    object.remove("plugins");
    if matches!(provider, "openai" | "grok") {
        object.remove("max_tokens");
        body["max_completion_tokens"] = json!(OPENROUTER_MAX_TOKENS);
    }
    body
}

async fn claude_completion(
    messages: Vec<Value>,
    tools: Option<Value>,
    cancel: &mut watch::Receiver<bool>,
) -> Result<(Value, String), String> {
    let key = secrets::get("anthropic-api-key")
        .ok_or_else(|| "No Claude API key is configured in the OS credential manager.".to_string())?;
    let client = client(Duration::from_secs(60)).map_err(|error| format!("Claude: {error}"))?;
    let body = claude_request_body(messages, tools)?;
    let response = send_json(
        &client,
        format!("{ANTHROPIC_BASE}/messages"),
        &key,
        "claude",
        body,
        cancel,
    )
    .await
    .map_err(|error| error.message)?;
    let model = response["model"].as_str().unwrap_or(CLAUDE_MODEL).to_string();
    Ok((normalize_claude_response(response)?, model))
}

fn claude_request_body(messages: Vec<Value>, tools: Option<Value>) -> Result<Value, String> {
    let mut system = Vec::new();
    let mut converted = Vec::new();
    for message in messages {
        match message["role"].as_str().unwrap_or_default() {
            "system" => {
                if let Some(text) = message["content"].as_str() {
                    system.push(text.to_string());
                }
            }
            "tool" => {
                let tool_use_id = message["tool_call_id"]
                    .as_str()
                    .ok_or_else(|| "Claude tool result is missing its call ID.".to_string())?;
                converted.push(json!({
                    "role": "user",
                    "content": [{
                        "type": "tool_result",
                        "tool_use_id": tool_use_id,
                        "content": message["content"].as_str().unwrap_or_default(),
                    }]
                }));
            }
            "user" | "assistant" => {
                let role = message["role"].as_str().unwrap_or_default();
                let mut content = anthropic_content_blocks(&message["content"])?;
                if role == "assistant" {
                    if let Some(calls) = message["tool_calls"].as_array() {
                        for call in calls {
                            let function = &call["function"];
                            let input = function["arguments"]
                                .as_str()
                                .and_then(|args| serde_json::from_str::<Value>(args).ok())
                                .unwrap_or_else(|| json!({}));
                            content.push(json!({
                                "type": "tool_use",
                                "id": call["id"],
                                "name": function["name"],
                                "input": input,
                            }));
                        }
                    }
                }
                converted.push(json!({ "role": role, "content": content }));
            }
            _ => return Err("Claude received an unsupported message role.".into()),
        }
    }
    let mut body = json!({
        "model": CLAUDE_MODEL,
        "max_tokens": OPENROUTER_MAX_TOKENS,
        "messages": converted,
    });
    if !system.is_empty() {
        body["system"] = json!(system.join("\n\n"));
    }
    if let Some(tools) = tools {
        let anthropic_tools: Vec<Value> = tools
            .as_array()
            .into_iter()
            .flatten()
            .map(|tool| {
                let function = &tool["function"];
                json!({
                    "name": function["name"],
                    "description": function["description"],
                    "input_schema": function["parameters"],
                })
            })
            .collect();
        if !anthropic_tools.is_empty() {
            body["tools"] = json!(anthropic_tools);
        }
    }
    Ok(body)
}

fn anthropic_content_blocks(content: &Value) -> Result<Vec<Value>, String> {
    if let Some(text) = content.as_str() {
        if text.is_empty() {
            return Ok(Vec::new());
        }
        return Ok(vec![json!({ "type": "text", "text": text })]);
    }
    let Some(blocks) = content.as_array() else {
        return Ok(Vec::new());
    };
    blocks
        .iter()
        .map(|block| match block["type"].as_str() {
            Some("text") => Ok(json!({ "type": "text", "text": block["text"] })),
            Some("image_url") => {
                let url = block["image_url"]["url"]
                    .as_str()
                    .ok_or_else(|| "Claude image input is missing its data URL.".to_string())?;
                let (media_type, data) = parse_data_url(url)?;
                if !matches!(media_type, "image/jpeg" | "image/png" | "image/gif" | "image/webp") {
                    return Err("Claude image input must be a JPEG, PNG, GIF, or WebP image.".into());
                }
                Ok(json!({
                    "type": "image",
                    "source": { "type": "base64", "media_type": media_type, "data": data }
                }))
            }
            Some("file") => {
                let file = &block["file"];
                let data_url = file["file_data"]
                    .as_str()
                    .ok_or_else(|| "Claude document input is missing its data URL.".to_string())?;
                let (media_type, data) = parse_data_url(data_url)?;
                if media_type != "application/pdf" {
                    return Err("Claude document attachments currently support PDF files only.".into());
                }
                Ok(json!({
                    "type": "document",
                    "source": { "type": "base64", "media_type": media_type, "data": data }
                }))
            }
            Some(other) => Err(format!("Claude does not support this message content: {other}.")),
            None => Err("Claude received an invalid message content block.".into()),
        })
        .collect()
}

fn parse_data_url(url: &str) -> Result<(&str, &str), String> {
    let (metadata, data) = url
        .strip_prefix("data:")
        .and_then(|rest| rest.split_once(','))
        .ok_or_else(|| "Expected a base64 data URL.".to_string())?;
    let media_type = metadata
        .strip_suffix(";base64")
        .ok_or_else(|| "Expected a base64-encoded data URL.".to_string())?;
    if media_type.is_empty() || data.is_empty() {
        return Err("The data URL is empty.".into());
    }
    Ok((media_type, data))
}

fn normalize_claude_response(response: Value) -> Result<Value, String> {
    let content = response["content"]
        .as_array()
        .ok_or_else(|| "Claude returned an invalid message.".to_string())?;
    let mut text = String::new();
    let mut tool_calls = Vec::new();
    for block in content {
        match block["type"].as_str() {
            Some("text") => {
                if let Some(value) = block["text"].as_str() {
                    text.push_str(value);
                }
            }
            Some("tool_use") => {
                let arguments = serde_json::to_string(&block["input"])
                    .map_err(|error| format!("Could not decode Claude tool arguments: {error}"))?;
                tool_calls.push(json!({
                    "id": block["id"],
                    "type": "function",
                    "function": { "name": block["name"], "arguments": arguments }
                }));
            }
            Some("thinking" | "redacted_thinking") | None => {}
            Some(other) => return Err(format!("Claude returned unsupported content: {other}.")),
        }
    }
    let finish_reason = if tool_calls.is_empty() { "stop" } else { "tool_calls" };
    Ok(json!({
        "model": response["model"],
        "choices": [{
            "message": {
                "role": "assistant",
                "content": if text.is_empty() { Value::Null } else { json!(text) },
                "tool_calls": tool_calls,
            },
            "finish_reason": finish_reason,
        }]
    }))
}

fn grok_request_body(messages: Vec<Value>, tools: Option<Value>) -> Value {
    compatible_request_body("grok", GROK_MODEL, messages, tools)
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
    } else if provider == "openrouter" {
        request = request
            .bearer_auth(credential)
            .header("HTTP-Referer", "https://github.com/dev-praj/Macha_by_Prajwal.git")
            .header("X-Title", "Macha");
    } else if provider == "claude" {
        request = request
            .header("x-api-key", credential)
            .header("anthropic-version", "2023-06-01");
    } else {
        request = request.bearer_auth(credential);
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
                match provider {
                    "ollama" => "Ollama",
                    "grok" => "Grok",
                    "claude" => "Claude",
                    "openai" => "OpenAI",
                    "gemini" => "Gemini",
                    _ => "OpenRouter",
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
    } else if provider == "openrouter" {
        request = request
            .bearer_auth(credential)
            .header("HTTP-Referer", "https://github.com/dev-praj/Macha_by_Prajwal.git")
            .header("X-Title", "Macha");
    } else if provider == "claude" {
        request = request
            .header("x-api-key", credential)
            .header("anthropic-version", "2023-06-01");
    } else {
        request = request.bearer_auth(credential);
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
        affordable_tokens, anthropic_content_blocks, append_stream_event, claude_request_body,
        compatible_request_body, grok_request_body, normalize_claude_response,
        ollama_failure_allows_fallback, openrouter_model, reduced_max_tokens, request_body,
        send_json, send_openrouter_with_retry_at, ProviderMode,
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
        assert_eq!(ProviderMode::parse("grokOnly"), ProviderMode::GrokOnly);
        assert_eq!(ProviderMode::parse("claudeOnly"), ProviderMode::ClaudeOnly);
        assert_eq!(ProviderMode::parse("openAiOnly"), ProviderMode::OpenAiOnly);
        assert_eq!(ProviderMode::parse("geminiOnly"), ProviderMode::GeminiOnly);
        assert_eq!(ProviderMode::parse("voiceAuto"), ProviderMode::VoiceAuto);
        assert_eq!(ProviderMode::parse("invalid"), ProviderMode::Auto);
    }

    #[test]
    fn grok_request_body_uses_xai_model_and_openai_tool_format_without_openrouter_plugins() {
        let tools = serde_json::json!([{
            "type": "function",
            "function": { "name": "test_tool", "parameters": { "type": "object" } }
        }]);
        let body = grok_request_body(
            vec![serde_json::json!({"role":"user","content":"hello"})],
            Some(tools.clone()),
        );

        assert_eq!(body["model"], "grok-4.7");
        assert_eq!(body["tools"], tools);
        assert!(body.get("plugins").is_none());
        assert!(body.get("max_tokens").is_none());
        assert_eq!(body["max_completion_tokens"], 1024);
        assert_eq!(body["parallel_tool_calls"], false);
    }

    #[test]
    fn direct_openai_compatible_providers_use_supported_models_and_no_openrouter_fields() {
        let messages = vec![serde_json::json!({"role":"user","content":"hello"})];
        let openai = compatible_request_body("openai", "gpt-4.1-mini", messages.clone(), None);
        assert_eq!(openai["model"], "gpt-4.1-mini");
        assert_eq!(openai["max_completion_tokens"], 1024);
        assert!(openai.get("max_tokens").is_none());
        assert!(openai.get("plugins").is_none());

        let gemini = compatible_request_body("gemini", "gemini-3.8-flash", messages, None);
        assert_eq!(gemini["model"], "gemini-3.8-flash");
        assert_eq!(gemini["max_tokens"], 1024);
        assert!(gemini.get("plugins").is_none());
    }

    #[test]
    fn claude_request_translates_system_tools_and_tool_results() {
        let messages = vec![
            serde_json::json!({"role":"system","content":"Be helpful."}),
            serde_json::json!({
                "role":"assistant",
                "content":"",
                "tool_calls":[{
                    "id":"call-1",
                    "type":"function",
                    "function":{"name":"open_website","arguments":"{\"url\":\"https://example.com\"}"}
                }]
            }),
            serde_json::json!({
                "role":"tool",
                "tool_call_id":"call-1",
                "content":"Opened."
            }),
        ];
        let tools = serde_json::json!([{
            "type":"function",
            "function":{
                "name":"open_website",
                "description":"Open a website.",
                "parameters":{"type":"object","properties":{"url":{"type":"string"}}}
            }
        }]);
        let body = claude_request_body(messages, Some(tools)).unwrap();

        assert_eq!(body["model"], "claude-sonnet-5-5");
        assert_eq!(body["system"], "Be helpful.");
        assert_eq!(body["tools"][0]["name"], "open_website");
        assert_eq!(body["tools"][0]["input_schema"]["type"], "object");
        assert_eq!(body["messages"][0]["content"][0]["type"], "tool_use");
        assert_eq!(body["messages"][0]["content"][0]["input"]["url"], "https://example.com");
        assert_eq!(body["messages"][1]["content"][0]["type"], "tool_result");
        assert_eq!(body["messages"][1]["content"][0]["tool_use_id"], "call-1");
    }

    #[test]
    fn claude_response_translates_text_and_tool_use_to_chat_completion_shape() {
        let response = normalize_claude_response(serde_json::json!({
            "model":"claude-sonnet-5-5",
            "content":[
                {"type":"text","text":"Opening site."},
                {"type":"tool_use","id":"tool-1","name":"open_website","input":{"url":"https://example.com"}}
            ],
            "stop_reason":"tool_use"
        })).unwrap();
        assert_eq!(response["choices"][0]["message"]["content"], "Opening site.");
        assert_eq!(response["choices"][0]["message"]["tool_calls"][0]["id"], "tool-1");
        assert_eq!(
            serde_json::from_str::<Value>(
                response["choices"][0]["message"]["tool_calls"][0]["function"]["arguments"]
                    .as_str()
                    .unwrap()
            )
            .unwrap()["url"],
            "https://example.com"
        );
        assert_eq!(response["choices"][0]["finish_reason"], "tool_calls");
    }

    #[test]
    fn claude_image_content_is_translated_to_anthropic_base64_source() {
        let data = anthropic_content_blocks(&serde_json::json!([{
            "type":"image_url",
            "image_url":{"url":"data:image/png;base64,aGVsbG8="}
        }])).unwrap();
        assert_eq!(data[0]["type"], "image");
        assert_eq!(data[0]["source"]["media_type"], "image/png");
        assert_eq!(data[0]["source"]["data"], "aGVsbG8=");
    }

    #[tokio::test]
    async fn grok_http_requests_use_bearer_auth_and_decode_chat_completions() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let endpoint = format!("http://{}/chat/completions", listener.local_addr().unwrap());
        let server = tokio::spawn(async move {
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
            let headers = String::from_utf8_lossy(&bytes[..body_offset - 4]).to_ascii_lowercase();
            let body: Value = serde_json::from_slice(
                &bytes[body_offset..body_offset + content_length],
            )
            .unwrap();
            let payload = r#"{"model":"grok-4.7","choices":[{"message":{"role":"assistant","content":"hello","tool_calls":[]},"finish_reason":"stop"}]}"#;
            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                payload.len(),
                payload
            );
            stream.write_all(response.as_bytes()).await.unwrap();
            (headers, body)
        });

        let client = Client::builder().build().unwrap();
        let (sender, mut cancel) = watch::channel(false);
        let response = send_json(
            &client,
            endpoint,
            "test-key",
            "grok",
            grok_request_body(vec![], None),
            &mut cancel,
        )
        .await
        .unwrap();
        drop(sender);

        let (headers, body) = server.await.unwrap();
        assert!(headers.contains("authorization: bearer test-key"));
        assert_eq!(body["model"], "grok-4.7");
        assert_eq!(response["choices"][0]["message"]["content"], "hello");
    }

    #[tokio::test]
    async fn claude_http_requests_use_anthropic_headers() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let endpoint = format!("http://{}/messages", listener.local_addr().unwrap());
        let server = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut bytes = Vec::new();
            let mut chunk = [0; 1024];
            let body_offset = loop {
                let count = stream.read(&mut chunk).await.unwrap();
                assert!(count > 0, "client closed before sending HTTP headers");
                bytes.extend_from_slice(&chunk[..count]);
                if let Some(offset) = bytes.windows(4).position(|part| part == b"\r\n\r\n") {
                    break offset + 4;
                }
            };
            let headers = String::from_utf8_lossy(&bytes[..body_offset - 4]).to_ascii_lowercase();
            let payload = r#"{"model":"claude-sonnet-5-5","content":[{"type":"text","text":"hello"}],"stop_reason":"end_turn"}"#;
            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                payload.len(),
                payload
            );
            stream.write_all(response.as_bytes()).await.unwrap();
            headers
        });

        let client = Client::builder().build().unwrap();
        let (sender, mut cancel) = watch::channel(false);
        let response = send_json(
            &client,
            endpoint,
            "test-key",
            "claude",
            serde_json::json!({"model":"claude-sonnet-5-5","max_tokens":1024,"messages":[]}),
            &mut cancel,
        )
        .await
        .unwrap();
        drop(sender);

        let headers = server.await.unwrap();
        assert!(headers.contains("x-api-key: test-key"));
        assert!(headers.contains("anthropic-version: 2023-06-01"));
        assert!(!headers.contains("authorization:"));
        assert_eq!(response["model"], "claude-sonnet-5-5");
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
