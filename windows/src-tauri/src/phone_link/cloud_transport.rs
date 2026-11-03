//! Supabase Realtime and PostgREST pipe. The existing phone-link server still
//! owns all message parsing and control allowlists; this module only moves its
//! line stream through encrypted cloud envelopes.

use std::sync::Arc;
use std::time::Duration;

use base64::Engine as _;
use futures_util::{Sink, SinkExt, StreamExt};
use serde_json::{json, Value};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::sync::Mutex;
use tokio::task::JoinHandle;
use tokio_tungstenite::tungstenite::Message;

use super::{cloud_crypto::{Codec, Envelope, Identity}, pairing::SecretStore, server};
use tokio_tungstenite::tungstenite::client::IntoClientRequest;

pub struct Handle(tauri::async_runtime::JoinHandle<()>);
impl Handle { pub fn stop(self) { self.0.abort(); } }

pub fn start(shared: Arc<server::Shared>, url: String, key: String, pairing_id: String, _store: Arc<dyn SecretStore>) -> Handle {
    Handle(tauri::async_runtime::spawn(supervise(shared, url, key, pairing_id)))
}

/// Account mode uses the same encrypted protocol, but has no QR pairing ID and
/// never shares or modifies the legacy nearby-link credentials.
pub fn start_remote(shared: Arc<server::Shared>, tunnel: crate::remote_mode::RemoteTunnel) -> Handle {
    Handle(tauri::async_runtime::spawn(supervise_remote(shared, tunnel)))
}

async fn supervise_remote(shared: Arc<server::Shared>, tunnel: crate::remote_mode::RemoteTunnel) {
    let client = match reqwest::Client::builder().timeout(Duration::from_secs(15)).build() { Ok(v) => v, Err(_) => return };
    let mut delay = Duration::from_secs(1);
    loop {
        let current = match crate::remote_mode::active_remote_tunnels().await {
            Ok(peers) => peers.into_iter().find(|peer| peer.peer_device_id == tunnel.peer_device_id && peer.own_device_id == tunnel.own_device_id),
            Err(error) => {
                crate::log::line(format!("Remote account tunnel could not refresh: {error}"));
                tokio::time::sleep(delay).await;
                delay = (delay * 2).min(Duration::from_secs(60));
                continue;
            }
        };
        let Some(current) = current else { return };
        let attempt = run_connection(&client, &current.url, &current.anon_key, &current.access_token,
            &current.link_id, &current.own_device_id, &current.peer_device_id, current.shared_key,
            shared.clone(), Some(&current.user_id), Some(&current.link_id)).await;
        if attempt.is_ok() { delay = Duration::from_secs(1); }
        else if let Err(error) = attempt { crate::log::line(format!("Remote account tunnel ended: {error}")); }
        tokio::time::sleep(delay).await;
        delay = (delay * 2).min(Duration::from_secs(60));
    }
}

async fn supervise(shared: Arc<server::Shared>, url: String, key: String, pairing_id: String) {
    let client = match reqwest::Client::builder().timeout(Duration::from_secs(15)).build() { Ok(v) => v, Err(_) => return };
    let mut delay = Duration::from_secs(1);
    let (link, phone_device, peer_device, shared_key) = if let Some(link) = secrets_value("phone-cloud-link-id") {
        let Some(device) = secrets_value("phone-cloud-device-id") else { return };
        let Some(peer) = secrets_value("phone-cloud-peer-device-id") else { return };
        let Some(key_text) = secrets_value("phone-cloud-link-key") else { return };
        let Ok(raw) = base64::engine::general_purpose::URL_SAFE_NO_PAD.decode(key_text) else { return };
        let Ok(shared_key) = <[u8; 32]>::try_from(raw) else { return };
        (link, device, peer, shared_key)
    } else {
        match wait_for_phone(&client, &url, &key, &pairing_id).await {
            Ok(found) => found,
            Err(error) => { crate::log::line(format!("phone cloud pairing failed: {error}")); return; }
        }
    };

    loop {
        let session = match crate::cloud::session(&client, &url, &key).await {
            Ok(s) => s,
            Err(e) => { crate::log::line(format!("phone cloud sign-in failed: {e}")); tokio::time::sleep(delay).await; delay = (delay * 2).min(Duration::from_secs(60)); continue; }
        };
        let attempt = run_connection(&client, &url, &key, &session.access_token, &link, &phone_device, &peer_device, shared_key, shared.clone(), None, None).await;
        if attempt.is_ok() { delay = Duration::from_secs(1); }
        else if let Err(error) = attempt { crate::log::line(format!("phone cloud connection ended: {error}")); }
        tokio::time::sleep(delay).await;
        delay = (delay * 2).min(Duration::from_secs(60));
    }
}

