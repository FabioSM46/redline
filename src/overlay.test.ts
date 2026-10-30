import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { describe, it } from "node:test";
import {
  BACKUP_FILE,
  chosenMetrics,
  hasOwnFigures,
  isLastMetric,
  OVERLAY_METRICS,
  OVERLAY_POSITIONS,
  OVERLAY_PRESETS,
  type OverlayMetric,
  type OverlaySettings,
  type OverlayStatus,
  overlayDescription,
  PRESET_METRICS,
  savedNote,
  shownFigures,
  toggleMetric,
} from "./overlay.ts";

const settings = (overrides: Partial<OverlaySettings> = {}): OverlaySettings => ({
  preset: "standard",
  position: "top-left",
  metrics: [],
  toggle_key: "Shift_R+F12",
  ...overrides,
});

describe("overlayDescription", () => {
  it("says what each preset shows, with the loads MangoHud draws", () => {
    assert.equal(overlayDescription(settings({ preset: "minimal" })), "Shows FPS and frametime.");
    assert.equal(
      overlayDescription(settings({ preset: "standard" })),
      "Shows GPU load, GPU temperature, GPU power, GPU core clock, CPU load, CPU temperature, FPS and frametime.",
    );
    assert.equal(
      overlayDescription(settings({ preset: "full" })),
      "Shows GPU load, GPU temperature, GPU power, GPU core clock, CPU load, CPU temperature, VRAM, RAM, FPS and frametime.",
    );
  });

  it("says off starts hidden and what its key shows", () => {
    assert.equal(
      overlayDescription(settings({ preset: "off", toggle_key: "F10" })),
      "Hidden when a game starts. F10 shows FPS and frametime.",
    );
  });

  it("follows the Advanced list, also under off", () => {
    assert.equal(overlayDescription(settings({ metrics: ["ram"] })), "Shows RAM.");
    assert.equal(
      overlayDescription(settings({ preset: "off", metrics: ["vram"] })),
      "Hidden when a game starts. Shift_R+F12 shows VRAM.",
    );
  });
});

describe("shownFigures", () => {
  it("adds the load to a GPU figure and to the CPU temperature", () => {
    assert.deepEqual(shownFigures(["gpu_power"]), ["GPU load", "GPU power"]);
    assert.deepEqual(shownFigures(["cpu_temp"]), ["CPU load", "CPU temperature"]);
  });

  it("adds FPS to frametime, and not the reverse", () => {
    assert.deepEqual(shownFigures(["frametime"]), ["FPS", "frametime"]);
    assert.deepEqual(shownFigures(["fps"]), ["FPS"]);
  });

  it("keeps the order of the file, whatever the order of the list", () => {
    assert.deepEqual(shownFigures(["fps", "ram", "gpu_temp"]), ["GPU load", "GPU temperature", "RAM", "FPS"]);
  });
});

describe("chosenMetrics", () => {
  it("is the preset's when the list is empty, the list otherwise", () => {
    assert.equal(hasOwnFigures(settings()), false);
    assert.deepEqual(chosenMetrics(settings()), PRESET_METRICS.standard);
    assert.equal(hasOwnFigures(settings({ metrics: ["ram"] })), true);
    assert.deepEqual(chosenMetrics(settings({ metrics: ["ram"] })), ["ram"]);
  });
});

describe("toggleMetric", () => {
  it("starts from the preset's figures and keeps the fixed order", () => {
    assert.deepEqual(toggleMetric(settings({ preset: "minimal" }), "ram").metrics, ["fps", "frametime", "ram"]);
    assert.deepEqual(toggleMetric(settings({ metrics: ["ram", "fps"] }), "vram").metrics, ["fps", "vram", "ram"]);
  });

  it("removes a chosen figure", () => {
    assert.deepEqual(toggleMetric(settings({ preset: "minimal" }), "frametime").metrics, ["fps"]);
  });

  it("goes back to the preset's list when it matches it", () => {
    assert.deepEqual(toggleMetric(settings({ preset: "minimal", metrics: ["fps"] }), "frametime").metrics, []);
  });

  it("never empties the list: empty would mean the preset's figures", () => {
    const one = settings({ metrics: ["ram"] });
    assert.equal(isLastMetric(one, "ram"), true);
    assert.equal(isLastMetric(one, "vram"), false);
    assert.equal(isLastMetric(settings(), "fps"), false);
    assert.deepEqual(toggleMetric(one, "ram"), one);
  });

  it("changes nothing else", () => {
    const before = settings({ preset: "off", position: "bottom-right", toggle_key: "F10" });
    assert.deepEqual(toggleMetric(before, "ram"), { ...before, metrics: ["fps", "frametime", "ram"] });
  });
});

