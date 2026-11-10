//! Account based device linking. This path is opt-in and independent of the
//! QR-paired local phone-link transport.

use base64::Engine as _;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::Digest;
use tauri::{AppHandle, Manager, WebviewWindow};
use url::Url;
use std::sync::atomic::{AtomicBool, Ordering};

use crate::{phone_link::cloud_crypto::Identity, secrets};

const REFRESH: &str = "remote-auth-refresh-token";
const USER_ID: &str = "remote-auth-user-id";
const EMAIL: &str = "remote-auth-email";
const DEVICE_ID: &str = "remote-device-id";
const PRIVATE_KEY: &str = "remote-device-private-key";
const SKIPPED: &str = "remote-signin-skipped";
const CHOICE_RECORDED: &str = "remote-signin-choice-recorded";
static OAUTH_CANCELLED: AtomicBool = AtomicBool::new(false);
static REMOTE_REFRESH_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

fn project() -> Result<(String, String), String> {
    let url = option_env!("SUPABASE_URL").unwrap_or_default().trim().trim_end_matches('/').to_string();
    let key = option_env!("SUPABASE_ANON_KEY").unwrap_or_default().trim().to_string();
    if url.is_empty() || key.is_empty() {
        return Err("Remote sync is not configured in this build.".into());
    }
    let url = crate::cloud::validate_project_url(&url).map_err(|_| "Remote sync is not configured correctly in this build.".to_string())?;
    let key = crate::cloud::validate_anon_key(&key).map_err(|_| "Remote sync is not configured correctly in this build.".to_string())?;
    Ok((url, key))
}

