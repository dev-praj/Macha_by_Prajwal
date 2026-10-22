// Nixi Hotkeys — low-level keyboard hook for Right Ctrl (PTT) and Right Alt (Dictation).

use std::sync::atomic::{AtomicBool, AtomicI32, AtomicIsize, AtomicU32, Ordering};
use std::sync::OnceLock;
use tauri::{AppHandle, Emitter};

static PTT_ACTIVE: AtomicBool = AtomicBool::new(false);
static DICTATE_ACTIVE: AtomicBool = AtomicBool::new(false);
static VOICE_CHORD_ACTIVE: AtomicBool = AtomicBool::new(false);
static LEFT_CTRL_DOWN: AtomicBool = AtomicBool::new(false);
static LEFT_ALT_DOWN: AtomicBool = AtomicBool::new(false);
static HOOK_HANDLE: AtomicIsize = AtomicIsize::new(0);
static MOUSE_HOOK_HANDLE: AtomicIsize = AtomicIsize::new(0);
static OBJECT_DRAG_ACTIVE: AtomicBool = AtomicBool::new(false);
static OBJECT_DRAG_START_X: AtomicI32 = AtomicI32::new(0);
static OBJECT_DRAG_START_Y: AtomicI32 = AtomicI32::new(0);
static OBJECT_REGION_VALID: AtomicBool = AtomicBool::new(false);
static OBJECT_REGION_X: AtomicI32 = AtomicI32::new(0);
static OBJECT_REGION_Y: AtomicI32 = AtomicI32::new(0);
static OBJECT_REGION_WIDTH: AtomicU32 = AtomicU32::new(0);
static OBJECT_REGION_HEIGHT: AtomicU32 = AtomicU32::new(0);
static THREAD_ID: AtomicU32 = AtomicU32::new(0);
static LAST_FOREGROUND: AtomicIsize = AtomicIsize::new(0);
static APP_HANDLE: OnceLock<AppHandle> = OnceLock::new();
static DICTATION_SETUP_TEST: AtomicBool = AtomicBool::new(false);

#[derive(Clone, Copy, serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct DictationHotkeyPayload {
    setup_test: bool,
}

fn emit_event(event: &str) {
    if let Some(app) = APP_HANDLE.get() {
        let _ = app.emit(event, ());
    }
}

fn emit_dictation(event: &str) {
    if let Some(app) = APP_HANDLE.get() {
        let _ = app.emit(
            event,
            DictationHotkeyPayload {
                setup_test: DICTATION_SETUP_TEST.load(Ordering::SeqCst),
            },
        );
    }
}

#[tauri::command]
pub fn set_dictation_setup_test_active(app: AppHandle, active: bool) {
    DICTATION_SETUP_TEST.store(active, Ordering::SeqCst);
    let _ = app.emit("dictation-setup-test-active", active);
}

#[tauri::command]
pub fn dictation_setup_test_active() -> bool {
    DICTATION_SETUP_TEST.load(Ordering::SeqCst)
}

#[cfg(any(windows, test))]
fn right_alt_still_held(rmenu_down: bool, menu_down: bool, lmenu_down: bool) -> bool {
    rmenu_down || (menu_down && !lmenu_down)
}

#[derive(Clone, Copy, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ScreenRegion {
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
}

#[derive(Clone, Copy, serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct VoiceChordStop {
    selected_region: Option<ScreenRegion>,
}

#[cfg(windows)]
use windows::core::PCWSTR;
#[cfg(windows)]
use windows::Win32::Foundation::{HINSTANCE, HWND, LPARAM, LRESULT, WPARAM};
#[cfg(windows)]
use windows::Win32::Graphics::Gdi::HBRUSH;
#[cfg(windows)]
use windows::Win32::System::RemoteDesktop::{
    WTSRegisterSessionNotification, WTSUnRegisterSessionNotification, NOTIFY_FOR_THIS_SESSION,
};
#[cfg(windows)]
use windows::Win32::System::Threading::GetCurrentThreadId;
#[cfg(windows)]
use windows::Win32::UI::Input::KeyboardAndMouse::{
    GetAsyncKeyState, VK_LCONTROL, VK_LMENU, VK_MENU, VK_RCONTROL, VK_RMENU,
};
#[cfg(windows)]
use windows::Win32::UI::WindowsAndMessaging::{
    CallNextHookEx, CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW,
    GetForegroundWindow, GetMessageW, KillTimer, PostThreadMessageW, RegisterClassExW, SetTimer,
    SetWindowsHookExW, TranslateMessage, UnhookWindowsHookEx, UnregisterClassW, CS_HREDRAW,
    CS_VREDRAW, HCURSOR, HHOOK, HICON, KBDLLHOOKSTRUCT, MSLLHOOKSTRUCT, MSG, WH_KEYBOARD_LL,
    WH_MOUSE_LL, WINDOW_EX_STYLE, WM_KEYDOWN, WM_KEYUP, WM_LBUTTONDOWN, WM_LBUTTONUP, WM_MOUSEMOVE,
    WM_QUIT, WM_SYSKEYDOWN, WM_SYSKEYUP, WM_TIMER, WM_WTSSESSION_CHANGE, WNDCLASSEXW, WS_POPUP,
};

