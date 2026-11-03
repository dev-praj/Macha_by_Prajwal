use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;
use rand::RngCore;
use reqwest::Client;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::time::{timeout, Duration};
use url::Url;

use crate::{platform, provider_auth, secrets};

const OPENROUTER_AUTH: &str = "https://openrouter.ai/auth";
const OPENROUTER_KEY_EXCHANGE: &str = "https://openrouter.ai/api/v1/auth/keys";
const CALLBACK_PATH: &str = "/oauth/callback";
const CALLBACK_TIMEOUT: Duration = Duration::from_secs(300);

pub async fn connect_openrouter() -> Result<secrets::OpenRouterAccount, String> {
    let feature = provider_auth::features()?
        .into_iter()
        .find(|feature| feature.provider == "openrouter" && feature.mode == "oauth")
        .ok_or("OpenRouter account connection is not supported.")?;
    if !feature.enabled {
        return Err("OpenRouter account connection is disabled by the local feature flag.".into());
    }

    let mut random = [0u8; 32];
    rand::rngs::OsRng.fill_bytes(&mut random);
    let state = URL_SAFE_NO_PAD.encode(random);
    rand::rngs::OsRng.fill_bytes(&mut random);
    let verifier = URL_SAFE_NO_PAD.encode(random);
    let challenge = URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()));

    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .map_err(|error| format!("Could not start the OpenRouter sign-in callback: {error}"))?;
    let port = listener
        .local_addr()
        .map_err(|error| format!("Could not inspect the OpenRouter callback address: {error}"))?
        .port();
    let callback_url = format!("http://localhost:{port}{CALLBACK_PATH}");
    let authorize_url = authorization_url(&callback_url, &challenge, &state)?;
    platform::open_url(authorize_url.as_str())?;

    let code = timeout(
        CALLBACK_TIMEOUT,
        wait_for_callback(listener, port, &state),
    )
    .await
    .map_err(|_| "OpenRouter sign-in timed out after five minutes.".to_string())??;

    let client = Client::builder()
        .timeout(Duration::from_secs(20))
        .build()
        .map_err(|error| format!("Could not prepare the OpenRouter key exchange: {error}"))?;
    let response = client
        .post(OPENROUTER_KEY_EXCHANGE)
        .json(&json!({
            "code": code,
            "code_verifier": verifier,
            "code_challenge_method": "S256",
        }))
        .send()
        .await
        .map_err(|_| "Could not reach OpenRouter to finish sign-in. Check your connection and retry.".to_string())?;
    let status = response.status();
    let body: Value = response.json().await.map_err(|_| {
        "OpenRouter returned an invalid response while finishing sign-in.".to_string()
    })?;
    if !status.is_success() {
        return Err(format!(
            "OpenRouter rejected sign-in (HTTP {}). Restart the connection flow and try again.",
            status.as_u16()
        ));
    }
    let key = body
        .get("key")
        .and_then(Value::as_str)
        .filter(|key| !key.is_empty())
        .ok_or("OpenRouter did not return an API key. Restart the connection flow and try again.")?;
    secrets::add_openrouter_oauth_account(key)
}

fn authorization_url(callback_url: &str, challenge: &str, state: &str) -> Result<Url, String> {
    let mut url = Url::parse(OPENROUTER_AUTH)
        .map_err(|error| format!("Could not prepare OpenRouter sign-in: {error}"))?;
    url.query_pairs_mut()
        .append_pair("callback_url", callback_url)
        .append_pair("code_challenge", challenge)
        .append_pair("code_challenge_method", "S256")
        .append_pair("key_label", "Macha")
        .append_pair("state", state);
    Ok(url)
}

async fn wait_for_callback(
    listener: TcpListener,
    port: u16,
    expected_state: &str,
) -> Result<String, String> {
    loop {
        let (mut stream, address) = listener
            .accept()
            .await
            .map_err(|error| format!("Could not receive the OpenRouter sign-in callback: {error}"))?;
        if !address.ip().is_loopback() {
            continue;
        }
        let target = match read_request_target(&mut stream).await {
            Ok(target) => target,
            Err(error) => {
                respond(&mut stream, false).await;
                return Err(error);
            }
        };
        let callback = Url::parse(&format!("http://localhost:{port}{target}"))
            .map_err(|_| "OpenRouter returned an invalid sign-in callback.".to_string())?;
        if callback.path() != CALLBACK_PATH {
            respond(&mut stream, false).await;
            continue;
        }
        let query: Vec<(String, String)> = callback.query_pairs().into_owned().collect();
        if query_value(&query, "error").is_some() {
            respond(&mut stream, false).await;
            return Err("OpenRouter sign-in was cancelled or denied.".into());
        }
        if query_value(&query, "state") != Some(expected_state) {
            respond(&mut stream, false).await;
            continue;
        }
        let Some(code) = query_value(&query, "code") else {
            respond(&mut stream, false).await;
            return Err("OpenRouter did not return a sign-in code.".into());
        };
        respond(&mut stream, true).await;
        return Ok(code.to_string());
    }
}