fn secrets_value(key: &str) -> Option<String> { crate::secrets::get_internal(key) }

async fn wait_for_phone(client: &reqwest::Client, url: &str, key: &str, pairing_id: &str) -> Result<(String, String, String, [u8; 32]), String> {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(125);
    // The access token returned here lasts long enough for the two-minute
    // pairing window. Refreshing it on every poll needlessly rotates the
    // single-use refresh token and can race the setup/test requests.
    let session = crate::cloud::session(client, url, key).await?;
    while tokio::time::Instant::now() < deadline {
        let value = crate::cloud::rpc(client, url, key, &session.access_token, "pair_status", json!({"pairing_id":pairing_id})).await?;
        if value.get("claimed").and_then(Value::as_bool) == Some(true) {
            let link = value.get("link_id").and_then(Value::as_str).ok_or("missing link id")?.to_string();
            let peer_public = value.get("phone_pubkey").and_then(Value::as_str).ok_or("missing phone key")?;
            let laptop_device = value.get("device_id").and_then(Value::as_str).ok_or("missing laptop device")?.to_string();
            let phone_device = value.get("phone_device_id").and_then(Value::as_str).ok_or("missing phone device")?.to_string();
            let private = secrets_value("phone-cloud-private-key").ok_or("laptop cloud key was not found in Credential Manager")?;
            let identity = Identity::from_private_b64(&private).ok_or("laptop cloud key is invalid")?;
            let shared = identity.derive(peer_public, &link)?;
            let code = confirmation_code(&shared);
            for (name, content) in [
                ("phone-cloud-link-id", link.clone()),
                ("phone-cloud-device-id", laptop_device.clone()),
                ("phone-cloud-peer-device-id", phone_device.clone()),
                ("phone-cloud-link-key", base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(shared)),
                ("phone-cloud-peer-public-key", peer_public.to_string()),
                ("phone-cloud-confirm-code", code.clone()),
            ] { crate::secrets::set_internal(name, &content).map_err(|e| format!("could not save cloud link: {e}"))?; }
            return Ok((link, laptop_device, phone_device, shared));
        }
        if value.get("expires_at").and_then(Value::as_str).is_some_and(|_| value.get("claimed").and_then(Value::as_bool) == Some(false)) {
            // The session remains pollable until cleanup removes it; its two-minute token is checked by pair_claim.
        }
        tokio::time::sleep(Duration::from_millis(1500)).await;
    }
    Err("the QR pairing expired before the phone claimed it".into())
}

fn confirmation_code(key: &[u8; 32]) -> String {
    format!("{:06}", u32::from_be_bytes([key[0], key[1], key[2], key[3]]) % 1_000_000)
}

