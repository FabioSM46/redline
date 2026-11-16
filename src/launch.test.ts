import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { describe, it } from "node:test";
import {
  type Availability,
  LAUNCH_SWITCHES,
  type LaunchChoices,
  type LaunchOptionsStatus,
  type LaunchSwitch,
  launchSwitches,
  withChoice,
} from "./launch.ts";

const HERE: Availability = { available: true, unavailable_reason: null };
const NOTE = "The watermark appears only with some builds.";
const UPGRADE_NOTE = "The upgrade happens only with some builds.";

const status = (overrides: Partial<LaunchOptionsStatus> = {}): LaunchOptionsStatus => ({
  choices: { overlay: true, frame_generation: false, upscaler_upgrade: false, upscaler_indicator: false },
  overlay: HERE,
  frame_generation: HERE,
  upscaler_upgrade_note: UPGRADE_NOTE,
  upscaler_indicator_note: NOTE,
  command: "mangohud %command%",
  ...overrides,
});

const row = (value: LaunchOptionsStatus, choice: keyof LaunchChoices): LaunchSwitch => {
  const found = launchSwitches(value).find((s) => s.choice === choice);
  if (found === undefined) throw new Error(`${choice} not found`);
  return found;
};

describe("launchSwitches", () => {
  it("shows Overlay, Frame generation, Upscaler upgrade and Upscaler watermark, in that order", () => {
    assert.deepEqual(
      launchSwitches(status()).map((s) => s.label),
      ["Overlay", "Frame generation", "Upscaler upgrade", "Upscaler watermark"],
    );
  });

  it("shows the saved choices when every tool is there", () => {
    const all = status({
      choices: { overlay: false, frame_generation: true, upscaler_upgrade: true, upscaler_indicator: false },
    });
    assert.deepEqual(
      launchSwitches(all).map((s) => [s.checked, s.disabled]),
      [
        [false, false],
        [true, false],
        [true, false],
        [false, false],
      ],
    );
  });

  it("disables a switch whose tool is missing, off, with the backend's sentence", () => {
    const reason = "The frame generation layer is not installed. Install it.";
    const missing = status({
      choices: { overlay: true, frame_generation: true, upscaler_upgrade: false, upscaler_indicator: false },
      frame_generation: { available: false, unavailable_reason: reason },
    });
    const frames = row(missing, "frame_generation");
    assert.equal(frames.disabled, true);
    assert.equal(frames.checked, false);
    assert.equal(frames.note, reason);
    assert.equal(row(missing, "overlay").disabled, false);
  });

  it("treats a missing overlay the same way", () => {
    const reason = "The overlay tool is not installed.";
    const missing = status({ overlay: { available: false, unavailable_reason: reason } });
    const overlay = row(missing, "overlay");
    assert.deepEqual([overlay.disabled, overlay.checked, overlay.note], [true, false, reason]);
  });

  it("has no sentence for a tool that is there", () => {
    assert.equal(row(status(), "overlay").note, null);
    assert.equal(row(status(), "frame_generation").note, null);
  });

  it("never disables the watermark, and shows the backend's note about the builds", () => {
    const nothing: Availability = { available: false, unavailable_reason: "Missing." };
    const watermark = row(status({ overlay: nothing, frame_generation: nothing }), "upscaler_indicator");
    assert.equal(watermark.disabled, false);
    assert.equal(watermark.note, NOTE);
  });

  it("never disables the upgrade, and shows the backend's note about the builds", () => {
    const nothing: Availability = { available: false, unavailable_reason: "Missing." };
    const upgrade = row(status({ overlay: nothing, frame_generation: nothing }), "upscaler_upgrade");
    assert.equal(upgrade.disabled, false);
    assert.equal(upgrade.note, UPGRADE_NOTE);
  });

  it("says what the upgrade needs from the game and that it changes how the game renders", () => {
    const { description } = row(status(), "upscaler_upgrade");
    assert.equal(description.includes("version 3.1 or later"), true);
    assert.equal(description.includes("changes how the game renders"), true);
  });

  it("says the watermark shows something only while the upgrade is active", () => {
    assert.equal(row(status(), "upscaler_indicator").description.includes("only while the upscaler upgrade is active"), true);
  });
});

describe("withChoice", () => {
  const saved: LaunchChoices = {
    overlay: true,
    frame_generation: true,
    upscaler_upgrade: false,
    upscaler_indicator: false,
  };

  it("replaces one choice and sends the other three as saved", () => {
    assert.deepEqual(withChoice(saved, "upscaler_upgrade", true), {
      overlay: true,
      frame_generation: true,
      upscaler_upgrade: true,
      upscaler_indicator: false,
    });
    assert.deepEqual(withChoice(saved, "upscaler_indicator", true), {
      overlay: true,
      frame_generation: true,
      upscaler_upgrade: false,
      upscaler_indicator: true,
    });
    assert.deepEqual(withChoice(saved, "overlay", false), {
      overlay: false,
      frame_generation: true,
      upscaler_upgrade: false,
      upscaler_indicator: false,
    });
  });

  it("does not change the saved choices", () => {
    withChoice(saved, "frame_generation", false);
    assert.equal(saved.frame_generation, true);
  });
});

describe("wording", () => {
  it("names no third-party product for frame generation or the upscaler", () => {
    for (const { label, description } of LAUNCH_SWITCHES) {
      assert.equal(/afmf|fsr|amd|radeon|adrenalin|fluid motion/i.test(`${label} ${description}`), false, label);
    }
  });
});

describe("mirrors of the backend", () => {
  const launchRs = readFileSync(new URL("../src-tauri/src/launch_options.rs", import.meta.url), "utf8");
  const settingsRs = readFileSync(new URL("../src-tauri/src/settings.rs", import.meta.url), "utf8");
  const libRs = readFileSync(new URL("../src-tauri/src/lib.rs", import.meta.url), "utf8");

  /** The field names of the `pub struct <name>`, in order. */
  function fields(source: string, name: string): string[] {
    const start = source.indexOf(`pub struct ${name} {`);
    assert.equal(start >= 0, true, `${name} not found`);
    const body = source.slice(start, source.indexOf("\n}", start));
    return [...body.matchAll(/^\s+pub (\w+):/gm)].map((m) => m[1]);
  }

  it("has the same fields", () => {
    const value = status();
    assert.deepEqual(fields(launchRs, "LaunchOptionsStatus"), Object.keys(value));
    assert.deepEqual(fields(launchRs, "Availability"), Object.keys(value.overlay));
    assert.deepEqual(fields(settingsRs, "LaunchChoices"), Object.keys(value.choices));
  });

  it("has a switch for every choice, in the backend's order", () => {
    assert.deepEqual(
      LAUNCH_SWITCHES.map((s) => s.choice),
      fields(settingsRs, "LaunchChoices"),
    );
  });

  it("calls registered commands, with the parameter name Tauri matches", () => {
    for (const command of ["launch_options_status", "set_launch_options"]) {
      assert.equal(libRs.includes(`launch_options::${command},`), true);
    }
    assert.equal(/pub fn set_launch_options<[^)]*\bchoices: LaunchChoices,/.test(launchRs), true);
  });
});

describe("the Overlay page", () => {
  const page = readFileSync(new URL("./sections/performance/Overlay.tsx", import.meta.url), "utf8");

  it("shows the backend's command and no longer reads the overlay's launch hint", () => {
    assert.equal(page.includes("launch_hint"), false);
    assert.equal(page.includes('"launch_options_status"'), true);
    assert.equal(page.includes('"set_launch_options", { choices:'), true);
  });
});
