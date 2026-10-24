import { fileName, type FileDiff } from "../core/diff";
import { h, svg } from "./dom";
import { ICONS } from "./icons";

export interface DiffCardHooks {
  dismiss(): void;
  open(path: string): void;
}

export function buildDiffCard(diff: FileDiff, hooks: DiffCardHooks): HTMLElement {
  const head = h(
    "div",
    { class: "diff-head" },
    h(
      "button",
      { class: "diff-back", title: "Back", onclick: () => hooks.dismiss() },
      svg(ICONS.chevronLeft, 9, { stroke: 2.4 }),
      h("b", { text: fileName(diff.path) }),
    ),
    h("span", { class: "grow" }),
    diff.added > 0 ? h("span", { class: "diff-added", text: `+${diff.added}` }) : null,
    diff.removed > 0 ? h("span", { class: "diff-removed", text: `−${diff.removed}` }) : null,
    h(
      "button",
      { class: "icon-btn", title: "Open in VS Code", onclick: () => hooks.open(diff.path) },
      svg(ICONS.arrowUpRight, 8),
    ),
  );

  const lines = diff.hunks.flatMap((hunk) => hunk.lines);
  let content: HTMLElement;
  if (diff.tooLarge) {
    content = h("div", { class: "diff-note", text: "Diff too large to display" });
  } else if (lines.length === 0) {
    content = h("div", { class: "diff-note", text: "No changes" });
  } else {
    content = h("div", { class: "diff-lines" });
    for (const line of lines) {
      content.append(
        h(
          "div",
          { class: `diff-line ${line.kind}` },
          h("span", {
            class: "diff-symbol",
            text: line.kind === "added" ? "+" : line.kind === "removed" ? "−" : " ",
          }),
          h("span", { class: "diff-text", text: line.text }),
        ),
      );
    }
  }

  return h("div", { class: "diff-card" }, head, content);
}