#[cfg(windows)]
unsafe extern "system" fn ll_keyboard_proc(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if code >= 0 {
        let kbd = *(lparam.0 as *const KBDLLHOOKSTRUCT);
        let msg = wparam.0 as u32;
        let is_down = msg == WM_KEYDOWN || msg == WM_SYSKEYDOWN;
        let is_up = msg == WM_KEYUP || msg == WM_SYSKEYUP;

        if kbd.vkCode == VK_RCONTROL.0 as u32 {
            if is_down {
                if !PTT_ACTIVE.swap(true, Ordering::SeqCst) {
                    emit_event("nixi-ptt-start");
                }
            } else if is_up {
                if PTT_ACTIVE.swap(false, Ordering::SeqCst) {
                    emit_event("nixi-ptt-stop");
                }
            }
        } else if kbd.vkCode == VK_RMENU.0 as u32 {
            if is_down {
                if !DICTATE_ACTIVE.swap(true, Ordering::SeqCst) {
                    emit_dictation("nixi-dictate-start");
                }
            } else if is_up {
                if DICTATE_ACTIVE.swap(false, Ordering::SeqCst) {
                    emit_dictation("nixi-dictate-stop");
                }
            }
        } else if kbd.vkCode == VK_LCONTROL.0 as u32 {
            if is_down {
                LEFT_CTRL_DOWN.store(true, Ordering::SeqCst);
                start_voice_chord_if_ready();
            } else if is_up {
                LEFT_CTRL_DOWN.store(false, Ordering::SeqCst);
                stop_voice_chord();
            }
        } else if kbd.vkCode == VK_LMENU.0 as u32 {
            if is_down {
                LEFT_ALT_DOWN.store(true, Ordering::SeqCst);
                start_voice_chord_if_ready();
            } else if is_up {
                LEFT_ALT_DOWN.store(false, Ordering::SeqCst);
                stop_voice_chord();
            }
        }
    }

    let hook_raw = HOOK_HANDLE.load(Ordering::SeqCst);
    let hhook = if hook_raw != 0 {
        Some(HHOOK(hook_raw as *mut _))
    } else {
        None
    };
    CallNextHookEx(hhook, code, wparam, lparam)
}

#[cfg(windows)]
unsafe extern "system" fn ll_mouse_proc(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    let mut suppress_input = false;
    if code >= 0 && VOICE_CHORD_ACTIVE.load(Ordering::SeqCst) {
        let mouse = *(lparam.0 as *const MSLLHOOKSTRUCT);
        match wparam.0 as u32 {
            WM_LBUTTONDOWN => {
                OBJECT_DRAG_START_X.store(mouse.pt.x, Ordering::SeqCst);
                OBJECT_DRAG_START_Y.store(mouse.pt.y, Ordering::SeqCst);
                OBJECT_REGION_VALID.store(false, Ordering::SeqCst);
                OBJECT_DRAG_ACTIVE.store(true, Ordering::SeqCst);
                suppress_input = true;
            }
            WM_LBUTTONUP if OBJECT_DRAG_ACTIVE.swap(false, Ordering::SeqCst) => {
                if let Some(region) = screen_region_from_points(
                    OBJECT_DRAG_START_X.load(Ordering::SeqCst),
                    OBJECT_DRAG_START_Y.load(Ordering::SeqCst),
                    mouse.pt.x,
                    mouse.pt.y,
                ) {
                    OBJECT_REGION_X.store(region.x, Ordering::SeqCst);
                    OBJECT_REGION_Y.store(region.y, Ordering::SeqCst);
                    OBJECT_REGION_WIDTH.store(region.width, Ordering::SeqCst);
                    OBJECT_REGION_HEIGHT.store(region.height, Ordering::SeqCst);
                    OBJECT_REGION_VALID.store(true, Ordering::SeqCst);
                }
                suppress_input = true;
            }
            WM_MOUSEMOVE if OBJECT_DRAG_ACTIVE.load(Ordering::SeqCst) => {
                suppress_input = true;
            }
            _ => {}
        }
    }

    if suppress_input {
        return LRESULT(1);
    }

    let hook_raw = MOUSE_HOOK_HANDLE.load(Ordering::SeqCst);
    let hhook = if hook_raw != 0 {
        Some(HHOOK(hook_raw as *mut _))
    } else {
        None
    };
    CallNextHookEx(hhook, code, wparam, lparam)
}

