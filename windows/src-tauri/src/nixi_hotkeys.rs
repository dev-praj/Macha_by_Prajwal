// Nixi Hotkeys — low-level keyboard hook for Right Ctrl (PTT) and Right Alt (Dictation).

use std::sync::atomic::{AtomicBool, AtomicIsize, AtomicU32, Ordering};
use std::sync::OnceLock;
use tauri::{AppHandle, Emitter};

use crate::island;

static PTT_ACTIVE: AtomicBool = AtomicBool::new(false);
static DICTATE_ACTIVE: AtomicBool = AtomicBool::new(false);
static HOOK_HANDLE: AtomicIsize = AtomicIsize::new(0);
static THREAD_ID: AtomicU32 = AtomicU32::new(0);
static LAST_FOREGROUND: AtomicIsize = AtomicIsize::new(0);
static APP_HANDLE: OnceLock<AppHandle> = OnceLock::new();

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
    GetAsyncKeyState, VK_RCONTROL, VK_RMENU,
};
#[cfg(windows)]
use windows::Win32::UI::WindowsAndMessaging::{
    CallNextHookEx, CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW,
    GetForegroundWindow, GetMessageW, KillTimer, PostThreadMessageW, RegisterClassExW,
    SetTimer, SetWindowsHookExW, TranslateMessage, UnhookWindowsHookEx,
    UnregisterClassW, CS_HREDRAW, CS_VREDRAW, HCURSOR, HHOOK, HICON, KBDLLHOOKSTRUCT,
    MSG, WH_KEYBOARD_LL, WINDOW_EX_STYLE, WNDCLASSEXW, WS_POPUP,
    WM_KEYDOWN, WM_KEYUP, WM_QUIT, WM_SYSKEYDOWN, WM_SYSKEYUP, WM_TIMER,
    WM_WTSSESSION_CHANGE,
};

#[cfg(windows)]
unsafe extern "system" fn ll_keyboard_proc(
    code: i32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    if code >= 0 {
        let kbd = *(lparam.0 as *const KBDLLHOOKSTRUCT);
        let msg = wparam.0 as u32;
        let is_down = msg == WM_KEYDOWN || msg == WM_SYSKEYDOWN;
        let is_up = msg == WM_KEYUP || msg == WM_SYSKEYUP;

        if kbd.vkCode == VK_RCONTROL.0 as u32 {
            if is_down {
                if !PTT_ACTIVE.swap(true, Ordering::SeqCst) {
                    if let Some(app) = APP_HANDLE.get() {
                        let _ = app.emit_to(island::WINDOW_LABEL, "nixi-ptt-start", ());
                    }
                }
            } else if is_up {
                if PTT_ACTIVE.swap(false, Ordering::SeqCst) {
                    if let Some(app) = APP_HANDLE.get() {
                        let _ = app.emit_to(island::WINDOW_LABEL, "nixi-ptt-stop", ());
                    }
                }
            }
        } else if kbd.vkCode == VK_RMENU.0 as u32 {
            if is_down {
                if !DICTATE_ACTIVE.swap(true, Ordering::SeqCst) {
                    if let Some(app) = APP_HANDLE.get() {
                        let _ = app.emit_to(island::WINDOW_LABEL, "nixi-dictate-start", ());
                    }
                }
            } else if is_up {
                if DICTATE_ACTIVE.swap(false, Ordering::SeqCst) {
                    if let Some(app) = APP_HANDLE.get() {
                        let _ = app.emit_to(island::WINDOW_LABEL, "nixi-dictate-stop", ());
                    }
                }
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
                if let Some(app) = APP_HANDLE.get() {
                    let _ = app.emit_to(island::WINDOW_LABEL, "nixi-ptt-stop", ());
                }
            }
        }
    }

    if DICTATE_ACTIVE.load(Ordering::SeqCst) {
        let state = unsafe { GetAsyncKeyState(VK_RMENU.0 as i32) };
        if (state as u16 & 0x8000) == 0 {
            if DICTATE_ACTIVE.swap(false, Ordering::SeqCst) {
                crate::log::line(
                    "nixi_hotkeys: stuck Right Alt (dictate) detected, synthesized stop",
                );
                if let Some(app) = APP_HANDLE.get() {
                    let _ = app.emit_to(island::WINDOW_LABEL, "nixi-dictate-stop", ());
                }
            }
        }
    }
}

pub fn force_clear_active_states() {
    if PTT_ACTIVE.swap(false, Ordering::SeqCst) {
        crate::log::line("nixi_hotkeys: clearing active PTT state");
        if let Some(app) = APP_HANDLE.get() {
            let _ = app.emit_to(island::WINDOW_LABEL, "nixi-ptt-stop", ());
        }
    }
    if DICTATE_ACTIVE.swap(false, Ordering::SeqCst) {
        crate::log::line("nixi_hotkeys: clearing active dictate state");
        if let Some(app) = APP_HANDLE.get() {
            let _ = app.emit_to(island::WINDOW_LABEL, "nixi-dictate-stop", ());
        }
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
