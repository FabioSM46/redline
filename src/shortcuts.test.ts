import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { describe, it } from "node:test";
import { copyError, copyText, EXECUTABLE, SHORTCUT_ACTIONS, shortcutCommand } from "./shortcuts.ts";

const ACTIONS_RS = new URL("../src-tauri/src/actions.rs", import.meta.url);
const CARGO_TOML = new URL("../src-tauri/Cargo.toml", import.meta.url);
const TAURI_CONFS = ["tauri.conf.json", "tauri.linux.conf.json", "tauri.release.linux.conf.json"].map(
  (name) => new URL(`../src-tauri/${name}`, import.meta.url),
);

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

describe("EXECUTABLE", () => {
  it("is the Cargo package name, with no [[bin]] renaming it", () => {
    const cargo = readFileSync(CARGO_TOML, "utf8");
    assert.equal(cargo.startsWith(`[package]\nname = "${EXECUTABLE}"\n`), true, "package name in Cargo.toml");
    assert.equal(cargo.includes("[[bin]]"), false, "a [[bin]] section would rename the executable");
  });

  it("is the Linux productName, and no Tauri config sets mainBinaryName", () => {
    const [base, linux, release] = TAURI_CONFS.map((url) => JSON.parse(readFileSync(url, "utf8")));
    assert.equal(linux.productName, EXECUTABLE);
    assert.equal(release.productName ?? EXECUTABLE, EXECUTABLE);
    for (const conf of [base, linux, release]) assert.equal(conf.mainBinaryName, undefined);
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
  async function outcome(promise: Promise<void>): Promise<string> {
    return promise.then(
      () => "copied",
      (e: Error) => `rejected: ${e.message}`,
    );
  }

  it("writes the text to the clipboard without touching the fallback", async () => {
    let written = "";
    let fallbackUsed = false;
    const result = await outcome(
      copyText(
        { writeText: async (text) => void (written = text) },
        "redline --save-replay",
        () => (fallbackUsed = true),
      ),
    );
    assert.deepEqual([result, written, fallbackUsed], ["copied", "redline --save-replay", false]);
  });

  it("rejects with a readable reason when there is no clipboard and no fallback", async () => {
    assert.equal(await outcome(copyText(undefined, "x")), "rejected: the clipboard is not available");
  });

  it("uses the fallback when there is no clipboard", async () => {
    let copied = "";
    assert.equal(await outcome(copyText(undefined, "x", (text) => ((copied = text), true))), "copied");
    assert.equal(copied, "x");
  });

  it("uses the fallback when the clipboard refuses, and keeps its reason if the fallback fails", async () => {
    const refuses = { writeText: () => Promise.reject(new Error("denied")) };
    assert.equal(await outcome(copyText(refuses, "x", () => true)), "copied");
    assert.equal(await outcome(copyText(refuses, "x", () => false)), "rejected: denied");
    assert.equal(
      await outcome(
        copyText(refuses, "x", () => {
          throw new Error("no execCommand");
        }),
      ),
      "rejected: denied",
    );
  });

  it("turns a synchronous throw from writeText into a rejection", async () => {
    const throws = {
      writeText: (): Promise<void> => {
        throw new TypeError("Illegal invocation");
      },
    };
    assert.equal(await outcome(copyText(throws, "x")), "rejected: Illegal invocation");
    assert.equal(await outcome(copyText(throws, "x", () => true)), "copied");
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
