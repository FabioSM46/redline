import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { describe, it } from "node:test";
import {
  type ClockControl,
  type ClocksStatus,
  type CurveRules,
  type FanSettings,
  type FanStatus,
  type PmfwSettings,
  type PmfwStatus,
  canApply,
  canReset,
  changeLabel,
  clampWatts,
  formatCountdown,
  lactUnreachable,
  NO_PENDING,
  outcomeSentence,
  dependencyAction,
  dependencyDetail,
  NOT_ENABLED_BY_LACT,
  notEnabledByLact,
  type OverdriveDependency,
  overdriveAction,
  overdriveBlocked,
  overdriveView,
  planConfirmable,
  PENDING_EVENT,
  type PendingChange,
  pendingReducer,
  planExpired,
  type PowerLimit,
  powerRange,
  secondsLeft,
  SETTLED_EVENT,
  stillPending,
  TUNING_CHANGES,
  type TuningStatus,
} from "./tuning.ts";

const plan = {
  action: "disable" as const,
  steps: [],
  reboot_required: true,
  dependencies: [],
  saved_profiles: null,
  token: "t",
  expires_ms: 1_000,
};
const fanItem: OverdriveDependency = {
  gpu_id: "0000:0d:00.0",
  kind: "fan",
  profile: null,
  sentence: "The GPU has a fan curve stored in LACT.",
};

const power: PowerLimit = { current_w: 250, min_w: 150, max_w: 300, default_w: 250 };

const clockControl: ClockControl = { current: 0, min: -200, max: 0 };

const clocks: ClocksStatus = {
  unavailable_reason: null,
  max_core_clock: null,
  core_clock_offset: clockControl,
  max_vram_clock: null,
  voltage_offset: clockControl,
};

const status: TuningStatus = {
  available: true,
  unavailable_reason: null,
  warning: null,
  lact_version: "0.10.1",
  overdrive_enabled: false,
  reboot_pending: false,
  power,
  fan: null,
  clocks: null,
  profile: null,
  pending: null,
};

const pmfw: PmfwStatus = {
  acoustic_limit: { current: 2900, min: 500, max: 3300 },
  acoustic_target: null,
  target_temperature: null,
  minimum_pwm: null,
  zero_rpm: true,
  zero_rpm_threshold: null,
};

const rules: CurveRules = {
  points: 5,
  temperature: { min_c: 25, max_c: 100 },
  speed: { min_pct: 15, max_pct: 100 },
  whole_percent: true,
};

const fan: FanStatus = {
  configurable: true,
  control_enabled: true,
  mode: "curve",
  static_speed: 35,
  curve: [{ temperature_c: 40, speed_pct: 30 }],
  temperature_range: { min_c: 25, max_c: 100 },
  pmfw,
  curve_rules: rules,
};

const pmfwSettings: Required<PmfwSettings> = {
  zero_rpm: true,
  zero_rpm_threshold: 50,
  target_temperature: 90,
  acoustic_limit: 2900,
  acoustic_target: 1450,
  minimum_pwm: 15,
};

const fanSettings: Required<FanSettings> = { mode: "curve", static_speed: 35, curve: [], pmfw: pmfwSettings };

const change: PendingChange = { gpu_id: "0000:03:00.0", what: "power_limit", deadline_ms: 10_000 };
const other: PendingChange = { gpu_id: "0000:0c:00.0", what: "power_limit", deadline_ms: 20_000 };

describe("powerRange", () => {
  it("needs both ends", () => {
    assert.deepEqual(powerRange(power), { min: 150, max: 300 });
    assert.equal(powerRange({ ...power, min_w: null }), null);
    assert.equal(powerRange({ ...power, max_w: null }), null);
    assert.equal(powerRange(null), null);
  });

  it("refuses an inverted range", () => {
    assert.equal(powerRange({ ...power, min_w: 310 }), null);
  });
});

describe("clampWatts", () => {
  const range = { min: 150, max: 300 };

  it("rounds to whole watts and keeps the value in range", () => {
    assert.equal(clampWatts(212.4, range), 212);
    assert.equal(clampWatts(212.6, range), 213);
    assert.equal(clampWatts(90, range), 150);
    assert.equal(clampWatts(400, range), 300);
  });

  it("is null for something that is not a number", () => {
    assert.equal(clampWatts(Number.NaN, range), null);
    assert.equal(clampWatts(Number.POSITIVE_INFINITY, range), null);
  });
});

