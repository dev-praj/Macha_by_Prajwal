use serde_json::Value;

#[cfg(windows)]
mod imp {
    use base64::engine::general_purpose::STANDARD;
    use base64::Engine;
    use serde_json::Value;
    use std::io::Read;
    use std::process::{Command, Stdio};
    use std::thread;
    use std::time::{Duration, Instant};

    const INPUT_MOUSE: u32 = 0;
    const INPUT_KEYBOARD: u32 = 1;
    const MOUSEEVENTF_LEFTDOWN: u32 = 0x0002;
    const MOUSEEVENTF_LEFTUP: u32 = 0x0004;
    const MOUSEEVENTF_RIGHTDOWN: u32 = 0x0008;
    const MOUSEEVENTF_RIGHTUP: u32 = 0x0010;
    const MOUSEEVENTF_MIDDLEDOWN: u32 = 0x0020;
    const MOUSEEVENTF_MIDDLEUP: u32 = 0x0040;
    const MOUSEEVENTF_WHEEL: u32 = 0x0800;
    const MOUSEEVENTF_HWHEEL: u32 = 0x1000;
    const KEYEVENTF_KEYUP: u32 = 0x0002;
    const KEYEVENTF_UNICODE: u32 = 0x0004;
    const SW_RESTORE: i32 = 9;
    const SWP_SHOWWINDOW: u32 = 0x0040;
    const MAX_WINDOWS: usize = 50;
    const MAX_OUTPUT: usize = 32_000;
    #[repr(C)]
    #[derive(Clone, Copy)]
    struct MouseInput {
        dx: i32,
        dy: i32,
        mouse_data: u32,
        flags: u32,
        time: u32,
        extra_info: usize,
    }

    #[repr(C)]
    #[derive(Clone, Copy)]
    struct KeyboardInput {
        virtual_key: u16,
        scan: u16,
        flags: u32,
        time: u32,
        extra_info: usize,
    }

    #[repr(C)]
    union InputData {
        mouse: MouseInput,
        keyboard: KeyboardInput,
    }

    #[repr(C)]
    struct Input {
        kind: u32,
        data: InputData,
    }

    #[repr(C)]
    #[derive(Default, Clone, Copy)]
    struct Rect {
        left: i32,
        top: i32,
        right: i32,
        bottom: i32,
    }

    #[repr(transparent)]
    #[derive(Clone, Copy, PartialEq, Eq)]
    struct Hwnd(isize);

    type Bool = i32;
    type EnumWindowsCallback = unsafe extern "system" fn(Hwnd, isize) -> Bool;

    #[link(name = "user32")]
    unsafe extern "system" {
        fn SendInput(count: u32, inputs: *const Input, size: i32) -> u32;
        fn SetCursorPos(x: i32, y: i32) -> Bool;
        fn GetCursorPos(point: *mut Point) -> Bool;
        fn GetSystemMetrics(index: i32) -> i32;
        fn EnumWindows(callback: Option<EnumWindowsCallback>, data: isize) -> Bool;
        fn IsWindowVisible(window: Hwnd) -> Bool;
        fn GetWindowTextLengthW(window: Hwnd) -> i32;
        fn GetWindowTextW(window: Hwnd, text: *mut u16, max_count: i32) -> i32;
        fn GetForegroundWindow() -> Hwnd;
        fn SetForegroundWindow(window: Hwnd) -> Bool;
        fn ShowWindow(window: Hwnd, command: i32) -> Bool;
        fn GetWindowRect(window: Hwnd, rect: *mut Rect) -> Bool;
        fn SetWindowPos(
            window: Hwnd,
            insert_after: Hwnd,
            x: i32,
            y: i32,
            width: i32,
            height: i32,
            flags: u32,
        ) -> Bool;
    }

    #[repr(C)]
    struct Point {
        x: i32,
        y: i32,
    }

    struct WindowEntry {
        handle: Hwnd,
        title: String,
    }

    struct BoundedOutput {
        bytes: Vec<u8>,
        truncated: bool,
    }

