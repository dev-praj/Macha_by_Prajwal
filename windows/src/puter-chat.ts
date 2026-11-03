export {};

const MODEL = "openai/gpt-5.4-nano";

declare global {
  interface Window {
    puter?: {
      ai?: {
        chat?: (prompt: string, options: { model: string }) => Promise<unknown>;
      };
    };
  }
}

function requiredElement<T extends Element>(selector: string): T {
  const element = document.querySelector<T>(selector);
  if (!element) throw new Error(`Puter chat page is missing ${selector}.`);
  return element;
}

const form = requiredElement<HTMLFormElement>("#chat-form");
const promptField = requiredElement<HTMLTextAreaElement>("#prompt");
const sendButton = requiredElement<HTMLButtonElement>("#send");
const messages = requiredElement<HTMLElement>("#messages");
const status = requiredElement<HTMLElement>("#status");

function appendMessage(text: string, kind: "user" | "assistant" | "error"): HTMLElement {
  const element = document.createElement("div");
  element.className = `message ${kind}`;
  element.textContent = text;
  messages.append(element);
  element.scrollIntoView({ block: "end", behavior: "smooth" });
  return element;
}

function responseText(result: unknown): string {
  if (typeof result !== "object" || result === null || !("message" in result)) {
    throw new Error("Puter returned an unexpected response.");
  }

  const message = result.message;
  if (typeof message !== "object" || message === null || !("content" in message)) {
    throw new Error("Puter returned an unexpected response.");
  }
  const content = message.content;
  if (typeof content !== "string" || !content.trim()) {
    throw new Error("Puter returned an empty response.");
  }
  return content.trim();
}

form.addEventListener("submit", (event) => {
  event.preventDefault();
  void sendMessage();
});

promptField.addEventListener("keydown", (event) => {
  if (event.key === "Enter" && !event.shiftKey) {
    event.preventDefault();
    form.requestSubmit();
  }
});

async function sendMessage(): Promise<void> {
  const prompt = promptField.value.trim();
  if (!prompt || sendButton.disabled) return;

  const ai = window.puter?.ai;
  if (!ai?.chat) {
    const message = "Puter.js did not load. Check your internet connection and reload this page.";
    appendMessage(message, "error");
    status.textContent = "Puter.js unavailable";
    return;
  }

  appendMessage(prompt, "user");
  promptField.value = "";
  sendButton.disabled = true;
  status.textContent = "Waiting for Puter…";

  try {
    const result = await ai.chat(prompt, { model: MODEL });
    appendMessage(responseText(result), "assistant");
    status.textContent = `Answered with ${MODEL}`;
  } catch (error) {
    const message = error instanceof Error ? error.message : "The Puter request failed.";
    appendMessage(message, "error");
    status.textContent = "Request failed";
  } finally {
    sendButton.disabled = false;
    promptField.focus();
  }
}