async fn run_connection(
    client: &reqwest::Client, url: &str, api_key: &str, access: &str,
    link_id: &str, own_device: &str, peer_device: &str, key: [u8; 32], shared: Arc<server::Shared>,
    account_user_id: Option<&str>, counter_scope: Option<&str>,
) -> Result<(), String> {
    let parsed = url::Url::parse(url).map_err(|e| e.to_string())?;
    let key_query: String = url::form_urlencoded::byte_serialize(api_key.as_bytes()).collect();
    let ws_url = format!("wss://{}/realtime/v1/websocket?apikey={}&vsn=1.0.0", parsed.host_str().unwrap_or_default(), key_query);
    let mut request = ws_url.into_client_request().map_err(|e| e.to_string())?;
    request.headers_mut().insert("apikey", api_key.parse().map_err(|_| "invalid project key")?);
    let (socket, _) = tokio::time::timeout(Duration::from_secs(15), tokio_tungstenite::connect_async(request)).await.map_err(|_| "Realtime connection timed out")?.map_err(|e| format!("Realtime could not connect: {e}"))?;
    let (mut sink, mut stream) = socket.split();
    let topic = account_user_id.map_or_else(|| format!("realtime:link:{link_id}"), |user_id| format!("realtime:user:{user_id}"));
    sink.send(Message::Text(json!({
        "topic":topic,"event":"phx_join","ref":"join-1","join_ref":"join-1",
        "payload":{"access_token":access,"config":{"private":true,"broadcast":{"self":false,"ack":true}}}
    }).to_string().into())).await.map_err(|e| e.to_string())?;
    let join_deadline = tokio::time::Instant::now() + Duration::from_secs(12);
    loop {
        let frame = tokio::time::timeout_at(join_deadline, stream.next()).await.map_err(|_| "Realtime private-channel join timed out")?.ok_or("Realtime closed during join")?.map_err(|e| e.to_string())?;
        if let Message::Text(text) = frame {
            let value: Value = serde_json::from_str(&text).unwrap_or(Value::Null);
            if value.get("event").and_then(Value::as_str) == Some("phx_reply") && value.get("ref").and_then(Value::as_str) == Some("join-1") {
                if value.pointer("/payload/status").and_then(Value::as_str) != Some("ok") {
                    return Err(value.pointer("/payload/response/reason").and_then(Value::as_str).unwrap_or("private channel rejected; check SQL policies").into());
                }
                break;
            }
        }
    }

    let (local, remote) = tokio::io::duplex(128 * 1024);
    let shared_for_server = shared.clone();
    let _server_task = AbortOnDrop(tokio::spawn(async move { server::run_with(remote, &shared_for_server, None).await; }));
    let (read, mut write) = tokio::io::split(local);
    let (send_seq, receive_seq) = load_counters(counter_scope);
    let codec = Arc::new(Mutex::new(Codec::with_counters(key, send_seq, receive_seq)));
    let sink = Arc::new(Mutex::new(sink));
    let client_owned = client.clone();
    let url_owned = url.to_string();
    let api_key_owned = api_key.to_string();
    let access_owned = access.to_string();
    let topic_owned = topic.clone();
    let link_owned = link_id.to_string();
    let account_user_owned = account_user_id.map(str::to_string);
    let counter_scope_owned = counter_scope.map(str::to_string);
    let from_owned = own_device.to_string();
    let to_owned = peer_device.to_string();
    let codec_send = codec.clone();
    let sink_send = sink.clone();
    let _sender = AbortOnDrop(tokio::spawn(async move {
        let mut lines = BufReader::new(read).lines();
        while let Ok(Some(line)) = lines.next_line().await {
            let envelope = {
                let mut codec = codec_send.lock().await;
                let Ok(envelope) = codec.seal(&line, "data") else { break };
                save_counters(&codec, counter_scope_owned.as_deref());
                envelope
            };
            if send_envelope(&client_owned, &url_owned, &api_key_owned, &access_owned, &link_owned, &from_owned, &to_owned, &topic_owned, &sink_send, envelope, account_user_owned.as_deref()).await.is_err() { break; }
        }
    }));

    let mut queue_tick = tokio::time::interval(Duration::from_secs(2));
    let mut heartbeat = tokio::time::interval(Duration::from_secs(20));
    let refresh_deadline = tokio::time::sleep(Duration::from_secs(45 * 60));
    tokio::pin!(refresh_deadline);
    let mut ref_id: u64 = 2;
    loop {
        tokio::select! {
            frame = stream.next() => {
                let Some(frame) = frame else { return Err("Realtime channel closed".into()) };
                let frame = frame.map_err(|e| e.to_string())?;
                if let Message::Text(text) = frame {
                    if let Some(envelope) = envelope_from_broadcast(&text) {
                        accept_envelope(client, url, api_key, access, link_id, own_device, peer_device, &topic, &sink, &codec, &mut write, envelope, None, account_user_id, counter_scope).await?;
                    }
                }
            }
            _ = queue_tick.tick() => {
                for envelope in read_queue(client, url, api_key, access, own_device, account_user_id).await? {
                    let id = envelope.id.clone();
                    accept_envelope(client, url, api_key, access, link_id, own_device, peer_device, &topic, &sink, &codec, &mut write, envelope, Some(&id), account_user_id, counter_scope).await?;
                }
            }
            _ = heartbeat.tick() => {
                sink.lock().await.send(Message::Text(json!({"topic":"phoenix","event":"heartbeat","payload":{},"ref":format!("hb-{ref_id}"),"join_ref":null}).to_string().into())).await.map_err(|e| e.to_string())?;
                ref_id += 1;
            }
            _ = &mut refresh_deadline, if account_user_id.is_some() => return Err("refreshing the account Realtime session".into()),
        }
    }
}

