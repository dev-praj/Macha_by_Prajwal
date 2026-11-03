import assert from "node:assert/strict";
import { describe, it } from "node:test";
import { HUB_GROUPS } from "../src/settings/hub-pages.ts";

describe("settings hub navigation", () => {
  it("keeps the Flow, Notch, and Macha feature groups available", () => {
    assert.deepEqual(HUB_GROUPS.map((group) => group.title), ["Flow", "Notch", "Macha"]);
    const ids = HUB_GROUPS.flatMap((group) => group.items.map((item) => item.id));
    assert.equal(new Set(ids).size, ids.length);
    for (const id of [
      "dictation", "notetaker", "statistics", "dictionary", "snippets", "styles", "flow-settings",
      "general", "agents", "chat", "voice", "integrations", "phone", "connection", "shortcuts",
      "memory", "workflows", "automation", "vision",
    ]) {
      assert.ok(ids.includes(id), `missing hub page: ${id}`);
    }
    assert.ok(!ids.includes("supabase"), "the legacy bring-your-own-Supabase wizard must stay out of normal navigation");
  });
});
