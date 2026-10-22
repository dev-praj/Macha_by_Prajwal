interface NixiRecordingSession {
  mode: "ptt" | "dictate";
  stopRequested: boolean;
  startPromise: Promise<boolean>;
  recorder: MediaRecorder | null;
  chunks: Blob[];
}

export class NixiAudioRecorder {
  private mediaStream: MediaStream | null = null;
  private activeSession: NixiRecordingSession | null = null;

  async start(mode: "ptt" | "dictate"): Promise<boolean> {
    if (this.activeSession) {
      console.warn(`[nixi] ignored ${mode} start while ${this.activeSession.mode} is active`);
      return false;
    }
    const session: NixiRecordingSession = {
      mode,
      stopRequested: false,
      startPromise: Promise.resolve(false),
      recorder: null,
      chunks: [],
    };
    this.activeSession = session;
    session.startPromise = this.startSession(session);
    return session.startPromise;
  }

  private async startSession(session: NixiRecordingSession): Promise<boolean> {
    let stream: MediaStream | null = null;
    try {
      stream = this.mediaStream?.active
        ? this.mediaStream
        : await navigator.mediaDevices.getUserMedia({
          audio: {
            channelCount: 1,
            echoCancellation: true,
            noiseSuppression: true,
            autoGainControl: true,
          },
        });
      if (session.stopRequested || this.activeSession !== session) {
        if (stream !== this.mediaStream) {
          for (const track of stream.getTracks()) track.stop();
        }
        return false;
      }
      this.mediaStream = stream;
      const mimeType = [
        "audio/webm;codecs=opus",
        "audio/webm",
        "audio/mp4",
      ].find((type) => MediaRecorder.isTypeSupported(type)) ?? "";
      const recorder = mimeType
        ? new MediaRecorder(stream, { mimeType })
        : new MediaRecorder(stream);
      session.recorder = recorder;
      recorder.addEventListener("dataavailable", (event) => {
        if (event.data.size > 0) session.chunks.push(event.data);
      });
      try {
        recorder.start(100);
      } catch {
        recorder.start();
      }
      return true;
    } catch (err) {
      console.error("[nixi] failed to start audio recording", err);
      if (stream && stream !== this.mediaStream) {
        for (const track of stream.getTracks()) track.stop();
      }
      if (this.activeSession === session) this.activeSession = null;
      return false;
    }
  }

  async stop(): Promise<{ blob: Blob; mode: "ptt" | "dictate" } | null> {
    const session = this.activeSession;
    if (!session) return null;
    session.stopRequested = true;
    const started = await session.startPromise;
    if (this.activeSession !== session) return null;
    const recorder = session.recorder;
    if (!started || !recorder || recorder.state !== "recording") {
      this.activeSession = null;
      return null;
    }

    return new Promise((resolve) => {
      recorder.addEventListener(
        "stop",
        () => {
          const blob = new Blob(session.chunks, {
            type: recorder.mimeType || "audio/webm",
          });
          session.chunks = [];
          if (this.activeSession === session) this.activeSession = null;
          resolve({ blob, mode: session.mode });
        },
        { once: true },
      );
      try {
        recorder.stop();
      } catch (err) {
        console.error("[nixi] error stopping recorder", err);
        if (this.activeSession === session) this.activeSession = null;
        resolve(null);
      }
    });
  }

  destroy() {
    const session = this.activeSession;
    this.activeSession = null;
    if (session?.recorder?.state === "recording") {
      try {
        session.recorder.stop();
      } catch (error) {
        console.error("[nixi] error stopping recorder during cleanup", error);
      }
    }
    if (this.mediaStream) {
      this.mediaStream.getTracks().forEach((track) => track.stop());
      this.mediaStream = null;
    }
  }
}
