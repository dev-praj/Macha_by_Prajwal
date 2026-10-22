"""Offline speech-to-text service used by Macha."""

from pathlib import Path
import inspect
import json
import sys
import tempfile

import av
import numpy as np
from flask import Flask, jsonify, request
from faster_whisper import WhisperModel


def resource_path(relative):
    base_path = getattr(sys, "_MEIPASS", Path(__file__).resolve().parent)
    return str(Path(base_path) / relative)


MODEL_NAME = (
    resource_path("whisper-base.en")
    if getattr(sys, "frozen", False)
    else "base.en"
)
TRANSCRIPTION_LANGUAGE = "en"
DEVICE = "cpu"
COMPUTE_TYPE = "int8"
BEAM_SIZE = 1
VAD_FILTER = True
# Scores worse than this are noise or a guess. Real speech on base.en
# normally lands above this, including short names.
MIN_LOGPROB = -1.5
# Whisper's own "this is not speech" probability. Above this, the text is
# usually a hallucination such as a prompted wake phrase.
MAX_NO_SPEECH_PROB = 0.5
MAX_COMPRESSION_RATIO = 2.4
SAMPLE_RATE = 16_000
HOST = "127.0.0.1"
PORT = 5005
MAX_AUDIO_BYTES = 25 * 1024 * 1024

app = Flask(__name__)
app.config["MAX_CONTENT_LENGTH"] = MAX_AUDIO_BYTES
model = WhisperModel(MODEL_NAME, device=DEVICE, compute_type=COMPUTE_TYPE)
TRANSCRIBE_PARAMS = set(inspect.signature(model.transcribe).parameters)


def decode_audio(path):
    resampler = av.audio.resampler.AudioResampler(
        format="s16",
        layout="mono",
        rate=SAMPLE_RATE,
    )
    samples = []
    with av.open(str(path), mode="r") as container:
        for frame in container.decode(audio=0):
            samples.extend(resampler.resample(frame))
        samples.extend(resampler.resample(None))
    if not samples:
        return np.array([], dtype=np.float32)
    pcm = np.concatenate([frame.to_ndarray().reshape(-1) for frame in samples])
    return pcm.astype(np.float32) / 32768.0


def parse_vocabulary(raw):
    if not raw.strip():
        return []
    try:
        words = json.loads(raw)
    except json.JSONDecodeError as error:
        raise ValueError("Dictation vocabulary must be valid JSON.") from error
    if not isinstance(words, list) or len(words) > 50:
        raise ValueError("Dictation vocabulary must contain at most 50 words.")

    normalized = []
    seen = set()
    for word in words:
        if not isinstance(word, str):
            raise ValueError("Dictation vocabulary entries must be text.")
        word = word.strip()
        if not word:
            continue
        if len(word) > 64 or any(ord(char) < 32 for char in word):
            raise ValueError("Dictation vocabulary words must be 64 characters or fewer.")
        key = word.casefold()
        if key not in seen:
            normalized.append(word)
            seen.add(key)
    return normalized


@app.after_request
def add_cors_headers(response):
    response.headers["Access-Control-Allow-Origin"] = "*"
    response.headers["Access-Control-Allow-Headers"] = "Content-Type"
    response.headers["Access-Control-Allow-Methods"] = "GET, POST, OPTIONS"
    return response


@app.get("/health")
def health():
    return jsonify({"status": "ok"})


@app.route("/transcribe", methods=["POST", "OPTIONS"])
def transcribe():
    if request.method == "OPTIONS":
        return ("", 204)
    audio = request.files.get("audio")
    if audio is None or not audio.filename:
        return jsonify({"error": "An audio file is required."}), 400

    # wake_hint is accepted and ignored. Using it as Whisper's initial_prompt
    # makes noise and unrelated speech come back as "Hey Macha".
    is_wake = request.form.get("mode", "").strip() == "wake"
    try:
        vocabulary = parse_vocabulary(request.form.get("vocabulary", ""))
    except ValueError as error:
        return jsonify({"error": str(error)}), 400

    temp_path = None
    try:
        with tempfile.NamedTemporaryFile(
            suffix=Path(audio.filename).suffix or ".webm", delete=False
        ) as temp:
            temp_path = Path(temp.name)
            audio.save(temp)

        try:
            audio_data = decode_audio(temp_path)
        except Exception:
            app.logger.exception("Audio decode failed")
            return jsonify({"error": "Could not decode the audio file."}), 400

        if audio_data.size == 0:
            return jsonify({"text": "", "confidence": -100.0})

        kwargs = {
            "beam_size": BEAM_SIZE,
            "temperature": 0.0,
            "vad_filter": VAD_FILTER,
            "task": "transcribe",
            "language": TRANSCRIPTION_LANGUAGE,
            "condition_on_previous_text": False,
            "without_timestamps": True,
        }
        if "no_speech_threshold" in TRANSCRIBE_PARAMS:
            kwargs["no_speech_threshold"] = 0.6
        if vocabulary and "initial_prompt" in TRANSCRIBE_PARAMS:
            kwargs["initial_prompt"] = (
                "Use these spellings for names and terms only when they are spoken: "
                + ", ".join(vocabulary)
                + "."
            )
        if is_wake and "vad_parameters" in TRANSCRIBE_PARAMS:
            kwargs["vad_parameters"] = {
                "min_silence_duration_ms": 300,
                "speech_pad_ms": 240,
            }
        segments, _ = model.transcribe(audio_data, **kwargs)
        segments = list(segments)
        text = " ".join(segment.text.strip() for segment in segments).strip()
        if not segments or not text:
            return jsonify({
                "text": "",
                "confidence": -100.0,
                "no_speech_prob": 1.0,
                "compression_ratio": 0.0,
            })
        average_logprob = sum(segment.avg_logprob for segment in segments) / len(segments)
        no_speech_prob = max(segment.no_speech_prob for segment in segments)
        compression_ratio = max(segment.compression_ratio for segment in segments)
        # Keep the raw logprob. Clamping it hid unusable transcripts from the app.
        if is_wake and (
            no_speech_prob >= MAX_NO_SPEECH_PROB
            or average_logprob < MIN_LOGPROB
            or compression_ratio > MAX_COMPRESSION_RATIO
        ):
            text = ""
        return jsonify({
            "text": text,
            "confidence": round(average_logprob, 3),
            "no_speech_prob": round(no_speech_prob, 3),
            "compression_ratio": round(compression_ratio, 3),
        })
    except Exception:
        app.logger.exception("Audio transcription failed")
        return jsonify({"error": "Audio transcription failed."}), 500
    finally:
        if temp_path is not None:
            temp_path.unlink(missing_ok=True)


if __name__ == "__main__":
    app.run(host=HOST, port=PORT, debug=False, threaded=True)
