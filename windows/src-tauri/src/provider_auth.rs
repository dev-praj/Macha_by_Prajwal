use reqwest::header::{HeaderMap, HeaderValue, AUTHORIZATION};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;

use crate::{secrets, settings};

const FEATURES_FILE: &str = "provider-auth.json";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Provider {
    OpenRouter,
    Grok,
    Anthropic,
    OpenAi,
    Gemini,
    Ollama,
}

impl Provider {
    pub const ALL: [Self; 6] = [
        Self::OpenRouter,
        Self::Grok,
        Self::Anthropic,
        Self::OpenAi,
        Self::Gemini,
        Self::Ollama,
    ];

    pub fn from_id(id: &str) -> Option<Self> {
        match id {
            "openrouter" => Some(Self::OpenRouter),
            "grok" => Some(Self::Grok),
            "anthropic" => Some(Self::Anthropic),
            "openai" => Some(Self::OpenAi),
            "gemini" | "google" => Some(Self::Gemini),
            "ollama" => Some(Self::Ollama),
            _ => None,
        }
    }

    pub fn id(self) -> &'static str {
        match self {
            Self::OpenRouter => "openrouter",
            Self::Grok => "grok",
            Self::Anthropic => "anthropic",
            Self::OpenAi => "openai",
            Self::Gemini => "gemini",
            Self::Ollama => "ollama",
        }
    }

    fn key_name(self) -> Option<&'static str> {
        match self {
            Self::OpenRouter => None,
            Self::Grok => Some("grok-api-key"),
            Self::Anthropic => Some("anthropic-api-key"),
            Self::OpenAi => Some("openai-api-key"),
            Self::Gemini => Some("gemini-api-key"),
            Self::Ollama => None,
        }
    }

    fn default_enabled(self, mode: AuthMode) -> bool {
        matches!(
            (self, mode),
            (Self::OpenRouter, AuthMode::ApiKey)
                | (Self::OpenRouter, AuthMode::OAuth)
                | (Self::Grok, AuthMode::ApiKey)
                | (Self::Anthropic, AuthMode::ApiKey)
                | (Self::OpenAi, AuthMode::ApiKey)
                | (Self::Gemini, AuthMode::ApiKey)
                | (Self::Ollama, AuthMode::LocalCli)
        )
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AuthMode {
    ApiKey,
    OAuth,
    LocalCli,
}

impl AuthMode {
    fn id(self) -> &'static str {
        match self {
            Self::ApiKey => "api_key",
            Self::OAuth => "oauth",
            Self::LocalCli => "local_cli",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "api_key" => Some(Self::ApiKey),
            "oauth" => Some(Self::OAuth),
            "local_cli" => Some(Self::LocalCli),
            _ => None,
        }
    }
}

#[derive(Debug, Clone)]
pub struct ProviderConnection {
    pub provider: Provider,
    pub mode: AuthMode,
    pub account_id: Option<String>,
}

impl ProviderConnection {
    pub fn new(provider: Provider, mode: AuthMode) -> Self {
        Self {
            provider,
            mode,
            account_id: None,
        }
    }

    pub fn with_account_id(mut self, account_id: impl Into<String>) -> Self {
        self.account_id = Some(account_id.into());
        self
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderModeFeature {
    pub provider: String,
    pub mode: String,
    pub enabled: bool,
}

#[derive(Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ProviderAuthSettings {
    #[serde(default)]
    provider_modes: HashMap<String, bool>,
    #[serde(default)]
    api_key_fallback: bool,
}

pub trait ProviderAdapter {
    fn supported_modes(&self) -> &'static [AuthMode];
    fn get_auth_headers(&self, connection: &ProviderConnection) -> Result<HeaderMap, String>;
    fn refresh(&self, connection: &ProviderConnection) -> Result<(), String>;
    fn validate(&self, connection: &ProviderConnection) -> Result<(), String>;
    fn revoke(&self, connection: &ProviderConnection) -> Result<(), String>;
}

const API_KEY_MODES: &[AuthMode] = &[AuthMode::ApiKey];
const LOCAL_CLI_MODES: &[AuthMode] = &[AuthMode::LocalCli];

impl ProviderAdapter for Provider {
    fn supported_modes(&self) -> &'static [AuthMode] {
        match self {
            Self::OpenRouter => &[AuthMode::ApiKey, AuthMode::OAuth],
            Self::Grok | Self::Anthropic | Self::OpenAi | Self::Gemini => API_KEY_MODES,
            Self::Ollama => LOCAL_CLI_MODES,
        }
    }

