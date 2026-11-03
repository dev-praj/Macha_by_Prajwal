//! Thin inference boundary shared by local and hosted providers.
//!
//! Existing Ollama and OpenRouter implementations remain in llm_client.rs and
//! are called unchanged by their adapters. Copilot crosses the existing native
//! to Node-service boundary; its GitHub token never enters this process.

use serde_json::{json, Value};
use tokio::sync::{mpsc, watch};

use crate::llm_client::ProviderError;

/// Provider ordering for automatic selection. Callers pass whether the
/// authenticated Copilot service reports at least one usable account.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ProviderKind {
    Copilot,
    OpenRouter,
    Ollama,
}

pub(crate) fn automatic_provider_order(copilot_available: bool) -> &'static [ProviderKind] {
    if copilot_available {
        &[
            ProviderKind::Copilot,
            ProviderKind::OpenRouter,
            ProviderKind::Ollama,
        ]
    } else {
        // Preserve the established Auto order when the user has no usable
        // Copilot account.
        &[ProviderKind::Ollama, ProviderKind::OpenRouter]
    }
}

pub(crate) fn active_copilot_accounts(accounts: &Value) -> Vec<String> {
    let items = accounts
        .as_array()
        .or_else(|| accounts.get("accounts").and_then(Value::as_array));
    let Some(items) = items else {
        return Vec::new();
    };
    items
        .iter()
        .filter_map(|item| {
        if item.get("status").and_then(Value::as_str) == Some("connected")
            && item.get("subscriptionStatus").and_then(Value::as_str) == Some("active")
        {
            item.get("id").and_then(Value::as_str).map(str::to_owned)
        } else {
            None
        }
        })
        .collect()
}

#[derive(Clone)]
pub(crate) struct CompletionRequest {
    pub messages: Vec<Value>,
    pub tools: Option<Value>,
    pub model: String,
}

pub(crate) struct ProviderResponse {
    pub response: Value,
    pub model: String,
}

#[derive(Clone, Debug)]
pub(crate) struct ProviderHealth {
    pub connected: bool,
    pub available: bool,
    pub message: Option<String>,
}

pub(crate) trait AiProvider {
    async fn complete(
        &self,
        request: CompletionRequest,
        cancel: &mut watch::Receiver<bool>,
    ) -> Result<ProviderResponse, ProviderError>;

    async fn stream(
        &self,
        request: CompletionRequest,
        cancel: &mut watch::Receiver<bool>,
        deltas: &mpsc::UnboundedSender<String>,
        emitted_text: &mut bool,
    ) -> Result<ProviderResponse, ProviderError>;

    async fn run_agent(
        &self,
        request: CompletionRequest,
        cancel: &mut watch::Receiver<bool>,
    ) -> Result<ProviderResponse, ProviderError> {
        self.complete(request, cancel).await
    }

    async fn tool_call(
        &self,
        mut request: CompletionRequest,
        tools: Value,
        cancel: &mut watch::Receiver<bool>,
    ) -> Result<ProviderResponse, ProviderError> {
        request.tools = Some(tools);
        self.complete(request, cancel).await
    }

    async fn health(&self) -> ProviderHealth;
}

pub(crate) struct OllamaProvider {
    pub wait_for_start: bool,
    pub response_timeout: Option<std::time::Duration>,
    pub max_tokens: Option<u32>,
}

impl AiProvider for OllamaProvider {
    async fn complete(
        &self,
        request: CompletionRequest,
        cancel: &mut watch::Receiver<bool>,
    ) -> Result<ProviderResponse, ProviderError> {
        let response = crate::llm_client::ollama_completion_existing(
            request.messages,
            request.tools,
            &request.model,
            self.wait_for_start,
            cancel,
            self.response_timeout,
            self.max_tokens,
        ).await?;
        Ok(ProviderResponse { response, model: request.model })
    }

    async fn stream(
        &self,
        request: CompletionRequest,
        cancel: &mut watch::Receiver<bool>,
        deltas: &mpsc::UnboundedSender<String>,
        emitted_text: &mut bool,
    ) -> Result<ProviderResponse, ProviderError> {
        let response = crate::llm_client::ollama_completion_stream_existing(
            request.messages,
            request.tools,
            &request.model,
            self.wait_for_start,
            cancel,
            self.response_timeout,
            self.max_tokens,
            deltas,
            emitted_text,
        ).await?;
        Ok(ProviderResponse { response, model: request.model })
    }

    async fn health(&self) -> ProviderHealth {
        let status = crate::llm_client::ollama_status(false).await;
        ProviderHealth { connected: true, available: status.reachable, message: status.error }
    }
}

pub(crate) struct OpenRouterProvider {
    pub response_timeout: std::time::Duration,
    pub max_tokens: u32,
}

