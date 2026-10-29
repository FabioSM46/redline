import assert from "node:assert/strict";
import { describe, it } from "node:test";
import type { GpuSnapshot } from "./gpu.ts";
import { HIGH_TEMPERATURE_C, homeFigures, isHighTemperature } from "./home.ts";

const empty: GpuSnapshot = {
  temp_edge_c: null,
  temp_junction_c: null,
  temp_mem_c: null,
  power_w: null,
  power_cap_w: null,
  fan_rpm: null,
  sclk_mhz: null,
  mclk_mhz: null,
  vddgfx_mv: null,
  gpu_busy_pct: null,
  mem_busy_pct: null,
  vram_used_mib: null,
  vram_total_mib: null,
};

const full: GpuSnapshot = {
  temp_edge_c: 52,
  temp_junction_c: 68,
  temp_mem_c: 60,
  power_w: 141.6,
  power_cap_w: 250,
  fan_rpm: 1200,
  sclk_mhz: 2450,
  mclk_mhz: 1250,
  vddgfx_mv: 1050,
  gpu_busy_pct: 97,
  mem_busy_pct: 40,
  vram_used_mib: 4096,
  vram_total_mib: 16384,
};

describe("isHighTemperature", () => {
  it("is high from each sensor's threshold up", () => {
    assert.equal(isHighTemperature("edge", HIGH_TEMPERATURE_C.edge - 1), false);
    assert.equal(isHighTemperature("edge", HIGH_TEMPERATURE_C.edge), true);
    assert.equal(isHighTemperature("junction", HIGH_TEMPERATURE_C.edge), false);
    assert.equal(isHighTemperature("junction", HIGH_TEMPERATURE_C.junction), true);
    assert.equal(isHighTemperature("memory", HIGH_TEMPERATURE_C.memory + 5), true);
  });

  it("never flags a missing reading", () => {
    assert.equal(isHighTemperature("edge", null), false);
    assert.equal(isHighTemperature("edge", undefined), false);
    assert.equal(isHighTemperature("edge", Number.NaN), false);
  });

  it("keeps the hotspot threshold above the edge one", () => {
    assert.equal(HIGH_TEMPERATURE_C.junction > HIGH_TEMPERATURE_C.edge, true);
  });
});

describe("homeFigures", () => {
  it("puts the five main figures first, with units", () => {
    const { primary } = homeFigures(full);
    assert.deepEqual(
      primary.map((f) => [f.label, f.value, f.detail]),
      [
        ["Temperature", "52 °C", undefined],
        ["Power", "142 W", "of 250 W cap"],
        ["GPU clock", "2450 MHz", undefined],
        ["GPU usage", "97 %", undefined],
        ["VRAM", "4096 MiB", "25 % of 16384 MiB"],
      ],
    );
  });

  it("keeps hotspot, memory temperature, fan, VRAM clock and voltage secondary", () => {
    const { secondary } = homeFigures(full);
    assert.deepEqual(
      secondary.map((f) => [f.label, f.value]),
      [
        ["Hotspot", "68 °C"],
        ["Memory temperature", "60 °C"],
        ["Fan", "1200 rpm"],
        ["VRAM clock", "1250 MHz"],
        ["Voltage", "1050 mV"],
      ],
    );
  });

  it("shows a dash for every missing sensor, never zero", () => {
    const { primary, secondary } = homeFigures(empty);
    for (const f of [...primary, ...secondary]) {
      assert.equal(f.value, "—", f.label);
      assert.equal(f.high, false, f.label);
    }
    assert.equal(primary[1].detail, "of — cap");
    assert.equal(primary[4].detail, "— of —");
  });

  it("flags only the temperatures past their threshold", () => {
    const hot = { ...full, temp_edge_c: 93, temp_junction_c: 99, temp_mem_c: 96 };
    const { primary, secondary } = homeFigures(hot);
    assert.equal(primary[0].high, true);
    assert.equal(secondary[0].high, false);
    assert.equal(secondary[1].high, true);
    assert.equal([...primary.slice(1), ...secondary.slice(2)].every((f) => !f.high), true);
  });
});
