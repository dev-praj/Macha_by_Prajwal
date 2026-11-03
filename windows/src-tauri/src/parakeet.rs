use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use base64::Engine;
use serde::Serialize;
use sha2::{Digest, Sha256};
use sherpa_onnx::{OfflineRecognizer, OfflineRecognizerConfig, OfflineTransducerModelConfig};
use tauri::{AppHandle, Emitter};

const MODEL_ID: &str = "parakeet-tdt-0.6b-v3-int8";
const MAX_AUDIO_BYTES: usize = 20 * 1024 * 1024;
const SAMPLE_RATE: u32 = 16_000;

struct ModelFile {
    name: &'static str,
    url: &'static str,
    size: u64,
    sha256: &'static str,
}

const FILES: &[ModelFile] = &[
    ModelFile {
        name: "encoder.int8.onnx",
        url: "https://huggingface.co/csukuangfj/sherpa-onnx-nemo-parakeet-tdt-0.6b-v3-int8/resolve/2bda32ec70b097a55adaa07d9a7173915b43cc78/encoder.int8.onnx",
        size: 652_184_281,
        sha256: "acfc2b4456377e15d04f0243af540b7fe7c992f8d898d751cf134c3a55fd2247",
    },
    ModelFile {
        name: "decoder.int8.onnx",
        url: "https://huggingface.co/csukuangfj/sherpa-onnx-nemo-parakeet-tdt-0.6b-v3-int8/resolve/2bda32ec70b097a55adaa07d9a7173915b43cc78/decoder.int8.onnx",
        size: 11_845_275,
        sha256: "179e50c43d1a9de79c8a24149a2f9bac6eb5981823f2a2ed88d655b24248db4e",
    },
    ModelFile {
        name: "joiner.int8.onnx",
        url: "https://huggingface.co/csukuangfj/sherpa-onnx-nemo-parakeet-tdt-0.6b-v3-int8/resolve/2bda32ec70b097a55adaa07d9a7173915b43cc78/joiner.int8.onnx",
        size: 6_355_277,
        sha256: "3164c13fc2821009440d20fcb5fdc78bff28b4db2f8d0f0b329101719c0948b3",
    },
    ModelFile {
        name: "tokens.txt",
        url: "https://huggingface.co/csukuangfj/sherpa-onnx-nemo-parakeet-tdt-0.6b-v3-int8/resolve/2bda32ec70b097a55adaa07d9a7173915b43cc78/tokens.txt",
        size: 93_939,
        sha256: "d58544679ea4bc6ac563d1f545eb7d474bd6cfa467f0a6e2c1dc1c7d37e3c35d",
    },
];

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase", tag = "state")]
enum ModelState {
    Missing,
    Downloading { fraction: f64 },
    Ready,
    Failed { error: String },
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelsStatus {
    parakeet: ModelState,
    download_mb: u64,
    shared_with_coucou: bool,
}

#[derive(Default)]
struct DownloadState {
    running: bool,
    fraction: f64,
    error: Option<String>,
}

fn download_state() -> &'static Mutex<DownloadState> {
    static STATE: OnceLock<Mutex<DownloadState>> = OnceLock::new();
    STATE.get_or_init(|| Mutex::new(DownloadState::default()))
}

fn macha_model_dir() -> PathBuf {
    crate::settings::local_dir().join("Models").join(MODEL_ID)
}

fn coucou_model_dir() -> Option<PathBuf> {
    std::env::var_os("LOCALAPPDATA")
        .map(PathBuf::from)
        .map(|base| base.join("Coucou").join("Models").join(MODEL_ID))
        .filter(|path| models_present(path))
}

fn models_present(directory: &Path) -> bool {
    FILES.iter().all(|file| {
        std::fs::metadata(directory.join(file.name))
            .is_ok_and(|metadata| metadata.len() == file.size)
    })
}

fn model_dir() -> (PathBuf, bool) {
    if let Some(path) = coucou_model_dir() {
        (path, true)
    } else {
        (macha_model_dir(), false)
    }
}

