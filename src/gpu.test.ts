import assert from "node:assert/strict";
import { describe, it } from "node:test";
import { type GpuInfo, type GpuSnapshot, pickGpuId, showGpuSelector, statusBarItems } from "./gpu.ts";

const igpu: GpuInfo = { id: "0000:0e:00.0", name: "Integrated GPU", pci_slot: "0000:0e:00.0" };
const dgpu: GpuInfo = { id: "0000:03:00.0", name: "Discrete GPU", pci_slot: "0000:03:00.0" };

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

describe("showGpuSelector", () => {
  it("hides the selector with zero or one GPU", () => {
    assert.equal(showGpuSelector([]), false);
    assert.equal(showGpuSelector([dgpu]), false);
  });

  it("shows the selector with more than one GPU", () => {
    assert.equal(showGpuSelector([dgpu, igpu]), true);
  });
});

describe("pickGpuId", () => {
  it("returns null when no GPU is listed", () => {
    assert.equal(pickGpuId([], null), null);
    assert.equal(pickGpuId([], dgpu.id), null);
  });

  it("starts from the first GPU listed", () => {
    assert.equal(pickGpuId([dgpu, igpu], null), dgpu.id);
  });

  it("keeps a choice that is still listed", () => {
    assert.equal(pickGpuId([dgpu, igpu], igpu.id), igpu.id);
  });

  it("falls back to the first GPU when the chosen one is gone", () => {
    assert.equal(pickGpuId([dgpu], igpu.id), dgpu.id);
  });
});

describe("statusBarItems", () => {
  it("shows name, temperature and power", () => {
    const snapshot = { ...empty, temp_edge_c: 41.2, power_w: 32.4 };
    assert.deepEqual(statusBarItems("Discrete GPU", { status: "ok", snapshot }), [
      "Discrete GPU",
      "41 °C",
      "32 W",
    ]);
  });

  it("shows a dash for a sensor the GPU does not expose", () => {
    const snapshot = { ...empty, temp_edge_c: 41 };
    assert.deepEqual(statusBarItems("Discrete GPU", { status: "ok", snapshot }), ["Discrete GPU", "41 °C", "—"]);
  });

  it("shows dashes while loading and after an error", () => {
    assert.deepEqual(statusBarItems("Discrete GPU", { status: "loading" }), ["Discrete GPU", "—", "—"]);
    assert.deepEqual(statusBarItems("Discrete GPU", { status: "error", message: "gone" }), [
      "Discrete GPU",
      "—",
      "—",
    ]);
  });
});