async fn send_envelope<S>(
    client: &reqwest::Client, url: &str, api_key: &str, access: &str, link: &str, from: &str, to: &str,
    topic: &str, sink: &Arc<Mutex<S>>, envelope: Envelope, account_user_id: Option<&str>,
) -> Result<(), String>
where S: Sink<Message> + Unpin, S::Error: std::fmt::Display {
    let expires_at = (time::OffsetDateTime::now_utc()+time::Duration::minutes(10)).format(&time::format_description::well_known::Rfc3339).map_err(|e| e.to_string())?;
    let (table, row) = if let Some(user_id) = account_user_id {
        ("remote_queued_messages", json!({"id":envelope.id,"user_id":user_id,"from_device":from,"to_device":to,
            "ciphertext":serde_json::to_string(&envelope).map_err(|e| e.to_string())?,"nonce":envelope.nonce,"expires_at":expires_at}))
    } else {
        ("queued_messages", json!({"id":envelope.id,"link_id":link,"from_device":from,"to_device":to,
            "message_type":envelope.kind,"seq":envelope.seq,"sent_at":envelope.ts,
            "nonce":envelope.nonce,"ciphertext":envelope.ciphertext,"expires_at":expires_at}))
    };
    client.post(format!("{url}/rest/v1/{table}")).header("apikey", api_key).bearer_auth(access).header("Prefer", "return=minimal").json(&row).send().await.map_err(|e| e.to_string())?.error_for_status().map_err(|e| format!("Could not queue encrypted cloud message: {e}"))?;
    let payload = json!({"topic":topic,"event":"broadcast","payload":{"type":"broadcast","event":"macha_envelope","payload":{"envelope":envelope}},"ref":format!("msg-{}",envelope.seq),"join_ref":"join-1"});
    sink.lock().await.send(Message::Text(payload.to_string().into())).await.map_err(|e| e.to_string())
}

fn envelope_from_broadcast(text: &str) -> Option<Envelope> {
    let value: Value = serde_json::from_str(text).ok()?;
    if value.get("event")?.as_str()? != "broadcast" || value.pointer("/payload/event")?.as_str()? != "macha_envelope" { return None; }
    serde_json::from_value(value.pointer("/payload/payload/envelope")?.clone()).ok()
}

async fn read_queue(client: &reqwest::Client, url: &str, api_key: &str, access: &str, to_device: &str, account_user_id: Option<&str>) -> Result<Vec<Envelope>, String> {
    let target = if account_user_id.is_some() {
        format!("{url}/rest/v1/remote_queued_messages?select=id,ciphertext&to_device=eq.{to_device}&order=created_at.asc")
    } else {
        format!("{url}/rest/v1/queued_messages?select=id,message_type,seq,sent_at,nonce,ciphertext&to_device=eq.{to_device}&order=created_at.asc")
    };
    let response = client.get(target).header("apikey", api_key).bearer_auth(access).send().await.map_err(|e| e.to_string())?.error_for_status().map_err(|e| format!("Could not read queued cloud messages: {e}"))?;
    let rows: Vec<Value> = response.json().await.map_err(|e| e.to_string())?;
    rows.into_iter().map(|row| if account_user_id.is_some() {
        serde_json::from_str(row["ciphertext"].as_str().ok_or("Queued account message was malformed")?).map_err(|e| e.to_string())
    } else { serde_json::from_value(json!({
        "id":row["id"],"type":row["message_type"],"seq":row["seq"],"ts":row["sent_at"],"nonce":row["nonce"],"ciphertext":row["ciphertext"]
    })).map_err(|e| e.to_string()) }).collect()
}

