import DOMPurify from "dompurify";
import katex from "katex";
import MarkdownIt from "markdown-it";
import texmath from "markdown-it-texmath";
import { normalizeAssistantMarkdown } from "./markdown-normalize";
import "katex/dist/katex.min.css";
import "markdown-it-texmath/css/texmath.css";

export { normalizeAssistantMarkdown } from "./markdown-normalize";

const markdown = new MarkdownIt({
  html: false,
  linkify: false,
  breaks: true,
}).use(texmath, {
  engine: katex,
  delimiters: "dollars",
  katexOptions: {
    maxExpand: 1000,
    maxSize: 10,
    strict: "ignore",
    throwOnError: false,
    trust: false,
  },
});

function escapeHtml(text: string): string {
  return text.replace(/[&<>"']/g, (character) => ({
    "&": "&amp;",
    "<": "&lt;",
    ">": "&gt;",
    '"': "&quot;",
    "'": "&#39;",
  })[character] ?? character);
}

export function renderAssistantMarkdown(markdownText: string): string {
  if (!markdownText) return "";
  try {
    return DOMPurify.sanitize(markdown.render(normalizeAssistantMarkdown(markdownText)), {
      USE_PROFILES: { html: true, mathMl: true },
    });
  } catch (error) {
    console.error("[macha] could not render assistant Markdown", error);
    return escapeHtml(markdownText).replace(/\n/g, "<br>");
  }
}
