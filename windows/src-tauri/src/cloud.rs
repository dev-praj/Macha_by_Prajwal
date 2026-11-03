//! Supabase project configuration and first-run checks for the optional phone
//! cloud relay. No privileged project credential is accepted by this module.

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use base64::Engine as _;
use tauri::{Emitter, Manager, State, WebviewWindow};
use tokio_tungstenite::{connect_async, tungstenite::Message};
use url::Url;

use crate::{secrets, settings, Shared};

const REFRESH_TOKEN: &str = "phone-cloud-refresh-token";
const GOOGLE_EMAIL: &str = "phone-cloud-google-email";
const GOOGLE_USER_ID: &str = "phone-cloud-google-user-id";
const CALLBACK: &str = "http://127.0.0.1:43827/oauth/callback";
const SCHEMA_VERSION: i64 = 3;
static SESSION_REFRESH_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CloudConfig {
    pub project_url: String,
    pub anon_key: String,
    pub enabled: bool,
    pub wizard_skipped: bool,
    pub google_email: Option<String>,
}
#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Check {
    pub name: String,
    pub ok: bool,
    pub detail: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CloudTest {
    pub ready: bool,
    pub checks: Vec<Check>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CloudPairData {
    pub url: String,
    pub anon_key: String,
    pub pairing_id: String,
    pub token: String,
    pub laptop_pubkey: String,
}

#[derive(Debug)]
pub(crate) struct Session {
    pub(crate) access_token: String,
    pub(crate) refresh_token: String,
    pub(crate) user_id: String,
    pub(crate) email: String,
}

pub fn validate_project_url(value: &str) -> Result<String, String> {
    let value = value.trim();
    let parsed = Url::parse(value).map_err(|_| "Use your project URL, for example https://abc123.supabase.co".to_string())?;
    let host = parsed.host_str().unwrap_or_default();
    let project_ref = host.strip_suffix(".supabase.co").unwrap_or_default();
    if parsed.scheme() != "https"
        || project_ref.is_empty()
        || project_ref.len() > 63
        || project_ref.contains('.')
        || project_ref.starts_with('-')
        || project_ref.ends_with('-')
        || !project_ref.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
        || parsed.path() != "/"
        || parsed.query().is_some()
        || parsed.fragment().is_some()
        || parsed.port().is_some()
    {
        return Err("Use the project URL in the form https://<project-ref>.supabase.co".into());
    }
    Ok(format!("https://{host}"))
}

pub fn validate_anon_key(value: &str) -> Result<String, String> {
    let key = value.trim();
    if key.starts_with("sb_secret_") || key.eq_ignore_ascii_case("service_role") {
        return Err("This is a Supabase secret/service_role key. Macha only accepts a publishable or anon key; this value was not saved.".into());
    }
    if key.starts_with("sb_publishable_") {
        if key.len() <= "sb_publishable_".len() + 8 || !key["sb_publishable_".len()..].bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-') {
            return Err("That publishable key does not look valid.".into());
        }
        return Ok(key.to_string());
    }
    let mut segments = key.split('.');
    let (Some(header), Some(claims), Some(signature), None) = (segments.next(), segments.next(), segments.next(), segments.next()) else {
        return Err("Use a Supabase anon JWT or a key beginning with sb_publishable_.".into());
    };
    if header.is_empty() || signature.is_empty() {
        return Err("That anon key does not look like a complete JWT.".into());
    }
    use base64::Engine;
    let decoded = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(claims)
        .or_else(|_| base64::engine::general_purpose::URL_SAFE.decode(claims))
        .map_err(|_| "That key is not a valid JWT.".to_string())?;
    let payload: Value = serde_json::from_slice(&decoded).map_err(|_| "That key is not a valid JWT.".to_string())?;
    let role = payload.get("role").and_then(Value::as_str).unwrap_or_default();
    if role == "service_role" {
        return Err("This is a service_role key. Macha only accepts an anon/publishable key; this value was not saved.".into());
    }
    if role != "anon" {
        return Err("The JWT does not have the Supabase anon role. This value was not saved.".into());
    }
    Ok(key.to_string())
}

fn check(name: &str, result: Result<(), String>) -> Check {
    match result {
        Ok(()) => Check { name: name.into(), ok: true, detail: "Passed".into() },
        Err(detail) => Check { name: name.into(), ok: false, detail },
    }
}

fn only_settings(window: &WebviewWindow) -> Result<(), String> {
    if window.label() == "settings" { Ok(()) } else { Err("Cloud settings can only be changed from Settings.".into()) }
}

