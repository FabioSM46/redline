import assert from "node:assert/strict";
import { describe, it } from "node:test";
import type { GpuSnapshot } from "./gpu.ts";
import {
  CHARTS,
  type ChartSpec,
  DEFAULT_WINDOW,
  formatClock,
  GAP_MS,
  type HistorySample,
  isWindowS,
  niceCeil,
  toColumns,
  WINDOWS,
  xRange,
  yRange,
} from "./metrics.ts";

const EMPTY: GpuSnapshot = {
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

function sample(timestamp_ms: number, values: Partial<GpuSnapshot>): HistorySample {
  return { timestamp_ms, snapshot: { ...EMPTY, ...values } };
}

function chart(id: string): ChartSpec {
  const found = CHARTS.find((c) => c.id === id);
  if (!found) throw new Error(`no chart ${id}`);
  return found;
}

describe("windows", () => {
  it("offers exactly the windows gpu_history accepts", () => {
    assert.deepEqual(
      WINDOWS.map((w) => w.seconds),
      [60, 300, 900],
    );
    assert.deepEqual(
      WINDOWS.map((w) => w.label),
      ["1 min", "5 min", "15 min"],
    );
    assert.equal(isWindowS(DEFAULT_WINDOW), true);
  });

  it("rejects any other value", () => {
    for (const bad of [0, 30, 61, 600, 3600, Number.NaN]) {
      assert.equal(isWindowS(bad), false, String(bad));
    }
  });
});

describe("CHARTS", () => {
  it("has the four charts with their units", () => {
    assert.deepEqual(
      CHARTS.map((c) => [c.id, c.unit, c.series.map((s) => s.label)]),
      [
        ["temperature", "°C", ["Edge", "Hotspot"]],
        ["clocks", "MHz", ["GPU", "VRAM"]],
        ["power", "W", ["Power", "Cap"]],
        ["usage", "%", ["GPU", "VRAM"]],
      ],
    );
  });

  it("draws the power cap as a limit", () => {
    assert.equal(chart("power").series[1].role, "limit");
  });
});

describe("toColumns", () => {
  it("returns empty columns for an empty history", () => {
    assert.deepEqual(toColumns([], chart("temperature")), [[], [], []]);
  });

  it("converts milliseconds to seconds and keeps the order", () => {
    const history = [sample(10_000, { temp_edge_c: 40, temp_junction_c: 50 }), sample(11_000, { temp_edge_c: 41, temp_junction_c: 52 })];
    assert.deepEqual(toColumns(history, chart("temperature")), [
      [10, 11],
      [40, 41],
      [50, 52],
    ]);
  });

  it("keeps a missing sensor as null, never 0", () => {
    const history = [sample(1_000, { power_w: 120, power_cap_w: null }), sample(2_000, { power_w: null, power_cap_w: 200 })];
    assert.deepEqual(toColumns(history, chart("power")), [
      [1, 2],
      [120, null],
      [null, 200],
    ]);
  });

  it("turns a non-finite value into a gap", () => {
    const [, gpu] = toColumns([sample(1_000, { gpu_busy_pct: Number.NaN })], chart("usage"));
    assert.deepEqual(gpu, [null]);
  });

  it("derives VRAM usage from used and total memory", () => {
    const history = [
      sample(1_000, { gpu_busy_pct: 90, vram_used_mib: 2048, vram_total_mib: 8192 }),
      sample(2_000, { gpu_busy_pct: 91, vram_used_mib: 2048, vram_total_mib: null }),
    ];
    assert.deepEqual(toColumns(history, chart("usage")), [
      [1, 2],
      [90, 91],
      [25, null],
    ]);
  });

  it("breaks the line across a hole in the sampling", () => {
    const history = [sample(1_000, { sclk_mhz: 500, mclk_mhz: 96 }), sample(1_000 + GAP_MS + 1_000, { sclk_mhz: 2400, mclk_mhz: 1000 })];
    assert.deepEqual(toColumns(history, chart("clocks")), [
      [1, 3, 5],
      [500, null, 2400],
      [96, null, 1000],
    ]);
  });

  it("does not break the line on the normal one-second step or a small jitter", () => {
    const history = [sample(1_000, { sclk_mhz: 1 }), sample(2_000, { sclk_mhz: 2 }), sample(1_000 + 2_000 + GAP_MS - 1_000, { sclk_mhz: 3 })];
    const [xs] = toColumns(history, chart("clocks"));
    assert.equal(xs.length, 3);
  });

  it("sorts samples that arrive out of order", () => {
    const history = [sample(2_000, { temp_edge_c: 2 }), sample(1_000, { temp_edge_c: 1 })];
    const [xs, edge] = toColumns(history, chart("temperature"));
    assert.deepEqual(xs, [1, 2]);
    assert.deepEqual(edge, [1, 2]);
  });
});

describe("xRange", () => {
  it("is null without samples", () => {
    assert.equal(xRange([], 60), null);
  });

  it("spans the whole window ending at the newest sample", () => {
    const history = [sample(100_000, {}), sample(101_000, {})];
    assert.deepEqual(xRange(history, 60), [41, 101]);
    assert.deepEqual(xRange(history, 900), [-799, 101]);
  });
});

describe("yRange", () => {
  it("keeps usage fixed at 0–100", () => {
    assert.deepEqual(yRange(chart("usage"), 12), [0, 100]);
    assert.deepEqual(yRange(chart("usage"), null), [0, 100]);
  });

  it("starts at 0 and ends on a round number above the data", () => {
    assert.deepEqual(yRange(chart("temperature"), 72), [0, 80]);
    assert.deepEqual(yRange(chart("clocks"), 2600), [0, 3000]);
    assert.deepEqual(yRange(chart("power"), 180), [0, 200]);
  });

  it("has a usable range without data", () => {
    assert.deepEqual(yRange(chart("power"), null), [0, 1]);
    assert.deepEqual(yRange(chart("power"), 0), [0, 1]);
  });
});

describe("niceCeil", () => {
  it("rounds up to a multiple of half a power of ten", () => {
    assert.equal(niceCeil(1), 1);
    assert.equal(niceCeil(11), 15);
    assert.equal(niceCeil(21), 25);
    assert.equal(niceCeil(26), 30);
    assert.equal(niceCeil(79.2), 80);
    assert.equal(niceCeil(2860), 3000);
  });

  it("falls back to 1 for zero or a negative value", () => {
    assert.equal(niceCeil(0), 1);
    assert.equal(niceCeil(-5), 1);
  });
});

describe("formatClock", () => {
  it("formats local time with or without seconds", () => {
    const seconds = new Date(2026, 0, 2, 9, 5, 7).getTime() / 1000;
    assert.equal(formatClock(seconds, true), "09:05:07");
    assert.equal(formatClock(seconds, false), "09:05");
  });
});
