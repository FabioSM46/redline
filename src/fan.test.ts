import assert from "node:assert/strict";
import { describe, it } from "node:test";
import {
  addPoint,
  availabilitySentence,
  clampPmfw,
  currentSetting,
  curveProblem,
  curveSensor,
  defaultCurve,
  describeFan,
  draftChanged,
  fanAvailability,
  fanDetail,
  fanSettings,
  hasPmfw,
  initialCurve,
  initialDraft,
  movePoint,
  offeredSettings,
  pmfwApplies,
  pmfwRange,
  removePoint,
  snapSpeed,
  speedAt,
  ticks,
} from "./fan.ts";
import type { CurvePoint, CurveRules, FanStatus, PmfwStatus } from "./tuning.ts";

/** A firmware curve: exactly 5 points, 25–100 °C, 15–100 %, whole percents. */
const firmware: CurveRules = {
  points: 5,
  temperature: { min_c: 25, max_c: 100 },
  speed: { min_pct: 15, max_pct: 100 },
  whole_percent: true,
};

/** LACT's own curve: any number of points, 0–120 °C, 0–100 %. */
const free: CurveRules = {
  points: null,
  temperature: { min_c: 0, max_c: 120 },
  speed: { min_pct: 0, max_pct: 100 },
  whole_percent: false,
};

const noPmfw: PmfwStatus = {
  acoustic_limit: null,
  acoustic_target: null,
  target_temperature: null,
  minimum_pwm: null,
  zero_rpm: null,
  zero_rpm_threshold: null,
};

const pmfw: PmfwStatus = {
  acoustic_limit: { current: 2900, min: 500, max: 3300 },
  acoustic_target: { current: 1450, min: 500, max: 3300 },
  target_temperature: { current: 95, min: 25, max: 110 },
  minimum_pwm: { current: 15, min: 0, max: 100 },
  zero_rpm: true,
  zero_rpm_threshold: { current: 50, min: null, max: null },
};

const curve = (...points: [number, number][]): CurvePoint[] =>
  points.map(([temperature_c, speed_pct]) => ({ temperature_c, speed_pct }));

const fan: FanStatus = {
  configurable: true,
  control_enabled: false,
  mode: null,
  static_speed: null,
  curve: null,
  temperature_range: { min_c: 25, max_c: 100 },
  pmfw,
  curve_rules: firmware,
};

describe("availability", () => {
  const status = { overdrive_enabled: true, reboot_pending: false };

  it("is ready with rules", () => {
    assert.equal(fanAvailability(fan, status), "ready");
    assert.equal(availabilitySentence("ready"), null);
    assert.deepEqual(offeredSettings(firmware), ["auto", "static", "curve"]);
  });

  it("asks for overdrive, then a reboot, when LACT cannot drive the fan", () => {
    const locked = { ...fan, configurable: false, curve_rules: null };
    assert.equal(fanAvailability(locked, { overdrive_enabled: false, reboot_pending: false }), "overdrive");
    assert.equal(fanAvailability(locked, { overdrive_enabled: false, reboot_pending: true }), "reboot");
    assert.equal(fanAvailability(locked, status), "not-controllable");
    assert.equal(fanAvailability(locked, { overdrive_enabled: null, reboot_pending: false }), "not-controllable");
    assert.equal(/overdrive/.test(availabilitySentence("overdrive") ?? ""), true);
    assert.deepEqual(offeredSettings(null), ["auto"]);
  });

  it("says when a controllable fan has no readable rules", () => {
    assert.equal(fanAvailability({ ...fan, curve_rules: null }, status), "no-rules");
  });
});

describe("currentSetting", () => {
  it("reads automatic, LACT's mode, or unknown", () => {
    assert.equal(currentSetting(fan), "auto");
    assert.equal(currentSetting({ ...fan, mode: "static" }), "auto");
    assert.equal(currentSetting({ ...fan, control_enabled: true, mode: "curve" }), "curve");
    assert.equal(currentSetting({ ...fan, control_enabled: true }), null);
    assert.equal(describeFan({ ...fan, control_enabled: true, mode: "static", static_speed: 40 }), "Fixed 40 %");
  });
});