fn status() -> ModelsStatus {
    let downloads = download_state().lock().unwrap();
    let (directory, shared_with_coucou) = model_dir();
    let parakeet = if downloads.running {
        ModelState::Downloading {
            fraction: downloads.fraction,
        }
    } else if models_present(&directory) {
        ModelState::Ready
    } else if let Some(error) = &downloads.error {
        ModelState::Failed {
            error: error.clone(),
        }
    } else {
        ModelState::Missing
    };
    ModelsStatus {
        parakeet,
        download_mb: FILES.iter().map(|file| file.size).sum::<u64>() / 1_000_000,
        shared_with_coucou,
    }
}

fn emit_status(app: &AppHandle) {
    let _ = app.emit("speech-models", status());
}

#[tauri::command]
pub fn speech_models_status() -> ModelsStatus {
    status()
}

#[tauri::command]
pub fn speech_models_download(app: AppHandle) {
    if status().parakeet == ModelState::Ready {
        return;
    }
    {
        let mut downloads = download_state().lock().unwrap();
        if downloads.running {
            return;
        }
        downloads.running = true;
        downloads.fraction = 0.0;
        downloads.error = None;
    }
    emit_status(&app);
    tauri::async_runtime::spawn(async move {
        let result = download_models(&app).await;
        {
            let mut downloads = download_state().lock().unwrap();
            downloads.running = false;
            if let Err(error) = &result {
                downloads.error = Some(error.clone());
            }
        }
        if result.is_ok() {
            crate::log::line("Parakeet speech model is ready");
            preload();
        } else if let Err(error) = result {
            crate::log::line(format!("Parakeet model download failed: {error}"));
        }
        emit_status(&app);
    });
}

async fn download_models(app: &AppHandle) -> Result<(), String> {
    let directory = macha_model_dir();
    std::fs::create_dir_all(&directory).map_err(|error| error.to_string())?;
    let total_size = FILES.iter().map(|file| file.size).sum::<u64>();
    let mut completed = 0_u64;
    let mut last_emit = Instant::now();
    let client = reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(20))
        .read_timeout(Duration::from_secs(60))
        .user_agent(concat!("Macha/", env!("CARGO_PKG_VERSION")))
        .build()
        .map_err(|error| error.to_string())?;

    for file in FILES {
        let final_path = directory.join(file.name);
        if std::fs::metadata(&final_path).is_ok_and(|metadata| metadata.len() == file.size) {
            completed += file.size;
            continue;
        }
        let part_path = directory.join(format!("{}.part", file.name));
        let mut response = client
            .get(file.url)
            .send()
            .await
            .map_err(|error| error.to_string())?;
        if !response.status().is_success() {
            return Err(format!(
                "{} download failed with HTTP {}.",
                file.name,
                response.status().as_u16()
            ));
        }

        let mut output = std::fs::File::create(&part_path).map_err(|error| error.to_string())?;
        let mut hasher = Sha256::new();
        let mut received = 0_u64;
        while let Some(chunk) = response.chunk().await.map_err(|error| error.to_string())? {
            received = received.saturating_add(chunk.len() as u64);
            if received > file.size {
                let _ = std::fs::remove_file(&part_path);
                return Err(format!("{} exceeded its expected file size.", file.name));
            }
            output
                .write_all(&chunk)
                .map_err(|error| error.to_string())?;
            hasher.update(&chunk);
            if last_emit.elapsed() >= Duration::from_millis(250) {
                last_emit = Instant::now();
                download_state().lock().unwrap().fraction =
                    (completed + received) as f64 / total_size as f64;
                emit_status(app);
            }
        }
        output.flush().map_err(|error| error.to_string())?;
        drop(output);
        let digest = format!("{:x}", hasher.finalize());
        if received != file.size || digest != file.sha256 {
            let _ = std::fs::remove_file(&part_path);
            return Err(format!("{} failed its integrity check.", file.name));
        }
        if final_path.exists() {
            std::fs::remove_file(&final_path).map_err(|error| error.to_string())?;
        }
        std::fs::rename(&part_path, &final_path).map_err(|error| error.to_string())?;
        completed += file.size;
    }
    Ok(())
}

#[tauri::command]
pub fn speech_models_remove(app: AppHandle) -> Result<(), String> {
    if download_state().lock().unwrap().running {
        return Err("Wait for the Parakeet download to finish before removing it.".into());
    }
    if coucou_model_dir().is_some() {
        return Err("Parakeet is shared from Coucou and was not removed.".into());
    }
    unload();
    let directory = macha_model_dir();
    if directory.exists() {
        std::fs::remove_dir_all(&directory)
            .map_err(|error| format!("Could not remove Parakeet files: {error}"))?;
    }
    download_state().lock().unwrap().error = None;
    emit_status(&app);
    Ok(())
}

