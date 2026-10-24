import assert from "node:assert/strict";
import { describe, it } from "node:test";
import {
  buildFileDiff,
  fileName,
  fromEdit,
  fromNew,
  makeDiffStep,
  parseDiffStep,
  toOneLine,
} from "../src/core/diff.ts";

describe("live file diffs", () => {
  it("counts edits and retains nearby context", () => {
    const diff = fromEdit("before\nold\nafter", "before\nnew\nafter", "src\\app.ts");

    assert.equal(diff.added, 1);
    assert.equal(diff.removed, 1);
    assert.equal(diff.path, "src\\app.ts");
    assert.deepEqual(
      diff.hunks.flatMap((hunk) => hunk.lines).map((line) => line.kind),
      ["context", "removed", "added", "context"],
    );
  });

  it("counts new files and marks oversized content without rendering a partial diff", () => {
    const created = fromNew("one\ntwo\n", "new.txt");
    const oversized = fromNew("x".repeat(200 * 1024 + 1), "large.txt");

    assert.equal(created.added, 2);
    assert.equal(created.isNewFile, true);
    assert.equal(oversized.tooLarge, true);
    assert.equal(oversized.hunks.length, 0);
  });

  it("builds diffs only for supported file-edit tool calls", () => {
    assert.equal(
      buildFileDiff("Edit", {
        file_path: "a.ts",
        old_string: "const n = 1;",
        new_string: "const n = 2;",
      })?.added,
      1,
    );
    assert.equal(buildFileDiff("Bash", { command: "echo hello" }), null);
  });

  it("encodes clickable ticker steps and extracts filenames portably", () => {
    const step = makeDiffStep("app.ts", 3, 1, 42);

    assert.deepEqual(parseDiffStep(step), {
      filename: "app.ts",
      added: 3,
      removed: 1,
      diffId: 42,
    });
    assert.equal(fileName("C:\\project\\app.ts"), "app.ts");
    assert.equal(toOneLine("# Answer\n\nSecond paragraph"), "Answer");
  });
});
