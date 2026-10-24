use std::collections::HashMap;
use std::sync::{LazyLock, Mutex};
use std::time::Duration;

use base64::Engine;
use crate::integrations;
use crate::island::WINDOW_LABEL;
use crate::log;
use serde::Serialize;
use sha2::{Digest, Sha256};
use tauri::{AppHandle, Emitter};
use windows::core::Interface;
use windows::Media::MediaPlaybackAutoRepeatMode;
use windows::Media::Control::{
    GlobalSystemMediaTransportControlsSession, GlobalSystemMediaTransportControlsSessionManager,
    GlobalSystemMediaTransportControlsSessionMediaProperties,
    GlobalSystemMediaTransportControlsSessionPlaybackStatus,
};
use windows::Storage::Streams::DataReader;
use windows::Win32::Foundation::CloseHandle;
use windows::Win32::Media::Audio::{
    eMultimedia, eRender, IAudioSessionControl2, IAudioSessionManager2, IMMDeviceEnumerator,
    ISimpleAudioVolume, MMDeviceEnumerator,
};
use windows::Win32::System::Com::{CoCreateInstance, CLSCTX_ALL};
use windows::Win32::System::Threading::{
    OpenProcess, QueryFullProcessImageNameW, PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION,
};
use windows::Win32::System::WinRT::{RoInitialize, RoUninitialize, RO_INIT_MULTITHREADED};