fn only_settings(window: &WebviewWindow) -> Result<(), String> {
    if matches!(window.label(), "settings" | "signin") { Ok(()) } else { Err("Remote account settings can only be changed from Settings or the sign-in screen.".into()) }
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RemoteStatus {
    configured: bool,
    signed_in: bool,
    email: Option<String>,
    device_id: Option<String>,
    device_status: Option<String>,
    skipped: bool,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub struct RemoteDevice {
    id: String,
    #[serde(rename = "type")]
    device_type: String,
    name: String,
    pubkey: String,
    status: String,
    last_seen: Option<String>,
}

#[derive(Debug)]
struct Session { access: String, refresh: String, user_id: String, email: String }

/// Short-lived credentials and key material for one approved phone tunnel.
/// These values stay in Rust memory and are never sent to the webview.
#[derive(Clone)]
pub(crate) struct RemoteTunnel {
    pub(crate) url: String,
    pub(crate) anon_key: String,
    pub(crate) user_id: String,
    pub(crate) access_token: String,
    pub(crate) link_id: String,
    pub(crate) own_device_id: String,
    pub(crate) peer_device_id: String,
    pub(crate) shared_key: [u8; 32],
}

#[tauri::command]
pub fn remote_status() -> RemoteStatus {
    clear_legacy_project_config();
    let configured = project().is_ok();
    let signed_in = secrets::get_internal(REFRESH).is_some();
    RemoteStatus {
        configured,
        signed_in,
        email: secrets::get_internal(EMAIL),
        device_id: secrets::get_internal(DEVICE_ID),
        device_status: secrets::get_internal("remote-device-status"),
        skipped: sign_in_was_skipped(),
    }
}

fn clear_legacy_project_config() {
    // Remove values saved by the retired bring-your-own-project UI on upgrade.
    for key in ["remote-supabase-url", "remote-supabase-publishable-key"] {
        if secrets::get_internal(key).is_some() { let _ = secrets::set_internal(key, ""); }
    }
}

pub fn startup_sign_in_gate() -> bool {
    clear_legacy_project_config();
    sign_in_gate_required(
        secrets::get_internal(REFRESH).is_some(),
        sign_in_was_skipped(),
    )
}

fn sign_in_was_skipped() -> bool {
    secrets::get_internal(SKIPPED).as_deref() == Some("true")
        && secrets::get_internal(CHOICE_RECORDED).as_deref() == Some("true")
}

fn sign_in_gate_required(signed_in: bool, skipped: bool) -> bool {
    !signed_in && !skipped
}

fn finish_gate(app: &AppHandle) {
    if let Some(window) = app.get_webview_window("signin") { let _ = window.hide(); }
    if let Some(window) = crate::island::window(app) { let _ = window.show(); }
}

pub fn dismiss_sign_in_gate(app: &AppHandle) {
    let _ = secrets::set_internal(SKIPPED, "true");
    let _ = secrets::set_internal(CHOICE_RECORDED, "true");
    finish_gate(app);
}

/// Bring the existing choice screen forward on a repeated app launch. Showing
/// this page never starts OAuth; only its Google button does that.
pub fn open_sign_in_gate(app: &AppHandle) {
    if let Some(island) = crate::island::window(app) { let _ = island.hide(); }
    let window = app.get_webview_window("signin").or_else(|| crate::create_sign_in_window(app));
    if let Some(window) = window {
        let _ = window.show();
        let _ = window.set_focus();
    } else if let Some(island) = crate::island::window(app) {
        let _ = island.show();
    }
}

#[tauri::command]
pub fn remote_skip_sign_in(window: WebviewWindow, app: AppHandle) -> Result<(), String> {
    only_settings(&window)?;
    OAUTH_CANCELLED.store(true, Ordering::SeqCst);
    secrets::set_internal(SKIPPED, "true")?;
    secrets::set_internal(CHOICE_RECORDED, "true")?;
    finish_gate(&app);
    Ok(())
}

#[tauri::command]
pub fn remote_finish_sign_in_gate(window: WebviewWindow, app: AppHandle) -> Result<(), String> {
    only_settings(&window)?;
    finish_gate(&app);
    Ok(())
}

#[tauri::command]
pub fn remote_cancel_sign_in(window: WebviewWindow) -> Result<(), String> {
    only_settings(&window)?;
    OAUTH_CANCELLED.store(true, Ordering::SeqCst);
    Ok(())
}

#[tauri::command]
pub async fn remote_sign_in(window: WebviewWindow) -> Result<RemoteStatus, String> {
    only_settings(&window)?;
    let (url, key) = project()?;
    let client = client()?;
    ensure_google_provider_enabled(&client, &url, &key).await?;
    OAUTH_CANCELLED.store(false, Ordering::SeqCst);
    let (listener, callback, verifier, challenge) = start_pkce_listener()?;
    let mut authorize = Url::parse(&format!("{url}/auth/v1/authorize")).map_err(|e| e.to_string())?;
    authorize.query_pairs_mut()
        .append_pair("provider", "google")
        .append_pair("apikey", &key)
        .append_pair("redirect_to", &callback)
        .append_pair("code_challenge", &challenge)
        .append_pair("code_challenge_method", "s256")
        .append_pair("query_params", r#"{"prompt":"select_account"}"#);
    crate::platform::open_url(authorize.as_str())?;
    let callback_for_listener = callback.clone();
    let code = tokio::task::spawn_blocking(move || receive_callback(listener, &callback_for_listener)).await.map_err(|e| format!("Sign-in callback task failed: {e}"))??;
    // The browser reaches the loopback page in front of Macha. Bring the waiting
    // sign-in window back to the foreground so it can finish the PKCE exchange.
    let _ = window.show();
    let _ = window.set_focus();
    let endpoint = format!("{url}/auth/v1/token?grant_type=pkce");
    let response = client.post(endpoint).header("apikey", &key).json(&json!({"auth_code":code,"code_verifier":verifier})).send().await.map_err(|e| format!("Could not exchange the Google sign-in code: {e}"))?;
    let status = response.status();
    let body: Value = response.json().await.unwrap_or(Value::Null);
    if !status.is_success() { return Err(format!("Supabase sign-in failed (HTTP {}): {}", status.as_u16(), error_text(&body))); }
    let session = parse_session(&body)?;
    // The app service verifies the Supabase access token and upserts the
    // account under Google's stable provider ID. Tokens stay in Rust and are
    // never returned to the webview.
    crate::api_service::register_google_user(&session.access).await?;
    // Persist a completed Google login before calling the device RPC. Registration
    // can fail transiently (or before migrations are installed); that must not turn
    // a successful OAuth callback into an apparent sign-out.
    secrets::set_internal(REFRESH, &session.refresh)?;
    secrets::set_internal(USER_ID, &session.user_id)?;
    secrets::set_internal(EMAIL, &session.email)?;
    secrets::set_internal(SKIPPED, "false")?;
    secrets::set_internal(CHOICE_RECORDED, "true")?;
    let (device_id, device_status) = register_current_device(&client, &url, &key, &session).await?;
    secrets::set_internal(DEVICE_ID, &device_id)?;
    secrets::set_internal("remote-device-status", &device_status)?;
    Ok(remote_status())
}

async fn register_current_device(client: &reqwest::Client, url: &str, key: &str, session: &Session) -> Result<(String, String), String> {
    let identity = secrets::get_internal(PRIVATE_KEY)
        .and_then(|private| Identity::from_private_b64(&private))
        .unwrap_or_else(Identity::generate);
    secrets::set_internal(PRIVATE_KEY, &identity.private_b64())?;
    let registered = rpc(client, url, key, &session.access, "register_device", json!({
        "p_type":"laptop", "p_name":computer_name(), "p_pubkey":identity.public_b64()
    })).await?;
    let device_id = registered.get("device_id").and_then(Value::as_str)
        .ok_or("Supabase did not return the registered device ID")?.to_string();
    let device_status = registered.get("status").and_then(Value::as_str).unwrap_or("pending").to_string();
    Ok((device_id, device_status))
}

#[tauri::command]
pub async fn remote_devices(window: WebviewWindow) -> Result<Vec<RemoteDevice>, String> {
    only_settings(&window)?;
    let (url, key) = project()?;
    let client = client()?;
    let session = refresh(&client, &url, &key).await?;
    if secrets::get_internal(DEVICE_ID).is_none() {
        let (device_id, status) = register_current_device(&client, &url, &key, &session).await?;
        secrets::set_internal(DEVICE_ID, &device_id)?;
        secrets::set_internal("remote-device-status", &status)?;
    }
    let rows = rpc(&client, &url, &key, &session.access, "list_devices", json!({})).await?;
    let mut devices: Vec<RemoteDevice> = serde_json::from_value(rows).map_err(|e| format!("Could not parse the device list: {e}"))?;
    let mut own_id = secrets::get_internal(DEVICE_ID).unwrap_or_default();
    let own_status = devices.iter().find(|device| device.id == own_id && device.device_type == "laptop").map(|device| device.status.clone());
    let has_other_approved = devices.iter().any(|device| device.id != own_id && device.status == "approved");
    if own_status.is_none() || (matches!(own_status.as_deref(), Some("pending" | "revoked")) && !has_other_approved) {
        // Retry registration after recovery migrations are applied. The RPC
        // leaves this device pending whenever another approved device exists.
        let (device_id, status) = register_current_device(&client, &url, &key, &session).await?;
        own_id = device_id.clone();
        secrets::set_internal(DEVICE_ID, &device_id)?;
        secrets::set_internal("remote-device-status", &status)?;
        let rows = rpc(&client, &url, &key, &session.access, "list_devices", json!({})).await?;
        devices = serde_json::from_value(rows).map_err(|e| format!("Could not parse the device list: {e}"))?;
    }
    if let Some(status) = devices.iter().find(|device| device.id == own_id && device.device_type == "laptop")
        .map(|device| device.status.as_str()) {
        secrets::set_internal("remote-device-status", status)?;
    }
    Ok(devices)
}

#[tauri::command]
pub async fn copilot_device_start(window: WebviewWindow) -> Result<Value, String> {
    only_settings(&window)?;
    let (url, key) = project()?;
    let client = client()?;
    let session = match refresh(&client, &url, &key).await {
        Ok(session) => session,
        Err(_) if secrets::get_internal(REFRESH).is_none() => {
            return Err("Sign in to Macha to connect GitHub Copilot".into());
        }
        Err(error) => return Err(error),
    };
    crate::api_service::copilot_start(&session.access).await
}

#[tauri::command]
pub async fn copilot_device_poll(window: WebviewWindow, flow_id: String) -> Result<Value, String> {
    only_settings(&window)?;
    let (url, key) = project()?;
    let client = client()?;
    let session = refresh(&client, &url, &key).await?;
    crate::api_service::copilot_poll(&session.access, &flow_id).await
}

#[tauri::command]
pub async fn copilot_accounts(window: WebviewWindow) -> Result<Value, String> {
    only_settings(&window)?;
    let (url, key) = project()?;
    let client = client()?;
    let session = refresh(&client, &url, &key).await?;
    crate::api_service::copilot_accounts(&session.access).await
}

#[tauri::command]
pub async fn copilot_disconnect(window: WebviewWindow, account_id: String) -> Result<(), String> {
    only_settings(&window)?;
    if account_id.len() != 36 || !account_id.chars().all(|c| c.is_ascii_hexdigit() || c == '-') {
        return Err("Copilot account ID is invalid.".into());
    }
    let (url, key) = project()?;
    let client = client()?;
    let session = refresh(&client, &url, &key).await?;
    crate::api_service::copilot_disconnect(&session.access, &account_id).await
}

pub(crate) async fn copilot_provider_completion(
    body: &Value,
    cancel: &mut tokio::sync::watch::Receiver<bool>,
) -> Result<Value, String> {
    let (url, key) = project()?;
    let client = client()?;
    let session = refresh(&client, &url, &key).await?;
    tokio::select! {
        result = crate::api_service::copilot_provider_completion(&session.access, body) => result,
        changed = cancel.changed() => {
            if changed.is_err() || *cancel.borrow() { Err("Request cancelled.".into()) }
            else { Err("Copilot request was interrupted.".into()) }
        }
    }
}

pub(crate) async fn copilot_provider_status() -> Result<Value, String> {
    let (url, key) = project()?;
    let client = client()?;
    let session = refresh(&client, &url, &key).await?;
    crate::api_service::copilot_provider_accounts(&session.access).await
}

pub(crate) async fn copilot_provider_stream(
    body: &Value,
    cancel: &mut tokio::sync::watch::Receiver<bool>,
    deltas: &tokio::sync::mpsc::UnboundedSender<String>,
    emitted_text: &mut bool,
) -> Result<Value, String> {
    let (url, key) = project()?;
    let client = client()?;
    let session = refresh(&client, &url, &key).await?;
    crate::api_service::copilot_provider_stream(&session.access, body, cancel, deltas, emitted_text).await
}

/// Finds approved account peers for the Windows message tunnel. This stays
/// separate from QR pairing and returns no peers while signed out or pending.
pub(crate) async fn active_remote_tunnels() -> Result<Vec<RemoteTunnel>, String> {
    if secrets::get_internal(REFRESH).is_none() { return Ok(Vec::new()); }
    let (url, anon_key) = project()?;
    let client = client()?;
    let session = refresh(&client, &url, &anon_key).await?;
    let own_id = secrets::get_internal(DEVICE_ID).ok_or("The signed-in laptop is not registered for Remote mode")?;
    let value = rpc(&client, &url, &anon_key, &session.access, "list_devices", json!({})).await?;
    let devices: Vec<RemoteDevice> = serde_json::from_value(value).map_err(|e| format!("Could not parse the Remote device list: {e}"))?;
    let Some(own) = devices.iter().find(|device| device.id == own_id && device.device_type == "laptop" && device.status == "approved") else {
        return Ok(Vec::new());
    };
    let own_device_id = own.id.clone();
    let private = secrets::get_internal(PRIVATE_KEY).ok_or("The laptop's Remote device key is missing. Sign in again.")?;
    let identity = Identity::from_private_b64(&private).ok_or("The laptop's Remote device key is invalid. Sign in again.")?;
    let _ = rpc(&client, &url, &anon_key, &session.access, "touch_device", json!({"p_device_id":own_id})).await;
    devices.into_iter().filter(|device| device.device_type == "phone" && device.status == "approved").map(|peer| {
        let link_id = stable_account_link_id(&session.user_id, &own_device_id, &peer.id);
        let shared_key = identity.derive(&peer.pubkey, &link_id)?;
        Ok(RemoteTunnel {
            url: url.clone(), anon_key: anon_key.clone(), user_id: session.user_id.clone(),
            access_token: session.access.clone(), link_id, own_device_id: own_device_id.clone(),
            peer_device_id: peer.id, shared_key,
        })
    }).collect()
}

fn stable_account_link_id(user_id: &str, first_device: &str, second_device: &str) -> String {
    let mut devices = [first_device, second_device];
    devices.sort_unstable();
    let material = format!("macha-remote-link-v1\n{user_id}\n{}\n{}", devices[0], devices[1]);
    let mut bytes: [u8; 16] = sha2::Sha256::digest(material.as_bytes())[..16].try_into().expect("fixed digest size");
    bytes[6] = (bytes[6] & 0x0f) | 0x50;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    let hex = bytes.iter().map(|byte| format!("{byte:02x}")).collect::<String>();
    format!("{}-{}-{}-{}-{}", &hex[0..8], &hex[8..12], &hex[12..16], &hex[16..20], &hex[20..32])
}

#[tauri::command]
pub async fn remote_approve_device(window: WebviewWindow, device_id: String) -> Result<(), String> {
    only_settings(&window)?;
    let own = secrets::get_internal(DEVICE_ID).ok_or("This laptop is not registered for Remote mode")?;
    let (url, key) = project()?;
    let client = client()?;
    let session = refresh(&client, &url, &key).await?;
    let result = rpc(&client, &url, &key, &session.access, "approve_device", json!({"p_device_id":device_id,"p_approver_device_id":own})).await?;
    if result == Value::Bool(false) { return Err("Device approval was not accepted. It may no longer be pending.".into()); }
    Ok(())
}

#[tauri::command]
pub async fn remote_deny_device(window: WebviewWindow, device_id: String) -> Result<(), String> {
    remote_revoke_device(window, device_id).await
}

#[tauri::command]
pub async fn remote_revoke_device(window: WebviewWindow, device_id: String) -> Result<(), String> {
    only_settings(&window)?;
    let (url, key) = project()?;
    let client = client()?;
    let session = refresh(&client, &url, &key).await?;
    let _ = rpc(&client, &url, &key, &session.access, "revoke_device", json!({"p_device_id":device_id})).await?;
    if secrets::get_internal(DEVICE_ID).as_deref() == Some(device_id.as_str()) {
        secrets::set_internal("remote-device-status", "revoked")?;
    }
    Ok(())
}

#[tauri::command]
pub async fn remote_sign_out(window: WebviewWindow) -> Result<(), String> {
    only_settings(&window)?;
    let session_result = async {
        let (url, key) = project()?;
        let client = client()?;
        let session = refresh(&client, &url, &key).await?;
        let _ = client.post(format!("{url}/auth/v1/logout")).header("apikey", &key).bearer_auth(&session.access).send().await;
        Ok::<(), String>(())
    }.await;
    clear_remote_session_secrets()?;
    secrets::set_internal(SKIPPED, "true")?;
    secrets::set_internal(CHOICE_RECORDED, "true")?;
    session_result
}

#[tauri::command]
pub async fn remote_delete_account(window: WebviewWindow) -> Result<(), String> {
    only_settings(&window)?;
    let (url, key) = project()?;
    let client = client()?;
    let session = refresh(&client, &url, &key).await?;
    rpc(&client, &url, &key, &session.access, "delete_my_account_data", json!({})).await?;
    clear_remote_secrets()?;
    secrets::set_internal(SKIPPED, "true")?;
    secrets::set_internal(CHOICE_RECORDED, "true")?;
    Ok(())
}

async fn refresh(client: &reqwest::Client, url: &str, key: &str) -> Result<Session, String> {
    let _guard = REMOTE_REFRESH_LOCK.lock().await;
    let refresh = secrets::get_internal(REFRESH).ok_or("Sign in with Google to use Remote mode")?;
    let response = client.post(format!("{url}/auth/v1/token?grant_type=refresh_token")).header("apikey", key)
        .json(&json!({"refresh_token":refresh})).send().await.map_err(|e| format!("Could not refresh Remote sign-in: {e}"))?;
    let status = response.status();
    let body: Value = response.json().await.unwrap_or(Value::Null);
    if !status.is_success() {
        if status.as_u16() == 400 && invalid_refresh_token(&body) {
            clear_expired_session()?;
            return Err("Your saved Google sign-in expired. Sign in with Google again.".into());
        }
        return Err(format!("Supabase session refresh failed (HTTP {}): {}", status.as_u16(), error_text(&body)));
    }
    let session = parse_session(&body)?;
    if secrets::get_internal(USER_ID).as_deref().is_some_and(|id| id != session.user_id) {
        return Err("The signed-in Google account changed. Sign out and pair both devices with the same account.".into());
    }
    secrets::set_internal(REFRESH, &session.refresh)?;
    Ok(session)
}

fn invalid_refresh_token(body: &Value) -> bool {
    body.get("error_code").and_then(Value::as_str) == Some("refresh_token_not_found")
        || ["msg", "message", "error_description"].iter().any(|field| {
            body.get(*field).and_then(Value::as_str).is_some_and(|message| {
                let message = message.to_ascii_lowercase();
                message.contains("invalid refresh token") || message.contains("refresh token not found")
            })
        })
}

fn clear_expired_session() -> Result<(), String> {
    // Keep this installation's private device key so a fresh login to the same
    // account can recover its existing device registration instead of duplicating it.
    for name in [REFRESH, USER_ID, EMAIL, DEVICE_ID, "remote-device-status"] {
        secrets::set_internal(name, "")?;
    }
    secrets::set_internal(SKIPPED, "false")?;
    secrets::set_internal(CHOICE_RECORDED, "false")?;
    Ok(())
}

fn parse_session(body: &Value) -> Result<Session, String> {
    let user = body.get("user").ok_or("Supabase Auth did not return a user")?;
    if user.pointer("/app_metadata/provider").and_then(Value::as_str) != Some("google") {
        return Err("Use a Google account to enable Remote mode".into());
    }
    let email = user.get("email").and_then(Value::as_str).filter(|s| !s.is_empty()).ok_or("Google did not return an email address")?;
    if user.get("email_confirmed_at").and_then(Value::as_str).is_none() { return Err("Use a verified Google account for Remote mode".into()); }
    Ok(Session {
        access: body.get("access_token").and_then(Value::as_str).ok_or("Supabase did not return an access token")?.into(),
        refresh: body.get("refresh_token").and_then(Value::as_str).ok_or("Supabase did not return a refresh token")?.into(),
        user_id: user.get("id").and_then(Value::as_str).ok_or("Supabase did not return an account ID")?.into(),
        email: email.into(),
    })
}

async fn rpc(client: &reqwest::Client, url: &str, key: &str, access: &str, name: &str, args: Value) -> Result<Value, String> {
    let response = client.post(format!("{url}/rest/v1/rpc/{name}")).header("apikey", key).bearer_auth(access)
        .json(&args).send().await.map_err(|e| format!("Could not call Remote {name}: {e}"))?;
    let status = response.status();
    let body = response.text().await.unwrap_or_default();
    if !status.is_success() { return Err(format!("Remote {name} failed (HTTP {}): {}", status.as_u16(), body.chars().take(240).collect::<String>())); }
    if body.is_empty() { return Ok(Value::Null); }
    serde_json::from_str(&body).map_err(|e| format!("Supabase returned invalid data for {name}: {e}"))
}

fn client() -> Result<reqwest::Client, String> {
    reqwest::Client::builder().timeout(std::time::Duration::from_secs(20)).build().map_err(|e| e.to_string())
}

async fn ensure_google_provider_enabled(client: &reqwest::Client, url: &str, key: &str) -> Result<(), String> {
    let response = client.get(format!("{url}/auth/v1/settings")).header("apikey", key).send().await
        .map_err(|e| format!("Could not check Google sign-in settings for your Supabase project: {e}"))?;
    let status = response.status();
    let body: Value = response.json().await.unwrap_or(Value::Null);
    if !status.is_success() {
        return Err(format!("Could not check Google sign-in settings in Supabase (HTTP {}). Check the project URL and publishable/anon key, then retry.", status.as_u16()));
    }
    if body.pointer("/external/google").and_then(Value::as_bool) != Some(true) {
        return Err("Google sign-in is unavailable right now. Try again later.".into());
    }
    Ok(())
}

fn error_text(value: &Value) -> String {
    value.get("msg").or_else(|| value.get("message")).or_else(|| value.get("error_description"))
        .and_then(Value::as_str).unwrap_or("check the Google provider setup").chars().take(220).collect()
}

fn computer_name() -> String {
    std::env::var("COMPUTERNAME").or_else(|_| std::env::var("HOSTNAME")).ok().filter(|s| !s.trim().is_empty())
        .unwrap_or_else(|| "Windows laptop".into()).chars().take(80).collect()
}

fn start_pkce_listener() -> Result<(std::net::TcpListener, String, String, String), String> {
    let listener = std::net::TcpListener::bind(("127.0.0.1", 0))
        .map_err(|e| format!("Could not reserve a local Google sign-in callback port: {e}"))?;
    let callback = format!("http://127.0.0.1:{}/oauth/callback", listener.local_addr().map_err(|e| e.to_string())?.port());
    listener.set_nonblocking(true).map_err(|e| e.to_string())?;
    let mut verifier = [0u8; 32];
    rand::RngCore::fill_bytes(&mut rand::thread_rng(), &mut verifier);
    let verifier = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(verifier);
    use sha2::Digest;
    let challenge = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(sha2::Sha256::digest(verifier.as_bytes()));
    Ok((listener, callback, verifier, challenge))
}

fn receive_callback(listener: std::net::TcpListener, callback_url: &str) -> Result<String, String> {
    use std::io::{Read, Write};
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(180);
    loop {
        if OAUTH_CANCELLED.load(Ordering::SeqCst) { return Err("Google sign-in was cancelled.".into()); }
        match listener.accept() {
            Ok((mut stream, address)) => {
                if !address.ip().is_loopback() { continue; }
                stream.set_read_timeout(Some(std::time::Duration::from_secs(5))).ok();
                let mut bytes = [0u8; 8192];
                let n = stream.read(&mut bytes).map_err(|e| format!("Could not read OAuth callback: {e}"))?;
                let request = String::from_utf8_lossy(&bytes[..n]);
                let path = request.lines().next().and_then(|line| line.split_whitespace().nth(1)).ok_or("Invalid OAuth callback")?;
                if !path.starts_with("/oauth/callback") { continue; }
                let base = Url::parse(callback_url).map_err(|e| e.to_string())?;
                let mut callback = base;
                callback.set_query(path.split_once('?').map(|(_, q)| q).or(Some("")));
                let code = callback.query_pairs().find(|(k, _)| k == "code").map(|(_, v)| v.into_owned());
                let error = callback.query_pairs().find(|(k, _)| k == "error_description" || k == "error").map(|(_, v)| v.into_owned());
                let text = if code.is_some() { "Google callback received. Return to Macha to finish signing in." } else { "Google sign-in was cancelled or failed. Return to Macha and try again." };
                let body = format!("<!doctype html><meta charset=utf-8><title>Macha</title><p>{text}</p>");
                let response = format!("HTTP/1.1 200 OK\r\nContent-Type: text/html; charset=utf-8\r\nConnection: close\r\nContent-Length: {}\r\n\r\n{}", body.len(), body);
                let _ = stream.write_all(response.as_bytes());
                return code.ok_or_else(|| error.unwrap_or_else(|| "Google sign-in was cancelled".into()));
            }
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock && std::time::Instant::now() < deadline => std::thread::sleep(std::time::Duration::from_millis(100)),
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => return Err("Google sign-in timed out after three minutes".into()),
            Err(e) => return Err(format!("Could not receive the OAuth callback: {e}")),
        }
    }
}

fn clear_remote_secrets() -> Result<(), String> {
    for name in [REFRESH, USER_ID, EMAIL, DEVICE_ID, PRIVATE_KEY, "remote-device-status"] {
        secrets::set_internal(name, "")?;
    }
    Ok(())
}

fn clear_remote_session_secrets() -> Result<(), String> {
    for name in [REFRESH, USER_ID, EMAIL] { secrets::set_internal(name, "")?; }
    Ok(())
}

#[cfg(test)]
mod tests {
    #[test]
    fn detects_only_invalid_refresh_token_responses() {
        assert!(super::invalid_refresh_token(&serde_json::json!({"error_code":"refresh_token_not_found"})));
        assert!(super::invalid_refresh_token(&serde_json::json!({"msg":"Invalid Refresh Token: Refresh Token Not Found"})));
        assert!(!super::invalid_refresh_token(&serde_json::json!({"msg":"network timeout"})));
    }

    #[test]
    fn sign_in_gate_respects_saved_session_and_explicit_skip() {
        assert!(!super::sign_in_gate_required(true, false));
        assert!(!super::sign_in_gate_required(false, true));
        assert!(super::sign_in_gate_required(false, false));
    }

    #[test]
    fn confirmation_code_is_order_independent_and_eight_digits() {
        let first = confirmation_code_for_test("phone-key", "laptop-key");
        assert_eq!(first, confirmation_code_for_test("laptop-key", "phone-key"));
        assert_eq!(first.len(), 9);
        assert_eq!(first.as_bytes()[4], b'-');
    }

    fn confirmation_code_for_test(a: &str, b: &str) -> String {
        use sha2::Digest;
        let material = [a, b].into_iter().collect::<Vec<_>>();
        let mut keys = material;
        keys.sort_unstable();
        let text = format!("macha-remote-device-confirm-v1\n{}\n{}", keys[0], keys[1]);
        let digest = sha2::Sha256::digest(text.as_bytes());
        let n = ((digest[0] as u32) << 24) | ((digest[1] as u32) << 16) | ((digest[2] as u32) << 8) | digest[3] as u32;
        let n = n as u64 % 100_000_000;
        format!("{:04}-{:04}", n / 10_000, n % 10_000)
    }
}