    fn get_auth_headers(&self, connection: &ProviderConnection) -> Result<HeaderMap, String> {
        ensure_supported(self, connection)?;
        if !is_mode_enabled(connection.provider, connection.mode)? {
            return Err(format!(
                "{} {} authentication is disabled by the local feature flag.",
                connection.provider.id(),
                connection.mode.id()
            ));
        }

        match connection.mode {
            AuthMode::ApiKey | AuthMode::OAuth => {
                let credential = match self {
                    Self::OpenRouter => {
                        let id = connection.account_id.as_deref().ok_or_else(|| {
                            "OpenRouter account ID is required for API-key authentication."
                                .to_string()
                        })?;
                        secrets::openrouter_key_for_account(id).map_err(|_| {
                            "OpenRouter could not read the configured credential.".to_string()
                        })?
                    }
                    _ => {
                        let key_name = self.key_name().expect("API-key adapter has a key name");
                        secrets::get(key_name).ok_or_else(|| {
                            format!(
                                "No {} API key is configured in the OS credential manager.",
                                self.key_error_name()
                            )
                        })?
                    }
                };
                api_key_headers(*self, &credential)
            }
            AuthMode::LocalCli => {
                let mut headers = HeaderMap::new();
                headers.insert(AUTHORIZATION, HeaderValue::from_static("Bearer ollama"));
                Ok(headers)
            }
        }
    }

    fn refresh(&self, connection: &ProviderConnection) -> Result<(), String> {
        ensure_supported(self, connection)?;
        match connection.mode {
            AuthMode::ApiKey | AuthMode::OAuth | AuthMode::LocalCli => Ok(()),
        }
    }

    fn validate(&self, connection: &ProviderConnection) -> Result<(), String> {
        self.get_auth_headers(connection).map(|_| ())
    }

    fn revoke(&self, connection: &ProviderConnection) -> Result<(), String> {
        ensure_supported(self, connection)?;
        match connection.mode {
            AuthMode::ApiKey | AuthMode::OAuth => match self {
                Self::OpenRouter => {
                    let id = connection.account_id.as_deref().ok_or_else(|| {
                        "OpenRouter account ID is required to disconnect.".to_string()
                    })?;
                    secrets::remove_openrouter_account(id)
                }
                _ => secrets::clear(self.key_name().expect("API-key adapter has a key name")),
            },
            AuthMode::LocalCli => Ok(()),
        }
    }
}

impl Provider {
    fn display_name(self) -> &'static str {
        match self {
            Self::OpenRouter => "OpenRouter",
            Self::Grok => "Grok",
            Self::Anthropic => "Claude",
            Self::OpenAi => "OpenAI",
            Self::Gemini => "Gemini",
            Self::Ollama => "Ollama",
        }
    }

    fn key_error_name(self) -> &'static str {
        match self {
            Self::OpenRouter => "OpenRouter",
            Self::Grok => "grok",
            Self::Anthropic => "Claude",
            Self::OpenAi => "openai",
            Self::Gemini => "gemini",
            Self::Ollama => "Ollama",
        }
    }
}

pub fn auth_headers(connection: &ProviderConnection) -> Result<HeaderMap, String> {
    connection.provider.refresh(connection)?;
    connection.provider.get_auth_headers(connection)
}

pub fn validate(connection: &ProviderConnection) -> Result<(), String> {
    connection.provider.validate(connection)
}

pub fn revoke(connection: &ProviderConnection) -> Result<(), String> {
    connection.provider.revoke(connection)
}

pub fn features() -> Result<Vec<ProviderModeFeature>, String> {
    let mut features = Vec::new();
    for provider in Provider::ALL {
        for mode in provider.supported_modes() {
            features.push(ProviderModeFeature {
                provider: provider.id().to_string(),
                mode: mode.id().to_string(),
                enabled: is_mode_enabled(provider, *mode)?,
            });
        }
    }
    Ok(features)
}

pub fn set_feature_enabled(provider: &str, mode: &str, enabled: bool) -> Result<(), String> {
    let provider =
        Provider::from_id(provider).ok_or_else(|| format!("Unknown AI provider {provider:?}."))?;
    let mode =
        AuthMode::parse(mode).ok_or_else(|| format!("Unknown authentication mode {mode:?}."))?;
    ensure_supported(&provider, &ProviderConnection::new(provider, mode))?;
    let mut auth_settings = load_settings()?;
    auth_settings
        .provider_modes
        .insert(mode_key(provider, mode), enabled);
    save_settings(&auth_settings)
}

pub fn api_key_fallback_enabled() -> Result<bool, String> {
    Ok(load_settings()?.api_key_fallback)
}

pub fn set_api_key_fallback_enabled(enabled: bool) -> Result<(), String> {
    let mut auth_settings = load_settings()?;
    auth_settings.api_key_fallback = enabled;
    save_settings(&auth_settings)
}

fn ensure_supported(provider: &Provider, connection: &ProviderConnection) -> Result<(), String> {
    if *provider != connection.provider {
        return Err("Provider connection does not match its adapter.".into());
    }
    if !provider.supported_modes().contains(&connection.mode) {
        return Err(format!(
            "{} does not support {} authentication.",
            provider.display_name(),
            connection.mode.id()
        ));
    }
    Ok(())
}