describe("curveProblem", () => {
  it("accepts a valid curve in any order", () => {
    assert.equal(curveProblem(curve([90, 100], [30, 20], [50, 30], [70, 60], [80, 80]), firmware), null);
  });

  it("needs the firmware's exact point count", () => {
    assert.equal(/exactly 5 points/.test(curveProblem(curve([30, 20]), firmware) ?? ""), true);
    assert.equal(/at least one point/.test(curveProblem([], free) ?? ""), true);
    assert.equal(curveProblem(curve([30, 20]), free), null);
  });

  it("refuses values out of range, fractions on a firmware curve, duplicates and a falling speed", () => {
    assert.equal(/between 25 °C/.test(curveProblem(curve([20, 20], [50, 30], [60, 40], [70, 60], [80, 80]), firmware) ?? ""), true);
    assert.equal(/between 15 %/.test(curveProblem(curve([30, 10], [50, 30], [60, 40], [70, 60], [80, 80]), firmware) ?? ""), true);
    assert.equal(/whole percent/.test(curveProblem(curve([30, 20.5], [50, 30], [60, 40], [70, 60], [80, 80]), firmware) ?? ""), true);
    assert.equal(curveProblem(curve([30, 20.5]), free), null);
    assert.equal(/two points at 30/.test(curveProblem(curve([30, 20], [30, 30]), free) ?? ""), true);
    assert.equal(/slow down/.test(curveProblem(curve([30, 50], [60, 40]), free) ?? ""), true);
    assert.equal(/whole degrees/.test(curveProblem(curve([30.5, 50]), free) ?? ""), true);
  });
});

describe("snapSpeed", () => {
  it("clamps and rounds to the GPU's precision", () => {
    assert.equal(snapSpeed(10, firmware), 15);
    assert.equal(snapSpeed(140, firmware), 100);
    assert.equal(snapSpeed(40.6, firmware), 41);
    assert.equal(snapSpeed(40.66, free), 40.7);
  });
});

describe("initial curve", () => {
  it("spans the range with the firmware's point count", () => {
    const points = defaultCurve(firmware);
    assert.equal(points.length, 5);
    assert.deepEqual(points[0], { temperature_c: 25, speed_pct: 15 });
    assert.deepEqual(points[4], { temperature_c: 100, speed_pct: 100 });
    assert.equal(curveProblem(points, firmware), null);
    assert.equal(defaultCurve(free).length, 5);
    assert.equal(curveProblem(defaultCurve({ ...firmware, temperature: { min_c: 40, max_c: 42 } }), {
      ...firmware,
      points: null,
      temperature: { min_c: 40, max_c: 42 },
    }), null);
  });

  it("starts from the stored curve, sorted and snapped, when it fits", () => {
    const stored = curve([90, 100], [30, 20.4], [50, 30], [70, 60], [80, 80]);
    assert.deepEqual(initialCurve(stored, firmware), curve([30, 20], [50, 30], [70, 60], [80, 80], [90, 100]));
  });

  it("falls back to the default curve when the stored one does not fit", () => {
    assert.deepEqual(initialCurve(curve([30, 20]), firmware), defaultCurve(firmware));
    assert.deepEqual(initialCurve(null, firmware), defaultCurve(firmware));
    assert.deepEqual(initialCurve(curve([30, 50], [60, 40]), free), defaultCurve(free));
  });
});