fn recognizer_slot() -> &'static Mutex<Option<Arc<OfflineRecognizer>>> {
    static RECOGNIZER: OnceLock<Mutex<Option<Arc<OfflineRecognizer>>>> = OnceLock::new();
    RECOGNIZER.get_or_init(|| Mutex::new(None))
}

fn recognizer_lock() -> &'static Mutex<()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
}

fn recognizer() -> Result<Arc<OfflineRecognizer>, String> {
    if let Some(recognizer) = recognizer_slot().lock().unwrap().clone() {
        return Ok(recognizer);
    }
    let (directory, _) = model_dir();
    if !models_present(&directory) {
        return Err("Download the Parakeet model in Dictation settings first.".into());
    }

    let _guard = recognizer_lock().lock().unwrap();
    if let Some(recognizer) = recognizer_slot().lock().unwrap().clone() {
        return Ok(recognizer);
    }
    let mut config = OfflineRecognizerConfig::default();
    config.model_config.transducer = OfflineTransducerModelConfig {
        encoder: Some(
            directory
                .join("encoder.int8.onnx")
                .to_string_lossy()
                .into_owned(),
        ),
        decoder: Some(
            directory
                .join("decoder.int8.onnx")
                .to_string_lossy()
                .into_owned(),
        ),
        joiner: Some(
            directory
                .join("joiner.int8.onnx")
                .to_string_lossy()
                .into_owned(),
        ),
    };
    config.model_config.tokens = Some(directory.join("tokens.txt").to_string_lossy().into_owned());
    config.model_config.model_type = Some("nemo_transducer".into());
    config.model_config.num_threads = std::thread::available_parallelism()
        .map(|count| count.get().clamp(1, 4) as i32)
        .unwrap_or(2);
    let recognizer = OfflineRecognizer::create(&config)
        .map(Arc::new)
        .ok_or_else(|| "Parakeet could not load its model files.".to_string())?;
    *recognizer_slot().lock().unwrap() = Some(recognizer.clone());
    crate::log::line("Parakeet recognition model loaded");
    Ok(recognizer)
}

pub fn preload() {
    if !models_present(&model_dir().0) || recognizer_slot().lock().unwrap().is_some() {
        return;
    }
    std::thread::spawn(|| {
        if let Err(error) = recognizer() {
            crate::log::line(format!("Parakeet preload failed: {error}"));
        }
    });
}

fn unload() {
    *recognizer_slot().lock().unwrap() = None;
}

fn wav_samples(encoded: &str) -> Result<Vec<f32>, String> {
    if encoded.len() > MAX_AUDIO_BYTES.saturating_mul(4).div_ceil(3) {
        return Err("The audio recording is too large to transcribe.".into());
    }
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(encoded)
        .map_err(|_| "The audio recording could not be decoded.".to_string())?;
    if bytes.len() < 44
        || &bytes[..4] != b"RIFF"
        || &bytes[8..12] != b"WAVE"
        || u16::from_le_bytes([bytes[20], bytes[21]]) != 1
        || u16::from_le_bytes([bytes[22], bytes[23]]) != 1
        || u32::from_le_bytes([bytes[24], bytes[25], bytes[26], bytes[27]]) != SAMPLE_RATE
        || u16::from_le_bytes([bytes[34], bytes[35]]) != 16
        || (bytes.len() - 44) % 2 != 0
    {
        return Err("The recording must be mono 16 kHz PCM audio.".into());
    }
    let audio_data = &bytes[44..];
    if audio_data.len() > MAX_AUDIO_BYTES {
        return Err("The audio recording is too large to transcribe.".into());
    }
    Ok(audio_data
        .chunks_exact(2)
        .map(|sample| i16::from_le_bytes([sample[0], sample[1]]) as f32 / 32768.0)
        .collect())
}

