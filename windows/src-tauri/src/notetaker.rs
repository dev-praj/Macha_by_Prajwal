use std::collections::HashMap;
use std::fs::{self, File};
use std::io::{BufWriter, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use rand::Rng;
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter, Manager, State};

const SAMPLE_RATE: u32 = 16_000;
const CHUNK_SAMPLES: usize = SAMPLE_RATE as usize * 240;
const STT_URL: &str = "http://127.0.0.1:5005";

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum NoteStatus {
    Recording,
    Processing,
    Ready,
    Failed,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NoteSpeaker {
    pub id: String,
    pub name: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NoteLine {
    pub id: String,
    pub speaker: String,
    pub start: f64,
    pub end: f64,
    pub text: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Note {
    pub id: String,
    pub title: String,
    pub start: u64,
    pub end: Option<u64>,
    pub status: NoteStatus,
    pub speakers: Vec<NoteSpeaker>,
    pub lines: Vec<NoteLine>,
    pub error: Option<String>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct NotesState {
    notes: Vec<Note>,
    recording: Option<String>,
    working: Vec<String>,
}

#[derive(Default)]
pub struct NoteTaker {
    notes: Mutex<Vec<Note>>,
    recording: Mutex<Option<Recording>>,
    working: Mutex<Vec<String>>,
    starting: AtomicBool,
}

struct Recording {
    id: String,
    mic: Capture,
    system: Option<Capture>,
    mic_track: Arc<Mutex<WavWriter>>,
    system_track: Option<Arc<Mutex<WavWriter>>>,
    io_error: Arc<Mutex<Option<String>>>,
}

struct Capture {
    stop: Option<mpsc::Sender<()>>,
    thread: Option<std::thread::JoinHandle<()>>,
}

struct WavWriter {
    output: Option<BufWriter<File>>,
    samples: u32,
}

struct Resampler {
    step: f64,
    pos: f64,
    buffer: Vec<f32>,
    cutoff: f64,
}

#[derive(Deserialize)]
struct SttResponse {
    #[serde(default)]
    segments: Vec<SttSegment>,
    error: Option<String>,
}

#[derive(Deserialize)]
struct SttSegment {
    start: f64,
    end: f64,
    text: String,
}

impl WavWriter {
    fn create(path: &Path) -> std::io::Result<Self> {
        let mut output = BufWriter::new(File::create(path)?);
        write_wav_header(&mut output, 0)?;
        Ok(Self {
            output: Some(output),
            samples: 0,
        })
    }

    fn write(&mut self, samples: &[f32]) -> std::io::Result<()> {
        for sample in samples {
            let pcm = (sample.clamp(-1.0, 1.0) * 32767.0) as i16;
            self.output
                .as_mut()
                .ok_or_else(|| std::io::Error::other("audio writer is closed"))?
                .write_all(&pcm.to_le_bytes())?;
        }
        self.samples = self.samples.saturating_add(samples.len() as u32);
        Ok(())
    }

    fn finish(&mut self) -> std::io::Result<()> {
        use std::io::{Seek, SeekFrom};
        let Some(mut output) = self.output.take() else {
            return Ok(());
        };
        output.flush()?;
        let mut file = output.into_inner().map_err(|error| error.into_error())?;
        file.seek(SeekFrom::Start(0))?;
        write_wav_header(&mut file, self.samples)?;
        file.flush()
    }
}

fn write_wav_header(output: &mut impl Write, samples: u32) -> std::io::Result<()> {
    let data_bytes = samples.saturating_mul(2);
    output.write_all(b"RIFF")?;
    output.write_all(&36u32.saturating_add(data_bytes).to_le_bytes())?;
    output.write_all(b"WAVEfmt ")?;
    output.write_all(&16u32.to_le_bytes())?;
    output.write_all(&1u16.to_le_bytes())?;
    output.write_all(&1u16.to_le_bytes())?;
    output.write_all(&SAMPLE_RATE.to_le_bytes())?;
    output.write_all(&(SAMPLE_RATE * 2).to_le_bytes())?;
    output.write_all(&2u16.to_le_bytes())?;
    output.write_all(&16u16.to_le_bytes())?;
    output.write_all(b"data")?;
    output.write_all(&data_bytes.to_le_bytes())
}

impl Resampler {
    fn new(input_rate: u32) -> Self {
        let step = input_rate as f64 / SAMPLE_RATE as f64;
        Self {
            step,
            pos: 16.0,
            buffer: vec![0.0; 16],
            cutoff: (1.0 / step).min(1.0) * 0.92,
        }
    }

    fn process(&mut self, input: &[f32], output: &mut Vec<f32>) {
        if (self.step - 1.0).abs() < 1e-9 {
            output.extend_from_slice(input);
            return;
        }
        self.buffer.extend_from_slice(input);
        while self.pos + 16.0 < self.buffer.len() as f64 {
            let center = self.pos.floor() as isize;
            let fraction = self.pos - center as f64;
            let mut sum = 0.0;
            let mut norm = 0.0;
            for k in -15..=16 {
                let index = center + k;
                if index < 0 || index as usize >= self.buffer.len() {
                    continue;
                }
                let x = k as f64 - fraction;
                let angle = std::f64::consts::PI * x * self.cutoff;
                let sinc = if angle.abs() < 1e-9 {
                    1.0
                } else {
                    angle.sin() / angle
                };
                let n = (x + 16.0) / 32.0;
                let window = 0.42 - 0.5 * (2.0 * std::f64::consts::PI * n).cos()
                    + 0.08 * (4.0 * std::f64::consts::PI * n).cos();
                let weight = sinc * window;
                sum += self.buffer[index as usize] as f64 * weight;
                norm += weight;
            }
            output.push(if norm.abs() > 1e-9 {
                (sum / norm) as f32
            } else {
                0.0
            });
            self.pos += self.step;
        }
        let keep_from = (self.pos.floor() as isize - 16).max(0) as usize;
        if keep_from > 0 {
            self.buffer.drain(..keep_from);
            self.pos -= keep_from as f64;
        }
    }
}

impl Capture {
    fn start(
        system_audio: bool,
        track: Arc<Mutex<WavWriter>>,
        io_error: Arc<Mutex<Option<String>>>,
    ) -> Result<Self, String> {
        let (ready_tx, ready_rx) = mpsc::channel::<Result<(), String>>();
        let (stop_tx, stop_rx) = mpsc::channel::<()>();
        let thread = std::thread::Builder::new()
            .name("macha-notetaker-audio".into())
            .spawn(move || {
                let stream = match build_stream(system_audio, track, io_error) {
                    Ok(stream) => stream,
                    Err(error) => {
                        let _ = ready_tx.send(Err(error));
                        return;
                    }
                };
                if let Err(error) = stream.play() {
                    let _ = ready_tx.send(Err(error.to_string()));
                    return;
                }
                let _ = ready_tx.send(Ok(()));
                let _ = stop_rx.recv();
                drop(stream);
            })
            .map_err(|error| error.to_string())?;

        match ready_rx.recv_timeout(Duration::from_secs(5)) {
            Ok(Ok(())) => Ok(Self {
                stop: Some(stop_tx),
                thread: Some(thread),
            }),
            Ok(Err(error)) => Err(error),
            Err(_) => Err("The audio device did not respond.".into()),
        }
    }

    fn stop(&mut self) {
        if let Some(stop) = self.stop.take() {
            let _ = stop.send(());
        }
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

impl Drop for Capture {
    fn drop(&mut self) {
        self.stop();
    }
}

fn build_stream(
    system_audio: bool,
    track: Arc<Mutex<WavWriter>>,
    io_error: Arc<Mutex<Option<String>>>,
) -> Result<cpal::Stream, String> {
    let host = cpal::default_host();
    let device = if system_audio {
        host.default_output_device()
    } else {
        host.default_input_device()
    }
    .ok_or_else(|| {
        if system_audio {
            "No default output device is available."
        } else {
            "No microphone is available."
        }
        .to_string()
    })?;
    let supported = if system_audio {
        device.default_output_config()
    } else {
        device.default_input_config()
    }
    .map_err(|error| error.to_string())?;
    let sample_rate = supported.sample_rate();
    let channels = supported.channels().max(1) as usize;
    let sample_format = supported.sample_format();
    let config: cpal::StreamConfig = supported.into();
    let mut resampler = Resampler::new(sample_rate);
    let mut mono = Vec::with_capacity(4096);
    let mut converted = Vec::with_capacity(4096);
    let mut push = move |input: &mut dyn Iterator<Item = f32>| {
        mono.clear();
        let mut sum = 0.0;
        let mut count = 0;
        for sample in input {
            sum += sample;
            count += 1;
            if count == channels {
                mono.push((sum / channels as f32) as f32);
                sum = 0.0;
                count = 0;
            }
        }
        converted.clear();
        resampler.process(&mono, &mut converted);
        if !converted.is_empty() {
            if let Err(error) = track.lock().unwrap().write(&converted) {
                *io_error.lock().unwrap() = Some(error.to_string());
            }
        }
    };
    let error_handler = |error: cpal::Error| crate::log::line(format!("notetaker audio: {error}"));
    let stream = match sample_format {
        cpal::SampleFormat::F32 => device.build_input_stream::<f32, _, _>(
            config,
            move |data: &[f32], _| push(&mut data.iter().copied()),
            error_handler,
            None,
        ),
        cpal::SampleFormat::I16 => device.build_input_stream::<i16, _, _>(
            config,
            move |data: &[i16], _| push(&mut data.iter().map(|sample| *sample as f32 / 32768.0)),
            error_handler,
            None,
        ),
        cpal::SampleFormat::I32 => device.build_input_stream::<i32, _, _>(
            config,
            move |data: &[i32], _| {
                push(&mut data.iter().map(|sample| *sample as f32 / 2_147_483_648.0))
            },
            error_handler,
            None,
        ),
        cpal::SampleFormat::U16 => device.build_input_stream::<u16, _, _>(
            config,
            move |data: &[u16], _| {
                push(
                    &mut data
                        .iter()
                        .map(|sample| (*sample as f32 - 32768.0) / 32768.0),
                )
            },
            error_handler,
            None,
        ),
        format => return Err(format!("Unsupported audio sample format: {format:?}")),
    };
    stream.map_err(|error| error.to_string())
}

fn notes_dir() -> PathBuf {
    crate::platform::local_dir().join("Notes")
}

fn note_dir(id: &str) -> PathBuf {
    notes_dir().join(id)
}

fn new_id() -> String {
    format!("{:032x}", rand::thread_rng().gen::<u128>())
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

fn save_note(note: &Note) -> Result<(), String> {
    let directory = note_dir(&note.id);
    crate::platform::ensure_private_dir(&directory).map_err(|error| error.to_string())?;
    let json = serde_json::to_vec_pretty(note).map_err(|error| error.to_string())?;
    let temporary = directory.join("note.json.tmp");
    let destination = directory.join("note.json");
    fs::write(&temporary, json).map_err(|error| error.to_string())?;
    if destination.exists() {
        fs::remove_file(&destination).map_err(|error| error.to_string())?;
    }
    fs::rename(temporary, destination).map_err(|error| error.to_string())
}

fn changed(app: &AppHandle, note: &Note) {
    if let Err(error) = save_note(note) {
        crate::log::line(format!(
            "notetaker: could not save note {}: {error}",
            note.id
        ));
    }
    let _ = app.emit("notes-changed", &note.id);
}

fn update(app: &AppHandle, taker: &NoteTaker, id: &str, change: impl FnOnce(&mut Note)) {
    let updated = {
        let mut notes = taker.notes.lock().unwrap();
        let Some(note) = notes.iter_mut().find(|note| note.id == id) else {
            return;
        };
        change(note);
        note.clone()
    };
    changed(app, &updated);
}

pub fn setup(app: &AppHandle) {
    let mut notes = Vec::new();
    if let Ok(entries) = fs::read_dir(notes_dir()) {
        for entry in entries.flatten() {
            match fs::read_to_string(entry.path().join("note.json"))
                .ok()
                .and_then(|data| serde_json::from_str::<Note>(&data).ok())
            {
                Some(mut note) => {
                    if note.status == NoteStatus::Recording {
                        note.status = NoteStatus::Failed;
                        note.error = Some("Macha closed before this recording was stopped.".into());
                        note.end.get_or_insert(note.start);
                        let _ = save_note(&note);
                    }
                    notes.push(note);
                }
                None => crate::log::line(format!(
                    "notetaker: skipped unreadable note in {}",
                    entry.path().display()
                )),
            }
        }
    }
    notes.sort_by(|left, right| right.start.cmp(&left.start));
    let processing: Vec<String> = notes
        .iter()
        .filter(|note| note.status == NoteStatus::Processing)
        .map(|note| note.id.clone())
        .collect();
    app.manage(NoteTaker {
        notes: Mutex::new(notes),
        ..NoteTaker::default()
    });
    for id in processing {
        let app = app.clone();
        std::thread::spawn(move || process_note(&app, &id));
    }
}

fn start(app: &AppHandle, taker: &NoteTaker) -> Result<String, String> {
    if taker.recording.lock().unwrap().is_some() || taker.starting.swap(true, Ordering::Relaxed) {
        return Err("A Notetaker recording is already in progress.".into());
    }
    let result = start_inner(app, taker);
    taker.starting.store(false, Ordering::Relaxed);
    result
}

fn start_inner(app: &AppHandle, taker: &NoteTaker) -> Result<String, String> {
    let id = new_id();
    let start = now_ms();
    let title = format!("Call · {}", local_title());
    let directory = note_dir(&id);
    crate::platform::ensure_private_dir(&directory).map_err(|error| error.to_string())?;
    let mic_track = Arc::new(Mutex::new(
        WavWriter::create(&directory.join("mic.wav")).map_err(|error| error.to_string())?,
    ));
    let io_error = Arc::new(Mutex::new(None));
    let mic = match Capture::start(false, mic_track.clone(), io_error.clone()) {
        Ok(capture) => capture,
        Err(error) => {
            let _ = mic_track.lock().unwrap().finish();
            drop(mic_track);
            let _ = fs::remove_dir_all(&directory);
            return Err(format!("Could not start microphone recording: {error}"));
        }
    };
    let mut system_track = match WavWriter::create(&directory.join("system.wav")) {
        Ok(track) => Some(Arc::new(Mutex::new(track))),
        Err(error) => {
            crate::log::line(format!(
                "notetaker: could not create system audio file: {error}"
            ));
            None
        }
    };
    let system = if let Some(track) = system_track.as_ref() {
        match Capture::start(true, track.clone(), io_error.clone()) {
            Ok(capture) => Some(capture),
            Err(error) => {
                crate::log::line(format!("notetaker: system audio unavailable: {error}"));
                if let Some(track) = system_track.take() {
                    let _ = track.lock().unwrap().finish();
                    drop(track);
                }
                let _ = fs::remove_file(directory.join("system.wav"));
                None
            }
        }
    } else {
        None
    };
    let note = Note {
        id: id.clone(),
        title,
        start,
        end: None,
        status: NoteStatus::Recording,
        speakers: vec![
            NoteSpeaker {
                id: "me".into(),
                name: "Me".into(),
            },
            NoteSpeaker {
                id: "others".into(),
                name: "Call audio".into(),
            },
        ],
        lines: Vec::new(),
        error: None,
    };
    if let Err(error) = save_note(&note) {
        drop(system);
        drop(mic);
        let _ = mic_track.lock().unwrap().finish();
        if let Some(track) = system_track.as_ref() {
            let _ = track.lock().unwrap().finish();
        }
        drop(mic_track);
        drop(system_track);
        let _ = fs::remove_dir_all(&directory);
        return Err(format!("Could not save the new note: {error}"));
    }
    taker.notes.lock().unwrap().insert(0, note.clone());
    *taker.recording.lock().unwrap() = Some(Recording {
        id: id.clone(),
        mic,
        system,
        mic_track,
        system_track: system_track.take(),
        io_error,
    });
    let _ = app.emit("notes-changed", &id);
    crate::log::line(format!("notetaker: started recording {id}"));
    Ok(id)
}

fn local_title() -> String {
    let timestamp = crate::platform::local_time();
    format!(
        "{:04}-{:02}-{:02} {:02}:{:02}",
        timestamp.year, timestamp.month, timestamp.day, timestamp.hour, timestamp.minute
    )
}

pub fn shutdown(app: &AppHandle) {
    stop_recording(app, false);
}

fn stop_recording(app: &AppHandle, process_after: bool) {
    let Some(state) = app.try_state::<NoteTaker>() else {
        return;
    };
    let taker = state.inner();
    let Some(mut recording) = taker.recording.lock().unwrap().take() else {
        return;
    };
    recording.mic.stop();
    if let Some(system) = recording.system.as_mut() {
        system.stop();
    }
    let mut errors = Vec::new();
    if let Err(error) = recording.mic_track.lock().unwrap().finish() {
        errors.push(format!("microphone audio: {error}"));
    }
    if let Some(track) = recording.system_track.take() {
        if let Err(error) = track.lock().unwrap().finish() {
            errors.push(format!("system audio: {error}"));
        }
    }
    if let Some(error) = recording.io_error.lock().unwrap().take() {
        errors.push(format!("could not write captured audio: {error}"));
    }
    update(app, &taker, &recording.id, |note| {
        note.end = Some(now_ms());
        if errors.is_empty() {
            note.status = NoteStatus::Processing;
        } else {
            note.status = NoteStatus::Failed;
            note.error = Some(errors.join("; "));
        }
    });
    if process_after && errors.is_empty() {
        let app = app.clone();
        let id = recording.id;
        std::thread::spawn(move || process_note(&app, &id));
    }
}

async fn transcribe_chunk(audio: Vec<u8>) -> Result<Vec<SttSegment>, String> {
    let part = reqwest::multipart::Part::bytes(audio)
        .file_name("recording.wav")
        .mime_str("audio/wav")
        .map_err(|error| error.to_string())?;
    let form = reqwest::multipart::Form::new().part("audio", part);
    let response = reqwest::Client::new()
        .post(format!("{STT_URL}/transcribe"))
        .timeout(Duration::from_secs(180))
        .multipart(form)
        .send()
        .await
        .map_err(|error| format!("Local speech service request failed: {error}"))?;
    let status = response.status();
    let body: SttResponse = response
        .json()
        .await
        .map_err(|error| format!("Invalid response from local speech service: {error}"))?;
    if !status.is_success() {
        return Err(body
            .error
            .unwrap_or_else(|| format!("Local speech service returned HTTP {status}.")));
    }
    Ok(body.segments)
}

fn process_track(
    app: &AppHandle,
    taker: &NoteTaker,
    id: &str,
    path: &Path,
    speaker: &str,
) -> Result<(), String> {
    if !path.exists() {
        return Ok(());
    }
    let mut input = File::open(path).map_err(|error| error.to_string())?;
    let mut header = [0u8; 44];
    input
        .read_exact(&mut header)
        .map_err(|error| format!("Invalid recorded audio: {error}"))?;
    if &header[..4] != b"RIFF" || &header[8..12] != b"WAVE" {
        return Err("The recorded audio file is not a valid WAV file.".into());
    }
    let mut offset_samples = 0usize;
    loop {
        let mut pcm = vec![0u8; CHUNK_SAMPLES * 2];
        let mut filled = 0;
        while filled < pcm.len() {
            match input.read(&mut pcm[filled..]) {
                Ok(0) => break,
                Ok(read) => filled += read,
                Err(error) => return Err(error.to_string()),
            }
        }
        pcm.truncate(filled - filled % 2);
        if pcm.is_empty() {
            break;
        }
        let mut wav = Vec::with_capacity(44 + pcm.len());
        write_wav_header(&mut wav, (pcm.len() / 2) as u32).map_err(|error| error.to_string())?;
        wav.extend_from_slice(&pcm);
        let segments = tauri::async_runtime::block_on(transcribe_chunk(wav))?;
        let offset_seconds = offset_samples as f64 / SAMPLE_RATE as f64;
        if !segments.is_empty() {
            update(app, taker, id, |note| {
                note.lines
                    .extend(segments.into_iter().filter_map(|segment| {
                        let text = segment.text.trim().to_string();
                        (!text.is_empty()).then(|| NoteLine {
                            id: new_id(),
                            speaker: speaker.into(),
                            start: offset_seconds + segment.start.max(0.0),
                            end: offset_seconds + segment.end.max(segment.start),
                            text,
                        })
                    }));
                note.lines
                    .sort_by(|left, right| left.start.total_cmp(&right.start));
            });
        }
        offset_samples += pcm.len() / 2;
        if pcm.len() < CHUNK_SAMPLES * 2 {
            break;
        }
    }
    Ok(())
}

fn process_note(app: &AppHandle, id: &str) {
    let Some(state) = app.try_state::<NoteTaker>() else {
        return;
    };
    let taker = state.inner();
    {
        let mut working = taker.working.lock().unwrap();
        if working.iter().any(|working_id| working_id == id) {
            return;
        }
        working.push(id.to_string());
    }
    let _ = app.emit("notes-changed", id);
    let directory = note_dir(id);
    let result = process_track(app, &taker, id, &directory.join("mic.wav"), "me")
        .and_then(|()| process_track(app, &taker, id, &directory.join("system.wav"), "others"));
    taker
        .working
        .lock()
        .unwrap()
        .retain(|working_id| working_id != id);
    match result {
        Ok(()) => {
            update(app, &taker, id, |note| {
                note.status = NoteStatus::Ready;
                note.error = None;
            });
            let _ = fs::remove_file(directory.join("mic.wav"));
            let _ = fs::remove_file(directory.join("system.wav"));
        }
        Err(error) => {
            crate::log::line(format!("notetaker: transcript failed for {id}: {error}"));
            update(app, &taker, id, |note| {
                note.status = NoteStatus::Failed;
                note.error = Some(error);
            });
        }
    }
}

#[tauri::command]
pub fn notes_state(taker: State<NoteTaker>) -> NotesState {
    NotesState {
        notes: taker.notes.lock().unwrap().clone(),
        recording: taker
            .recording
            .lock()
            .unwrap()
            .as_ref()
            .map(|recording| recording.id.clone()),
        working: taker.working.lock().unwrap().clone(),
    }
}

#[tauri::command]
pub async fn notes_start(app: AppHandle) -> Result<String, String> {
    let response = reqwest::Client::new()
        .get(format!("{STT_URL}/health"))
        .timeout(Duration::from_secs(2))
        .send()
        .await
        .map_err(|_| "Macha's local speech service is not running. Start voice setup in Settings and try again.".to_string())?;
    if !response.status().is_success() {
        return Err(format!(
            "Macha's local speech service returned HTTP {}.",
            response.status()
        ));
    }
    tauri::async_runtime::spawn_blocking(move || {
        let taker = app.state::<NoteTaker>();
        start(&app, &taker)
    })
    .await
    .map_err(|error| format!("Could not start Notetaker: {error}"))?
}

#[tauri::command]
pub fn notes_stop(app: AppHandle) {
    stop_recording(&app, true);
}

#[tauri::command]
pub fn notes_rename(
    app: AppHandle,
    taker: State<NoteTaker>,
    id: String,
    title: String,
) -> Result<(), String> {
    let title = title.trim();
    if title.is_empty() || title.chars().count() > 120 {
        return Err("A note title must contain between 1 and 120 characters.".into());
    }
    let updated = {
        let mut notes = taker.notes.lock().unwrap();
        let note = notes
            .iter_mut()
            .find(|note| note.id == id)
            .ok_or("Note not found.")?;
        note.title = title.to_string();
        note.clone()
    };
    changed(&app, &updated);
    Ok(())
}

#[tauri::command]
pub fn notes_rename_speaker(
    app: AppHandle,
    taker: State<NoteTaker>,
    id: String,
    speaker: String,
    name: String,
) -> Result<(), String> {
    let name = name.trim();
    if name.is_empty() || name.chars().count() > 80 {
        return Err("A speaker name must contain between 1 and 80 characters.".into());
    }
    let updated = {
        let mut notes = taker.notes.lock().unwrap();
        let note = notes
            .iter_mut()
            .find(|note| note.id == id)
            .ok_or("Note not found.")?;
        let target = note
            .speakers
            .iter_mut()
            .find(|entry| entry.id == speaker)
            .ok_or("Speaker not found.")?;
        target.name = name.to_string();
        note.clone()
    };
    changed(&app, &updated);
    Ok(())
}

#[tauri::command]
pub fn notes_delete(app: AppHandle, taker: State<NoteTaker>, id: String) -> Result<(), String> {
    if taker
        .recording
        .lock()
        .unwrap()
        .as_ref()
        .is_some_and(|recording| recording.id == id)
        || taker
            .working
            .lock()
            .unwrap()
            .iter()
            .any(|working| working == &id)
    {
        return Err("Stop or wait for this recording before deleting it.".into());
    }
    if !taker.notes.lock().unwrap().iter().any(|note| note.id == id) {
        return Err("Note not found.".into());
    }
    fs::remove_dir_all(note_dir(&id))
        .map_err(|error| format!("Could not delete note audio: {error}"))?;
    taker.notes.lock().unwrap().retain(|note| note.id != id);
    let _ = app.emit("notes-changed", id);
    Ok(())
}

#[tauri::command]
pub fn notes_retry(app: AppHandle, taker: State<NoteTaker>, id: String) -> Result<(), String> {
    if taker
        .working
        .lock()
        .unwrap()
        .iter()
        .any(|working| working == &id)
    {
        return Err("This note is already being processed.".into());
    }
    {
        let mut notes = taker.notes.lock().unwrap();
        let note = notes
            .iter_mut()
            .find(|note| note.id == id)
            .ok_or("Note not found.")?;
        if note.status != NoteStatus::Failed {
            return Err("Only failed notes can be retried.".into());
        }
        if !note_dir(&id).join("mic.wav").is_file() {
            return Err("The recording audio needed to retry transcription is unavailable.".into());
        }
        note.status = NoteStatus::Processing;
        note.error = None;
        note.lines.clear();
        save_note(note)?;
    }
    let _ = app.emit("notes-changed", &id);
    std::thread::spawn(move || process_note(&app, &id));
    Ok(())
}

#[tauri::command]
pub fn notes_markdown(taker: State<NoteTaker>, id: String) -> Result<String, String> {
    let notes = taker.notes.lock().unwrap();
    let note = notes
        .iter()
        .find(|note| note.id == id)
        .ok_or("Note not found.")?;
    let speakers: HashMap<&str, &str> = note
        .speakers
        .iter()
        .map(|speaker| (speaker.id.as_str(), speaker.name.as_str()))
        .collect();
    let mut markdown = format!("# {}\n\n", note.title);
    for line in &note.lines {
        let speaker = speakers
            .get(line.speaker.as_str())
            .copied()
            .unwrap_or(line.speaker.as_str());
        let seconds = line.start.max(0.0);
        markdown.push_str(&format!(
            "**{}** ({:02}:{:02}) {}\n\n",
            speaker,
            (seconds / 60.0).floor() as u64,
            (seconds % 60.0).floor() as u64,
            line.text
        ));
    }
    Ok(markdown)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wav_header_declares_mono_16khz_pcm() {
        let mut header = Vec::new();
        write_wav_header(&mut header, 2).unwrap();
        assert_eq!(&header[..4], b"RIFF");
        assert_eq!(&header[8..12], b"WAVE");
        assert_eq!(&header[22..24], &1u16.to_le_bytes());
        assert_eq!(&header[24..28], &SAMPLE_RATE.to_le_bytes());
        assert_eq!(&header[40..44], &4u32.to_le_bytes());
    }

    #[test]
    fn equal_rate_resampling_preserves_samples() {
        let input = [0.25, -0.5, 0.75];
        let mut output = Vec::new();
        Resampler::new(SAMPLE_RATE).process(&input, &mut output);
        assert_eq!(output, input);
    }
}
