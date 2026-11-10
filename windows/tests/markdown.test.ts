import assert from "node:assert/strict";
import { describe, it } from "node:test";
import { normalizeAssistantMarkdown } from "../src/core/markdown-normalize.ts";

describe("assistant Markdown normalization", () => {
  it("keeps compact numbered steps separate without splitting math", () => {
    const sample = "To find $a$ and $b$ from $z = ax + by$: 1. **Differentiate with respect to x:** $$\\frac{\\partial z}{\\partial x} = a$$ 2. **Differentiate with respect to y:** $$\\frac{\\partial z}{\\partial y} = b$$";
    const normalized = normalizeAssistantMarkdown(sample);

    assert.match(normalized, /\$z = ax \+ by\$/);
    assert.match(normalized, /\n\n1\. \*\*Differentiate with respect to x:/);
    assert.match(normalized, /\n\n2\. \*\*Differentiate with respect to y:/);
    assert.equal((normalized.match(/\$\$/g) ?? []).length, 4);
  });

  it("normalizes only closed bracket-delimited math outside code", () => {
    const normalized = normalizeAssistantMarkdown(
      "`\\(x\\)` and \\(a + b\\), then \\[x^2\\]. Open: \\(x",
    );

    assert.equal(normalized, "`\\(x\\)` and $a + b$, then $$x^2$$. Open: \\(x");
  });

  it("leaves incomplete dollar math available as plain text while streaming", () => {
    const normalized = normalizeAssistantMarkdown("Still calculating: $\\frac{a}{b}");
    assert.equal(normalized, "Still calculating: $\\frac{a}{b}");
  });

  it("does not close unfinished bracket or display math", () => {
    const text = "Inline: \\(x and display: \\[y and $$z";
    assert.equal(normalizeAssistantMarkdown(text), text);
  });

  it("puts inline headings and bullet steps on separate blocks", () => {
    const normalized = normalizeAssistantMarkdown("Summary: ## Details - first step - second step");
    assert.equal(normalized, "Summary:\n\n## Details\n\n- first step\n\n- second step");
  });
});
