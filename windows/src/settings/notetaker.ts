import { Bridge, onEvent, type Note, type NotesState } from "../core/bridge";
import { h, clear } from "../views/dom";

function formatTime(seconds: number): string {
  const value = Math.max(0, Math.floor(seconds));
  return `${Math.floor(value / 60)}:${String(value % 60).padStart(2, "0")}`;
}

function formatDate(timestamp: number): string {
  return new Date(timestamp).toLocaleString(undefined, {
    dateStyle: "medium",
    timeStyle: "short",
  });
}

export function notetakerSection(): HTMLElement {
  let state: NotesState = { notes: [], recording: null, working: [] };
  let selected: string | null = null;
  let refreshing = false;
  const feedback = h("div", { class: "hint", role: "status" });
  const controls = h("div", { class: "row" });
  const list = h("div", { style: "display:flex;flex-direction:column;gap:6px;max-height:280px;overflow:auto" });
  const detail = h("div", { style: "display:flex;flex-direction:column;gap:12px;min-width:0" });
  const section = h(
    "section",
    {},
    h("h2", {}, h("span", { text: "Notetaker" })),
    h("p", {
      class: "hint",
      text: "Record your microphone and, when available, computer audio. Macha transcribes recordings using its local speech service. Tell everyone in the conversation before recording.",
    }),
    controls,
    feedback,
    h("div", {
      style: "display:grid;grid-template-columns:minmax(180px, 0.8fr) minmax(0, 1.7fr);gap:18px;margin-top:14px",
    }, list, detail),
  );

  const current = (): Note | undefined => state.notes.find((note) => note.id === selected);

  const render = () => {
    if (selected && !state.notes.some((note) => note.id === selected)) selected = null;
    if (!selected) selected = state.recording ?? state.notes[0]?.id ?? null;
    clear(controls);
    clear(list);
    clear(detail);
    const isRecording = state.recording !== null;
    const start = h("button", {
      type: "button",
      class: isRecording ? "danger" : "secondary",
      text: isRecording ? "Stop recording" : "Record a call",
    });
    start.addEventListener("click", () => {
      feedback.textContent = "";
      if (isRecording) {
        void Bridge.notesStop().catch((error: unknown) => {
          feedback.textContent = `Could not stop recording: ${String(error)}`;
        }).finally(refresh);
      } else {
        start.disabled = true;
        void Bridge.notesStart().then((id) => {
          selected = id;
          feedback.textContent = "Recording started. Audio is kept locally until transcription completes.";
        }).catch((error: unknown) => {
          feedback.textContent = `Could not start recording: ${String(error)}`;
        }).finally(refresh);
      }
    });
    controls.append(start);

    if (!state.notes.length) {
      list.append(h("p", { class: "hint", text: "Your recordings will appear here." }));
    }
    for (const note of state.notes) {
      const active = note.id === selected;
      const status = note.status === "recording"
        ? "Recording…"
        : state.working.includes(note.id) || note.status === "processing"
          ? "Transcribing…"
          : note.status === "failed"
            ? "Transcription failed"
            : formatDate(note.start);
      list.append(h("button", {
        type: "button",
        class: active ? "secondary" : "",
        style: "text-align:left;white-space:normal",
        "aria-pressed": String(active),
        onclick: () => {
          selected = note.id;
          render();
        },
      }, h("strong", { text: note.title }), h("br"), h("span", { class: "hint", text: status })));
    }

    const note = current();
    if (!note) {
      detail.append(h("p", { class: "hint", text: "Select a recording to see its transcript." }));
      return;
    }
    const title = h("input", {
      type: "text",
      value: note.title,
      maxlength: "120",
      "aria-label": "Recording title",
    }) as HTMLInputElement;
    const saveTitle = h("button", { type: "button", class: "secondary", text: "Save title" });
    saveTitle.addEventListener("click", () => {
      void Bridge.notesRename(note.id, title.value).then(refresh).catch((error: unknown) => {
        feedback.textContent = `Could not rename recording: ${String(error)}`;
      });
    });
    const noteStatus = note.status === "recording"
      ? "Recording microphone and available system audio…"
      : state.working.includes(note.id) || note.status === "processing"
        ? "Transcription is running locally…"
        : note.status === "failed"
          ? note.error ?? "Transcription failed."
          : `${formatDate(note.start)}${note.end ? ` · ${Math.max(1, Math.round((note.end - note.start) / 60_000))} min` : ""}`;
    detail.append(
      h("div", { class: "row" }, title, saveTitle),
      h("p", { class: "hint", text: noteStatus }),
    );

    if (note.status === "failed") {
      const retry = h("button", { type: "button", class: "secondary", text: "Retry transcription" });
      retry.addEventListener("click", () => {
        void Bridge.notesRetry(note.id).then(refresh).catch((error: unknown) => {
          feedback.textContent = `Could not retry transcription: ${String(error)}`;
        });
      });
      detail.append(retry);
    }
    if (note.lines.length) {
      const copy = h("button", { type: "button", class: "secondary", text: "Copy transcript as Markdown" });
      copy.addEventListener("click", () => {
        void Bridge.notesMarkdown(note.id).then((markdown) => navigator.clipboard.writeText(markdown)).then(() => {
          feedback.textContent = "Transcript copied.";
        }).catch((error: unknown) => {
          feedback.textContent = `Could not copy transcript: ${String(error)}`;
        });
      });
      detail.append(copy);
      for (const person of note.speakers) {
        const rename = h("button", {
          type: "button",
          class: "secondary",
          text: `Rename ${person.name}`,
        });
        rename.addEventListener("click", () => {
          const replacement = window.prompt("Speaker name", person.name);
          if (replacement === null) return;
          void Bridge.notesRenameSpeaker(note.id, person.id, replacement).then(refresh).catch((error: unknown) => {
            feedback.textContent = `Could not rename speaker: ${String(error)}`;
          });
        });
        detail.append(rename);
      }
      for (const line of note.lines) {
        const name = note.speakers.find((speaker) => speaker.id === line.speaker)?.name ?? line.speaker;
        detail.append(h("div", {
          style: "display:grid;grid-template-columns:auto 1fr;gap:10px;padding:10px 0;border-bottom:1px solid rgba(255,255,255,0.08)",
        }, h("span", { class: "hint", text: formatTime(line.start) }), h("div", {}, h("strong", { text: name }), h("p", { text: line.text, style: "white-space:pre-wrap;margin:6px 0 0" }))));
      }
    } else if (note.status === "ready") {
      detail.append(h("p", { class: "hint", text: "No speech was recognized in this recording." }));
    }

    if (note.status !== "recording" && !state.working.includes(note.id)) {
      const remove = h("button", { type: "button", class: "danger", text: "Delete recording" });
      remove.addEventListener("click", () => {
        if (!window.confirm("Delete this recording, its transcript, and saved audio?")) return;
        void Bridge.notesDelete(note.id).then(() => {
          selected = null;
          return refresh();
        }).catch((error: unknown) => {
          feedback.textContent = `Could not delete recording: ${String(error)}`;
        });
      });
      detail.append(remove);
    }
  };

  const refresh = async () => {
    if (refreshing) return;
    refreshing = true;
    try {
      state = await Bridge.notesState();
      render();
    } catch (error) {
      feedback.textContent = `Could not load Notetaker recordings: ${String(error)}`;
    } finally {
      refreshing = false;
    }
  };

  void onEvent("notes-changed", () => void refresh()).catch((error: unknown) => {
    feedback.textContent = `Could not subscribe to Notetaker updates: ${String(error)}`;
  });
  window.setInterval(() => {
    if (!section.isConnected) return;
    if (state.recording || state.working.length) void refresh();
  }, 1500);
  void refresh();
  return section;
}
