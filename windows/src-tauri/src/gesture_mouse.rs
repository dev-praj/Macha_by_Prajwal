use std::io::{BufRead, BufReader, Read, Write};
#[cfg(windows)]
use std::net::{Ipv4Addr, SocketAddr, SocketAddrV4, TcpStream};
use std::path::PathBuf;
use std::process::Child;
use std::sync::Mutex;
use std::thread;
use std::time::{Duration, Instant};

#[cfg(windows)]
use std::ffi::OsString;
#[cfg(windows)]
use std::process::{Command, Stdio};

use tauri::{AppHandle, Manager, State};

use crate::log;

#[cfg(windows)]
const DASHBOARD_ADDR: SocketAddr = SocketAddr::V4(SocketAddrV4::new(Ipv4Addr::LOCALHOST, 8765));

#[cfg(windows)]
const IMPORT_CHECK: &str = "import sys; assert sys.version_info >= (3, 9), 'Python 3.9 or newer is required'; import aiohttp, cv2, mediapipe, numpy, pyautogui, tkinter; from mediapipe.tasks import python as mp_tasks; from mediapipe.tasks.python.vision import HandLandmarker, HandLandmarkerOptions, RunningMode; from PIL import Image, ImageTk";

#[derive(Default)]
pub struct GestureMouseProcess {
    child: Mutex<Option<Child>>,
}

impl GestureMouseProcess {
    pub fn stop(&self) -> Result<(), String> {
        let mut child_guard = self.child.lock().unwrap();
        let Some(child) = child_guard.as_mut() else {
            return Ok(());
        };

        if child
            .try_wait()
            .map_err(|error| format!("Could not check Gesture Mouse: {error}"))?
            .is_none()
        {
            if let Some(stdin) = child.stdin.as_mut() {
                if let Err(error) = stdin.write_all(b"stop\n").and_then(|_| stdin.flush()) {
                    log::line(format!(
                        "could not send graceful stop to Gesture Mouse: {error}"
                    ));
                }
            }

            let deadline = Instant::now() + Duration::from_secs(3);
            loop {
                if child
                    .try_wait()
                    .map_err(|error| format!("Could not check Gesture Mouse: {error}"))?
                    .is_some()
                {
                    break;
                }
                if Instant::now() >= deadline {
                    child
                        .kill()
                        .map_err(|error| format!("Could not stop Gesture Mouse: {error}"))?;
                    child
                        .wait()
                        .map_err(|error| format!("Could not close Gesture Mouse: {error}"))?;
                    break;
                }
                thread::sleep(Duration::from_millis(100));
            }
        }
        *child_guard = None;
        log::line("Gesture Mouse stopped");
        Ok(())
    }

    fn is_running(&self) -> Result<bool, String> {
        let mut child_guard = self.child.lock().unwrap();
        let Some(child) = child_guard.as_mut() else {
            return Ok(false);
        };

        match child
            .try_wait()
            .map_err(|error| format!("Could not check Gesture Mouse: {error}"))?
        {
            None => Ok(true),
            Some(_) => {
                *child_guard = None;
                Ok(false)
            }
        }
    }
}

#[tauri::command]
pub fn gesture_mouse_start(
    app: AppHandle,
    process: State<'_, GestureMouseProcess>,
) -> Result<(), String> {
    #[cfg(not(windows))]
    {
        let _ = (app, process);
        return Err("Hand Gesture Mouse is currently available only on Windows.".into());
    }

    #[cfg(windows)]
    {
        let mut child_guard = process.child.lock().unwrap();
        if let Some(child) = child_guard.as_mut() {
            if child
                .try_wait()
                .map_err(|error| format!("Could not check Gesture Mouse: {error}"))?
                .is_none()
            {
                return Err("Hand Gesture Mouse is already running.".into());
            }
            *child_guard = None;
        }

        let (mut command, runner) = if let Some(executable) = packaged_executable_path(&app) {
            let runner = executable.to_string_lossy().into_owned();
            (Command::new(executable), runner)
        } else {
            let script = script_path(&app)?;
            let python = find_python()?;
            let runner = python.program.to_string_lossy().into_owned();
            let mut command = Command::new(&python.program);
            command.args(&python.args).arg(script);
            (command, runner)
        };
        command
            .arg("--no-browser")
            .arg("--managed")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .env("PYTHONUNBUFFERED", "1");
        let mut child = crate::platform::no_console(&mut command)
            .spawn()
            .map_err(|error| format!("Could not start Gesture Mouse: {error}"))?;

        if let Some(stdout) = child.stdout.take() {
            relay_output(stdout, "stdout");
        }
        if let Some(stderr) = child.stderr.take() {
            relay_output(stderr, "stderr");
        }
        if let Err(error) = wait_for_dashboard(&mut child) {
            let _ = child.kill();
            let _ = child.wait();
            return Err(error);
        }
        *child_guard = Some(child);
        log::line(format!("Gesture Mouse started with {runner}"));
        Ok(())
    }
}