fn screen_region_from_points(x1: i32, y1: i32, x2: i32, y2: i32) -> Option<ScreenRegion> {
    let left = i64::from(x1.min(x2));
    let top = i64::from(y1.min(y2));
    let width = i64::from(x1).max(i64::from(x2)) - left;
    let height = i64::from(y1).max(i64::from(y2)) - top;
    if width < 12 || height < 12 {
        return None;
    }
    Some(ScreenRegion {
        x: i32::try_from(left).ok()?,
        y: i32::try_from(top).ok()?,
        width: u32::try_from(width).ok()?,
        height: u32::try_from(height).ok()?,
    })
}

#[cfg(windows)]
fn start_voice_chord_if_ready() {
    if LEFT_CTRL_DOWN.load(Ordering::SeqCst)
        && LEFT_ALT_DOWN.load(Ordering::SeqCst)
        && !VOICE_CHORD_ACTIVE.swap(true, Ordering::SeqCst)
    {
        OBJECT_DRAG_ACTIVE.store(false, Ordering::SeqCst);
        OBJECT_REGION_VALID.store(false, Ordering::SeqCst);
        emit_event("voice-chord-start");
    }
}

#[cfg(any(windows, test))]
fn voice_chord_is_pressed(left_ctrl_down: bool, left_alt_down: bool) -> bool {
    left_ctrl_down && left_alt_down
}

