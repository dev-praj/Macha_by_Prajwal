import assert from "node:assert/strict";
import { describe, it } from "node:test";
import {
  resolveLanguage,
  setLanguage,
  t,
} from "../src/i18n/i18n.ts";

describe("interface languages", () => {
  it("resolves supported system locales and safely falls back to English", () => {
    assert.equal(resolveLanguage("", ["fr-CA"]), "fr");
    assert.equal(resolveLanguage("", ["pt-PT"]), "pt-BR");
    assert.equal(resolveLanguage("", ["zh-TW", "de"]), "de");
    assert.equal(resolveLanguage("", ["xx"]), "en");
    assert.equal(resolveLanguage("ru", []), "ru");
  });

  it("loads translations on demand and falls back to English keys", async () => {
    setLanguage("fr");
    await new Promise((resolve) => setTimeout(resolve, 0));

    assert.equal(t("Settings"), "Réglages");
    assert.equal(t("A Macha-only label"), "A Macha-only label");
    setLanguage("en");
    assert.equal(t("Settings"), "Settings");
  });
});
