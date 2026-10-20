use std::time::Duration;

use crate::integrations;
use crate::island::WINDOW_LABEL;
use crate::log;
use serde::Serialize;
use tauri::{AppHandle, Emitter};
use windows::Media::Control::{
    GlobalSystemMediaTransportControlsSession, GlobalSystemMediaTransportControlsSessionManager,
    GlobalSystemMediaTransportControlsSessionPlaybackStatus,
};
use windows::Win32::System::WinRT::{RoInitialize, RoUninitialize, RO_INIT_MULTITHREADED};

const POLL_INTERVAL: Duration = Duration::from_secs(4);

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SpotifyTrack {
    pub name: String,
    pub artists: String,
    pub duration_ms: u64,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SpotifySnapshot {
    pub connected: bool,
    pub playing: bool,
    pub progress_ms: u64,
    pub track: Option<SpotifyTrack>,
}

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
    for index in 0..count {
        let session = sessions
            .GetAt(index)
            .map_err(|error| winrt_error("Could not read a Windows media session", error))?;
        let status = session
            .GetPlaybackInfo()
            .and_then(|info| info.PlaybackStatus())
            .map_err(|error| winrt_error("Could not read media playback status", error))?;
        if status == GlobalSystemMediaTransportControlsSessionPlaybackStatus::Playing {
            return Ok(Some(session));
        }
    }

    Ok(current.or_else(|| sessions.GetAt(0).ok()))
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
    let track = (!name.trim().is_empty()).then_some(SpotifyTrack {
        name,
        artists,
        duration_ms: end.saturating_sub(start).max(0) as u64 / 10_000,
    });

    Ok(SpotifySnapshot {
        connected: true,
        playing,
        progress_ms: position.saturating_sub(start).max(0) as u64 / 10_000,
        track,
    })
}

fn empty_snapshot() -> SpotifySnapshot {
    SpotifySnapshot {
        connected: false,
        playing: false,
        progress_ms: 0,
        track: None,
    }
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
