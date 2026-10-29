// GPU types returned by the backend and the pure helpers the shell uses. Tested in gpu.test.ts.

import { formatValue } from "./format.ts";

/** Mirrors `redline_core::sensors::GpuInfo` (`list_gpus`). */
export type GpuInfo = {
  id: string;
  name: string;
  pci_slot: string;
};

/** Mirrors `redline_core::sensors::GpuSnapshot` (`gpu_snapshot`). A missing sensor is null. */
export type GpuSnapshot = {
  temp_edge_c: number | null;
  temp_junction_c: number | null;
  temp_mem_c: number | null;
  power_w: number | null;
  power_cap_w: number | null;
  fan_rpm: number | null;
  sclk_mhz: number | null;
  mclk_mhz: number | null;
  vddgfx_mv: number | null;
  gpu_busy_pct: number | null;
  mem_busy_pct: number | null;
  vram_used_mib: number | null;
  vram_total_mib: number | null;
};

/** The latest sensor reading of the selected GPU, polled once per second by the shell. */
export type SnapshotState =
  | { status: "loading" }
  | { status: "ok"; snapshot: GpuSnapshot }
  | { status: "error"; message: string };

/** The selector is noise with a single GPU: it appears only when there is a choice to make. */
export function showGpuSelector(gpus: readonly GpuInfo[]): boolean {
  return gpus.length > 1;
}

/** Keeps the current choice while that GPU is still listed, otherwise falls back to the first. */
export function pickGpuId(gpus: readonly GpuInfo[], current: string | null): string | null {
  if (current !== null && gpus.some((g) => g.id === current)) {
    return current;
  }
  return gpus[0]?.id ?? null;
}

/** The status bar items: GPU name, temperature and power. A missing value is `—`. */
export function statusBarItems(name: string, state: SnapshotState): string[] {
  const snapshot = state.status === "ok" ? state.snapshot : null;
  return [name, formatValue(snapshot?.temp_edge_c, "°C"), formatValue(snapshot?.power_w, "W")];
}
