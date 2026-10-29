// The figures Home shows for the selected GPU. Pure functions, tested in home.test.ts.

import { formatValue, percentOf } from "./format.ts";
import type { GpuSnapshot } from "./gpu.ts";

/**
 * Temperatures from which Home colours a figure as high, in °C, one per sensor because each runs
 * at a different level: the hotspot is normally 10–20 °C above the edge, and the memory sits in
 * between. Below the point where amdgpu starts throttling, so the colour is a warning, not a
 * post-mortem.
 */
export const HIGH_TEMPERATURE_C = {
  edge: 90,
  junction: 100,
  memory: 95,
} as const;

export type TemperatureSensor = keyof typeof HIGH_TEMPERATURE_C;

/** True from the sensor's threshold up; a missing reading is never high. */
export function isHighTemperature(sensor: TemperatureSensor, celsius: number | null | undefined): boolean {
  return celsius != null && !Number.isNaN(celsius) && celsius >= HIGH_TEMPERATURE_C[sensor];
}

/** One figure on Home: a label, a value with its unit and an optional second line. */
export type Figure = {
  label: string;
  value: string;
  /** Context for the value (power cap, VRAM percent); omitted when there is none. */
  detail?: string;
  /** The value is past a threshold and is shown in the state colour. */
  high: boolean;
};

export type HomeFigures = {
  /** The large figures: read at a glance. */
  primary: Figure[];
  /** The smaller figures below them. */
  secondary: Figure[];
};

/** Splits a snapshot into Home's large and small figures. A missing sensor is `—`, never zero. */
export function homeFigures(s: GpuSnapshot): HomeFigures {
  const vramPct = percentOf(s.vram_used_mib, s.vram_total_mib);
  return {
    primary: [
      { label: "Temperature", value: formatValue(s.temp_edge_c, "°C"), high: isHighTemperature("edge", s.temp_edge_c) },
      { label: "Power", value: formatValue(s.power_w, "W"), detail: `of ${formatValue(s.power_cap_w, "W")} cap`, high: false },
      { label: "GPU clock", value: formatValue(s.sclk_mhz, "MHz"), high: false },
      { label: "GPU usage", value: formatValue(s.gpu_busy_pct, "%"), high: false },
      {
        label: "VRAM",
        value: formatValue(s.vram_used_mib, "MiB"),
        detail: `${formatValue(vramPct, "%")} of ${formatValue(s.vram_total_mib, "MiB")}`,
        high: false,
      },
    ],
    secondary: [
      {
        label: "Hotspot",
        value: formatValue(s.temp_junction_c, "°C"),
        high: isHighTemperature("junction", s.temp_junction_c),
      },
      { label: "Memory temperature", value: formatValue(s.temp_mem_c, "°C"), high: isHighTemperature("memory", s.temp_mem_c) },
      { label: "Fan", value: formatValue(s.fan_rpm, "rpm"), high: false },
      { label: "VRAM clock", value: formatValue(s.mclk_mhz, "MHz"), high: false },
      { label: "Voltage", value: formatValue(s.vddgfx_mv, "mV"), high: false },
    ],
  };
}
