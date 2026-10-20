// Mini Machas (pills + compact grid) — port of MiniBotCanvasView.
// Each canvas owns a BotEngine; the island's frame loop ticks every live one.

import { BotEngine, hexToRGB } from "./engine";
import { mountAvatar, type AvatarDisplay } from "./avatar";
import type { AgentTask } from "../core/state";

interface MiniBot {
  canvas: HTMLCanvasElement;
  engine: BotEngine;
  avatar: AvatarDisplay;
  avatarHost: HTMLElement;
  cssSize: number;
  taskId: string;
}

const live = new Map<HTMLCanvasElement, MiniBot>();

/**
 * Creates a mini Macha whose **body** is `bodySize` CSS pixels across.
 *
 * The engine draws the body at 60 % of its canvas, so the canvas is
 * `bodySize / 0.6` and is centred in a `bodySize` slot, overflowing it — the
 * same thing SwiftUI does with a `.frame(width: 22/0.6)` inside a
 * `.frame(width: 22)`. Sizing the canvas itself to `bodySize` would shrink the
 * whole drawing to 60 %, which is what used to happen.
 */
export function createMiniBot(task: AgentTask, bodySize: number): HTMLElement {
  const slot = document.createElement("span");
  slot.className = "mini";
  slot.style.width = `${bodySize}px`;
  slot.style.height = `${bodySize}px`;

  const canvas = document.createElement("canvas");
  const avatarHost = document.createElement("span");
  avatarHost.className = "mini-avatar-host custom-avatar-host";
  avatarHost.style.setProperty("--mini-avatar-color", task.color);
  const engineSize = bodySize / 0.6;
  const dpr = Math.min(2, window.devicePixelRatio || 1);
  canvas.width = Math.round(engineSize * dpr);
  canvas.height = Math.round(engineSize * dpr);
  canvas.style.width = `${engineSize}px`;
  canvas.style.height = `${engineSize}px`;
  slot.append(canvas, avatarHost);

  const engine = new BotEngine();
  engine.isMini = true;
  engine.bodyColor = hexToRGB(task.color);
  engine.setState(task.state, true);
  if (task.emote) engine.setPermanentEmote(task.emote);
  if (task.miniEye) {
    engine.permanentEye = task.miniEye;
    engine.eyeOverride = task.miniEye;
    engine.eyeOverrideUntil = Number.POSITIVE_INFINITY;
  }

  const avatar = mountAvatar(avatarHost);
  avatar.setState(task.state);
  live.set(canvas, { canvas, engine, avatar, avatarHost, cssSize: engineSize, taskId: task.id });
  return slot;
}

export function releaseMiniBot(canvas: HTMLCanvasElement) {
  live.get(canvas)?.avatar.destroy();
  live.delete(canvas);
}

/** Drops every canvas no longer in the document (views are rebuilt wholesale). */
export function pruneMiniBots() {
  for (const [canvas, mini] of live) {
    if (!canvas.isConnected) {
      mini.avatar.destroy();
      live.delete(canvas);
    }
  }
}

export function syncMiniBotStates(tasks: AgentTask[]) {
  for (const mb of live.values()) {
    const task = tasks.find((t) => t.id === mb.taskId);
    if (!task) continue;
    mb.engine.setState(task.state);
    mb.avatar.setState(task.state);
    mb.engine.bodyColor = hexToRGB(task.color);
    mb.avatarHost.style.setProperty("--mini-avatar-color", task.color);
  }
}

export function tickMiniBots(dt: number) {
  const dpr = Math.min(2, window.devicePixelRatio || 1);
  for (const mb of live.values()) {
    const customAvatar = mb.avatarHost.style.display !== "none";
    mb.canvas.style.display = customAvatar ? "none" : "block";
    if (customAvatar) continue;
    const ctx = mb.canvas.getContext("2d");
    if (!ctx) continue;
    mb.engine.update(dt);
    ctx.setTransform(dpr, 0, 0, dpr, 0, 0);
    ctx.clearRect(0, 0, mb.cssSize, mb.cssSize);
    mb.engine.draw(ctx, mb.cssSize, mb.cssSize);
  }
}

export const miniBotCount = () => live.size;