describe("canApply", () => {
  it("is true for a different value in range", () => {
    assert.equal(canApply(200, power), true);
  });

  it("is false for the limit already in force, out of range or without a range", () => {
    assert.equal(canApply(250, power), false);
    assert.equal(canApply(100, power), false);
    assert.equal(canApply(null, power), false);
    assert.equal(canApply(200, { ...power, max_w: null }), false);
  });

  it("allows a value when the current limit is unknown", () => {
    assert.equal(canApply(250, { ...power, current_w: null }), true);
  });
});

describe("canReset", () => {
  it("is false at the default and true away from it or when either is unknown", () => {
    assert.equal(canReset(power), false);
    assert.equal(canReset({ ...power, current_w: 200 }), true);
    assert.equal(canReset({ ...power, default_w: null }), true);
    assert.equal(canReset(null), false);
  });
});

describe("countdown", () => {
  it("rounds up and stops at zero", () => {
    assert.equal(secondsLeft(10_000, 5_000), 5);
    assert.equal(secondsLeft(10_000, 5_001), 5);
    assert.equal(secondsLeft(10_000, 9_999), 1);
    assert.equal(secondsLeft(10_000, 10_000), 0);
    assert.equal(secondsLeft(10_000, 12_000), 0);
    assert.equal(formatCountdown(4), "4 s");
  });
});

describe("pendingReducer", () => {
  it("records a pending change once, from the event or the reply", () => {
    const state = pendingReducer(NO_PENDING, { type: "pending", change });
    assert.deepEqual(state.pending, change);
    assert.equal(pendingReducer(state, { type: "pending", change: { ...change } }), state);
  });

  it("settles the pending change once and says how", () => {
    const pending = pendingReducer(NO_PENDING, { type: "pending", change });
    const settled = pendingReducer(pending, { type: "settled", result: { gpu_id: change.gpu_id, outcome: "expired" } });
    assert.equal(settled.pending, null);
    assert.deepEqual(settled.settled, { change, result: { gpu_id: change.gpu_id, outcome: "expired" } });
    assert.equal(pendingReducer(settled, { type: "settled", result: { gpu_id: change.gpu_id, outcome: "reverted" } }), settled);
  });

  it("ignores a settle for another GPU", () => {
    const pending = pendingReducer(NO_PENDING, { type: "pending", change });
    assert.equal(pendingReducer(pending, { type: "settled", result: { gpu_id: other.gpu_id, outcome: "confirmed" } }), pending);
  });

  it("never brings back a settled change from a late event or a stale status", () => {
    const settled = pendingReducer(pendingReducer(NO_PENDING, { type: "pending", change }), {
      type: "settled",
      result: { gpu_id: change.gpu_id, outcome: "confirmed" },
    });
    assert.equal(pendingReducer(settled, { type: "pending", change }), settled);
    assert.equal(pendingReducer(settled, { type: "status", change, nowMs: 0 }), settled);
  });

  it("takes a change from the status only while its deadline is ahead", () => {
    assert.deepEqual(pendingReducer(NO_PENDING, { type: "status", change, nowMs: 9_000 }).pending, change);
    assert.equal(pendingReducer(NO_PENDING, { type: "status", change, nowMs: 10_000 }), NO_PENDING);
  });

  it("keeps the change it already holds over a status reply", () => {
    const pending = pendingReducer(NO_PENDING, { type: "pending", change: other });
    assert.equal(pendingReducer(pending, { type: "status", change, nowMs: 0 }), pending);
  });

  it("settles from the answer of confirm or revert when the event is missed", () => {
    const pending = pendingReducer(NO_PENDING, { type: "pending", change });
    const result = { gpu_id: change.gpu_id, outcome: "confirmed" as const };
    assert.deepEqual(pendingReducer(pending, { type: "answered", change, result }), { pending: null, settled: { change, result } });
    assert.equal(pendingReducer(pending, { type: "answered", change: other, result }), pending);
  });

  it("a new change replaces the outcome of the previous one", () => {
    const settled = pendingReducer(pendingReducer(NO_PENDING, { type: "pending", change }), {
      type: "settled",
      result: { gpu_id: change.gpu_id, outcome: "reverted" },
    });
    assert.deepEqual(pendingReducer(settled, { type: "pending", change: other }), { pending: other, settled: null });
  });
});

describe("sentences", () => {
  it("names the change and every outcome", () => {
    assert.equal(changeLabel("power_limit"), "Power limit");
    assert.equal(changeLabel("fan"), "Fan");
    assert.equal(changeLabel("clocks"), "Clocks and voltage");
    assert.equal(changeLabel("voltage_curve" as never), "Tuning change");
    for (const outcome of ["confirmed", "reverted", "expired"] as const) {
      assert.equal(outcomeSentence({ gpu_id: "x", outcome }).length > 0, true);
    }
  });
});

