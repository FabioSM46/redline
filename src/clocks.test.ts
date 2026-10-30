import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { describe, it } from "node:test";
import {
  canApplyClocks,
  CLOCK_KEYS,
  clockLimits,
  clockRequest,
  clocksView,
  clockValue,
  hasFloorOnly,
  listedControls,
  NEEDS_OVERDRIVE,
  UNDERVOLT_ONLY,
  VOLTAGE_FLOOR_MV,
} from "./clocks.ts";
import type { ClocksStatus } from "./tuning.ts";

const rdna3: ClocksStatus = {
  unavailable_reason: null,
  max_core_clock: { current: 2500, min: 500, max: 2700 },
  core_clock_offset: null,
  max_vram_clock: { current: 1250, min: 97, max: 1300 },
  voltage_offset: { current: 0, min: -200, max: 0 },
};

const unavailable = (reason: string): ClocksStatus => ({
  unavailable_reason: reason,
  max_core_clock: null,
  core_clock_offset: null,
  max_vram_clock: null,
  voltage_offset: null,
});

const rdna2Voltage = { current: 0, min: null, max: null };

describe("clocksView", () => {
  const view = (clocks: ClocksStatus | null, rebootPending = false) =>
    clocksView({ clocks, reboot_pending: rebootPending }).kind;

  it("shows the controls the status lists", () => {
    assert.deepEqual(clocksView({ clocks: rdna3, reboot_pending: false }), { kind: "controls", clocks: rdna3 });
    assert.equal(view(rdna3, true), "controls");
  });

  it("tells overdrive off from an unsupported family by the reason", () => {
    assert.deepEqual(clocksView({ clocks: unavailable(NEEDS_OVERDRIVE), reboot_pending: false }), {
      kind: "overdrive",
      reason: NEEDS_OVERDRIVE,
    });
    assert.deepEqual(clocksView({ clocks: unavailable("Not this family."), reboot_pending: false }), {
      kind: "unsupported",
      reason: "Not this family.",
    });
  });

  it("does not ask to enable what already waits for a reboot", () => {
    assert.equal(view(unavailable(NEEDS_OVERDRIVE), true), "reboot");
    assert.equal(view(null, true), "reboot");
    assert.equal(view(unavailable("Not this family."), true), "unsupported");
  });

  it("says so when LACT reports nothing", () => {
    assert.equal(view(null), "unreported");
    assert.equal(view({ ...unavailable(""), unavailable_reason: null }), "unreported");
  });
});

describe("listedControls", () => {
  it("lists only what the GPU exposes", () => {
    assert.deepEqual(
      listedControls(rdna3).map((c) => c.key),
      ["max_core_clock", "max_vram_clock", "voltage_offset"],
    );
  });
});

describe("clockLimits", () => {
  it("is the firmware's range", () => {
    assert.deepEqual(clockLimits("max_core_clock", { current: 2500, min: 500, max: 2700 }), { min: 500, max: 2700 });
  });

  it("offers no clock without a firmware range", () => {
    assert.equal(clockLimits("max_core_clock", { current: 2500, min: null, max: null }), null);
    assert.equal(clockLimits("core_clock_offset", { current: 0, min: 10, max: -10 }), null);
  });

  it("keeps the voltage offset at or below 0", () => {
    assert.deepEqual(clockLimits("voltage_offset", { current: 0, min: -100, max: 50 }), { min: -100, max: 0 });
    assert.equal(clockLimits("voltage_offset", { current: 0, min: 10, max: 50 }), null);
  });

  it("gives a voltage offset without a range the backend's limit, and marks it as such", () => {
    assert.deepEqual(clockLimits("voltage_offset", rdna2Voltage), { min: VOLTAGE_FLOOR_MV, max: 0 });
    assert.equal(hasFloorOnly("voltage_offset", rdna2Voltage), true);
    assert.equal(hasFloorOnly("voltage_offset", { current: 0, min: -100, max: 0 }), false);
    assert.equal(hasFloorOnly("max_core_clock", rdna2Voltage), false);
  });
});

describe("clockValue", () => {
  it("shows zero as a value, and the unit of each control", () => {
    assert.equal(clockValue("voltage_offset", 0), "0 mV");
    assert.equal(clockValue("max_vram_clock", 1250), "1250 MHz");
    assert.equal(clockValue("voltage_offset", null), "—");
  });
});