    pub(super) fn execute(name: &str, arguments: &Value) -> Result<String, String> {
        match name {
            "windows_list" => list_windows(),
            "windows_click" => click(
                integer(arguments, "x")?,
                integer(arguments, "y")?,
                string(arguments, "button")?,
                integer_or(arguments, "clicks", 1)?,
            ),
            "windows_type" => type_text(
                integer(arguments, "x")?,
                integer(arguments, "y")?,
                string(arguments, "text")?,
                boolean_or(arguments, "clear", false)?,
                boolean_or(arguments, "pressEnter", false)?,
            ),
            "windows_move" => move_mouse(
                integer(arguments, "x")?,
                integer(arguments, "y")?,
                boolean_or(arguments, "drag", false)?,
            ),
            "windows_scroll" => scroll(arguments),
            "windows_shortcut" => shortcut(string(arguments, "shortcut")?),
            "windows_switch_window" => activate_window(string(arguments, "title")?),
            "windows_resize_window" => resize_window(
                string(arguments, "title")?,
                integer(arguments, "x")?,
                integer(arguments, "y")?,
                integer(arguments, "width")?,
                integer(arguments, "height")?,
            ),
            "powershell_command" => run_powershell(
                string(arguments, "command")?,
                integer_or(arguments, "timeout", 10)? as u64,
            ),
            _ => Err("Unsupported Windows automation action.".into()),
        }
    }