describe("movePoint", () => {
  const base = curve([30, 20], [50, 30], [70, 60], [80, 80], [90, 100]);

  it("clamps to the range and keeps whole values", () => {
    assert.deepEqual(movePoint(base, 0, { temperature_c: 10, speed_pct: 5 }, firmware)[0], { temperature_c: 25, speed_pct: 15 });
    assert.deepEqual(movePoint(base, 4, { temperature_c: 130, speed_pct: 120.4 }, firmware)[4], {
      temperature_c: 100,
      speed_pct: 100,
    });
    assert.deepEqual(movePoint(base, 1, { temperature_c: 55.4, speed_pct: 33.6 }, firmware)[1], {
      temperature_c: 55,
      speed_pct: 34,
    });
  });

  it("never crosses or lands on a neighbour's temperature", () => {
    assert.equal(movePoint(base, 1, { temperature_c: 85, speed_pct: 30 }, firmware)[1].temperature_c, 69);
    assert.equal(movePoint(base, 1, { temperature_c: 10, speed_pct: 30 }, firmware)[1].temperature_c, 31);
  });

  it("keeps the speed from falling by carrying the neighbours along", () => {
    const raised = movePoint(base, 1, { temperature_c: 50, speed_pct: 85 }, firmware);
    assert.deepEqual(raised.map((p) => p.speed_pct), [20, 85, 85, 85, 100]);
    const lowered = movePoint(base, 3, { temperature_c: 80, speed_pct: 25 }, firmware);
    assert.deepEqual(lowered.map((p) => p.speed_pct), [20, 25, 25, 25, 100]);
    assert.equal(curveProblem(raised, firmware), null);
    assert.equal(curveProblem(lowered, firmware), null);
  });

  it("ignores an index out of the curve", () => {
    assert.deepEqual(movePoint(base, 9, { temperature_c: 0, speed_pct: 0 }, firmware), base);
  });
});

describe("adding and removing points", () => {
  it("is refused when the firmware fixes the count", () => {
    const base = defaultCurve(firmware);
    assert.equal(addPoint(base, firmware), null);
    assert.equal(removePoint(base, 0, firmware), null);
  });

  it("adds in the widest gap, on the line, and keeps the curve valid", () => {
    const base = curve([20, 20], [40, 40], [100, 100]);
    const added = addPoint(base, free);
    assert.deepEqual(added, curve([20, 20], [40, 40], [70, 70], [100, 100]));
    assert.equal(curveProblem(added ?? [], free), null);
    assert.deepEqual(addPoint([], free), curve([60, 100]));
  });

  it("adds at a range end when that gap is the widest", () => {
    assert.deepEqual(addPoint(curve([100, 50], [120, 100]), free), curve([49, 50], [100, 50], [120, 100]));
  });

  it("returns null when every degree is taken", () => {
    const tight = { ...free, temperature: { min_c: 0, max_c: 1 } };
    assert.equal(addPoint(curve([0, 10], [1, 20]), tight), null);
  });

  it("removes a point but never the last one", () => {
    assert.deepEqual(removePoint(curve([30, 20], [60, 40]), 0, free), curve([60, 40]));
    assert.equal(removePoint(curve([60, 40]), 0, free), null);
    assert.equal(removePoint(curve([60, 40], [70, 50]), 5, free), null);
  });
});

describe("speedAt", () => {
  const base = curve([30, 20], [50, 40], [90, 100]);

  it("interpolates and stays flat past the ends", () => {
    assert.equal(speedAt(base, 10), 20);
    assert.equal(speedAt(base, 40), 30);
    assert.equal(speedAt(base, 70), 70);
    assert.equal(speedAt(base, 95), 100);
    assert.equal(speedAt([], 50), null);
    assert.equal(speedAt(base, Number.NaN), null);
  });
});

describe("firmware options", () => {
  it("apply in the modes set_fan accepts them in", () => {
    assert.equal(pmfwApplies("acoustic_limit", "auto"), true);
    assert.equal(pmfwApplies("acoustic_limit", "curve"), false);
    assert.equal(pmfwApplies("zero_rpm", "curve"), true);
    assert.equal(pmfwApplies("zero_rpm_threshold", "static"), false);
  });

  it("are editable only with a range", () => {
    assert.deepEqual(pmfwRange(pmfw.acoustic_limit), { min: 500, max: 3300 });
    assert.equal(pmfwRange(pmfw.zero_rpm_threshold), null);
    assert.equal(pmfwRange(null), null);
    assert.equal(hasPmfw(pmfw), true);
    assert.equal(hasPmfw(noPmfw), false);
    assert.equal(clampPmfw(3400.2, { min: 500, max: 3300 }), 3300);
    assert.equal(clampPmfw(1200.6, { min: 500, max: 3300 }), 1201);
    assert.equal(clampPmfw(Number.NaN, { min: 500, max: 3300 }), null);
  });
});