#[cfg(windows)]
fn wait_for_dashboard(child: &mut Child) -> Result<(), String> {
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        if let Some(status) = child
            .try_wait()
            .map_err(|error| format!("Could not check Gesture Mouse startup: {error}"))?
        {
            return Err(format!(
                "Gesture Mouse exited during startup with status {status}. Check Macha's log for startup errors."
            ));
        }
        if let Ok(mut stream) =
            TcpStream::connect_timeout(&DASHBOARD_ADDR, Duration::from_millis(250))
        {
            let _ = stream.set_read_timeout(Some(Duration::from_millis(500)));
            if stream
                .write_all(b"GET /health HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\n\r\n")
                .is_ok()
            {
                let mut response = String::new();
                if BufReader::new(stream).read_line(&mut response).is_ok()
                    && response.contains(" 200 ")
                {
                    return Ok(());
                }
            }
        }
        if Instant::now() >= deadline {
            return Err("Gesture Mouse started, but its local dashboard did not become ready. Check Macha's log for startup errors.".into());
        }
        thread::sleep(Duration::from_millis(100));
    }
}

#[tauri::command]
pub fn gesture_mouse_stop(process: State<'_, GestureMouseProcess>) -> Result<(), String> {
    process.stop()
}

#[tauri::command]
pub fn gesture_mouse_is_running(process: State<'_, GestureMouseProcess>) -> Result<bool, String> {
    process.is_running()
}

#[cfg(windows)]
struct PythonCommand {
    program: OsString,
    args: Vec<OsString>,
}

#[cfg(windows)]
fn find_python() -> Result<PythonCommand, String> {
    let project_root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
    let mut candidates = Vec::new();
    let app_data_venv = crate::platform::local_dir().join("gesture_mouse/.venv/Scripts/python.exe");
    if app_data_venv.is_file() {
        candidates.push(PythonCommand {
            program: app_data_venv.into_os_string(),
            args: Vec::new(),
        });
    }
    let gesture_venv = project_root.join("gesture_mouse/.venv/Scripts/python.exe");
    if gesture_venv.is_file() {
        candidates.push(PythonCommand {
            program: gesture_venv.into_os_string(),
            args: Vec::new(),
        });
    }
    let venv = project_root.join(".venv/Scripts/python.exe");
    if venv.is_file() {
        candidates.push(PythonCommand {
            program: venv.into_os_string(),
            args: Vec::new(),
        });
    }
    candidates.push(PythonCommand {
        program: OsString::from("python"),
        args: Vec::new(),
    });
    candidates.push(PythonCommand {
        program: OsString::from("py"),
        args: vec![OsString::from("-3")],
    });
    candidates.push(PythonCommand {
        program: OsString::from("py"),
        args: vec![OsString::from("-3.13")],
    });

    let mut failures = Vec::new();
    for candidate in candidates {
        let mut check = Command::new(&candidate.program);
        let output = crate::platform::no_console(&mut check)
            .args(&candidate.args)
            .args(["-c", IMPORT_CHECK])
            .stdin(Stdio::null())
            .output();
        match output {
            Ok(result) if result.status.success() => return Ok(candidate),
            Ok(result) => {
                let detail = String::from_utf8_lossy(&result.stderr).trim().to_string();
                failures.push(format!(
                    "{}: {}",
                    candidate.program.to_string_lossy(),
                    if detail.is_empty() {
                        "Python or required packages are unavailable".to_string()
                    } else {
                        detail
                    }
                ));
            }
            Err(error) => {
                failures.push(format!("{}: {error}", candidate.program.to_string_lossy()))
            }
        }
    }

    Err(format!(
        "Hand Gesture Mouse needs Python 3.9+ and its packages. Install them with the Python interpreter you want to use (for example, `py -3 -m pip install -r windows\\gesture_mouse\\requirements.txt`), then try again. {}",
        failures.join("; ")
    ))
}

fn script_path(app: &AppHandle) -> Result<PathBuf, String> {
    let development = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../gesture_mouse/app.py");
    if development.is_file() {
        return Ok(development);
    }

    let packaged = app
        .path()
        .resource_dir()
        .map_err(|error| format!("Could not locate Macha resources: {error}"))?
        .join("gesture_mouse/app.py");
    if packaged.is_file() {
        return Ok(packaged);
    }

    Err("Hand Gesture Mouse files are missing from this Macha installation.".into())
}

#[cfg(windows)]
fn packaged_executable_path(app: &AppHandle) -> Option<PathBuf> {
    app.path()
        .resource_dir()
        .ok()
        .map(|resources| resources.join("gesture_mouse/MachaGestureMouse.exe"))
        .filter(|path| path.is_file())
}

fn relay_output(reader: impl Read + Send + 'static, stream: &'static str) {
    thread::spawn(move || {
        for line in BufReader::new(reader).lines() {
            match line {
                Ok(line) => log::line(format!("Gesture Mouse {stream}: {line}")),
                Err(error) => {
                    log::line(format!("Could not read Gesture Mouse {stream}: {error}"));
                    break;
                }
            }
        }
    });
}