    fn string<'a>(arguments: &'a Value, key: &str) -> Result<&'a str, String> {
        arguments
            .get(key)
            .and_then(Value::as_str)
            .ok_or_else(|| format!("The {key} value is missing or invalid."))
    }

    fn integer(arguments: &Value, key: &str) -> Result<i32, String> {
        arguments
            .get(key)
            .and_then(Value::as_i64)
            .and_then(|number| i32::try_from(number).ok())
            .ok_or_else(|| format!("The {key} value is missing or invalid."))
    }

    fn integer_or(arguments: &Value, key: &str, default: i32) -> Result<i32, String> {
        match arguments.get(key) {
            Some(_) => integer(arguments, key),
            None => Ok(default),
        }
    }

    fn boolean_or(arguments: &Value, key: &str, default: bool) -> Result<bool, String> {
        match arguments.get(key) {
            Some(value) => value
                .as_bool()
                .ok_or_else(|| format!("The {key} value must be true or false.")),
            None => Ok(default),
        }
    }

    fn screen_size() -> (i32, i32) {
        unsafe { (GetSystemMetrics(0), GetSystemMetrics(1)) }
    }

    fn validate_point(x: i32, y: i32) -> Result<(), String> {
        let (width, height) = screen_size();
        if x < 0 || y < 0 || x >= width || y >= height {
            return Err(format!(
                "Coordinates ({x}, {y}) are outside the primary display ({width}x{height})."
            ));
        }
        Ok(())
    }

    fn send_input(input: Input) -> Result<(), String> {
        let sent = unsafe { SendInput(1, &input, std::mem::size_of::<Input>() as i32) };
        if sent != 1 {
            return Err("Windows did not accept the requested mouse or keyboard input.".into());
        }
        Ok(())
    }

    fn mouse(flags: u32, data: u32) -> Result<(), String> {
        send_input(Input {
            kind: INPUT_MOUSE,
            data: InputData {
                mouse: MouseInput {
                    dx: 0,
                    dy: 0,
                    mouse_data: data,
                    flags,
                    time: 0,
                    extra_info: 0,
                },
            },
        })
    }

    fn set_cursor(x: i32, y: i32) -> Result<(), String> {
        validate_point(x, y)?;
        if unsafe { SetCursorPos(x, y) } == 0 {
            return Err("Windows could not move the pointer to that location.".into());
        }
        Ok(())
    }

    fn click(x: i32, y: i32, button: &str, clicks: i32) -> Result<String, String> {
        set_cursor(x, y)?;
        let (down, up) = match button {
            "left" => (MOUSEEVENTF_LEFTDOWN, MOUSEEVENTF_LEFTUP),
            "right" => (MOUSEEVENTF_RIGHTDOWN, MOUSEEVENTF_RIGHTUP),
            "middle" => (MOUSEEVENTF_MIDDLEDOWN, MOUSEEVENTF_MIDDLEUP),
            _ => return Err("Unsupported mouse button.".into()),
        };
        for _ in 0..clicks {
            mouse(down, 0)?;
            mouse(up, 0)?;
            if clicks > 1 {
                thread::sleep(Duration::from_millis(70));
            }
        }
        Ok(format!("{button} click at ({x}, {y}) completed."))
    }

    fn move_mouse(x: i32, y: i32, drag: bool) -> Result<String, String> {
        validate_point(x, y)?;
        let mut start = Point { x: 0, y: 0 };
        if drag && unsafe { GetCursorPos(&mut start) } == 0 {
            return Err("Windows could not read the current pointer location.".into());
        }
        if drag {
            mouse(MOUSEEVENTF_LEFTDOWN, 0)?;
            let moved = set_cursor(x, y);
            let released = mouse(MOUSEEVENTF_LEFTUP, 0);
            moved?;
            released?;
            Ok(format!(
                "Dragged from ({}, {}) to ({x}, {y}).",
                start.x, start.y
            ))
        } else {
            set_cursor(x, y)?;
            Ok(format!("Moved the pointer to ({x}, {y})."))
        }
    }

    fn type_text(
        x: i32,
        y: i32,
        text: &str,
        clear: bool,
        press_enter: bool,
    ) -> Result<String, String> {
        set_cursor(x, y)?;
        mouse(MOUSEEVENTF_LEFTDOWN, 0)?;
        mouse(MOUSEEVENTF_LEFTUP, 0)?;
        if clear {
            shortcut("ctrl+a")?;
        }
        for unit in text.encode_utf16() {
            send_input(key_input(unit, KEYEVENTF_UNICODE))?;
            send_input(key_input(unit, KEYEVENTF_UNICODE | KEYEVENTF_KEYUP))?;
        }
        if press_enter {
            press_key(0x0D)?;
        }
        Ok(format!(
            "Typed {} characters at ({x}, {y}).",
            text.chars().count()
        ))
    }

    fn scroll(arguments: &Value) -> Result<String, String> {
        if let (Some(x), Some(y)) = (arguments.get("x"), arguments.get("y")) {
            let x = x
                .as_i64()
                .and_then(|value| i32::try_from(value).ok())
                .ok_or_else(|| "The x coordinate is invalid.".to_string())?;
            let y = y
                .as_i64()
                .and_then(|value| i32::try_from(value).ok())
                .ok_or_else(|| "The y coordinate is invalid.".to_string())?;
            set_cursor(x, y)?;
        }
        let axis = string(arguments, "axis")?;
        let direction = string(arguments, "direction")?;
        let steps = integer_or(arguments, "steps", 1)?;
        let horizontal = axis == "horizontal";
        let positive = matches!(
            (axis, direction),
            ("vertical", "up") | ("horizontal", "right")
        );
        let flags = if horizontal {
            MOUSEEVENTF_HWHEEL
        } else {
            MOUSEEVENTF_WHEEL
        };
        for _ in 0..steps {
            mouse(flags, (if positive { 120i32 } else { -120i32 }) as u32)?;
        }
        Ok(format!("Scrolled {direction} {steps} step(s)."))
    }

    fn key_input(key: u16, flags: u32) -> Input {
        Input {
            kind: INPUT_KEYBOARD,
            data: InputData {
                keyboard: KeyboardInput {
                    virtual_key: if flags & KEYEVENTF_UNICODE != 0 {
                        0
                    } else {
                        key
                    },
                    scan: if flags & KEYEVENTF_UNICODE != 0 {
                        key
                    } else {
                        0
                    },
                    flags,
                    time: 0,
                    extra_info: 0,
                },
            },
        }
    }

    fn press_key(key: u16) -> Result<(), String> {
        send_input(key_input(key, 0))?;
        send_input(key_input(key, KEYEVENTF_KEYUP))
    }

    fn shortcut(value: &str) -> Result<String, String> {
        let keys: Vec<u16> = value
            .split('+')
            .map(str::trim)
            .map(virtual_key)
            .collect::<Result<_, _>>()?;
        if keys.is_empty() || keys.len() > 4 {
            return Err("Shortcuts must contain one to four supported keys.".into());
        }
        for key in &keys {
            send_input(key_input(*key, 0))?;
        }
        for key in keys.iter().rev() {
            send_input(key_input(*key, KEYEVENTF_KEYUP))?;
        }
        Ok(format!("Pressed {value}."))
    }

    fn virtual_key(name: &str) -> Result<u16, String> {
        let key = name.to_ascii_lowercase();
        let named = match key.as_str() {
            "ctrl" | "control" => Some(0x11),
            "shift" => Some(0x10),
            "alt" => Some(0x12),
            "win" | "windows" | "meta" => Some(0x5B),
            "enter" | "return" => Some(0x0D),
            "escape" | "esc" => Some(0x1B),
            "tab" => Some(0x09),
            "space" => Some(0x20),
            "backspace" => Some(0x08),
            "delete" | "del" => Some(0x2E),
            "home" => Some(0x24),
            "end" => Some(0x23),
            "pageup" => Some(0x21),
            "pagedown" => Some(0x22),
            "up" => Some(0x26),
            "down" => Some(0x28),
            "left" => Some(0x25),
            "right" => Some(0x27),
            "insert" => Some(0x2D),
            _ => None,
        };
        if let Some(code) = named {
            return Ok(code);
        }
        if key.len() == 1 {
            let byte = key.as_bytes()[0];
            if byte.is_ascii_alphanumeric() {
                return Ok(byte.to_ascii_uppercase() as u16);
            }
        }
        if let Some(number) = key
            .strip_prefix('f')
            .and_then(|value| value.parse::<u16>().ok())
        {
            if (1..=24).contains(&number) {
                return Ok(0x70 + number - 1);
            }
        }
        Err(format!("Unsupported shortcut key: {name}."))
    }

    fn enumerate_windows() -> Vec<WindowEntry> {
        unsafe extern "system" fn collect(window: Hwnd, data: isize) -> Bool {
            if IsWindowVisible(window) == 0 {
                return 1;
            }
            let length = GetWindowTextLengthW(window);
            if length <= 0 {
                return 1;
            }
            let mut buffer = vec![0u16; (length as usize).saturating_add(1).min(2048)];
            let copied = GetWindowTextW(window, buffer.as_mut_ptr(), buffer.len() as i32);
            if copied <= 0 {
                return 1;
            }
            let title = String::from_utf16_lossy(&buffer[..copied as usize])
                .trim()
                .to_string();
            if title.is_empty() || title.to_ascii_lowercase().contains("coucou") {
                return 1;
            }
            let windows = &mut *(data as *mut Vec<WindowEntry>);
            if windows.len() < MAX_WINDOWS {
                windows.push(WindowEntry {
                    handle: window,
                    title,
                });
            }
            1
        }
        let mut windows = Vec::new();
        unsafe {
            EnumWindows(
                Some(collect),
                &mut windows as *mut Vec<WindowEntry> as isize,
            );
        }
        windows
    }

    fn list_windows() -> Result<String, String> {
        let windows = enumerate_windows();
        if windows.is_empty() {
            return Ok("No visible application windows were found.".into());
        }
        let foreground = unsafe { GetForegroundWindow() };
        let mut output = String::new();
        for entry in windows {
            let active = if entry.handle == foreground {
                " (active)"
            } else {
                ""
            };
            output.push_str(&format!("- {}{}\n", entry.title, active));
        }
        Ok(output)
    }

    fn matching_window(title: &str) -> Result<WindowEntry, String> {
        let matches: Vec<WindowEntry> = enumerate_windows()
            .into_iter()
            .filter(|window| window.title.to_lowercase().contains(&title.to_lowercase()))
            .collect();
        match matches.len() {
            0 => Err(format!("No visible window title contains “{title}”.")),
            1 => Ok(matches.into_iter().next().unwrap()),
            count => Err(format!(
                "More than one visible window title contains “{title}” ({count} matches). Use a more specific title."
            )),
        }
    }

    fn activate_window(title: &str) -> Result<String, String> {
        let window = matching_window(title)?;
        unsafe {
            ShowWindow(window.handle, SW_RESTORE);
            if SetForegroundWindow(window.handle) == 0 {
                return Err(format!("Windows could not activate “{}”.", window.title));
            }
        }
        Ok(format!("Switched to “{}”.", window.title))
    }

    fn resize_window(
        title: &str,
        x: i32,
        y: i32,
        width: i32,
        height: i32,
    ) -> Result<String, String> {
        let (screen_width, screen_height) = screen_size();
        if x < 0
            || y < 0
            || x.saturating_add(width) > screen_width
            || y.saturating_add(height) > screen_height
        {
            return Err(format!("The requested window bounds exceed the primary display ({screen_width}x{screen_height})."));
        }
        let window = matching_window(title)?;
        let mut rect = Rect::default();
        if unsafe { GetWindowRect(window.handle, &mut rect) } == 0 {
            return Err(format!("Could not inspect window “{}”.", window.title));
        }
        if unsafe { SetWindowPos(window.handle, Hwnd(0), x, y, width, height, SWP_SHOWWINDOW) } == 0
        {
            return Err(format!("Windows could not resize “{}”.", window.title));
        }
        Ok(format!(
            "Moved and resized “{}” to ({x}, {y}, {width}, {height}).",
            window.title
        ))
    }

    fn run_powershell(command: &str, timeout_seconds: u64) -> Result<String, String> {
        let encoded_command = STANDARD.encode(
            command
                .encode_utf16()
                .flat_map(u16::to_le_bytes)
                .collect::<Vec<_>>(),
        );
        let mut process = crate::platform::no_console(
            Command::new("powershell.exe")
                .args([
                    "-NoLogo",
                    "-NoProfile",
                    "-NonInteractive",
                    "-EncodedCommand",
                    &encoded_command,
                ])
                .stdin(Stdio::null())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped()),
        )
        .spawn()
        .map_err(|error| format!("Could not start PowerShell: {error}"))?;
        let stdout = process
            .stdout
            .take()
            .ok_or_else(|| "Could not read PowerShell output.".to_string())?;
        let stderr = process
            .stderr
            .take()
            .ok_or_else(|| "Could not read PowerShell errors.".to_string())?;
        let stdout_reader = thread::spawn(|| read_bounded(stdout));
        let stderr_reader = thread::spawn(|| read_bounded(stderr));
        let deadline = Instant::now() + Duration::from_secs(timeout_seconds);
        let mut timed_out = false;
        let status = loop {
            match process.try_wait() {
                Ok(Some(status)) => break status,
                Ok(None) if Instant::now() < deadline => thread::sleep(Duration::from_millis(50)),
                Ok(None) => {
                    timed_out = true;
                    process
                        .kill()
                        .map_err(|error| format!("Could not stop timed-out PowerShell: {error}"))?;
                    break process.wait().map_err(|error| {
                        format!("Could not collect PowerShell exit status: {error}")
                    })?;
                }
                Err(error) => {
                    let stop_result = process.kill();
                    return Err(match stop_result {
                        Ok(()) => format!("Could not monitor PowerShell: {error}; the process was stopped."),
                        Err(stop_error) => format!(
                            "Could not monitor PowerShell: {error}; stopping it also failed: {stop_error}."
                        ),
                    });
                }
            }
        };
        let stdout = stdout_reader
            .join()
            .map_err(|_| "Could not collect PowerShell output.".to_string())?
            .map_err(|error| format!("Could not read PowerShell output: {error}"))?;
        let stderr = stderr_reader
            .join()
            .map_err(|_| "Could not collect PowerShell errors.".to_string())?
            .map_err(|error| format!("Could not read PowerShell errors: {error}"))?;
        if timed_out {
            return Err(format!(
                "PowerShell exceeded the {timeout_seconds}-second time limit and was stopped. Output so far:\n{}",
                output_text(stdout, stderr)
            ));
        }
        let result = output_text(stdout, stderr);
        if !status.success() {
            return Err(format!(
                "PowerShell exited with {}.\n{}",
                status
                    .code()
                    .map_or_else(|| "no exit code".to_string(), |code| code.to_string()),
                result
            ));
        }
        Ok(if result.trim().is_empty() {
            "PowerShell completed successfully with no output.".into()
        } else {
            result
        })
    }

    fn read_bounded(mut reader: impl Read) -> std::io::Result<BoundedOutput> {
        let mut bytes = Vec::new();
        let mut truncated = false;
        let mut buffer = [0; 4096];
        loop {
            let count = reader.read(&mut buffer)?;
            if count == 0 {
                break;
            }
            let remaining = MAX_OUTPUT.saturating_sub(bytes.len());
            let retained = count.min(remaining);
            bytes.extend_from_slice(&buffer[..retained]);
            truncated |= retained < count;
        }
        Ok(BoundedOutput { bytes, truncated })
    }

    fn output_text(stdout: BoundedOutput, stderr: BoundedOutput) -> String {
        let mut text = String::from_utf8_lossy(&stdout.bytes).trim().to_string();
        let errors = String::from_utf8_lossy(&stderr.bytes).trim().to_string();
        if !errors.is_empty() {
            if !text.is_empty() {
                text.push('\n');
            }
            text.push_str(&errors);
        }
        if stdout.truncated || stderr.truncated {
            text.push_str("\n[Output truncated at 32 KB.]");
        }
        text
    }
}

#[cfg(windows)]
pub fn execute(name: &str, arguments: &Value) -> Result<String, String> {
    imp::execute(name, arguments)
}

#[cfg(not(windows))]
pub fn execute(_name: &str, _arguments: &Value) -> Result<String, String> {
    Err("Windows task automation is only available on Windows.".into())
}