#[tauri::command]
pub fn cloud_config(shared: State<Shared>) -> CloudConfig {
    let s = shared.settings.lock().unwrap();
    CloudConfig { project_url: s.phone_cloud_url.clone(), anon_key: s.phone_cloud_key.clone(), enabled: s.phone_cloud_enabled, wizard_skipped: s.phone_cloud_wizard_skipped, google_email: secrets::get_internal(GOOGLE_EMAIL) }
}

#[tauri::command]
pub fn cloud_set_config(window: WebviewWindow, shared: State<Shared>, project_url: String, anon_key: String) -> Result<CloudConfig, String> {
    only_settings(&window)?;
    let url = validate_project_url(&project_url)?;
    let key = validate_anon_key(&anon_key)?;
    let mut settings = shared.settings.lock().unwrap();
    let mut updated = settings.clone();
    updated.phone_cloud_url = url;
    updated.phone_cloud_key = key;
    updated.phone_cloud_wizard_skipped = false;
    settings::save(&updated).map_err(|e| format!("Could not save cloud settings: {e}"))?;
    *settings = updated.clone();
    Ok(CloudConfig { project_url: updated.phone_cloud_url, anon_key: updated.phone_cloud_key, enabled: updated.phone_cloud_enabled, wizard_skipped: updated.phone_cloud_wizard_skipped, google_email: secrets::get_internal(GOOGLE_EMAIL) })
}

#[tauri::command]
pub fn cloud_skip_setup(window: WebviewWindow, shared: State<Shared>) -> Result<(), String> {
    only_settings(&window)?;
    let mut settings = shared.settings.lock().unwrap();
    let mut updated = settings.clone();
    updated.phone_cloud_wizard_skipped = true;
    settings::save(&updated).map_err(|e| format!("Could not save settings: {e}"))?;
    *settings = updated;
    Ok(())
}

#[tauri::command]
pub fn cloud_start_setup(window: WebviewWindow) -> Result<(), String> {
    only_settings(&window)?;
    window.app_handle().emit_to("settings", "cloud-wizard-start", ()).map_err(|e| format!("Could not open cloud setup: {e}"))
}

fn parse_google_session(body: &Value) -> Result<Session, String> {
    let user = body.get("user").ok_or("Google sign-in did not return a user")?;
    let provider = user.pointer("/app_metadata/provider").and_then(Value::as_str).unwrap_or_default();
    let email = user.get("email").and_then(Value::as_str).unwrap_or_default();
    let confirmed = user.get("email_confirmed_at").and_then(Value::as_str).is_some();
    if provider != "google" || !confirmed || email.is_empty() {
        return Err("Sign in with a verified Google account. Anonymous and other provider accounts cannot use cloud pairing.".into());
    }
    Ok(Session {
        access_token: body.get("access_token").and_then(Value::as_str).ok_or("Supabase did not return an access token")?.into(),
        refresh_token: body.get("refresh_token").and_then(Value::as_str).filter(|s| !s.is_empty()).ok_or("Supabase did not return a refresh token")?.into(),
        user_id: user.get("id").and_then(Value::as_str).filter(|s| !s.is_empty()).ok_or("Google sign-in did not return a user ID")?.into(),
        email: email.into(),
    })
}

async fn request_session(client: &reqwest::Client, url: &str, key: &str, refresh: &str) -> Result<Session, String> {
    let endpoint = "token?grant_type=refresh_token";
    let mut target = Url::parse(url).map_err(|_| "Project URL is invalid".to_string())?;
    target.set_path(&format!("/auth/v1/{endpoint}"));
    let mut request = client.post(target).header("apikey", key);
    request = request.json(&json!({"refresh_token": refresh}));
    let response = request.send().await.map_err(|e| format!("Could not reach Supabase Auth: {e}"))?;
    let status = response.status();
    let body: Value = response.json().await.unwrap_or(Value::Null);
    if !status.is_success() {
        let message = body.get("msg").or_else(|| body.get("message")).or_else(|| body.get("error_description")).and_then(Value::as_str).unwrap_or("Request rejected");
        return Err(format!("Supabase Auth returned HTTP {}: {}", status.as_u16(), message.chars().take(220).collect::<String>()));
    }
    parse_google_session(&body)
}

