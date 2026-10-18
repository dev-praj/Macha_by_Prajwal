// Nixi status bar: expandable status text and detail drawer in compact pill.

import { Bridge, IS_TAURI, onNixiStatus } from "../core/bridge";
import type { NixiStatus } from "../core/bridge";

export class NixiStatusBar {
  readonly el: HTMLElement;
  readonly textEl: HTMLSpanElement;
  readonly chevronBtn: HTMLButtonElement;
  readonly detailEl: HTMLElement;

  private hideTimer: number | null = null;
  private unlisten: (() => void) | null = null;

  constructor() {
    this.el = document.createElement("div");
    this.el.id = "nixi-status-bar";
    this.el.className = "nixi-status-bar hidden";

    this.textEl = document.createElement("span");
    this.textEl.className = "nixi-status-text";

    this.chevronBtn = document.createElement("button");
    this.chevronBtn.className = "nixi-chevron";
    this.chevronBtn.setAttribute("aria-label", "Details");
    this.chevronBtn.textContent = "▾";

    this.detailEl = document.createElement("div");
    this.detailEl.className = "nixi-detail hidden";

    this.el.append(this.textEl, this.chevronBtn, this.detailEl);

    this.chevronBtn.addEventListener("click", (e) => {
      e.stopPropagation();
      this.toggleDetail();
    });

    if (IS_TAURI) {
      try {
        void onNixiStatus((payload: NixiStatus) => {
          try {
            this.setStatus(payload.text, payload.detail);
          } catch (err) {
            void Bridge.log(`[nixi] error handling status: ${String(err)}`);
          }
        })
          .then((unlistenFn) => {
            if (typeof unlistenFn === "function") {
              this.unlisten = unlistenFn;
            }
          })
          .catch((err: unknown) => {
            void Bridge.log(`[nixi] failed to subscribe to onNixiStatus: ${String(err)}`);
          });
      } catch (err) {
        void Bridge.log(`[nixi] failed to register onNixiStatus: ${String(err)}`);
      }
    }
  }

  /** Shows or updates the status bar with the given text and optional detail lines. */
  setStatus(text: string, detail?: string): void {
    const trimmed = text.trim();
    if (!trimmed) {
      this.clear();
      return;
    }

    this.textEl.textContent = trimmed;

    // Populate detail lines (split on \n)
    this.detailEl.replaceChildren();
    if (detail) {
      const lines = detail.split("\n");
      for (const rawLine of lines) {
        const line = rawLine.trim();
        if (!line) continue;
        const p = document.createElement("p");
        p.className = "nixi-detail-line";
        p.textContent = line;
        this.detailEl.append(p);
      }
    }

    // Show the bar
    this.el.classList.remove("hidden");

    // Reset 5s hide timer
    this.resetTimer();
  }

  /** Clears and hides the status bar. */
  clear(): void {
    if (this.hideTimer !== null) {
      window.clearTimeout(this.hideTimer);
      this.hideTimer = null;
    }
    this.el.classList.add("hidden");
    this.detailEl.classList.add("hidden");
    this.chevronBtn.classList.remove("expanded");
    this.chevronBtn.classList.remove("rotated");
    this.textEl.textContent = "";
    this.detailEl.replaceChildren();
  }

  /** Toggles detail pane visibility and chevron rotation. */
  toggleDetail(): void {
    this.detailEl.classList.toggle("hidden");
    const isExpanded = !this.detailEl.classList.contains("hidden");
    this.chevronBtn.classList.toggle("expanded", isExpanded);
    this.chevronBtn.classList.toggle("rotated", isExpanded);
  }

  private resetTimer(): void {
    if (this.hideTimer !== null) {
      window.clearTimeout(this.hideTimer);
      this.hideTimer = null;
    }
    this.hideTimer = window.setTimeout(() => {
      this.clear();
    }, 5000);
  }

  destroy(): void {
    if (this.hideTimer !== null) {
      window.clearTimeout(this.hideTimer);
      this.hideTimer = null;
    }
    if (this.unlisten) {
      this.unlisten();
      this.unlisten = null;
    }
  }
}
