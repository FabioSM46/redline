// Performance → Metrics: the four charts, the window choice and the conversion of `gpu_history`
// into uPlot columns. Pure functions, tested in metrics.test.ts.

import { percentOf } from "./format.ts";
import type { GpuSnapshot } from "./gpu.ts";

/** One element of `gpu_history`: wall-clock milliseconds and the reading taken then. */
export type HistorySample = {
  timestamp_ms: number;
  snapshot: GpuSnapshot;
};

/** The windows `gpu_history` accepts; anything else is an error in the backend. */
export const WINDOWS = [
  { seconds: 60, label: "1 min" },
  { seconds: 300, label: "5 min" },
  { seconds: 900, label: "15 min" },
] as const;

export type WindowS = (typeof WINDOWS)[number]["seconds"];

export const DEFAULT_WINDOW: WindowS = 300;

export function isWindowS(value: number): value is WindowS {
  return WINDOWS.some((w) => w.seconds === value);
}

/** `primary` and `secondary` are measured lines; `limit` is a threshold such as the power cap. */
export type SeriesRole = "primary" | "secondary" | "limit";

export type SeriesSpec = {
  label: string;
  role: SeriesRole;
  value: (snapshot: GpuSnapshot) => number | null;
};

export type ChartSpec = {
  id: string;
  title: string;
  unit: string;
  /** A scale that never moves (usage is always 0–100 %); otherwise it starts at 0 and fits the data. */
  fixedRange?: readonly [number, number];
  series: readonly SeriesSpec[];
};

export const CHARTS: readonly ChartSpec[] = [
  {
    id: "temperature",
    title: "Temperature",
    unit: "°C",
    series: [
      { label: "Edge", role: "primary", value: (s) => s.temp_edge_c },
      { label: "Hotspot", role: "secondary", value: (s) => s.temp_junction_c },
    ],
  },
  {
    id: "clocks",
    title: "Clocks",
    unit: "MHz",
    series: [
      { label: "GPU", role: "primary", value: (s) => s.sclk_mhz },
      { label: "VRAM", role: "secondary", value: (s) => s.mclk_mhz },
    ],
  },
  {
    id: "power",
    title: "Power",
    unit: "W",
    series: [
      { label: "Power", role: "primary", value: (s) => s.power_w },
      { label: "Cap", role: "limit", value: (s) => s.power_cap_w },
    ],
  },
  {
    id: "usage",
    title: "Usage",
    unit: "%",
    fixedRange: [0, 100],
    series: [
      { label: "GPU", role: "primary", value: (s) => s.gpu_busy_pct },
      { label: "VRAM", role: "secondary", value: (s) => percentOf(s.vram_used_mib, s.vram_total_mib) },
    ],
  },
];

/**
 * The sampler reads once per second. A longer silence (the machine was suspended, a read
 * stalled) is a hole in the data: the line breaks there instead of joining the two sides.
 */
export const GAP_MS = 3000;

/** uPlot's aligned columns: x in seconds, then one column per series. */
export type Columns = [number[], ...(number | null)[][]];

/**
 * History → uPlot columns for one chart. Samples are expected oldest first (the backend's
 * order); out-of-order ones are sorted rather than drawn backwards. A missing or non-finite
 * value is `null`, never 0, and a time gap longer than `GAP_MS` gets a `null` row so the line
 * breaks instead of bridging it.
 */
export function toColumns(samples: readonly HistorySample[], chart: ChartSpec): Columns {
  const sorted = isSorted(samples) ? samples : [...samples].sort((a, b) => a.timestamp_ms - b.timestamp_ms);
  const xs: number[] = [];
  const ys: (number | null)[][] = chart.series.map(() => []);
  let previous: number | null = null;
  for (const sample of sorted) {
    const t = sample.timestamp_ms;
    if (previous !== null && t - previous > GAP_MS) {
      xs.push((previous + t) / 2 / 1000);
      for (const column of ys) column.push(null);
    }
    xs.push(t / 1000);
    chart.series.forEach((series, i) => ys[i].push(finiteOrNull(series.value(sample.snapshot))));
    previous = t;
  }
  return [xs, ...ys];
}

/** The x range in seconds: the whole window, ending at the newest sample. */
export function xRange(samples: readonly HistorySample[], windowS: WindowS): [number, number] | null {
  if (samples.length === 0) return null;
  const newest = Math.max(...samples.map((s) => s.timestamp_ms)) / 1000;
  return [newest - windowS, newest];
}

/** The y range: fixed when the chart has one, otherwise from 0 to a little above the data. */
export function yRange(chart: ChartSpec, dataMax: number | null): [number, number] {
  if (chart.fixedRange) return [chart.fixedRange[0], chart.fixedRange[1]];
  if (dataMax === null || !Number.isFinite(dataMax) || dataMax <= 0) return [0, 1];
  return [0, niceCeil(dataMax * 1.1)];
}

/** Rounds up to a multiple of half a power of ten, so the top grid line is a round number. */
export function niceCeil(value: number): number {
  if (!(value > 0)) return 1;
  const step = 10 ** Math.floor(Math.log10(value)) / 2;
  return Math.ceil(value / step) * step;
}

/** Local wall-clock time of an x value (seconds), `HH:MM` or `HH:MM:SS`. */
export function formatClock(seconds: number, withSeconds: boolean): string {
  const d = new Date(seconds * 1000);
  const parts = [d.getHours(), d.getMinutes(), ...(withSeconds ? [d.getSeconds()] : [])];
  return parts.map((n) => String(n).padStart(2, "0")).join(":");
}

function finiteOrNull(value: number | null): number | null {
  return value !== null && Number.isFinite(value) ? value : null;
}

function isSorted(samples: readonly HistorySample[]): boolean {
  for (let i = 1; i < samples.length; i++) {
    if (samples[i].timestamp_ms < samples[i - 1].timestamp_ms) return false;
  }
  return true;
}