const POLL_INTERVAL: Duration = Duration::from_secs(4);

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SpotifyTrack {
    pub name: String,
    pub artists: String,
    pub album: String,
    pub duration_ms: u64,
    pub artwork_data_url: Option<String>,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SpotifySnapshot {
    pub connected: bool,
    pub playing: bool,
    pub progress_ms: u64,
    pub shuffle: bool,
    pub repeat: bool,
    pub volume: u8,
    pub spotify_app: bool,
    pub track: Option<SpotifyTrack>,
}

static ARTWORK_CACHE: LazyLock<Mutex<HashMap<String, String>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

struct WinRtApartment;

impl WinRtApartment {
    fn initialize() -> Result<Self, String> {
        unsafe { RoInitialize(RO_INIT_MULTITHREADED) }
            .map_err(|error| format!("Could not initialize Windows media controls: {error}"))?;
        Ok(Self)
    }
}

impl Drop for WinRtApartment {
    fn drop(&mut self) {
        unsafe { RoUninitialize() };
    }
}

fn winrt_error(context: &str, error: windows::core::Error) -> String {
    format!("{context}: {error}")
}

fn active_session(
    manager: &GlobalSystemMediaTransportControlsSessionManager,
) -> Result<Option<GlobalSystemMediaTransportControlsSession>, String> {
    let sessions = manager
        .GetSessions()
        .map_err(|error| winrt_error("Could not list Windows media sessions", error))?;
    let count = sessions
        .Size()
        .map_err(|error| winrt_error("Could not count Windows media sessions", error))?;
    if count == 0 {
        return Ok(None);
    }

    let current = manager.GetCurrentSession().ok();
    let mut fallback = current;
    for index in 0..count {
        let session = sessions
            .GetAt(index)
            .map_err(|error| winrt_error("Could not read a Windows media session", error))?;
        if session
            .SourceAppUserModelId()
            .is_ok_and(|id| id.to_string_lossy().to_ascii_lowercase().contains("spotify"))
        {
            return Ok(Some(session));
        }
        let status = session
            .GetPlaybackInfo()
            .and_then(|info| info.PlaybackStatus())
            .map_err(|error| winrt_error("Could not read media playback status", error))?;
        if status == GlobalSystemMediaTransportControlsSessionPlaybackStatus::Playing {
            fallback = Some(session);
        }
    }

    Ok(fallback.or_else(|| sessions.GetAt(0).ok()))
}

fn media_manager() -> Result<GlobalSystemMediaTransportControlsSessionManager, String> {
    GlobalSystemMediaTransportControlsSessionManager::RequestAsync()
        .and_then(|operation| operation.get())
        .map_err(|error| winrt_error("Could not access Windows media sessions", error))
}

fn read_snapshot() -> Result<SpotifySnapshot, String> {
    let _apartment = WinRtApartment::initialize()?;
    let manager = media_manager()?;
    let Some(session) = active_session(&manager)? else {
        return Ok(empty_snapshot());
    };

    let playback = session
        .GetPlaybackInfo()
        .map_err(|error| winrt_error("Could not read media playback info", error))?;
    let playing = playback
        .PlaybackStatus()
        .map_err(|error| winrt_error("Could not read media playback status", error))?
        == GlobalSystemMediaTransportControlsSessionPlaybackStatus::Playing;
    let timeline = session
        .GetTimelineProperties()
        .map_err(|error| winrt_error("Could not read media track position", error))?;
    let position = timeline
        .Position()
        .map_err(|error| winrt_error("Could not read media track position", error))?
        .Duration;
    let start = timeline
        .StartTime()
        .map_err(|error| winrt_error("Could not read media track start time", error))?
        .Duration;
    let end = timeline
        .EndTime()
        .map_err(|error| winrt_error("Could not read media track end time", error))?
        .Duration;
    let properties = session
        .TryGetMediaPropertiesAsync()
        .and_then(|operation| operation.get())
        .map_err(|error| winrt_error("Could not read current track details", error))?;
    let name = properties
        .Title()
        .map_err(|error| winrt_error("Could not read the current track title", error))?
        .to_string();
    let artists = properties
        .Artist()
        .map_err(|error| winrt_error("Could not read the current track artist", error))?
        .to_string();
    let album = properties
        .AlbumTitle()
        .map_err(|error| winrt_error("Could not read the current track album", error))?
        .to_string();
    let spotify_app = session
        .SourceAppUserModelId()
        .is_ok_and(|id| id.to_string_lossy().to_ascii_lowercase().contains("spotify"));
    let playback_info = session
        .GetPlaybackInfo()
        .map_err(|error| winrt_error("Could not read media playback controls", error))?;
    let shuffle = playback_info
        .IsShuffleActive()
        .and_then(|value| value.Value())
        .unwrap_or(false);
    let repeat = playback_info
        .AutoRepeatMode()
        .and_then(|value| value.Value())
        .is_ok_and(|mode| mode != MediaPlaybackAutoRepeatMode::None);
    let artwork_data_url = if spotify_app {
        artwork(&properties)
    } else {
        None
    };
    let track = (!name.trim().is_empty()).then_some(SpotifyTrack {
        name,
        artists,
        album,
        duration_ms: end.saturating_sub(start).max(0) as u64 / 10_000,
        artwork_data_url,
    });

    Ok(SpotifySnapshot {
        connected: true,
        playing,
        progress_ms: position.saturating_sub(start).max(0) as u64 / 10_000,
        shuffle,
        repeat,
        volume: if spotify_app {
            spotify_volume().unwrap_or(50)
        } else {
            50
        },
        spotify_app,
        track,
    })
}

fn empty_snapshot() -> SpotifySnapshot {
    SpotifySnapshot {
        connected: false,
        playing: false,
        progress_ms: 0,
        shuffle: false,
        repeat: false,
        volume: 50,
        spotify_app: false,
        track: None,
    }
}

fn artwork(
    properties: &GlobalSystemMediaTransportControlsSessionMediaProperties,
) -> Option<String> {
    let thumbnail = properties.Thumbnail().ok()?;
    let stream = thumbnail.OpenReadAsync().ok()?.get().ok()?;
    let size = stream.Size().ok()?;
    if size == 0 || size > 3 * 1024 * 1024 {
        return None;
    }
    let reader = DataReader::CreateDataReader(&stream).ok()?;
    let loaded = reader.LoadAsync(size as u32).ok()?.get().ok()?;
    let mut bytes = vec![0; loaded as usize];
    reader.ReadBytes(&mut bytes).ok()?;
    let mime = if bytes.starts_with(&[0xFF, 0xD8, 0xFF]) {
        "image/jpeg"
    } else if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        "image/png"
    } else if bytes.starts_with(b"RIFF") && bytes.get(8..12) == Some(b"WEBP") {
        "image/webp"
    } else {
        return None;
    };
    let key = format!("{mime}:{}", hex_digest(&bytes));
    if let Some(cached) = ARTWORK_CACHE.lock().ok()?.get(&key).cloned() {
        return Some(cached);
    }

    fn hex_digest(bytes: &[u8]) -> String {
        let digest = Sha256::digest(bytes);
        digest.iter().map(|byte| format!("{byte:02x}")).collect()
    }
    let url = format!(
        "data:{mime};base64,{}",
        base64::engine::general_purpose::STANDARD.encode(bytes)
    );
    let mut cache = ARTWORK_CACHE.lock().ok()?;
    if cache.len() >= 12 {
        cache.clear();
    }
    cache.insert(key, url.clone());
    Some(url)
}

