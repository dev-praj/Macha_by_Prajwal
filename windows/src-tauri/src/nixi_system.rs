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

pub fn capture_screen_region(
    x: i32,
    y: i32,
    width: u32,
    height: u32,
) -> Result<String, String> {
    if width < 12 || height < 12 || width > 7680 || height > 4320 {
        return Err("The selected screen region is too small or too large.".into());
    }
    let pixels = u64::from(width) * u64::from(height);
    if pixels > 16_777_216 {
        return Err("Select a smaller screen region and try again.".into());
    }

    #[cfg(target_os = "windows")]
    {
        let script = format!(
            r#"
Add-Type -AssemblyName System.Windows.Forms
Add-Type -AssemblyName System.Drawing
$x = {x}
$y = {y}
$width = {width}
$height = {height}
$bounds = [System.Windows.Forms.SystemInformation]::VirtualScreen
if ($x -lt $bounds.Left -or $y -lt $bounds.Top -or
    ($x + $width) -gt $bounds.Right -or ($y + $height) -gt $bounds.Bottom) {{
    throw "The selected region is outside the visible desktop."
}}
$source = New-Object System.Drawing.Bitmap($width, $height)
$graphics = [System.Drawing.Graphics]::FromImage($source)
$output = $null
$outputGraphics = $null
$stream = New-Object System.IO.MemoryStream
try {{
    $graphics.CopyFromScreen(
        [System.Drawing.Point]::new($x, $y),
        [System.Drawing.Point]::Empty,
        [System.Drawing.Size]::new($width, $height)
    )
    $scale = [Math]::Min(1.0, [Math]::Min(1200.0 / $width, 900.0 / $height))
    if ($scale -lt 1.0) {{
        $outputWidth = [Math]::Max(1, [int][Math]::Round($width * $scale))
        $outputHeight = [Math]::Max(1, [int][Math]::Round($height * $scale))
        $output = New-Object System.Drawing.Bitmap($outputWidth, $outputHeight)
        $outputGraphics = [System.Drawing.Graphics]::FromImage($output)
        $outputGraphics.DrawImage($source, 0, 0, $outputWidth, $outputHeight)
        $image = $output
    }} else {{
        $image = $source
    }}
    $codec = [System.Drawing.Imaging.ImageCodecInfo]::GetImageEncoders() |
        Where-Object {{ $_.MimeType -eq "image/jpeg" }} |
        Select-Object -First 1
    $parameters = New-Object System.Drawing.Imaging.EncoderParameters(1)
    $parameters.Param[0] = New-Object System.Drawing.Imaging.EncoderParameter(
        [System.Drawing.Imaging.Encoder]::Quality, [long]72
    )
    $image.Save($stream, $codec, $parameters)
    [Console]::Out.Write("data:image/jpeg;base64," + [Convert]::ToBase64String($stream.ToArray()))
}} finally {{
    $stream.Dispose()
    if ($outputGraphics) {{ $outputGraphics.Dispose() }}
    if ($output) {{ $output.Dispose() }}
    $graphics.Dispose()
    $source.Dispose()
}}
"#
        );

        let output = Command::new("powershell")
            .args(["-NoProfile", "-NonInteractive", "-Command", &script])
            .output()
            .map_err(|error| format!("Could not capture the selected screen region: {error}"))?;
        if !output.status.success() {
            let detail = String::from_utf8_lossy(&output.stderr).trim().to_string();
            return Err(if detail.is_empty() {
                "Could not capture the selected screen region.".into()
            } else {
                format!("Could not capture the selected screen region: {detail}")
            });
        }
        if output.stdout.len() > 2_500_000 {
            return Err("The selected image is too large to send. Select a smaller region.".into());
        }
        let data_url = String::from_utf8(output.stdout)
            .map_err(|error| format!("Screen capture returned invalid image data: {error}"))?;
        let data_url = data_url.trim();
        if !data_url.starts_with("data:image/jpeg;base64,") {
            return Err("Screen capture did not return a valid image.".into());
        }
        Ok(data_url.to_string())
    }

    #[cfg(not(target_os = "windows"))]
    {
        let _ = (x, y);
        Err("Screen-region capture is only supported on Windows.".into())
    }
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

#[tauri::command]
pub async fn nixi_capture_screen_region(
    x: i32,
    y: i32,
    width: u32,
    height: u32,
) -> Result<String, String> {
    tauri::async_runtime::spawn_blocking(move || capture_screen_region(x, y, width, height))
        .await
        .map_err(|error| format!("Screen-region capture task failed: {error}"))?
}

#[cfg(test)]
mod tests {
    use super::capture_screen_region;

    #[test]
    fn rejects_screen_regions_that_are_too_small_or_large() {
        assert!(capture_screen_region(0, 0, 11, 20).is_err());
        assert!(capture_screen_region(0, 0, 4097, 4096).is_err());
        assert!(capture_screen_region(0, 0, 7681, 100).is_err());
    }
}