#[cfg(windows)]
fn stop_voice_chord() {
    if VOICE_CHORD_ACTIVE.swap(false, Ordering::SeqCst) {
        OBJECT_DRAG_ACTIVE.store(false, Ordering::SeqCst);
        let selected_region = OBJECT_REGION_VALID
            .swap(false, Ordering::SeqCst)
            .then(|| ScreenRegion {
                x: OBJECT_REGION_X.load(Ordering::SeqCst),
                y: OBJECT_REGION_Y.load(Ordering::SeqCst),
                width: OBJECT_REGION_WIDTH.load(Ordering::SeqCst),
                height: OBJECT_REGION_HEIGHT.load(Ordering::SeqCst),
            });
        if let Some(app) = APP_HANDLE.get() {
            let _ = app.emit(
                "voice-chord-stop",
                VoiceChordStop { selected_region },
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{right_alt_still_held, screen_region_from_points, voice_chord_is_pressed};

    #[test]
    fn right_alt_hold_survives_layouts_that_report_generic_alt() {
        assert!(right_alt_still_held(true, false, false));
        assert!(right_alt_still_held(false, true, false));
        assert!(!right_alt_still_held(false, true, true));
        assert!(!right_alt_still_held(false, false, false));
    }

    #[test]
    fn voice_input_requires_both_left_modifiers() {
        assert!(!voice_chord_is_pressed(false, false));
        assert!(!voice_chord_is_pressed(true, false));
        assert!(!voice_chord_is_pressed(false, true));
        assert!(voice_chord_is_pressed(true, true));
    }

    #[test]
    fn screen_selection_normalizes_drag_direction_and_rejects_clicks() {
        assert_eq!(
            screen_region_from_points(180, 140, 40, 20).map(|region| (
                region.x,
                region.y,
                region.width,
                region.height
            )),
            Some((40, 20, 140, 120))
        );
        assert!(screen_region_from_points(20, 20, 28, 28).is_none());
    }
}

#[cfg(windows)]
unsafe extern "system" fn watcher_wndproc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    match msg {
        WM_WTSSESSION_CHANGE => {
            crate::log::line("nixi_hotkeys: session change received, clearing active states");
            force_clear_active_states();
            LRESULT(0)
        }
        WM_TIMER => {
            check_stuck_keys();
            LRESULT(0)
        }
        _ => DefWindowProcW(hwnd, msg, wparam, lparam),
    }
}

#[cfg(windows)]
fn check_stuck_keys() {
    let fg = unsafe { GetForegroundWindow() };
    let fg_raw = fg.0 as isize;
    let prev_fg = LAST_FOREGROUND.swap(fg_raw, Ordering::SeqCst);

    if prev_fg != 0 && prev_fg != fg_raw {
        if PTT_ACTIVE.load(Ordering::SeqCst) || DICTATE_ACTIVE.load(Ordering::SeqCst) {
            crate::log::line(
                "nixi_hotkeys: foreground window changed while hotkey active, checking key state",
            );
        }
    }

    if PTT_ACTIVE.load(Ordering::SeqCst) {
        let state = unsafe { GetAsyncKeyState(VK_RCONTROL.0 as i32) };
        if (state as u16 & 0x8000) == 0 {
            if PTT_ACTIVE.swap(false, Ordering::SeqCst) {
                crate::log::line("nixi_hotkeys: stuck Right Ctrl (PTT) detected, synthesized stop");
                emit_event("nixi-ptt-stop");
            }
        }
    }

    if DICTATE_ACTIVE.load(Ordering::SeqCst) {
        let rmenu_down = unsafe { GetAsyncKeyState(VK_RMENU.0 as i32) } as u16 & 0x8000 != 0;
        let menu_down = unsafe { GetAsyncKeyState(VK_MENU.0 as i32) } as u16 & 0x8000 != 0;
        let lmenu_down = unsafe { GetAsyncKeyState(VK_LMENU.0 as i32) } as u16 & 0x8000 != 0;
        if !right_alt_still_held(rmenu_down, menu_down, lmenu_down) {
            if DICTATE_ACTIVE.swap(false, Ordering::SeqCst) {
                crate::log::line(
                    "nixi_hotkeys: stuck Right Alt (dictate) detected, synthesized stop",
                );
                emit_dictation("nixi-dictate-stop");
            }
        }
    }

    if VOICE_CHORD_ACTIVE.load(Ordering::SeqCst) {
        let ctrl_state = unsafe { GetAsyncKeyState(VK_LCONTROL.0 as i32) };
        let alt_state = unsafe { GetAsyncKeyState(VK_LMENU.0 as i32) };
        if (ctrl_state as u16 & 0x8000) == 0 || (alt_state as u16 & 0x8000) == 0 {
            LEFT_CTRL_DOWN.store(false, Ordering::SeqCst);
            LEFT_ALT_DOWN.store(false, Ordering::SeqCst);
            crate::log::line(
                "nixi_hotkeys: released voice chord after detecting stuck modifier",
            );
            stop_voice_chord();
        }
    }
}

pub fn force_clear_active_states() {
    LEFT_CTRL_DOWN.store(false, Ordering::SeqCst);
    LEFT_ALT_DOWN.store(false, Ordering::SeqCst);
    OBJECT_DRAG_ACTIVE.store(false, Ordering::SeqCst);
    OBJECT_REGION_VALID.store(false, Ordering::SeqCst);
    stop_voice_chord();
    if PTT_ACTIVE.swap(false, Ordering::SeqCst) {
        crate::log::line("nixi_hotkeys: clearing active PTT state");
        emit_event("nixi-ptt-stop");
    }
    if DICTATE_ACTIVE.swap(false, Ordering::SeqCst) {
        crate::log::line("nixi_hotkeys: clearing active dictate state");
        emit_dictation("nixi-dictate-stop");
    }
}

#[cfg(windows)]
fn hook_thread_main() {
    let tid = unsafe { GetCurrentThreadId() };
    THREAD_ID.store(tid, Ordering::SeqCst);

    let class_name: Vec<u16> = "NixiHotkeyWatcherClass\0".encode_utf16().collect();
    let window_name: Vec<u16> = "NixiHotkeyWatcher\0".encode_utf16().collect();

    let wc = WNDCLASSEXW {
        cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
        style: CS_HREDRAW | CS_VREDRAW,
        lpfnWndProc: Some(watcher_wndproc),
        cbClsExtra: 0,
        cbWndExtra: 0,
        hInstance: HINSTANCE::default(),
        hIcon: HICON::default(),
        hCursor: HCURSOR::default(),
        hbrBackground: HBRUSH::default(),
        lpszMenuName: PCWSTR::null(),
        lpszClassName: PCWSTR(class_name.as_ptr()),
        hIconSm: HICON::default(),
    };

    let _ = unsafe { RegisterClassExW(&wc) };

    let hwnd = unsafe {
        CreateWindowExW(
            WINDOW_EX_STYLE::default(),
            PCWSTR(class_name.as_ptr()),
            PCWSTR(window_name.as_ptr()),
            WS_POPUP,
            0,
            0,
            0,
            0,
            None,
            None,
            None,
            None,
        )
    };

    let (hwnd, session_registered, timer_active) = match hwnd {
        Ok(h) => {
            let session_ok = unsafe {
                WTSRegisterSessionNotification(h, NOTIFY_FOR_THIS_SESSION).is_ok()
            };
            if !session_ok {
                crate::log::line(
                    "nixi_hotkeys: WTSRegisterSessionNotification failed to register",
                );
            }
            let timer_id = unsafe { SetTimer(Some(h), 1, 100, None) };
            (h, session_ok, timer_id != 0)
        }
        Err(e) => {
            crate::log::line(format!(
                "nixi_hotkeys: could not create session watcher window: {e}"
            ));
            (HWND::default(), false, false)
        }
    };

    match unsafe { SetWindowsHookExW(WH_KEYBOARD_LL, Some(ll_keyboard_proc), None, 0) } {
        Ok(hook) => {
            HOOK_HANDLE.store(hook.0 as isize, Ordering::SeqCst);
            crate::log::line("nixi_hotkeys: WH_KEYBOARD_LL hook successfully installed");
        }
        Err(e) => {
            crate::log::line(format!(
                "nixi_hotkeys: SetWindowsHookExW failed to install: {e}"
            ));
        }
    }

    match unsafe { SetWindowsHookExW(WH_MOUSE_LL, Some(ll_mouse_proc), None, 0) } {
        Ok(hook) => {
            MOUSE_HOOK_HANDLE.store(hook.0 as isize, Ordering::SeqCst);
            crate::log::line("nixi_hotkeys: WH_MOUSE_LL hook successfully installed");
        }
        Err(e) => {
            crate::log::line(format!(
                "nixi_hotkeys: mouse selection hook failed to install: {e}"
            ));
        }
    }

    let mut msg = MSG::default();
    unsafe {
        while GetMessageW(&mut msg, None, 0, 0).as_bool() {
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }

    // Teardown
    if timer_active && !hwnd.0.is_null() {
        let _ = unsafe { KillTimer(Some(hwnd), 1) };
    }
    if session_registered && !hwnd.0.is_null() {
        let _ = unsafe { WTSUnRegisterSessionNotification(hwnd) };
    }
    if !hwnd.0.is_null() {
        let _ = unsafe { DestroyWindow(hwnd) };
    }
    let _ = unsafe { UnregisterClassW(PCWSTR(class_name.as_ptr()), Some(HINSTANCE::default())) };

    let hook_raw = HOOK_HANDLE.swap(0, Ordering::SeqCst);
    if hook_raw != 0 {
        let _ = unsafe { UnhookWindowsHookEx(HHOOK(hook_raw as *mut _)) };
        crate::log::line("nixi_hotkeys: WH_KEYBOARD_LL hook uninstalled");
    }
    let mouse_hook_raw = MOUSE_HOOK_HANDLE.swap(0, Ordering::SeqCst);
    if mouse_hook_raw != 0 {
        let _ = unsafe { UnhookWindowsHookEx(HHOOK(mouse_hook_raw as *mut _)) };
        crate::log::line("nixi_hotkeys: WH_MOUSE_LL hook uninstalled");
    }
}

pub fn start(app: &AppHandle) {
    let _ = APP_HANDLE.set(app.clone());

    if THREAD_ID.load(Ordering::SeqCst) != 0 || HOOK_HANDLE.load(Ordering::SeqCst) != 0 {
        crate::log::line("nixi_hotkeys: hook already started");
        return;
    }

    #[cfg(windows)]
    {
        let run_res = app.run_on_main_thread(move || {
            if THREAD_ID.load(Ordering::SeqCst) != 0 {
                return;
            }
            if let Err(e) = std::thread::Builder::new()
                .name("nixi-hotkeys".into())
                .spawn(hook_thread_main)
            {
                crate::log::line(format!(
                    "nixi_hotkeys: failed to spawn hotkey hook thread: {e}"
                ));
            }
        });

        if let Err(e) = run_res {
            crate::log::line(format!(
                "nixi_hotkeys: run_on_main_thread failed ({e}), starting thread directly"
            ));
            if let Err(e) = std::thread::Builder::new()
                .name("nixi-hotkeys".into())
                .spawn(hook_thread_main)
            {
                crate::log::line(format!(
                    "nixi_hotkeys: failed to spawn hotkey hook thread: {e}"
                ));
            }
        }
    }

    #[cfg(not(windows))]
    {
        let _ = app;
    }
}

pub fn stop() {
    force_clear_active_states();

    #[cfg(windows)]
    {
        let thread_id = THREAD_ID.swap(0, Ordering::SeqCst);
        if thread_id != 0 {
            unsafe {
                let _ = PostThreadMessageW(thread_id, WM_QUIT, WPARAM(0), LPARAM(0));
            }
        }
    }
}