describe("clockRequest", () => {
  it("sends only what changed and says what", () => {
    const request = clockRequest(rdna3, { max_core_clock: "2500", voltage_offset: "-50" });
    assert.deepEqual(request.values, { voltage_offset: -50 });
    assert.deepEqual(request.errors, {});
    assert.equal(request.detail, "Voltage offset 0 mV → -50 mV");
    assert.equal(canApplyClocks(request), true);
  });

  it("joins several values, in block order", () => {
    const request = clockRequest(rdna3, { voltage_offset: "-50", max_core_clock: "2400" });
    assert.deepEqual(Object.keys(request.values), ["max_core_clock", "voltage_offset"]);
    assert.equal(request.detail, "Maximum core clock 2500 MHz → 2400 MHz, Voltage offset 0 mV → -50 mV");
  });

  it("has nothing to apply without a change", () => {
    assert.equal(canApplyClocks(clockRequest(rdna3, {})), false);
    assert.equal(canApplyClocks(clockRequest(rdna3, { voltage_offset: "0" })), false);
  });

  it("refuses a value out of range instead of moving it", () => {
    const request = clockRequest(rdna3, { max_core_clock: "2800", voltage_offset: "-50" });
    assert.equal(request.errors.max_core_clock, "Must be between 500 MHz and 2700 MHz.");
    assert.equal(request.values.max_core_clock, undefined);
    assert.equal(canApplyClocks(request), false);
  });

  it("refuses a raised voltage with the undervolt sentence", () => {
    assert.equal(clockRequest(rdna3, { voltage_offset: "10" }).errors.voltage_offset, UNDERVOLT_ONLY);
    const rdna2 = { ...rdna3, voltage_offset: rdna2Voltage };
    assert.equal(clockRequest(rdna2, { voltage_offset: "10" }).errors.voltage_offset, UNDERVOLT_ONLY);
  });

  it("holds a voltage offset without a range to the backend's limit", () => {
    const rdna2 = { ...rdna3, voltage_offset: rdna2Voltage };
    assert.deepEqual(clockRequest(rdna2, { voltage_offset: "-450" }).values, { voltage_offset: -450 });
    assert.equal(clockRequest(rdna2, { voltage_offset: "-451" }).errors.voltage_offset, "Must be between -450 mV and 0 mV.");
  });

  it("refuses what is not a whole number", () => {
    for (const text of ["", "  ", "12.5", "abc", "1e999"]) {
      assert.equal(clockRequest(rdna3, { max_core_clock: text }).errors.max_core_clock, "Enter a whole number of MHz.");
    }
  });

  it("leaves a value in force alone, even outside the range", () => {
    const outside = { ...rdna3, max_core_clock: { current: 2800, min: 500, max: 2700 } };
    const request = clockRequest(outside, { max_core_clock: "2800", voltage_offset: "-50" });
    assert.deepEqual(request.errors, {});
    assert.deepEqual(request.values, { voltage_offset: -50 });
  });

  it("never sends a control that cannot be edited or that the GPU does not list", () => {
    const fixed = { ...rdna3, max_core_clock: { current: 2500, min: null, max: null } };
    assert.deepEqual(clockRequest(fixed, { max_core_clock: "2400" }).values, {});
    assert.deepEqual(clockRequest(rdna3, { core_clock_offset: "-50" }).values, {});
  });
});

describe("mirrors of src-tauri/src/tuning/clocks.rs", () => {
  const rust = readFileSync(new URL("../src-tauri/src/tuning/clocks.rs", import.meta.url), "utf8");
  const sentence = (name: string) => {
    const match = new RegExp(`pub const ${name}: &str = "([^"]*)";`).exec(rust);
    return match?.[1].replace(/\\\n\s*/g, "");
  };

  it("has the backend's sentences", () => {
    assert.equal(sentence("NEEDS_OVERDRIVE"), NEEDS_OVERDRIVE);
    assert.equal(sentence("UNDERVOLT_ONLY"), UNDERVOLT_ONLY);
  });

  it("has the backend's voltage floor", () => {
    assert.equal(rust.includes(`const VOLTAGE_FLOOR_MV: i32 = ${VOLTAGE_FLOOR_MV};`), true);
  });

  it("has every key set_clocks accepts", () => {
    const keys = [...rust.matchAll(/^\s+key: "(\w+)",/gm)].map((m) => m[1]);
    assert.deepEqual(keys, [...CLOCK_KEYS]);
  });
});