pub(crate) async fn session(client: &reqwest::Client, url: &str, key: &str) -> Result<Session, String> {
    // Supabase rotates refresh tokens. Pair setup, connection retries and the
    // setup test can all ask for a session at once; serializing this operation
    // prevents a losing refresh from invalidating the user identity used by pairing.
    let _refresh_guard = SESSION_REFRESH_LOCK.lock().await;
    let refresh = secrets::get_internal(REFRESH_TOKEN).ok_or("Sign in with Google in Supabase cloud settings before pairing.")?;
    let session = request_session(client, url, key, &refresh).await?;
    if secrets::get_internal(GOOGLE_USER_ID).as_deref().is_some_and(|id| id != session.user_id) {
        return Err("The saved Google account changed. Sign in again before pairing.".into());
    }
    secrets::set_internal(REFRESH_TOKEN, &session.refresh_token).map_err(|e| format!("Could not save the device session in Windows Credential Manager: {e}"))?;
    secrets::set_internal(GOOGLE_USER_ID, &session.user_id).map_err(|e| format!("Could not save the Google account in Windows Credential Manager: {e}"))?;
    secrets::set_internal(GOOGLE_EMAIL, &session.email).map_err(|e| format!("Could not save the Google account in Windows Credential Manager: {e}"))?;
    Ok(session)
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GoogleSignInResult { pub email: String }

#[tauri::command]
pub async fn cloud_google_sign_in(window: WebviewWindow, shared: State<'_, Shared>) -> Result<GoogleSignInResult, String> {
    only_settings(&window)?;
    let (url, key) = { let s = shared.settings.lock().unwrap(); (validate_project_url(&s.phone_cloud_url)?, validate_anon_key(&s.phone_cloud_key)?) };
    let mut verifier_bytes = [0u8; 32];
    rand::RngCore::fill_bytes(&mut rand::thread_rng(), &mut verifier_bytes);
    let verifier = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(verifier_bytes);
    use sha2::Digest;
    let challenge = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(sha2::Sha256::digest(verifier.as_bytes()));
    let listener = std::net::TcpListener::bind("127.0.0.1:43827").map_err(|e| format!("Could not reserve the Google sign-in callback port: {e}. Close the other app using port 43827, then retry."))?;
    listener.set_nonblocking(false).map_err(|e| e.to_string())?;
    let mut authorize = Url::parse(&format!("{url}/auth/v1/authorize")).map_err(|e| e.to_string())?;
    authorize.query_pairs_mut()
        .append_pair("provider", "google")
        .append_pair("apikey", &key)
        .append_pair("redirect_to", CALLBACK)
        .append_pair("code_challenge", &challenge)
        .append_pair("code_challenge_method", "s256")
        .append_pair("scopes", "openid email profile")
        .append_pair("query_params", "{\"prompt\":\"select_account\"}");
    crate::platform::open_url(authorize.as_str())?;
    let code = tokio::task::spawn_blocking(move || -> Result<String, String> {
        use std::io::{Read, Write};
        listener.set_nonblocking(true).map_err(|e| e.to_string())?;
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(300);
        loop {
            match listener.accept() {
                Ok((mut stream, address)) => {
                    if !address.ip().is_loopback() { continue; }
                    stream.set_read_timeout(Some(std::time::Duration::from_secs(5))).ok();
                    let mut bytes = [0u8; 8192];
                    let n = stream.read(&mut bytes).map_err(|e| format!("Could not read sign-in callback: {e}"))?;
                    let request = String::from_utf8_lossy(&bytes[..n]);
                    let path = request.lines().next().and_then(|line| line.split_whitespace().nth(1)).ok_or("Invalid sign-in callback")?;
                    if !path.starts_with("/oauth/callback?") && path != "/oauth/callback" { continue; }
                    let callback = Url::parse(&format!("http://127.0.0.1:43827{path}")).map_err(|e| e.to_string())?;
                    let code = callback.query_pairs().find(|(k, _)| k == "code").map(|(_, v)| v.into_owned());
                    let error = callback.query_pairs().find(|(k, _)| k == "error_description" || k == "error").map(|(_, v)| v.into_owned());
                    let (status, message) = if code.is_some() { ("200 OK", "Google sign-in complete. You can return to Macha.") } else { ("400 Bad Request", "Google sign-in was cancelled or rejected. Return to Macha and try again.") };
                    let body = format!("<!doctype html><meta charset=utf-8><title>Macha sign-in</title><p>{message}</p>");
                    let response = format!("HTTP/1.1 {status}\r\nContent-Type: text/html; charset=utf-8\r\nConnection: close\r\nContent-Length: {}\r\n\r\n{}", body.len(), body);
                    let _ = stream.write_all(response.as_bytes());
                    return code.ok_or_else(|| error.unwrap_or_else(|| "Google sign-in was cancelled.".into()));
                }
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock && std::time::Instant::now() < deadline => std::thread::sleep(std::time::Duration::from_millis(100)),
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => return Err("Google sign-in timed out after five minutes.".into()),
                Err(e) => return Err(format!("Could not receive the Google sign-in callback: {e}")),
            }
        }
    }).await.map_err(|e| format!("Google sign-in callback failed: {e}"))??;
    let client = reqwest::Client::builder().timeout(std::time::Duration::from_secs(20)).build().map_err(|e| e.to_string())?;
    let mut endpoint = Url::parse(&url).map_err(|e| e.to_string())?;
    endpoint.set_path("/auth/v1/token"); endpoint.query_pairs_mut().append_pair("grant_type", "pkce");
    let response = client.post(endpoint).header("apikey", &key).json(&json!({"auth_code": code, "code_verifier": verifier})).send().await.map_err(|e| format!("Could not exchange Google sign-in code: {e}"))?;
    let status = response.status(); let body: Value = response.json().await.unwrap_or(Value::Null);
    if !status.is_success() { return Err(format!("Supabase could not finish Google sign-in (HTTP {}): {}", status.as_u16(), body.get("msg").or_else(|| body.get("message")).and_then(Value::as_str).unwrap_or("check the Google provider setup"))); }
    let session = parse_google_session(&body)?;
    secrets::set_internal(REFRESH_TOKEN, &session.refresh_token).map_err(|e| format!("Could not save Google session in Windows Credential Manager: {e}"))?;
    secrets::set_internal(GOOGLE_USER_ID, &session.user_id).map_err(|e| format!("Could not save Google account in Windows Credential Manager: {e}"))?;
    secrets::set_internal(GOOGLE_EMAIL, &session.email).map_err(|e| format!("Could not save Google account in Windows Credential Manager: {e}"))?;
    Ok(GoogleSignInResult { email: session.email })
}

pub async fn start_phone_pairing(url: &str, key: &str) -> Result<CloudPairData, String> {
    let url = validate_project_url(url)?;
    let key = validate_anon_key(key)?;
    let client = reqwest::Client::builder().timeout(std::time::Duration::from_secs(15)).build().map_err(|e| e.to_string())?;
    let session = session(&client, &url, &key).await?;
    if secrets::get_internal("phone-cloud-link-id").is_some() {
        let _ = rpc(&client, &url, &key, &session.access_token, "unpair", json!({})).await;
    }
    for name in ["phone-cloud-link-id", "phone-cloud-device-id", "phone-cloud-peer-device-id", "phone-cloud-link-key", "phone-cloud-peer-public-key", "phone-cloud-confirm-code", "phone-cloud-send-seq", "phone-cloud-receive-seq"] {
        secrets::set_internal(name, "").map_err(|e| format!("Could not reset the previous cloud pairing: {e}"))?;
    }
    let identity = crate::phone_link::cloud_crypto::Identity::generate();
    let response = rpc(&client, &url, &key, &session.access_token, "pair_start", json!({"laptop_pubkey": identity.public_b64()})).await?;
    let pairing_id = response.get("pairing_id").and_then(Value::as_str).ok_or("Supabase did not return a pairing id")?.to_string();
    let token = response.get("token").and_then(Value::as_str).ok_or("Supabase did not return a one-time pairing token")?.to_string();
    secrets::set_internal("phone-cloud-private-key", &identity.private_b64()).map_err(|e| format!("Could not save the laptop cloud key in Windows Credential Manager: {e}"))?;
    secrets::set_internal("phone-cloud-pairing-id", &pairing_id).map_err(|e| format!("Could not save the pairing session: {e}"))?;
    Ok(CloudPairData { url, anon_key: key, pairing_id, token, laptop_pubkey: identity.public_b64() })
}

pub(crate) async fn rpc(client: &reqwest::Client, url: &str, key: &str, jwt: &str, name: &str, args: Value) -> Result<Value, String> {
    let endpoint = format!("{url}/rest/v1/rpc/{name}");
    let response = client.post(endpoint).header("apikey", key).bearer_auth(jwt).json(&args).send().await.map_err(|e| format!("Could not call {name}: {e}"))?;
    let status = response.status();
    let body = response.text().await.unwrap_or_default();
    if !status.is_success() {
        if status.as_u16() == 404 { return Err("Cloud SQL is not installed yet. Go back to step 5 and run the bundled SQL script.".into()); }
        return Err(format!("Supabase returned HTTP {}: {}", status.as_u16(), body.chars().take(240).collect::<String>()));
    }
    serde_json::from_str(&body).map_err(|_| format!("Supabase returned an invalid response from {name}"))
}

async fn realtime_check(client: &reqwest::Client, url: &str, key: &str, jwt: &str) -> Result<(), String> {
    let link = rpc(client, url, key, jwt, "qb_realtime_test_start", json!({})).await?.as_str().ok_or("Could not create a temporary Realtime test link")?.to_string();
    let result = realtime_probe(client, url, key, jwt, &link).await;
    let _ = rpc(client, url, key, jwt, "qb_realtime_test_finish", json!({"probe_link": link})).await;
    result
}

async fn realtime_probe(_client: &reqwest::Client, url: &str, key: &str, jwt: &str, link: &str) -> Result<(), String> {
    use futures_util::{SinkExt, StreamExt};
    let host = Url::parse(url).map_err(|_| "Project URL is invalid".to_string())?;
    let encoded_key: String = url::form_urlencoded::byte_serialize(key.as_bytes()).collect();
    let ws_url = format!("wss://{}/realtime/v1/websocket?apikey={}&vsn=1.0.0", host.host_str().unwrap_or_default(), encoded_key);
    let (mut socket, _) = tokio::time::timeout(std::time::Duration::from_secs(10), connect_async(ws_url)).await.map_err(|_| "Realtime connection timed out".to_string())?.map_err(|e| format!("Realtime is unreachable: {e}"))?;
    let topic = format!("realtime:link:{link}");
    socket.send(Message::Text(json!({"topic":topic,"event":"phx_join","payload":{"config":{"broadcast":{"self":true,"ack":true},"private":true},"access_token":jwt},"ref":"join-1","join_ref":"join-1"}).to_string().into())).await.map_err(|e| format!("Could not join private Realtime channel: {e}"))?;
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(10);
    let mut joined = false;
    let mut sent = false;
    let marker = format!("macha-probe-{}", std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_nanos());
    while tokio::time::Instant::now() < deadline {
        let Some(frame) = tokio::time::timeout_at(deadline, socket.next()).await.map_err(|_| "Private Realtime test timed out".to_string())? else { break };
        let frame = frame.map_err(|e| format!("Realtime disconnected: {e}"))?;
        let Message::Text(text) = frame else { continue };
        let value: Value = serde_json::from_str(&text).unwrap_or(Value::Null);
        if value.get("event").and_then(Value::as_str) == Some("phx_reply") && !joined {
            let status = value.pointer("/payload/status").and_then(Value::as_str).unwrap_or_default();
            if status != "ok" { return Err(value.pointer("/payload/response/reason").and_then(Value::as_str).unwrap_or("Private channel join was rejected; check the SQL policies").to_string()); }
            joined = true;
            socket.send(Message::Text(json!({"topic":topic,"event":"broadcast","payload":{"type":"broadcast","event":"qb_setup_test","payload":{"marker":marker}},"ref":"test-1","join_ref":"join-1"}).to_string().into())).await.map_err(|e| format!("Could not send a Realtime test broadcast: {e}"))?;
            sent = true;
        }
        if value.get("event").and_then(Value::as_str) == Some("broadcast")
            && value.pointer("/payload/payload/marker").and_then(Value::as_str) == Some(marker.as_str()) { return Ok(()); }
    }
    if !joined { Err("Private Realtime channel could not be joined. Check the SQL policies.".into()) }
    else if !sent { Err("Realtime test broadcast could not be sent.".into()) }
    else { Err("The Realtime test broadcast was not received before timeout.".into()) }
}

#[tauri::command]
pub async fn cloud_test_connection(shared: State<'_, Shared>) -> Result<CloudTest, String> {
    let (url, key) = {
        let s = shared.settings.lock().unwrap();
        (s.phone_cloud_url.clone(), s.phone_cloud_key.clone())
    };
    let client = reqwest::Client::builder().timeout(std::time::Duration::from_secs(15)).build();
    let mut checks = Vec::new();
    let parsed_url = validate_project_url(&url);
    let parsed_key = validate_anon_key(&key);
    let (Ok(url), Ok(key), Ok(client)) = (parsed_url, parsed_key, client) else {
        checks.extend([
            Check { name: "Project reachable".into(), ok: false, detail: "Enter a valid project URL first.".into() },
            Check { name: "Key accepted".into(), ok: false, detail: "Enter a valid anon/publishable key. Secret keys are rejected and never saved.".into() },
            Check { name: "Google sign-in".into(), ok: false, detail: "Enter a valid project URL and anon/publishable key first.".into() },
            Check { name: "Database schema".into(), ok: false, detail: "Run the SQL in step 5 first.".into() },
            Check { name: "Private Realtime".into(), ok: false, detail: "Run the SQL in step 5 first.".into() },
        ]);
        return Ok(CloudTest { ready: false, checks });
    };

    // A health request distinguishes an unreachable/paused project from a bad key.
    let health = client.get(format!("{url}/auth/v1/health")).header("apikey", &key).send().await;
    let (reachability, key_status) = match health {
        Ok(response) if response.status().is_success() => (Ok(()), Ok(())),
        Ok(response) if response.status().is_server_error() => { let why = "The project returned a server error. It may be paused; restore it from the dashboard.".to_string(); (Err(why.clone()), Err(why)) },
        Ok(response) if response.status().as_u16() == 401 || response.status().as_u16() == 403 => (Ok(()), Err("The project responded but rejected this key.".into())),
        Ok(response) => (Err(format!("Project health check returned HTTP {}.", response.status().as_u16())), Err(format!("Supabase rejected the API key (HTTP {}).", response.status().as_u16()))),
        Err(e) => { let why = format!("Could not reach the project: {e}. Check your internet connection or restore a paused project."); (Err(why.clone()), Err(why)) },
    };
    checks.push(check("Project reachable", reachability));
    checks.push(check("Key accepted", key_status));

    let auth = session(&client, &url, &key).await;
    checks.push(check("Google sign-in", auth.as_ref().map(|_| ()).map_err(Clone::clone)));
    let Some(session) = auth.ok() else {
        checks.extend([
            Check { name: "Database schema".into(), ok: false, detail: "Sign in with Google, then run the latest bundled SQL script.".into() },
            Check { name: "Private Realtime".into(), ok: false, detail: "Google sign-in is required before this check.".into() },
        ]);
        return Ok(CloudTest { ready: false, checks });
    };
    let version = rpc(&client, &url, &key, &session.access_token, "qb_schema_version", json!({})).await;
    let version_ok = matches!(version, Ok(Value::Number(ref n)) if n.as_i64() == Some(SCHEMA_VERSION));
    checks.push(check("Database schema", if version_ok { Ok(()) } else { Err(version.err().unwrap_or_else(|| "The installed SQL schema is outdated. Run the latest setup SQL from step 5 again.".into())) }));
    if version_ok {
        checks.push(check("Private Realtime", realtime_check(&client, &url, &key, &session.access_token).await));
    } else {
        checks.push(Check { name: "Private Realtime".into(), ok: false, detail: "The latest setup SQL is required. Go back to step 5, run its SQL script again, then retest.".into() });
    }
    let ready = checks.iter().all(|c| c.ok);
    if ready {
        let mut current = shared.settings.lock().unwrap();
        let mut updated = current.clone();
        updated.phone_cloud_enabled = true;
        if settings::save(&updated).is_ok() { *current = updated; }
    }
    Ok(CloudTest { ready, checks })
}

#[cfg(test)]
mod tests {
    use super::{validate_anon_key, validate_project_url};
    use base64::Engine as _;

    #[test]
    fn accepts_only_supabase_project_urls() {
        assert_eq!(validate_project_url(" https://abc123.supabase.co/ ").unwrap(), "https://abc123.supabase.co");
        for bad in ["http://abc.supabase.co", "https://evil.example", "https://a.supabase.co/path", "https://a.supabase.co?x=1"] {
            assert!(validate_project_url(bad).is_err(), "accepted {bad}");
        }
    }

    #[test]
    fn rejects_privileged_supabase_keys_before_storage() {
        assert!(validate_anon_key("sb_secret_abcdefghijk").unwrap_err().contains("secret/service_role"));
        let payload = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(r#"{"role":"service_role"}"#);
        assert!(validate_anon_key(&format!("eyJhbGciOiJub25lIn0.{payload}.signature")).unwrap_err().contains("service_role"));
        assert_eq!(validate_anon_key("sb_publishable_abcdefghijk").unwrap(), "sb_publishable_abcdefghijk");
    }
}