describe("fanSettings", () => {
  it("starts from what LACT has, with nothing to apply", () => {
    const draft = initialDraft(fan);
    assert.equal(draft.setting, "auto");
    assert.equal(draftChanged(draft, fan), false);
  });

  it("sends a fixed speed or a curve with only their own field", () => {
    const draft = initialDraft(fan);
    assert.deepEqual(fanSettings({ ...draft, setting: "static", staticSpeed: 40 }, fan), {
      settings: { mode: "static", static_speed: 40 },
    });
    const result = fanSettings({ ...draft, setting: "curve" }, fan);
    assert.deepEqual(result, { settings: { mode: "curve", curve: defaultCurve(firmware) } });
    assert.equal(draftChanged({ ...draft, setting: "curve" }, fan), true);
  });

  it("sends only the firmware options that changed and apply in the mode", () => {
    const draft = { ...initialDraft(fan), pmfw: { acoustic_limit: 2000, zero_rpm: false, minimum_pwm: 15 } };
    assert.deepEqual(fanSettings(draft, fan), { settings: { mode: "auto", pmfw: { acoustic_limit: 2000, zero_rpm: false } } });
    assert.deepEqual(fanSettings({ ...draft, setting: "curve" }, fan), {
      settings: { mode: "curve", curve: defaultCurve(firmware), pmfw: { zero_rpm: false } },
    });
    assert.deepEqual(fanSettings({ ...draft, setting: "static", staticSpeed: 50 }, fan), {
      settings: { mode: "static", static_speed: 50 },
    });
    assert.equal(draftChanged({ ...initialDraft(fan), pmfw: { minimum_pwm: 15 } }, fan), false);
  });

  it("refuses a curve or speed the backend would refuse", () => {
    const draft = initialDraft(fan);
    assert.equal("problem" in fanSettings({ ...draft, setting: "curve", curve: curve([30, 20]) }, fan), true);
    assert.equal("problem" in fanSettings({ ...draft, setting: "static", staticSpeed: 5 }, fan), true);
    const noRules = { ...fan, curve_rules: null };
    assert.equal("problem" in fanSettings({ ...initialDraft(noRules), setting: "curve" }, noRules), true);
  });

  it("is unchanged on the stored curve and changed once a point moves", () => {
    const stored = { ...fan, control_enabled: true, mode: "curve" as const, curve: defaultCurve(firmware) };
    const draft = initialDraft(stored);
    assert.equal(draftChanged(draft, stored), false);
    const moved = { ...draft, curve: movePoint(draft.curve ?? [], 2, { temperature_c: 60, speed_pct: 70 }, firmware) };
    assert.equal(draftChanged(moved, stored), true);
  });

  it("says what changes in the confirmation panel", () => {
    assert.equal(fanDetail(fan, { mode: "static", static_speed: 40 }), "Automatic → Fixed 40 %");
    assert.equal(
      fanDetail(fan, { mode: "auto", pmfw: { zero_rpm: false, acoustic_limit: 2000 } }),
      "Automatic → Automatic; zero RPM off; acoustic limit 2900 RPM → 2000 RPM",
    );
    assert.equal(fanDetail(fan, { mode: "curve", curve: defaultCurve(firmware) }), "Automatic → Curve (5 points)");
  });
});

describe("chart helpers", () => {
  it("follows the hotspot on a firmware curve and the edge sensor otherwise", () => {
    assert.equal(curveSensor(firmware), "temp_junction_c");
    assert.equal(curveSensor(free), "temp_edge_c");
  });

  it("makes round ticks", () => {
    assert.deepEqual(ticks(25, 100, 5), [40, 60, 80, 100]);
    assert.deepEqual(ticks(0, 100, 4), [0, 25, 50, 75, 100]);
    assert.deepEqual(ticks(0, 120, 6), [0, 20, 40, 60, 80, 100, 120]);
    assert.deepEqual(ticks(5, 5, 4), [5]);
  });
});