fn spotify_audio() -> Vec<ISimpleAudioVolume> {
    let mut volumes = Vec::new();
    unsafe {
        let Ok(devices) =
            CoCreateInstance::<_, IMMDeviceEnumerator>(&MMDeviceEnumerator, None, CLSCTX_ALL)
        else {
            return volumes;
        };
        let Ok(device) = devices.GetDefaultAudioEndpoint(eRender, eMultimedia) else {
            return volumes;
        };
        let Ok(manager) = device.Activate::<IAudioSessionManager2>(CLSCTX_ALL, None) else {
            return volumes;
        };
        let Ok(sessions) = manager.GetSessionEnumerator() else {
            return volumes;
        };
        for index in 0..sessions.GetCount().unwrap_or(0) {
            let Ok(control) = sessions.GetSession(index) else {
                continue;
            };
            let Ok(control) = control.cast::<IAudioSessionControl2>() else {
                continue;
            };
            let pid = control.GetProcessId().unwrap_or(0);
            if pid != 0 && is_spotify_process(pid) {
                if let Ok(volume) = control.cast::<ISimpleAudioVolume>() {
                    volumes.push(volume);
                }
            }
        }
    }
    volumes
}

fn is_spotify_process(pid: u32) -> bool {
    unsafe {
        let Ok(process) = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) else {
            return false;
        };
        let mut buffer = [0u16; 1024];
        let mut length = buffer.len() as u32;
        let named = QueryFullProcessImageNameW(
            process,
            PROCESS_NAME_WIN32,
            windows::core::PWSTR(buffer.as_mut_ptr()),
            &mut length,
        );
        let _ = CloseHandle(process);
        if named.is_err() {
            return false;
        }
        String::from_utf16_lossy(&buffer[..length as usize])
            .rsplit(['\\', '/'])
            .next()
            .is_some_and(|name| name.eq_ignore_ascii_case("spotify.exe"))
    }
}

fn spotify_volume() -> Option<u8> {
    let volume = unsafe { spotify_audio().first()?.GetMasterVolume().ok()? };
    Some((volume * 100.0).round().clamp(0.0, 100.0) as u8)
}

fn run_media_action(
    session: &GlobalSystemMediaTransportControlsSession,
    action: &str,
    position_ms: Option<u64>,
) -> Result<bool, String> {
    match action {
        "play" => session
            .TryPlayAsync()
            .and_then(|operation| operation.get())
            .map_err(|error| winrt_error("Could not start media playback", error)),
        "pause" => session
            .TryPauseAsync()
            .and_then(|operation| operation.get())
            .map_err(|error| winrt_error("Could not pause media playback", error)),
        "next" => session
            .TrySkipNextAsync()
            .and_then(|operation| operation.get())
            .map_err(|error| winrt_error("Could not skip to the next track", error)),
        "previous" => session
            .TrySkipPreviousAsync()
            .and_then(|operation| operation.get())
            .map_err(|error| winrt_error("Could not return to the previous track", error)),
        "seek" => {
            let position_ms =
                position_ms.ok_or_else(|| "Track position is required.".to_string())?;
            let ticks = position_to_ticks(position_ms)?;
            session
                .TryChangePlaybackPositionAsync(ticks)
                .and_then(|operation| operation.get())
                .map_err(|error| winrt_error("Could not change the track position", error))
        }
        "shuffle" => session
            .TryChangeShuffleActiveAsync(position_ms.unwrap_or(0) != 0)
            .and_then(|operation| operation.get())
            .map_err(|error| winrt_error("Could not change Spotify shuffle", error)),
        "repeat" => session
            .TryChangeAutoRepeatModeAsync(if position_ms.unwrap_or(0) != 0 {
                MediaPlaybackAutoRepeatMode::List
            } else {
                MediaPlaybackAutoRepeatMode::None
            })
            .and_then(|operation| operation.get())
            .map_err(|error| winrt_error("Could not change Spotify repeat", error)),
        "volume" => {
            let percent = position_ms.unwrap_or(50).min(100);
            let level = (percent as f32) / 100.0;
            let volumes = spotify_audio();
            let mut updated = false;
            for volume in volumes {
                updated |= unsafe { volume.SetMasterVolume(level, std::ptr::null()).is_ok() };
            }
            Ok(updated)
        }
        _ => Err("Unsupported media playback action.".into()),
    }
}

