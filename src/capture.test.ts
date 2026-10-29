import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { describe, it } from "node:test";
import {
  captureChangeNotice,
  captureExplanation,
  captureOptions,
  CODEC_GROUPS,
  CODECS,
  FPS_MAX,
  FPS_MIN,
  FPS_PRESETS,
  fpsOptions,
  PORTAL,
  QUALITIES,
  QUALITY_OPTIONS,
} from "./capture.ts";

const RECORDER_RS = new URL("../src-tauri/crates/redline-core/src/recorder.rs", import.meta.url);

/** The string items of `pub const <name>: &[&str] = &[...]` in recorder.rs. */
function rustStrings(source: string, name: string): string[] {
  const start = source.indexOf(`pub const ${name}: &[&str] = &[`);
  assert.equal(start >= 0, true, `${name} not found in recorder.rs`);
  const body = source.slice(start, source.indexOf("];", start));
  return [...body.matchAll(/"([^"]*)"/g)].map((m) => m[1]);
}

describe("mirror of redline-core::recorder", () => {
  const source = readFileSync(RECORDER_RS, "utf8");

  it("has the same codecs in the same order", () => {
    assert.deepEqual([...CODECS], rustStrings(source, "CODECS"));
  });

  it("has the same qualities in the same order", () => {
    assert.deepEqual([...QUALITIES], rustStrings(source, "QUALITIES"));
  });

  it("has the same frame rate range and portal value", () => {
    assert.equal(source.includes(`pub const FPS: RangeInclusive<u32> = ${FPS_MIN}..=${FPS_MAX};`), true);
    assert.equal(source.includes(`pub const PORTAL: &str = "${PORTAL}";`), true);
  });
});

describe("CODEC_GROUPS", () => {
  const options = CODEC_GROUPS.flatMap((g) => g.options);

  it("starts with Automatic", () => {
    assert.deepEqual(options[0], { value: null, label: "Automatic" });
  });

  it("offers every codec exactly once, in the backend's order", () => {
    assert.deepEqual(
      options.slice(1).map((o) => o.value),
      [...CODECS],
    );
  });
});

describe("QUALITY_OPTIONS", () => {
  it("offers every quality, lowest first", () => {
    assert.deepEqual(
      QUALITY_OPTIONS.map((o) => [o.value, o.label]),
      [
        ["medium", "Medium"],
        ["high", "High"],
        ["very_high", "Very high"],
        ["ultra", "Ultra"],
      ],
    );
  });
});

describe("fpsOptions", () => {
  it("is the presets when the saved value is one of them", () => {
    assert.deepEqual(fpsOptions(60), [...FPS_PRESETS]);
  });

  it("adds a saved value outside the presets, in order", () => {
    assert.deepEqual(fpsOptions(75), [30, 60, 75, 90, 120, 144, 165, 240]);
    assert.deepEqual(fpsOptions(500), [...FPS_PRESETS, 500]);
  });

  it("does not add a value the backend would refuse", () => {
    assert.deepEqual(fpsOptions(0), [...FPS_PRESETS]);
    assert.deepEqual(fpsOptions(501), [...FPS_PRESETS]);
    assert.deepEqual(fpsOptions(59.94), [...FPS_PRESETS]);
  });

  it("keeps every preset inside the backend's range", () => {
    for (const fps of FPS_PRESETS) assert.equal(fps >= FPS_MIN && fps <= FPS_MAX, true);
  });
});

describe("captureOptions", () => {
  const portal = { id: "portal", label: "Choose with the desktop (portal)" };
  const dp2 = { id: "DP-2", label: "DP-2 (2560x1440)" };

  it("keeps the portal first", () => {
    assert.deepEqual(captureOptions([dp2, portal], "portal"), [portal, dp2]);
  });

  it("adds a saved monitor that is not connected now", () => {
    assert.deepEqual(captureOptions([portal, dp2], "HDMI-A-1"), [
      portal,
      dp2,
      { id: "HDMI-A-1", label: "HDMI-A-1 (not connected)" },
    ]);
  });

  it("always offers the portal, even on an empty list", () => {
    assert.deepEqual(
      captureOptions([], "portal").map((t) => t.id),
      ["portal"],
    );
  });
});

describe("capture sentences", () => {
  it("explains the portal and a monitor differently", () => {
    assert.equal(captureExplanation("portal").includes("desktop asks"), true);
    assert.equal(captureExplanation("DP-2").includes("DP-2"), true);
    assert.equal(captureExplanation("DP-2").includes("password"), true);
  });

  it("says nothing when the screen did not change", () => {
    assert.equal(captureChangeNotice("portal", "portal"), null);
    assert.equal(captureChangeNotice("DP-2", "DP-2"), null);
  });

  it("says the desktop asks again when going back to the portal", () => {
    assert.equal(captureChangeNotice("DP-2", "portal")?.includes("ask again"), true);
  });

  it("says the portal choice was cleared when choosing a monitor", () => {
    assert.equal(captureChangeNotice("portal", "DP-2")?.includes("cleared"), true);
    assert.equal(captureChangeNotice("DP-2", "HDMI-A-1")?.includes("cleared"), true);
  });
});