fn is_mode_enabled(provider: Provider, mode: AuthMode) -> Result<bool, String> {
    let auth_settings = load_settings()?;
    Ok(mode_enabled_in(&auth_settings, provider, mode))
}

fn mode_enabled_in(
    auth_settings: &ProviderAuthSettings,
    provider: Provider,
    mode: AuthMode,
) -> bool {
    auth_settings
        .provider_modes
        .get(&mode_key(provider, mode))
        .copied()
        .unwrap_or_else(|| provider.default_enabled(mode))
}

fn mode_key(provider: Provider, mode: AuthMode) -> String {
    format!("{}:{}", provider.id(), mode.id())
}

fn settings_path() -> PathBuf {
    settings::config_dir().join(FEATURES_FILE)
}

fn load_settings() -> Result<ProviderAuthSettings, String> {
    match std::fs::read(settings_path()) {
        Ok(bytes) => serde_json::from_slice(&bytes)
            .map_err(|error| format!("Provider authentication settings are invalid: {error}")),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            Ok(ProviderAuthSettings::default())
        }
        Err(error) => Err(format!(
            "Could not read provider authentication settings: {error}"
        )),
    }
}

fn save_settings(auth_settings: &ProviderAuthSettings) -> Result<(), String> {
    let directory = settings::config_dir();
    crate::platform::ensure_private_dir(&directory).map_err(|error| error.to_string())?;
    let bytes = serde_json::to_vec_pretty(auth_settings)
        .map_err(|error| format!("Could not encode provider authentication settings: {error}"))?;
    std::fs::write(settings_path(), bytes)
        .map_err(|error| format!("Could not save provider authentication settings: {error}"))
}

fn api_key_headers(provider: Provider, credential: &str) -> Result<HeaderMap, String> {
    let mut headers = HeaderMap::new();
    let value = HeaderValue::from_str(credential)
        .map_err(|_| "The configured credential is not valid for an HTTP header.".to_string())?;
    match provider {
        Provider::Anthropic => {
            headers.insert("x-api-key", value);
            headers.insert("anthropic-version", HeaderValue::from_static("2023-06-01"));
        }
        Provider::OpenRouter | Provider::Grok | Provider::OpenAi | Provider::Gemini => {
            let value = HeaderValue::from_str(&format!("Bearer {credential}"))
                .map_err(|_| "The configured credential is not valid for an HTTP header.".to_string())?;
            headers.insert(AUTHORIZATION, value);
        }
        Provider::Ollama => unreachable!("Ollama has no API-key mode"),
    }
    Ok(headers)
}
#[cfg(test)]
mod tests {
    use super::{api_key_headers, AuthMode, Provider, ProviderAdapter, ProviderAuthSettings};
    use reqwest::header::AUTHORIZATION;

    #[test]
    fn adapters_declare_only_the_modes_they_implement() {
        assert_eq!(
            Provider::OpenRouter.supported_modes(),
            &[AuthMode::ApiKey, AuthMode::OAuth]
        );
        assert_eq!(Provider::Gemini.supported_modes(), &[AuthMode::ApiKey]);
        assert_eq!(Provider::Ollama.supported_modes(), &[AuthMode::LocalCli]);
    }

    #[test]
    fn provider_mode_flags_default_to_existing_auth_paths() {
        let flags = ProviderAuthSettings::default();
        assert_eq!(flags.api_key_fallback, false);
        assert!(Provider::OpenAi.default_enabled(AuthMode::ApiKey));
        assert!(Provider::Ollama.default_enabled(AuthMode::LocalCli));
        assert!(!Provider::OpenAi.default_enabled(AuthMode::OAuth));
        assert!(flags.provider_modes.is_empty());
    }

    #[test]
    fn provider_mode_flags_can_be_disabled_and_fallback_is_opt_in() {
        let flags = ProviderAuthSettings {
            provider_modes: [("openai:api_key".to_string(), false)]
                .into_iter()
                .collect(),
            api_key_fallback: true,
        };
        assert!(!super::mode_enabled_in(
            &flags,
            Provider::OpenAi,
            AuthMode::ApiKey
        ));
        assert!(flags.api_key_fallback);
    }

    #[test]
    fn api_key_headers_preserve_provider_auth_schemes() {
        let openai = api_key_headers(Provider::OpenAi, "test-key").unwrap();
        assert_eq!(openai[AUTHORIZATION], "Bearer test-key");

        let anthropic = api_key_headers(Provider::Anthropic, "test-key").unwrap();
        assert_eq!(anthropic["x-api-key"], "test-key");
        assert_eq!(anthropic["anthropic-version"], "2023-06-01");
    }
}