impl AiProvider for OpenRouterProvider {
    async fn complete(
        &self,
        request: CompletionRequest,
        cancel: &mut watch::Receiver<bool>,
    ) -> Result<ProviderResponse, ProviderError> {
        let (response, model) = crate::llm_client::openrouter_completion_existing(
            request.messages,
            request.tools,
            &request.model,
            cancel,
            self.response_timeout,
            self.max_tokens,
        ).await.map_err(|message| ProviderError::local(message, false))?;
        Ok(ProviderResponse { response, model })
    }

    async fn stream(
        &self,
        request: CompletionRequest,
        cancel: &mut watch::Receiver<bool>,
        deltas: &mpsc::UnboundedSender<String>,
        emitted_text: &mut bool,
    ) -> Result<ProviderResponse, ProviderError> {
        let (response, model) = crate::llm_client::openrouter_completion_stream_existing(
            request.messages,
            request.tools,
            &request.model,
            cancel,
            self.response_timeout,
            self.max_tokens,
            deltas,
            emitted_text,
        ).await.map_err(|message| ProviderError::local(message, false))?;
        Ok(ProviderResponse { response, model })
    }

    async fn health(&self) -> ProviderHealth {
        match crate::secrets::openrouter_accounts() {
            Ok(accounts) => ProviderHealth {
                connected: !accounts.is_empty(),
                available: !accounts.is_empty(),
                message: None,
            },
            Err(error) => ProviderHealth { connected: false, available: false, message: Some(error) },
        }
    }
}

pub(crate) struct CopilotProvider {
    pub account_id: String,
}

impl CopilotProvider {
    fn request_body(&self, request: CompletionRequest) -> Value {
        json!({
            "accountId": self.account_id,
            // This UI setting contains OpenRouter model IDs. Let Copilot select
            // the account's configured SDK default instead of passing an
            // unrelated provider's model slug.
            "model": "default",
            "messages": request.messages,
            "tools": request.tools,
        })
    }
}

impl AiProvider for CopilotProvider {
    async fn complete(
        &self,
        request: CompletionRequest,
        cancel: &mut watch::Receiver<bool>,
    ) -> Result<ProviderResponse, ProviderError> {
        let model = request.model.clone();
        let body = self.request_body(request);
        let response = crate::remote_mode::copilot_provider_completion(&body, cancel).await
            .map_err(|message| ProviderError::local(message, false))?;
        Ok(ProviderResponse { response, model })
    }

    async fn stream(
        &self,
        request: CompletionRequest,
        cancel: &mut watch::Receiver<bool>,
        deltas: &mpsc::UnboundedSender<String>,
        emitted_text: &mut bool,
    ) -> Result<ProviderResponse, ProviderError> {
        let model = request.model.clone();
        let body = self.request_body(request);
        let response = crate::remote_mode::copilot_provider_stream(&body, cancel, deltas, emitted_text).await
            .map_err(|message| ProviderError::local(message, false))?;
        Ok(ProviderResponse { response, model })
    }

    async fn health(&self) -> ProviderHealth {
        match crate::remote_mode::copilot_provider_status().await {
            Ok(accounts) => {
                let available = active_copilot_accounts(&accounts)
                    .iter()
                    .any(|id| id == &self.account_id);
                ProviderHealth { connected: true, available, message: None }
            }
            Err(error) => ProviderHealth { connected: false, available: false, message: Some(error) },
        }
    }
}

#[cfg(test)]
mod selection_tests {
    use super::{
        active_copilot_accounts, automatic_provider_order, ProviderKind,
    };
    use serde_json::json;

    #[test]
    fn automatic_order_prefers_copilot_when_a_usable_account_exists() {
        assert_eq!(
            automatic_provider_order(true),
            &[
                ProviderKind::Copilot,
                ProviderKind::OpenRouter,
                ProviderKind::Ollama,
            ]
        );
    }

    #[test]
    fn automatic_order_keeps_legacy_order_without_copilot() {
        assert_eq!(
            automatic_provider_order(false),
            &[ProviderKind::Ollama, ProviderKind::OpenRouter]
        );
    }

    #[test]
    fn only_connected_active_copilot_accounts_are_usable() {
        assert_eq!(active_copilot_accounts(&json!({ "accounts": [
            { "id": "a", "status": "connected", "subscriptionStatus": "active" }
        ] })), vec!["a"]);
        assert_eq!(active_copilot_accounts(&json!({ "accounts": [
            { "id": "a", "status": "connected", "subscriptionStatus": "active" },
            { "id": "b", "status": "connected", "subscriptionStatus": "active" }
        ] })), vec!["a", "b"]);
        assert_eq!(active_copilot_accounts(&json!([
            { "status": "needs_reconnect", "subscriptionStatus": "active" },
            { "status": "connected", "subscriptionStatus": "inactive" }
        ])), Vec::<String>::new());
        assert_eq!(active_copilot_accounts(&json!([])), Vec::<String>::new());
    }
}