describe("status views", () => {
  it("sends to Dependencies only when LACT cannot be reached", () => {
    assert.equal(lactUnreachable({ ...status, available: false, lact_version: null }), true);
    assert.equal(lactUnreachable({ ...status, available: false }), false);
    assert.equal(lactUnreachable(status), false);
  });

  it("puts a pending reboot before the running kernel's bit", () => {
    assert.equal(overdriveView(status), "disabled");
    assert.equal(overdriveView({ ...status, reboot_pending: true }), "reboot");
    assert.equal(overdriveView({ ...status, overdrive_enabled: true }), "enabled");
    assert.equal(overdriveView({ ...status, overdrive_enabled: null }), "unknown");
  });

  it("an overdrive plan expires at its deadline", () => {
    assert.equal(planExpired(plan, 999), false);
    assert.equal(planExpired(plan, 1_000), true);
  });

  it("the switch goes the way overdrive is not, and nowhere while a reboot is due", () => {
    assert.equal(overdriveAction("enabled"), "disable");
    assert.equal(overdriveAction("disabled"), "enable");
    assert.equal(overdriveAction("reboot"), null);
    assert.equal(overdriveAction("unknown"), null);
  });

  it("a switch that cannot act gives one reason, the most immediate first", () => {
    const free = { running: false, locked: false, refusal: null };
    assert.equal(overdriveBlocked("enabled", free), null);
    assert.equal(overdriveBlocked("disabled", free), null);
    assert.equal(/request/.test(overdriveBlocked("enabled", { ...free, running: true }) ?? ""), true);
    assert.equal(/waiting for an answer/.test(overdriveBlocked("enabled", { ...free, locked: true }) ?? ""), true);
    assert.equal(/Reboot/.test(overdriveBlocked("reboot", free) ?? ""), true);
    assert.equal(/does not report/.test(overdriveBlocked("unknown", free) ?? ""), true);
    assert.equal(overdriveBlocked("enabled", { ...free, refusal: "Not through LACT." }), "Not through LACT.");
    assert.equal(/request/.test(overdriveBlocked("reboot", { ...free, running: true, locked: true }) ?? ""), true);
  });

  it("a plan with something depending on overdrive cannot be confirmed", () => {
    assert.equal(planConfirmable(plan), true);
    assert.equal(planConfirmable({ ...plan, token: null }), false);
    assert.equal(planConfirmable({ ...plan, dependencies: [fanItem] }), false);
  });

  it("an item's action says which profile it changes", () => {
    assert.equal(dependencyAction(fanItem), "Back to default");
    assert.equal(dependencyDetail(fanItem), "fan back to automatic");
    const clocksInProfile = { ...fanItem, kind: "clocks" as const, profile: "Gaming" };
    assert.equal(dependencyAction(clocksInProfile), 'Back to default in "Gaming"');
    assert.equal(dependencyDetail(clocksInProfile), 'back to factory values, saved in the profile "Gaming"');
  });

  it("tells a refusal that holds until LACT changes from any other", () => {
    assert.equal(notEnabledByLact(`${NOT_ENABLED_BY_LACT}, so LACT cannot disable it.`), true);
    assert.equal(notEnabledByLact("LACT could not change overdrive: failed"), false);
  });
});

