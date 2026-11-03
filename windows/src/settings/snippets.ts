import type { DictationSnippet } from "../core/state";
import { h, clear } from "../views/dom";

export function snippetsSection(
  initial: DictationSnippet[],
  persist: (snippets: DictationSnippet[]) => Promise<void>,
): HTMLElement {
  let snippets = initial.map((snippet) => ({ ...snippet }));
  let editingIndex: number | null = null;
  let saving = false;

  const feedback = h("p", { class: "hint", "aria-live": "polite" });
  const trigger = h("input", {
    type: "text",
    maxlength: "100",
    placeholder: "A short phrase you say, e.g. my signature",
    "aria-label": "Snippet trigger phrase",
  }) as HTMLInputElement;
  const text = h("textarea", {
    maxlength: "10000",
    placeholder: "The text to insert when you say the trigger",
    "aria-label": "Snippet text to insert",
  }) as HTMLTextAreaElement;
  const list = h("div", { class: "dictation-snippet-list" });
  const cancel = h("button", { class: "secondary", text: "Cancel", type: "button" });
  const save = h("button", { class: "primary", text: "Add snippet", type: "button" });

  const resetForm = () => {
    editingIndex = null;
    trigger.value = "";
    text.value = "";
    save.textContent = "Add snippet";
    cancel.hidden = true;
  };
  cancel.addEventListener("click", resetForm);

  const draw = () => {
    clear(list);
    if (snippets.length === 0) {
      list.append(h("p", {
        class: "hint",
        text: "No snippets yet. Add a trigger for a signature, address, booking link, or other text you often dictate.",
      }));
      return;
    }

    for (const [index, snippet] of snippets.entries()) {
      const triggerLabel = h("strong", { text: `“${snippet.trigger}”` });
      const preview = h("pre", { class: "dictation-snippet-preview", text: snippet.text });
      const edit = h("button", { class: "secondary", text: "Edit", type: "button" });
      edit.addEventListener("click", () => {
        editingIndex = index;
        trigger.value = snippet.trigger;
        text.value = snippet.text;
        save.textContent = "Save snippet";
        cancel.hidden = false;
        feedback.textContent = "";
        trigger.focus();
      });
      const remove = h("button", { class: "danger", text: "Remove", type: "button" });
      remove.addEventListener("click", () => {
        if (editingIndex === index) {
          resetForm();
        } else if (editingIndex !== null && index < editingIndex) {
          editingIndex -= 1;
        }
        void commit(snippets.filter((_, itemIndex) => itemIndex !== index));
      });
      list.append(
        h("article", { class: "dictation-snippet-item" },
          h("div", { class: "dictation-snippet-content" }, triggerLabel, preview),
          h("div", { class: "dictation-snippet-actions" }, edit, remove),
        ),
      );
    }
  };

  async function commit(next: DictationSnippet[]) {
    if (saving) return;
    saving = true;
    save.disabled = true;
    try {
      await persist(next);
      snippets = next;
      feedback.textContent = "Snippets saved.";
      resetForm();
      draw();
    } catch (error) {
      feedback.textContent = `Could not save snippets: ${error instanceof Error ? error.message : String(error)}`;
      console.error("[macha] could not save dictation snippets", error);
    } finally {
      saving = false;
      save.disabled = false;
    }
  }

  save.addEventListener("click", () => {
    const nextTrigger = trigger.value.trim();
    const nextText = text.value;
    if (!nextTrigger) {
      feedback.textContent = "Enter the phrase you will say to insert this snippet.";
      trigger.focus();
      return;
    }
    if (!nextText.trim()) {
      feedback.textContent = "Enter the text to insert.";
      text.focus();
      return;
    }
    const duplicate = snippets.findIndex((snippet, index) =>
      index !== editingIndex && snippet.trigger.toLocaleLowerCase() === nextTrigger.toLocaleLowerCase(),
    );
    if (duplicate !== -1) {
      feedback.textContent = "That trigger phrase already has a snippet.";
      trigger.focus();
      return;
    }
    if (editingIndex === null && snippets.length >= 100) {
      feedback.textContent = "You can add up to 100 snippets.";
      return;
    }
    const next = snippets.map((snippet, index) =>
      index === editingIndex ? { trigger: nextTrigger, text: nextText } : snippet,
    );
    if (editingIndex === null) next.push({ trigger: nextTrigger, text: nextText });
    void commit(next);
  });

  draw();
  cancel.hidden = true;
  return h(
    "section",
    {},
    h("h2", {}, h("span", { text: "Text snippets" })),
    h("p", {
      class: "hint",
      text: "Say a trigger phrase while dictating and Macha inserts its saved text. Snippet text is processed locally and is not sent to the speech service.",
    }),
    h("div", { class: "dictation-snippet-form" },
      h("label", {}, h("span", { text: "You say" }), trigger),
      h("label", {}, h("span", { text: "Text to insert" }), text),
      h("div", { class: "dictation-snippet-form-actions" }, cancel, save),
    ),
    list,
    feedback,
  );
}
