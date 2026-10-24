# Macha Notetaker

Notetaker records the default microphone and, where the audio driver permits it,
the default computer output. In Settings → Notetaker, start a recording before
the conversation and stop it when finished. Tell everyone involved that the
conversation is being recorded.

Transcription uses Macha's existing local speech service at `127.0.0.1:5005`.
It does not send audio to a remote service. The service must be running; if it
is unavailable, Notetaker reports the problem and allows a failed transcription
to be retried after the service is available.

Notes are saved under `%LOCALAPPDATA%\\Macha\\Notes`. Audio is removed after a
successful transcript; it is retained locally if transcription fails so it can
be retried. Deleting a note removes its saved transcript and any retained audio.
The transcript labels microphone audio as **Me** and computer output as
**Call audio**; individual-speaker diarization and AI-generated meeting
summaries are not included in this port.