describe("mirrors of src-tauri/src/tuning.rs", () => {
  const rust = readFileSync(new URL("../src-tauri/src/tuning.rs", import.meta.url), "utf8");
  const overdrive = readFileSync(new URL("../src-tauri/src/tuning/overdrive.rs", import.meta.url), "utf8");
  const fanSource = readFileSync(new URL("../src-tauri/src/tuning/fan.rs", import.meta.url), "utf8");
  const clocksSource = readFileSync(new URL("../src-tauri/src/tuning/clocks.rs", import.meta.url), "utf8");

  it("uses the same event names", () => {
    assert.equal(rust.includes(`PENDING_EVENT: &str = "${PENDING_EVENT}"`), true);
    assert.equal(rust.includes(`SETTLED_EVENT: &str = "${SETTLED_EVENT}"`), true);
  });

  it("has every field of the serialized structs", () => {
    const fields = (source: string, name: string) => {
      const body = source.slice(source.indexOf(`pub struct ${name} {`));
      return [...body.slice(0, body.indexOf("\n}")).matchAll(/^\s+pub (\w+):/gm)].map((m) => m[1]).sort();
    };
    assert.deepEqual(fields(rust, "TuningStatus"), Object.keys(status).sort());
    assert.deepEqual(fields(rust, "PowerLimit"), Object.keys(power).sort());
    assert.deepEqual(fields(rust, "PendingChange"), Object.keys(change).sort());
    assert.deepEqual(fields(rust, "Settled"), ["gpu_id", "outcome"]);
    assert.deepEqual(fields(clocksSource, "ClocksStatus"), Object.keys(clocks).sort());
    assert.deepEqual(fields(clocksSource, "ClockControl"), Object.keys(clockControl).sort());
    assert.deepEqual(fields(overdrive, "OverdrivePlan"), Object.keys(plan).sort());
    assert.deepEqual(fields(overdrive, "OverdriveDependency"), Object.keys(fanItem).sort());
  });

  it("has every field of the fan structs", () => {
    const fields = (source: string, name: string) => {
      const body = source.slice(source.indexOf(`pub struct ${name} {`));
      return [...body.slice(0, body.indexOf("\n}")).matchAll(/^\s+pub (\w+):/gm)].map((m) => m[1]).sort();
    };
    const keys = (value: object) => Object.keys(value).sort();
    assert.deepEqual(fields(rust, "FanStatus"), keys(fan));
    assert.deepEqual(fields(rust, "PmfwStatus"), keys(pmfw));
    assert.deepEqual(fields(rust, "PmfwValue"), ["current", "max", "min"]);
    assert.deepEqual(fields(rust, "CurvePoint"), keys(fan.curve?.[0] ?? {}));
    assert.deepEqual(fields(rust, "TemperatureRange"), keys(rules.temperature));
    assert.deepEqual(fields(fanSource, "CurveRules"), keys(rules));
    assert.deepEqual(fields(fanSource, "SpeedRange"), keys(rules.speed));
    assert.deepEqual(fields(fanSource, "FanSettings"), keys(fanSettings));
    assert.deepEqual(fields(fanSource, "PmfwSettings"), keys(pmfwSettings));
  });

  it("has the same overdrive kinds and refusal", () => {
    const body = overdrive.slice(overdrive.indexOf("pub enum OverdriveDependencyKind {"));
    const kinds = [...body.slice(0, body.indexOf("\n}")).matchAll(/^\s+(\w+),/gm)].map((m) => m[1].toLowerCase());
    assert.deepEqual(kinds, ["clocks", "fan"]);
    assert.equal(overdrive.includes(`NOT_ENABLED_BY_LACT: &str = "${NOT_ENABLED_BY_LACT}`), true);
  });

  it("has the same fan modes", () => {
    const variants = (source: string, name: string) => {
      const body = source.slice(source.indexOf(`pub enum ${name} {`));
      return [...body.slice(0, body.indexOf("}")).matchAll(/^\s+(\w+),/gm)].map((m) => m[1].toLowerCase()).sort();
    };
    const modes: FanStatus["mode"][] = ["static", "curve"];
    assert.deepEqual(variants(rust, "FanMode"), [...modes].sort());
    const settings: FanSettings["mode"][] = ["auto", "static", "curve"];
    assert.deepEqual(variants(fanSource, "FanSetting"), [...settings].sort());
  });

  it("has a label for every kind of change", () => {
    const body = rust.slice(rust.indexOf("pub enum TuningChange {"));
    const variants = [...body.slice(0, body.indexOf("}")).matchAll(/^\s+(\w+),/gm)].map((m) =>
      m[1].replace(/(?<!^)([A-Z])/g, "_$1").toLowerCase(),
    );
    assert.equal(variants.length > 0, true);
    for (const variant of variants) {
      assert.equal((TUNING_CHANGES as readonly string[]).includes(variant), true, variant);
    }
  });
});

describe("a missed tuning-settled", () => {
  it("ends the change as expired once the backend no longer holds it after the deadline", () => {
    const pending = pendingReducer(NO_PENDING, { type: "pending", change });
    assert.equal(pendingReducer(pending, { type: "gone", change, nowMs: 9_999 }), pending);
    assert.deepEqual(pendingReducer(pending, { type: "gone", change, nowMs: 10_000 }), {
      pending: null,
      settled: { change, result: { gpu_id: change.gpu_id, outcome: "expired" } },
    });
    assert.equal(pendingReducer(pending, { type: "gone", change: other, nowMs: 30_000 }), pending);
  });

  it("reads whether the status still holds the change", () => {
    assert.equal(stillPending({ pending: change }, change), true);
    assert.equal(stillPending({ pending: null }, change), false);
    assert.equal(stillPending({ pending: { ...change, deadline_ms: 11_000 } }, change), false);
  });
});