async fn read_request_target(stream: &mut TcpStream) -> Result<String, String> {
    let mut request = Vec::with_capacity(1024);
    let mut buffer = [0u8; 1024];
    loop {
        let count = stream
            .read(&mut buffer)
            .await
            .map_err(|error| format!("Could not read the OpenRouter callback: {error}"))?;
        if count == 0 {
            return Err("OpenRouter closed the sign-in callback unexpectedly.".into());
        }
        request.extend_from_slice(&buffer[..count]);
        if request.windows(4).any(|window| window == b"\r\n\r\n") {
            break;
        }
        if request.len() > 8192 {
            return Err("OpenRouter sign-in callback was too large.".into());
        }
    }
    let first_line = std::str::from_utf8(&request)
        .map_err(|_| "OpenRouter returned an invalid sign-in callback.".to_string())?
        .lines()
        .next()
        .ok_or("OpenRouter returned an empty sign-in callback.")?;
    let mut parts = first_line.split_whitespace();
    if parts.next() != Some("GET") {
        return Err("OpenRouter returned an invalid sign-in callback method.".into());
    }
    parts
        .next()
        .filter(|target| target.starts_with('/'))
        .map(str::to_string)
        .ok_or_else(|| "OpenRouter returned an invalid sign-in callback path.".into())
}

fn query_value<'a>(query: &'a [(String, String)], name: &str) -> Option<&'a str> {
    query
        .iter()
        .find(|(key, _)| key == name)
        .map(|(_, value)| value.as_str())
}

async fn respond(stream: &mut TcpStream, success: bool) {
    let (status, message) = if success {
        ("200 OK", "OpenRouter sign-in complete. You can return to Macha.")
    } else {
        ("400 Bad Request", "Sign-in could not be completed. Return to Macha.")
    };
    let body = format!(
        "<!doctype html><meta charset=utf-8><title>Macha sign-in</title><p>{message}</p>"
    );
    let response = format!(
        "HTTP/1.1 {status}\r\nContent-Type: text/html; charset=utf-8\r\nConnection: close\r\nContent-Length: {}\r\n\r\n{body}",
        body.len()
    );
    let _ = stream.write_all(response.as_bytes()).await;
}

#[cfg(test)]
mod tests {
    use super::{authorization_url, query_value, CALLBACK_PATH, OPENROUTER_AUTH};
    use url::Url;

    #[test]
    fn authorization_uses_loopback_callback_pkce_and_state() {
        let url = authorization_url(
            "http://localhost:49152/oauth/callback",
            "challenge",
            "random-state",
        )
        .unwrap();
        let query: Vec<(String, String)> = url.query_pairs().into_owned().collect();
        assert_eq!(
            format!("{}{}", url.origin().ascii_serialization(), url.path()),
            OPENROUTER_AUTH
        );
        assert_eq!(
            query_value(&query, "callback_url"),
            Some("http://localhost:49152/oauth/callback")
        );
        assert_eq!(query_value(&query, "code_challenge"), Some("challenge"));
        assert_eq!(
            query_value(&query, "code_challenge_method"),
            Some("S256")
        );
        assert_eq!(query_value(&query, "state"), Some("random-state"));
    }

    #[test]
    fn callback_path_is_fixed_and_query_values_are_matched_by_name() {
        let callback =
            Url::parse("http://localhost:49152/oauth/callback?state=s&code=c").unwrap();
        let query: Vec<(String, String)> = callback.query_pairs().into_owned().collect();
        assert_eq!(callback.path(), CALLBACK_PATH);
        assert_eq!(query_value(&query, "state"), Some("s"));
        assert_eq!(query_value(&query, "code"), Some("c"));
        assert_eq!(query_value(&query, "missing"), None);
    }
}