fn position_to_ticks(position_ms: u64) -> Result<i64, String> {
    position_ms
        .checked_mul(10_000)
        .and_then(|value| i64::try_from(value).ok())
        .ok_or_else(|| "Track position is out of range.".to_string())
}

#[tauri::command]
pub fn open_spotify() -> Result<(), String> {
    let desktop = std::env::var_os("APPDATA")
        .map(|base| std::path::PathBuf::from(base).join(r"Spotify\Spotify.exe"))
        .is_some_and(|path| path.is_file());
    let store = std::env::var_os("LOCALAPPDATA")
        .map(|base| std::path::PathBuf::from(base).join(r"Microsoft\WindowsApps\Spotify.exe"))
        .is_some_and(|path| std::fs::symlink_metadata(path).is_ok());
    let url = if desktop || store {
        "spotify:"
    } else {
        "https://www.spotify.com/download/windows/"
    };
    crate::platform::open_url(url)
}

#[tauri::command]
pub async fn spotify_control(action: String, position_ms: Option<u64>) -> Result<(), String> {
    tauri::async_runtime::spawn_blocking(move || {
        let _apartment = WinRtApartment::initialize()?;
        let manager = media_manager()?;
        let session = active_session(&manager)?.ok_or_else(|| {
            "No active media session. Start playback in Spotify or another media app first."
                .to_string()
        })?;
        let supported = run_media_action(&session, &action, position_ms)?;
        if supported {
            Ok(())
        } else {
            Err(format!(
                "The active media session does not support the {action} action."
            ))
        }
    })
    .await
    .map_err(|error| format!("Windows media-control task failed: {error}"))?
}

pub fn start(app: AppHandle) {
    tauri::async_runtime::spawn(async move {
        let mut ticker = tokio::time::interval(POLL_INTERVAL);
        loop {
            ticker.tick().await;
            if integrations::PAUSED.load(std::sync::atomic::Ordering::Relaxed)
                || !integrations::enabled(&app, "integration_spotify")
            {
                continue;
            }
            if let Err(error) = poll_once(app.clone()).await {
                log::line(format!("Windows media-session refresh failed: {error}"));
            }
        }
    });
}

pub async fn poll_once(app: AppHandle) -> Result<SpotifySnapshot, String> {
    match publish_snapshot(&app).await {
        Ok(snapshot) => Ok(snapshot),
        Err(error) => {
            log::line(format!("Windows media-session refresh failed: {error}"));
            let _ = app.emit_to(WINDOW_LABEL, "spotify-error", error.clone());
            Err(error)
        }
    }
}

async fn publish_snapshot(app: &AppHandle) -> Result<SpotifySnapshot, String> {
    let snapshot = tauri::async_runtime::spawn_blocking(read_snapshot)
        .await
        .map_err(|error| format!("Windows media-session task failed: {error}"))??;
    app.emit_to(WINDOW_LABEL, "spotify-update", snapshot.clone())
        .map_err(|error| format!("Could not update the music player: {error}"))?;
    Ok(snapshot)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_snapshot_has_no_connected_player_or_track() {
        let snapshot = empty_snapshot();

        assert!(!snapshot.connected);
        assert!(!snapshot.playing);
        assert_eq!(snapshot.progress_ms, 0);
        assert!(snapshot.track.is_none());
    }

    #[test]
    fn seek_positions_convert_milliseconds_to_winrt_ticks() {
        assert_eq!(position_to_ticks(12_345).unwrap(), 123_450_000);
        assert_eq!(
            position_to_ticks(u64::MAX),
            Err("Track position is out of range.".into())
        );
    }
}
