// Nixi System Controls — Phase 5E
// Handles volume adjustments with intensity parsing, media keys,
// Windows dark/light mode registry personalization, and screenshots.

use std::path::PathBuf;
use std::process::Command;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ScreenshotResult {
    pub success: bool,
    pub path: String,
    pub message: String,
}

/// Changes system volume by simulating volume keys.
/// `steps`: number of increments (typically 1 step = 2% on Windows).
pub fn adjust_volume(action: &str, steps: u8) -> Result<String, String> {
    #[cfg(target_os = "windows")]
    {
        use windows::Win32::UI::Input::KeyboardAndMouse::{
            VK_VOLUME_DOWN, VK_VOLUME_MUTE, VK_VOLUME_UP,
        };

        let vk = match action {
            "up" => VK_VOLUME_UP,
            "down" => VK_VOLUME_DOWN,
            "mute" | "unmute" => VK_VOLUME_MUTE,
            _ => return Err(format!("Unknown volume action: {action}")),
        };

        let count = if action == "mute" || action == "unmute" { 1 } else { steps.max(1) };
        for _ in 0..count {
            send_vk(vk.0);
            std::thread::sleep(std::time::Duration::from_millis(15));
        }

        let desc = match action {
            "up" => format!("Volume increased (+{}%)", count * 2),
            "down" => format!("Volume decreased (-{}%)", count * 2),
            "mute" => "Volume muted".to_string(),
            "unmute" => "Volume unmuted".to_string(),
            _ => "Volume adjusted".to_string(),
        };
        crate::log::line(format!("nixi_system: {desc}"));
        Ok(desc)
    }

    #[cfg(not(target_os = "windows"))]
    {
        let _ = (action, steps);
        Err("Volume adjustment is only supported on Windows".into())
    }
}

/// Sends media key: "play_pause", "next", "previous"
pub fn send_media_key(action: &str) -> Result<String, String> {
    #[cfg(target_os = "windows")]
    {
        use windows::Win32::UI::Input::KeyboardAndMouse::{
            VK_MEDIA_NEXT_TRACK, VK_MEDIA_PLAY_PAUSE, VK_MEDIA_PREV_TRACK,
        };

        let vk = match action {
            "play" | "pause" | "play_pause" => VK_MEDIA_PLAY_PAUSE,
            "next" => VK_MEDIA_NEXT_TRACK,
            "previous" => VK_MEDIA_PREV_TRACK,
            _ => return Err(format!("Unknown media action: {action}")),
        };

        send_vk(vk.0);
        let desc = format!("Media action: {action}");
        crate::log::line(format!("nixi_system: {desc}"));
        Ok(desc)
    }

    #[cfg(not(target_os = "windows"))]
    {
        let _ = action;
        Err("Media control is only supported on Windows".into())
    }
}

/// Toggles Windows dark mode by updating the Personalize registry keys.
pub fn toggle_dark_mode(enable: Option<bool>) -> Result<bool, String> {
    #[cfg(target_os = "windows")]
    {
        // Check current state if toggle requested
        let current_is_light = {
            let out = Command::new("powershell")
                .args([
                    "-NoProfile",
                    "-NonInteractive",
                    "-Command",
                    "Get-ItemPropertyValue -Path 'HKCU:\\Software\\Microsoft\\Windows\\CurrentVersion\\Themes\\Personalize' -Name 'AppsUseLightTheme' -ErrorAction SilentlyContinue",
                ])
                .output()
                .map_err(|e| format!("Could not read registry: {e}"))?;
            let val = String::from_utf8_lossy(&out.stdout).trim().to_string();
            val != "0"
        };

        let set_to_light = match enable {
            Some(dark) => !dark,
            None => !current_is_light,
        };

        let val = if set_to_light { "1" } else { "0" };
        let script = format!(
            "$path = 'HKCU:\\Software\\Microsoft\\Windows\\CurrentVersion\\Themes\\Personalize'; \
             Set-ItemProperty -Path $path -Name 'AppsUseLightTheme' -Value {val}; \
             Set-ItemProperty -Path $path -Name 'SystemUsesLightTheme' -Value {val}"
        );

        let status = Command::new("powershell")
            .args(["-NoProfile", "-NonInteractive", "-Command", &script])
            .status()
            .map_err(|e| format!("Failed to set theme: {e}"))?;

        if !status.success() {
            return Err("Failed to update registry settings".into());
        }

        let is_dark = !set_to_light;
        crate::log::line(format!("nixi_system: Dark mode set to {is_dark}"));
        Ok(is_dark)
    }

    #[cfg(not(target_os = "windows"))]
    {
        let _ = enable;
        Err("Theme personalization is only supported on Windows".into())
    }
}

