import assert from "node:assert/strict";
import { describe, it } from "node:test";
import { parseThemeSetting, resolveTheme, THEME_OPTIONS } from "./theme.ts";

describe("resolveTheme", () => {
  it("follows the desktop preference with system", () => {
    assert.equal(resolveTheme("system", true), "dark");
    assert.equal(resolveTheme("system", false), "light");
  });

  it("ignores the desktop preference with an explicit theme", () => {
    assert.equal(resolveTheme("light", true), "light");
    assert.equal(resolveTheme("light", false), "light");
    assert.equal(resolveTheme("dark", true), "dark");
    assert.equal(resolveTheme("dark", false), "dark");
  });
});

describe("parseThemeSetting", () => {
  it("keeps the three known values", () => {
    for (const { value } of THEME_OPTIONS) {
      assert.equal(parseThemeSetting(value), value);
    }
  });

  it("falls back to system for anything else", () => {
    assert.equal(parseThemeSetting("Dark"), "system");
    assert.equal(parseThemeSetting(""), "system");
    assert.equal(parseThemeSetting(undefined), "system");
    assert.equal(parseThemeSetting(null), "system");
  });
});

describe("THEME_OPTIONS", () => {
  it("lists System, Light and Dark in this order", () => {
    assert.deepEqual(
      THEME_OPTIONS.map((o) => [o.value, o.label]),
      [
        ["system", "System"],
        ["light", "Light"],
        ["dark", "Dark"],
      ],
    );
  });
});
