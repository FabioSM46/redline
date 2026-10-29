import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { describe, it } from "node:test";
import { copyError, copyText, EXECUTABLE, SHORTCUT_ACTIONS, shortcutCommand } from "./shortcuts.ts";

const ACTIONS_RS = new URL("../src-tauri/src/actions.rs", import.meta.url);

/** The `(arg, action)` pairs of `pub const ACTION_ARGS` in actions.rs, in order. */
function rustActionArgs(source: string): [string, string][] {
  const start = source.indexOf("pub const ACTION_ARGS: [ActionArg; ");
  assert.equal(start >= 0, true, "ACTION_ARGS not found in actions.rs");
  const body = source.slice(start, source.indexOf("];", start));
  return [...body.matchAll(/arg:\s*"([^"]*)",\s*action:\s*Action::(\w+)/g)].map((m) => [m[1], m[2]]);
}

describe("mirror of ACTION_ARGS in actions.rs", () => {
  const pairs = rustActionArgs(readFileSync(ACTIONS_RS, "utf8"));

  it("finds a non-empty list in actions.rs", () => {
    assert.equal(pairs.length > 0, true);
  });

  it("has the same arguments, mapped to the same actions, in the same order", () => {
    assert.deepEqual(
      SHORTCUT_ACTIONS.map((a) => [a.arg, a.action]),
      pairs,
    );
  });

  it("matches the declared array length", () => {
    const source = readFileSync(ACTIONS_RS, "utf8");
    assert.equal(source.includes(`pub const ACTION_ARGS: [ActionArg; ${SHORTCUT_ACTIONS.length}] = [`), true);
  });
});

describe("shortcutCommand", () => {
  it("is the executable followed by the argument", () => {
    assert.equal(EXECUTABLE, "redline");
    assert.deepEqual(
      SHORTCUT_ACTIONS.map((a) => shortcutCommand(a.arg)),
      ["redline --save-replay", "redline --toggle-replay", "redline --toggle-recording"],
    );
  });
});

describe("copyText", () => {
  it("writes the text to the clipboard", async () => {
    let written = "";
    await copyText({ writeText: async (text) => void (written = text) }, "redline --save-replay");
    assert.equal(written, "redline --save-replay");
  });

  it("rejects with a readable reason when there is no clipboard", async () => {
    let message = "";
    await copyText(undefined, "x").catch((e: Error) => (message = e.message));
    assert.equal(message, "the clipboard is not available");
  });

  it("passes on the clipboard's own rejection", async () => {
    let message = "";
    await copyText({ writeText: () => Promise.reject(new Error("denied")) }, "x").catch((e: Error) => (message = e.message));
    assert.equal(message, "denied");
  });
});

describe("copyError", () => {
  it("names the reason and the way out", () => {
    assert.equal(copyError(new Error("denied")), "Not copied: denied. Select the command and copy it by hand.");
    assert.equal(copyError("blocked"), "Not copied: blocked. Select the command and copy it by hand.");
    assert.equal(copyError(new Error("Write permission denied.")), "Not copied: Write permission denied. Select the command and copy it by hand.");
    assert.equal(copyError(new Error("")), "Not copied: unknown error. Select the command and copy it by hand.");
  });
});
