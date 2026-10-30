import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { describe, it } from "node:test";
import {
  addSource,
  addTrack,
  folderProblem,
  formatElapsed,
  formatLength,
  formatSize,
  recordingSince,
  removeSource,
  removeTrack,
  REPLAY_PRESETS,
  REPLAY_SECONDS_MAX,
  REPLAY_SECONDS_MIN,
  replayLengthOptions,
  sourceLabel,
  sourcesToAdd,
} from "./record.ts";

const RECORDER_RS = new URL("../src-tauri/crates/redline-core/src/recorder.rs", import.meta.url);

describe("mirror of the backend", () => {
  it("has the replay length range of redline-core::recorder", () => {
    const source = readFileSync(RECORDER_RS, "utf8");
    const range = `pub const REPLAY_SECONDS: RangeInclusive<u32> = ${REPLAY_SECONDS_MIN}..=${REPLAY_SECONDS_MAX
      .toLocaleString("en-US")
      .replace(/,/g, "_")};`;
    assert.equal(source.includes(range), true, range);
  });
});

describe("replay length", () => {
  it("offers the presets, ascending and inside the backend's range", () => {
    assert.deepEqual(replayLengthOptions(60), [...REPLAY_PRESETS]);
    for (const preset of REPLAY_PRESETS) {
      assert.equal(preset >= REPLAY_SECONDS_MIN && preset <= REPLAY_SECONDS_MAX, true);
    }
  });

  it("adds a saved length that is not a preset, in order", () => {
    assert.deepEqual(replayLengthOptions(45), [15, 30, 45, 60, 120, 180, 300, 600, 900, 1800]);
  });

  it("does not add a saved length the backend would refuse", () => {
    assert.deepEqual(replayLengthOptions(1), [...REPLAY_PRESETS]);
    assert.deepEqual(replayLengthOptions(90_000), [...REPLAY_PRESETS]);
    assert.deepEqual(replayLengthOptions(12.5), [...REPLAY_PRESETS]);
  });

  it("is written in words", () => {
    assert.equal(formatLength(15), "15 s");
    assert.equal(formatLength(60), "1 min");
    assert.equal(formatLength(90), "1 min 30 s");
    assert.equal(formatLength(3900), "1 h 5 min");
    assert.equal(formatLength(0), "0 s");
  });
});

describe("audio tracks", () => {
  const listed = [
    { id: "default_output", label: "Default output" },
    { id: "default_input", label: "Default input" },
    { id: "device:alsa_input.usb", label: "USB Microphone" },
    { id: "app:Discord", label: "Discord" },
  ];

  it("names listed sources with their label and saved ones from their id", () => {
    assert.equal(sourceLabel("device:alsa_input.usb", listed), "USB Microphone");
    assert.equal(sourceLabel("app:Discord", listed), "Discord");
    assert.equal(sourceLabel("app:Steam", listed), "Steam (not running)");
    assert.equal(sourceLabel("device:hdmi", listed), "hdmi (not connected)");
    assert.equal(sourceLabel("app-inverse:Discord", listed), "Everything except Discord");
    assert.equal(sourceLabel("default_input", []), "Default input");
    assert.equal(sourceLabel("something", []), "something");
  });

  it("offers only the sources a track does not have", () => {
    assert.deepEqual(
      sourcesToAdd(listed, ["default_output", "app:Discord"]).map((s) => s.id),
      ["default_input", "device:alsa_input.usb"],
    );
  });

  it("mixes several sources in one track, never twice the same", () => {
    const tracks = [["default_output"], ["default_input"]];
    assert.deepEqual(addSource(tracks, 0, "app:Discord"), [["default_output", "app:Discord"], ["default_input"]]);
    assert.deepEqual(addSource(tracks, 0, "default_output"), tracks);
    assert.deepEqual(tracks, [["default_output"], ["default_input"]], "the input is not changed");
  });

  it("removes a track left without sources", () => {
    const tracks = [["default_output", "app:Discord"], ["default_input"]];
    assert.deepEqual(removeSource(tracks, 0, "app:Discord"), [["default_output"], ["default_input"]]);
    assert.deepEqual(removeSource(tracks, 1, "default_input"), [["default_output", "app:Discord"]]);
  });

  it("adds and removes whole tracks", () => {
    assert.deepEqual(addTrack([], "default_output"), [["default_output"]]);
    assert.deepEqual(addTrack([["default_output"]], "default_input"), [["default_output"], ["default_input"]]);
    assert.deepEqual(removeTrack([["a"], ["b"], ["c"]], 1), [["a"], ["c"]]);
    assert.deepEqual(removeTrack([["a"]], 0), []);
  });
});

describe("folders", () => {
  it("accepts only full paths", () => {
    assert.equal(folderProblem("/games/clips"), null);
    assert.equal(folderProblem("  /games/clips  "), null);
    assert.equal(folderProblem(""), "Enter a folder.");
    assert.equal(folderProblem("   "), "Enter a folder.");
    assert.equal(folderProblem("clips"), "Use a full path, starting with /.");
    assert.equal(folderProblem("~/clips"), "Use a full path, starting with /.");
  });
});

describe("recording timer", () => {
  it("starts when the recording turns on and keeps its start", () => {
    const first = recordingSince({ recording: false, since: null }, true, 1000);
    assert.equal(first, 1000);
    assert.equal(recordingSince({ recording: true, since: first }, true, 5000), 1000);
  });

  it("stops when the recording turns off", () => {
    assert.equal(recordingSince({ recording: true, since: 1000 }, false, 5000), null);
  });

  it("has no start for a recording already running at the first reading", () => {
    assert.equal(recordingSince(null, true, 1000), null);
    assert.equal(recordingSince({ recording: true, since: null }, true, 5000), null);
  });

  it("is written as m:ss, h:mm:ss past an hour", () => {
    assert.equal(formatElapsed(0), "0:00");
    assert.equal(formatElapsed(42_900), "0:42");
    assert.equal(formatElapsed(605_000), "10:05");
    assert.equal(formatElapsed(3_661_000), "1:01:01");
    assert.equal(formatElapsed(-5), "0:00");
  });
});

describe("file size", () => {
  it("uses decimal units", () => {
    assert.equal(formatSize(0), "0 B");
    assert.equal(formatSize(999), "999 B");
    assert.equal(formatSize(1000), "1.0 kB");
    assert.equal(formatSize(820_000), "820 kB");
    assert.equal(formatSize(1_400_000_000), "1.4 GB");
    assert.equal(formatSize(25_000_000), "25 MB");
  });

  it("never shows 1000 of a unit", () => {
    assert.equal(formatSize(999_600), "1.0 MB");
  });

  it("shows a dash for an impossible size", () => {
    assert.equal(formatSize(-1), "—");
    assert.equal(formatSize(Number.NaN), "—");
  });
});
