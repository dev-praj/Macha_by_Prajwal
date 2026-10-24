// The phone uses Macha's configured provider clients but receives no computer-action tools.
// Credentials remain in the OS credential manager and never enter this module's settings.
//
// Failures are sorted into a few kinds here and only the kind goes on: a provider's message may
// contain part of a key, so it is neither forwarded nor logged.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use serde_json::{json, Value};
use tauri::{AppHandle, Manager};
use tokio::sync::mpsc::UnboundedSender;

use super::chat::{BoxFuture, ChatBackend, ChatConfig, ChatFail, ModelOption};
use crate::settings::Settings;
use crate::{llm_client, log, secrets, Shared};

/// Most models the user can allow, and the longest model name taken.
pub const MAX_ALLOWED: usize = 50;
const MAX_MODEL_CHARS: usize = 200;

const PROVIDERS: [&str; 6] = ["anthropic", "google", "grok", "openai", "openrouter", "ollama"];
const HISTORY_MESSAGES: usize = 20;
const SYSTEM_PROMPT: &str = "You are Macha, a helpful AI assistant. Answer the user's message clearly and directly. Do not claim to take actions on the computer.";

#[derive(Default)]
struct Conversation {
    generation: u64,
    messages: Vec<Value>,
}

pub struct AppChat {
    app: AppHandle,
    conversations: Arc<Mutex<HashMap<u64, Conversation>>>,
    sequence: AtomicU64,
}

impl AppChat {
    pub fn new(app: AppHandle) -> Self {
        Self { app, conversations: Arc::new(Mutex::new(HashMap::new())), sequence: AtomicU64::new(0) }
    }

    fn settings(&self) -> Settings {
        self.app.state::<Shared>().settings.lock().unwrap().clone()
    }
}

impl ChatBackend for AppChat {
    fn config(&self) -> ChatConfig {
        let s = self.settings();
        ChatConfig { enabled: s.phone_chat, allowed: options(&s.phone_chat_models) }
    }

    fn send(&self, conn: u64, model_id: &str, text: String, deltas: UnboundedSender<String>) -> BoxFuture<Result<String, ChatFail>> {
        let settings = self.settings();
        let conversations = self.conversations.clone();
        let generation = self.sequence.fetch_add(1, Ordering::Relaxed).wrapping_add(1);
        let history = {
            let mut conversations = self.conversations.lock().unwrap();
            let conversation = conversations.entry(conn).or_default();
            conversation.generation = generation;
            conversation.messages.clone()
        };
        let id = model_id.to_string();
        Box::pin(async move {
            let Some((provider, model)) = parse_model_id(&id) else { return Err(ChatFail::Internal) };
            if !settings.phone_chat || !clean_models(&settings.phone_chat_models).contains(&id) {
                return Err(ChatFail::Internal);
            }
            if let Some(key) = key_name(provider) {
                if secrets::get(key).is_none() {
                    return Err(ChatFail::NoKey);
                }
            }
            let mut messages = vec![json!({ "role": "system", "content": SYSTEM_PROMPT })];
            messages.extend(history.iter().cloned());
            messages.push(json!({ "role": "user", "content": text.clone() }));
            let (cancel_tx, mut cancel_rx) = tokio::sync::watch::channel(false);
            let result = llm_client::phone_chat(provider, model, messages, &mut cancel_rx).await;
            drop(cancel_tx);
            match result {
                Ok(answer) => {
                    let mut conversations = conversations.lock().unwrap();
                    let Some(conversation) = conversations.get_mut(&conn) else {
                        return Err(ChatFail::Internal);
                    };
                    if conversation.generation != generation {
                        return Err(ChatFail::Internal);
                    }
                    let mut history = history;
                    history.push(json!({ "role": "user", "content": text }));
                    history.push(json!({ "role": "assistant", "content": answer.clone() }));
                    if history.len() > HISTORY_MESSAGES {
                        history.drain(..history.len() - HISTORY_MESSAGES);
                    }
                    conversation.messages = history;
                    let _ = deltas.send(answer.clone());
                    Ok(answer)
                }
                Err(err) => {
                    let kind = classify(is_local(provider), &err);
                    // The kind only: the text may echo a key.
                    log::line(format!("phone link: chat via {provider} failed ({kind:?})"));
                    Err(kind)
                }
            }
        })
    }

    fn reset(&self, conn: u64) {
        self.conversations.lock().unwrap().remove(&conn);
    }

    fn reset_all(&self) {
        self.conversations.lock().unwrap().clear();
    }
}

// ── Pure helpers (tested below) ───────────────────────────────────────────────

fn is_local(provider: &str) -> bool {
    provider == "ollama"
}

/// The credential store entry a provider's key lives in; None for the model servers.
fn key_name(provider: &str) -> Option<&'static str> {
    match provider {
        "anthropic" => Some("anthropic-api-key"),
        "google" => Some("gemini-api-key"),
        "grok" => Some("grok-api-key"),
        "openai" => Some("openai-api-key"),
        _ => None,
    }
}

pub fn provider_name(provider: &str) -> String {
    match provider {
        "anthropic" => "Anthropic".into(),
        "google" => "Google".into(),
        "grok" => "Grok".into(),
        "openai" => "OpenAI".into(),
        "openrouter" => "OpenRouter".into(),
        "ollama" => "Ollama".into(),
        _ => "AI provider".into(),
    }
}

/// "provider/model" with a provider this build knows and a plain model name; the model may contain
/// slashes ("openrouter/auto" is the model of the "openrouter" provider written "openrouter/openrouter/auto").
pub fn parse_model_id(id: &str) -> Option<(&str, &str)> {
    let (provider, model) = id.split_once('/')?;
    let model = model.trim();
    if !PROVIDERS.contains(&provider) || model.is_empty() || model.chars().count() > MAX_MODEL_CHARS || model.chars().any(char::is_control) {
        return None;
    }
    Some((provider, model))
}