describe("savedNote", () => {
  it("does not imply that a file of the user's changed", () => {
    assert.equal(savedNote("user_config").includes("not changed"), true);
    assert.equal(savedNote("managed").includes("Saved to MangoHud.conf"), true);
    assert.equal(savedNote("absent").includes("MangoHud.conf"), false);
  });
});

describe("mirrors of the backend", () => {
  const overlayRs = readFileSync(new URL("../src-tauri/src/overlay.rs", import.meta.url), "utf8");
  const settingsRs = readFileSync(new URL("../src-tauri/src/settings.rs", import.meta.url), "utf8");
  const libRs = readFileSync(new URL("../src-tauri/src/lib.rs", import.meta.url), "utf8");

  /** The text between the braces of the `pub <kind> <name>` item. */
  function body(source: string, kind: "enum" | "struct", name: string): string {
    const start = source.indexOf(`pub ${kind} ${name} {`);
    assert.equal(start >= 0, true, `${name} not found`);
    return source.slice(start, source.indexOf("\n}", start));
  }

  /** The variant names of an enum, in order. */
  function variants(source: string, name: string): string[] {
    return [...body(source, "enum", name).matchAll(/^\s+([A-Z]\w*),/gm)].map((m) => m[1]);
  }

  /** The field names of a struct, in order. */
  function fields(source: string, name: string): string[] {
    return [...body(source, "struct", name).matchAll(/^\s+pub (\w+):/gm)].map((m) => m[1]);
  }

  const words = (variant: string) => variant.split(/(?=[A-Z])/).map((word) => word.toLowerCase());

  it("has the same presets, positions and metrics", () => {
    assert.deepEqual(
      variants(settingsRs, "OverlayPreset").map((v) => v.toLowerCase()),
      OVERLAY_PRESETS.map((p) => p.value),
    );
    assert.deepEqual(
      variants(settingsRs, "OverlayPosition").map((v) => words(v).join("-")),
      OVERLAY_POSITIONS.map((p) => p.value),
    );
    assert.deepEqual(
      variants(settingsRs, "OverlayMetric").map((v) => words(v).join("_")),
      OVERLAY_METRICS.map((m) => m.value),
    );
    assert.equal(/rename_all = "lowercase"\)\]\npub enum OverlayPreset/.test(settingsRs), true);
    assert.equal(/rename_all = "kebab-case"\)\]\npub enum OverlayPosition/.test(settingsRs), true);
    assert.equal(/rename_all = "snake_case"\)\]\npub enum OverlayMetric/.test(settingsRs), true);
  });

  it("has the same config states", () => {
    const config: OverlayStatus["config"][] = ["absent", "managed", "user_config"];
    assert.deepEqual(
      variants(overlayRs, "ConfigState").map((v) => words(v).join("_")),
      config,
    );
    assert.equal(/rename_all = "snake_case"\)\]\npub enum ConfigState/.test(overlayRs), true);
  });

  it("has the same fields", () => {
    const status: (keyof OverlayStatus)[] = ["available", "unavailable_reason", "config", "settings", "launch_hint"];
    const form: (keyof OverlaySettings)[] = ["preset", "position", "metrics", "toggle_key"];
    assert.deepEqual(fields(overlayRs, "OverlayStatus"), status);
    assert.deepEqual(fields(settingsRs, "OverlaySettings"), form);
  });

  it("has the same preset figures", () => {
    const list = (name: string): OverlayMetric[] => {
      const start = overlayRs.indexOf(`const ${name}: &[Metric] = &[`);
      assert.equal(start >= 0, true, `${name} not found`);
      const text = overlayRs.slice(start + `const ${name}: &[Metric] = `.length, overlayRs.indexOf(";", start));
      return [...text.matchAll(/Metric::(\w+)/g)].map((m) => words(m[1]).join("_") as OverlayMetric);
    };
    assert.deepEqual(PRESET_METRICS.minimal, list("MINIMAL"));
    assert.deepEqual(PRESET_METRICS.standard, list("STANDARD"));
    assert.deepEqual(PRESET_METRICS.full, list("FULL"));
    assert.equal(/OverlayPreset::Off \| OverlayPreset::Minimal => MINIMAL/.test(overlayRs), true);
  });

  it("names the same backup file", () => {
    assert.equal(overlayRs.includes(`pub const BACKUP_FILE: &str = "${BACKUP_FILE}";`), true);
  });

  it("calls registered commands, with the parameter name Tauri matches", () => {
    for (const command of ["overlay_status", "set_overlay", "take_over_overlay_config"]) {
      assert.equal(libRs.includes(`overlay::${command},`), true);
    }
    assert.equal(/pub fn set_overlay\([^)]*\bsettings: OverlaySettings,/.test(overlayRs), true);
  });
});