fn transcribe_samples(samples: &[f32], vocabulary: &[String]) -> Result<String, String> {
    if samples.is_empty() {
        return Ok(String::new());
    }
    let hotwords = normalized_vocabulary(vocabulary)?;
    let recognizer = recognizer()?;
    let mut results = Vec::new();
    for chunk in split_for_model(samples) {
        if chunk.len() < SAMPLE_RATE as usize / 4 {
            continue;
        }
        let stream = if hotwords.is_empty() {
            recognizer.create_stream()
        } else {
            recognizer.create_stream_with_hotwords(&hotwords)
        };
        stream.accept_waveform(SAMPLE_RATE as i32, chunk);
        recognizer.decode(&stream);
        if let Some(result) = stream.get_result() {
            let text = result.text.trim();
            if !text.is_empty() {
                results.push(text.to_string());
            }
        }
    }
    Ok(results.join(" "))
}

fn split_for_model(samples: &[f32]) -> Vec<&[f32]> {
    const MAX_SECONDS: usize = 28;
    let max_samples = SAMPLE_RATE as usize * MAX_SECONDS;
    let window = SAMPLE_RATE as usize / 10;
    let mut chunks = Vec::new();
    let mut start = 0;
    while start < samples.len() {
        if samples.len() - start <= max_samples {
            chunks.push(&samples[start..]);
            break;
        }
        let search_from = start + max_samples.saturating_sub(8 * SAMPLE_RATE as usize);
        let mut best = start + max_samples;
        let mut best_energy = f64::MAX;
        let mut at = search_from;
        while at + window <= start + max_samples {
            let energy = samples[at..at + window]
                .iter()
                .map(|sample| f64::from(*sample) * f64::from(*sample))
                .sum::<f64>();
            if energy < best_energy {
                best_energy = energy;
                best = at + window / 2;
            }
            at += window / 2;
        }
        chunks.push(&samples[start..best]);
        start = best;
    }
    chunks
}

fn normalized_vocabulary(vocabulary: &[String]) -> Result<String, String> {
    if vocabulary.len() > 50 {
        return Err("Dictation vocabulary must contain at most 50 words.".into());
    }
    let mut output = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for word in vocabulary {
        let word = word.trim();
        if word.is_empty() {
            continue;
        }
        if word.len() > 64 || word.chars().any(char::is_control) {
            return Err("Dictation vocabulary entries must be 64 characters or fewer.".into());
        }
        if seen.insert(word.to_lowercase()) {
            output.push(word);
        }
    }
    Ok(output.join(" "))
}

#[tauri::command]
pub async fn parakeet_transcribe(
    audio_wav_base64: String,
    vocabulary: Vec<String>,
) -> Result<String, String> {
    let samples = wav_samples(&audio_wav_base64)?;
    tauri::async_runtime::spawn_blocking(move || transcribe_samples(&samples, &vocabulary))
        .await
        .map_err(|error| format!("Parakeet transcription task failed: {error}"))?
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_invalid_or_unsupported_wav_data() {
        assert!(wav_samples("not base64").is_err());
        let mut wav = vec![0_u8; 48];
        wav[..4].copy_from_slice(b"RIFF");
        wav[8..12].copy_from_slice(b"WAVE");
        wav[20..22].copy_from_slice(&1_u16.to_le_bytes());
        wav[22..24].copy_from_slice(&1_u16.to_le_bytes());
        wav[24..28].copy_from_slice(&SAMPLE_RATE.to_le_bytes());
        wav[34..36].copy_from_slice(&16_u16.to_le_bytes());
        wav[44..48].copy_from_slice(&[0, 0, 0, 0]);
        let encoded = base64::engine::general_purpose::STANDARD.encode(wav);
        assert_eq!(wav_samples(&encoded).unwrap(), vec![0.0, 0.0]);
    }

    #[test]
    fn vocabulary_normalization_deduplicates_and_validates_terms() {
        assert_eq!(
            normalized_vocabulary(&["Macha".into(), " macha ".into(), "Rust".into()]).unwrap(),
            "Macha Rust"
        );
        assert!(normalized_vocabulary(&vec!["a".into(); 51]).is_err());
        assert!(normalized_vocabulary(&["bad\nterm".into()]).is_err());
    }

    #[test]
    #[ignore = "loads the installed 670 MB Parakeet model"]
    fn loads_the_available_parakeet_model() {
        assert!(recognizer().is_ok());
    }
}