/// The user's list, made safe: valid ids only, no duplicates, at most MAX_ALLOWED.
pub fn clean_models(ids: &[String]) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for id in ids {
        if parse_model_id(id).is_some() && !out.contains(id) && out.len() < MAX_ALLOWED {
            out.push(id.clone());
        }
    }
    out
}

pub fn validate_models(ids: &[String]) -> Result<Vec<String>, String> {
    if ids.len() > MAX_ALLOWED {
        return Err(format!("Choose no more than {MAX_ALLOWED} phone chat models."));
    }
    let mut out = Vec::new();
    for id in ids {
        if parse_model_id(id).is_none() {
            return Err(format!("Invalid phone chat model {id:?}. Use a supported provider/model ID."));
        }
        if !out.contains(id) {
            out.push(id.clone());
        }
    }
    Ok(out)
}

pub fn options(ids: &[String]) -> Vec<ModelOption> {
    clean_models(ids)
        .into_iter()
        .filter_map(|id| {
            let (provider, model) = parse_model_id(&id)?;
            let label = format!("{} · {}", provider_name(provider), model);
            Some(ModelOption { provider: provider.to_string(), label, id })
        })
        .collect()
}

/// Provider errors are reduced to safe categories before any message reaches the phone.
pub fn classify(local: bool, err: &str) -> ChatFail {
    if err.starts_with("No ") && err.contains(" API key is configured") {
        return ChatFail::NoKey;
    }
    let has = |code: &str| err.split(|c: char| !c.is_ascii_digit()).any(|w| w == code);
    if has("401") || has("403") {
        return ChatFail::Auth;
    }
    let has_status = err.split(|c: char| !c.is_ascii_digit()).any(|w| w.len() == 3 && w.starts_with(['4', '5']));
    if local && !has_status {
        return ChatFail::Unreachable;
    }
    if err.starts_with("Network error") || err.contains("connection failed") {
        return ChatFail::Unreachable;
    }
    ChatFail::Provider
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ids(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn model_ids_need_a_known_provider_and_a_plain_model() {
        assert_eq!(parse_model_id("openai/gpt-4o"), Some(("openai", "gpt-4o")));
        assert_eq!(parse_model_id("openrouter/openrouter/auto"), Some(("openrouter", "openrouter/auto")));
        assert_eq!(parse_model_id("ollama/llama3.2:latest"), Some(("ollama", "llama3.2:latest")));
        for bad in ["", "openai", "openai/", "openai/ ", "nope/gpt", "OpenAI/gpt", "openai/a\nb", &format!("openai/{}", "x".repeat(201))] {
            assert_eq!(parse_model_id(bad), None, "{bad:?}");
        }
    }

    #[test]
    fn the_allowed_list_is_cleaned_and_capped() {
        assert_eq!(clean_models(&ids(&["openai/gpt-4o", "bad", "openai/gpt-4o", "google/gemini"])), ids(&["openai/gpt-4o", "google/gemini"]));
        let many: Vec<String> = (0..80).map(|i| format!("openai/m{i}")).collect();
        assert_eq!(clean_models(&many).len(), MAX_ALLOWED);
        assert!(clean_models(&[]).is_empty());
    }

    #[test]
    fn phone_chat_model_settings_reject_invalid_ids_and_deduplicate() {
        assert_eq!(
            validate_models(&ids(&["openai/gpt-4o", "openai/gpt-4o", "ollama/qwen2.5"])).unwrap(),
            ids(&["openai/gpt-4o", "ollama/qwen2.5"]),
        );
        assert!(validate_models(&ids(&["not-a-provider/model"])).is_err());
        assert!(validate_models(&(0..=MAX_ALLOWED).map(|i| format!("openai/m{i}")).collect::<Vec<_>>()).is_err());
    }

    #[test]
    fn what_the_phone_is_told_about_a_model_is_its_id_provider_and_label_only() {
        let o = options(&ids(&["anthropic/claude-x", "ollama/qwen"]));
        assert_eq!(o[0], ModelOption { id: "anthropic/claude-x".into(), provider: "anthropic".into(), label: "Anthropic · claude-x".into() });
        assert_eq!(o[1].label, "Ollama · qwen");
    }

    #[test]
    fn failures_are_sorted_by_status_and_never_by_wording() {
        assert_eq!(classify(false, "Claude API 401 Unauthorized: invalid x-api-key"), ChatFail::Auth);
        assert_eq!(classify(false, "OpenAI rejected the API key (403). Check it in Settings."), ChatFail::Auth);
        assert_eq!(classify(false, "OpenAI rate limit reached (429): slow down"), ChatFail::Provider);
        assert_eq!(classify(false, "Claude API 500 Internal Server Error: boom"), ChatFail::Provider);
        assert_eq!(classify(false, "Network error: connection refused"), ChatFail::Unreachable);
        assert_eq!(classify(true, "Cannot reach http://127.0.0.1:11434. Is the server running?"), ChatFail::Unreachable);
        assert_eq!(classify(true, "Ollama 500: model crashed"), ChatFail::Provider);
        // 4010 or 14012 are not 401
        assert_eq!(classify(false, "something 14012 happened"), ChatFail::Provider);
    }

    #[test]
    fn keys_are_found_by_provider() {
        assert_eq!(key_name("anthropic"), Some("anthropic-api-key"));
        assert_eq!(key_name("openai"), Some("openai-api-key"));
        assert_eq!(key_name("ollama"), None);
        assert!(is_local("ollama") && !is_local("openai"));
    }
}