async fn accept_envelope<S>(
    client: &reqwest::Client, url: &str, api_key: &str, access: &str, link: &str, own: &str, peer: &str,
    topic: &str, sink: &Arc<Mutex<S>>, codec: &Arc<Mutex<Codec>>,
    write: &mut (impl tokio::io::AsyncWrite + Unpin), envelope: Envelope, queued_id: Option<&str>,
    account_user_id: Option<&str>, counter_scope: Option<&str>,
) -> Result<(), String>
where S: Sink<Message> + Unpin, S::Error: std::fmt::Display {
    if let Some(id) = queued_id {
        let table = if account_user_id.is_some() { "remote_queued_messages" } else { "queued_messages" };
        client.delete(format!("{url}/rest/v1/{table}?id=eq.{id}")).header("apikey", api_key).bearer_auth(access).send().await.map_err(|e| e.to_string())?.error_for_status().map_err(|e| format!("Could not acknowledge queued cloud message: {e}"))?;
    }
    let kind = envelope.kind.clone();
    let text = {
        let mut codec = codec.lock().await;
        match codec.open(&envelope) {
            Ok(s) => { save_counters(&codec, counter_scope); s },
            Err(_) => return Ok(()),
        }
    };
    if kind == "data" {
        write.write_all(text.as_bytes()).await.map_err(|e| e.to_string())?;
        write.write_all(b"\n").await.map_err(|e| e.to_string())?;
        let ack = {
            let mut codec = codec.lock().await;
            let ack = codec.seal(&envelope.id, "ack")?;
            save_counters(&codec, counter_scope);
            ack
        };
        send_envelope(client, url, api_key, access, link, own, peer, topic, sink, ack, account_user_id).await?;
    }
    Ok(())
}

fn load_counters(account_scope: Option<&str>) -> (u64, u64) {
    if let Some(scope) = account_scope {
        let map = secrets_value("remote-cloud-counters").and_then(|raw| serde_json::from_str::<Value>(&raw).ok()).unwrap_or(Value::Null);
        return (map.pointer(&format!("/{scope}/sent")).and_then(Value::as_u64).unwrap_or(0), map.pointer(&format!("/{scope}/received")).and_then(Value::as_u64).unwrap_or(0));
    }
    (secrets_value("phone-cloud-send-seq").and_then(|v| v.parse().ok()).unwrap_or(0), secrets_value("phone-cloud-receive-seq").and_then(|v| v.parse().ok()).unwrap_or(0))
}

fn save_counters(codec: &Codec, account_scope: Option<&str>) {
    let (sent, received) = codec.counters();
    if let Some(scope) = account_scope {
        let mut map = secrets_value("remote-cloud-counters").and_then(|raw| serde_json::from_str::<serde_json::Map<String, Value>>(&raw).ok()).unwrap_or_default();
        map.insert(scope.to_string(), json!({"sent":sent,"received":received}));
        if let Ok(serialized) = serde_json::to_string(&map) { let _ = crate::secrets::set_internal("remote-cloud-counters", &serialized); }
        return;
    }
    let _ = crate::secrets::set_internal("phone-cloud-send-seq", &sent.to_string());
    let _ = crate::secrets::set_internal("phone-cloud-receive-seq", &received.to_string());
}

struct AbortOnDrop(JoinHandle<()>);
impl Drop for AbortOnDrop { fn drop(&mut self) { self.0.abort(); } }