/// Captures a screenshot and saves it to `%USERPROFILE%\Pictures\Screenshots`.
pub fn capture_screenshot() -> Result<ScreenshotResult, String> {
    let mut screenshots_dir = match std::env::var_os("USERPROFILE") {
        Some(p) => PathBuf::from(p).join("Pictures").join("Screenshots"),
        None => std::env::temp_dir(),
    };

    if !screenshots_dir.exists() {
        let _ = std::fs::create_dir_all(&screenshots_dir);
    }
    if !screenshots_dir.exists() {
        screenshots_dir = std::env::temp_dir();
    }

    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    let filename = format!("Screenshot_{secs}.png");
    let dest = screenshots_dir.join(&filename);
    let dest_str = dest.to_string_lossy().to_string();

    #[cfg(target_os = "windows")]
    {
        let script = format!(
            "Add-Type -AssemblyName System.Windows.Forms; \
             Add-Type -AssemblyName System.Drawing; \
             $bounds = [System.Windows.Forms.Screen]::PrimaryScreen.Bounds; \
             $bmp = New-Object System.Drawing.Bitmap $bounds.Width, $bounds.Height; \
             $graphics = [System.Drawing.Graphics]::FromImage($bmp); \
             $graphics.CopyFromScreen($bounds.Location, [System.Drawing.Point]::Empty, $bounds.Size); \
             $bmp.Save('{dest_str}', [System.Drawing.Imaging.ImageFormat]::Png); \
             $graphics.Dispose(); \
             $bmp.Dispose()"
        );

        let status = Command::new("powershell")
            .args(["-NoProfile", "-NonInteractive", "-Command", &script])
            .status()
            .map_err(|e| format!("Failed to run screenshot capture: {e}"))?;

        if !status.success() || !dest.exists() {
            return Err("Screenshot capture script failed to produce file".into());
        }

        crate::log::line(format!("nixi_system: saved screenshot to {dest_str}"));
        Ok(ScreenshotResult {
            success: true,
            path: dest_str.clone(),
            message: format!("Saved screenshot to {dest_str}"),
        })
    }

    #[cfg(not(target_os = "windows"))]
    {
        Err("Screenshot is only supported on Windows".into())
    }
}

#[cfg(target_os = "windows")]
fn send_vk(vk: u16) {
    use windows::Win32::UI::Input::KeyboardAndMouse::{
        SendInput, INPUT, INPUT_0, INPUT_KEYBOARD, KEYBDINPUT, KEYEVENTF_KEYUP, VIRTUAL_KEY,
    };
    let inputs = [
        INPUT {
            r#type: INPUT_KEYBOARD,
            Anonymous: INPUT_0 {
                ki: KEYBDINPUT {
                    wVk: VIRTUAL_KEY(vk),
                    wScan: 0,
                    dwFlags: Default::default(),
                    time: 0,
                    dwExtraInfo: 0,
                },
            },
        },
        INPUT {
            r#type: INPUT_KEYBOARD,
            Anonymous: INPUT_0 {
                ki: KEYBDINPUT {
                    wVk: VIRTUAL_KEY(vk),
                    wScan: 0,
                    dwFlags: KEYEVENTF_KEYUP,
                    time: 0,
                    dwExtraInfo: 0,
                },
            },
        },
    ];
    unsafe {
        SendInput(&inputs, std::mem::size_of::<INPUT>() as i32);
    }
}

// ── Tauri Commands ──────────────────────────────────────────────────────────

#[tauri::command]
pub fn nixi_adjust_volume(action: String, steps: Option<u8>) -> Result<String, String> {
    adjust_volume(&action, steps.unwrap_or(2))
}

#[tauri::command]
pub fn nixi_send_media(action: String) -> Result<String, String> {
    send_media_key(&action)
}

#[tauri::command]
pub fn nixi_toggle_theme(enable_dark: Option<bool>) -> Result<bool, String> {
    toggle_dark_mode(enable_dark)
}

#[tauri::command]
pub fn nixi_capture_screenshot() -> Result<ScreenshotResult, String> {
    capture_screenshot()
}
