// The in-game overlay's types and wording. MangoHud is configured by the backend (D12): this file
// only mirrors `overlay.rs` and `settings.rs` and says what the choices do. Pure.

/** `OverlayPreset` in src-tauri/src/settings.rs. */
export type OverlayPreset = "off" | "minimal" | "standard" | "full";

/** `OverlayPosition` in src-tauri/src/settings.rs. */
export type OverlayPosition = "top-left" | "top-right" | "bottom-left" | "bottom-right";

/** `OverlayMetric` in src-tauri/src/settings.rs. */
export type OverlayMetric =
  | "fps"
  | "frametime"
  | "gpu_temp"
  | "gpu_power"
  | "gpu_core_clock"
  | "vram"
  | "cpu_temp"
  | "ram";

/** `OverlaySettings` in src-tauri/src/settings.rs. */
export type OverlaySettings = {
  preset: OverlayPreset;
  position: OverlayPosition;
  /** "Advanced": when not empty, these figures replace the ones of the preset. */
  metrics: OverlayMetric[];
  toggle_key: string;
};

/** `ConfigState` in src-tauri/src/overlay.rs: who owns `MangoHud.conf`. */
export type OverlayConfig = "absent" | "managed" | "user_config";

/** `OverlayStatus` in src-tauri/src/overlay.rs. */
export type OverlayStatus = {
  available: boolean;
  unavailable_reason: string | null;
  config: OverlayConfig;
  settings: OverlaySettings;
  launch_hint: string;
};

export const OVERLAY_PRESETS: readonly { value: OverlayPreset; label: string }[] = [
  { value: "off", label: "Off" },
  { value: "minimal", label: "Minimal" },
  { value: "standard", label: "Standard" },
  { value: "full", label: "Full" },
];

export const OVERLAY_POSITIONS: readonly { value: OverlayPosition; label: string }[] = [
  { value: "top-left", label: "Top left" },
  { value: "top-right", label: "Top right" },
  { value: "bottom-left", label: "Bottom left" },
  { value: "bottom-right", label: "Bottom right" },
];

export const OVERLAY_METRICS: readonly { value: OverlayMetric; label: string }[] = [
  { value: "fps", label: "FPS" },
  { value: "frametime", label: "Frametime" },
  { value: "gpu_temp", label: "GPU temperature" },
  { value: "gpu_power", label: "GPU power" },
  { value: "gpu_core_clock", label: "GPU core clock" },
  { value: "vram", label: "VRAM" },
  { value: "cpu_temp", label: "CPU temperature" },
  { value: "ram", label: "RAM" },
];

/** `BACKUP_FILE` in src-tauri/src/overlay.rs. */
export const BACKUP_FILE = "MangoHud.conf.redline-backup";

const MINIMAL: readonly OverlayMetric[] = ["fps", "frametime"];
const STANDARD: readonly OverlayMetric[] = [...MINIMAL, "gpu_temp", "gpu_power", "gpu_core_clock", "cpu_temp"];
const FULL: readonly OverlayMetric[] = [
  "fps",
  "frametime",
  "gpu_temp",
  "gpu_power",
  "gpu_core_clock",
  "vram",
  "cpu_temp",
  "ram",
];

/** The figures of each preset, as in `overlay.rs`. `off` starts hidden; its key shows these. */
export const PRESET_METRICS: Record<OverlayPreset, readonly OverlayMetric[]> = {
  off: MINIMAL,
  minimal: MINIMAL,
  standard: STANDARD,
  full: FULL,
};

/** The Advanced list replaces the preset's figures. */
export function hasOwnFigures(settings: OverlaySettings): boolean {
  return settings.metrics.length > 0;
}

/** The figures chosen: the Advanced list when there is one, otherwise the preset's. */
export function chosenMetrics(settings: OverlaySettings): readonly OverlayMetric[] {
  return hasOwnFigures(settings) ? settings.metrics : PRESET_METRICS[settings.preset];
}

/**
 * What MangoHud draws for `metrics`, in its order (`render` in overlay.rs). More than the list:
 * a GPU figure is drawn inside the line that holds the GPU load, the CPU temperature inside the
 * one with the CPU load, and frametime inside the FPS line.
 */
export function shownFigures(metrics: readonly OverlayMetric[]): string[] {
  const has = (metric: OverlayMetric) => metrics.includes(metric);
  const shown: string[] = [];
  if (has("gpu_temp") || has("gpu_power") || has("gpu_core_clock")) shown.push("GPU load");
  if (has("gpu_temp")) shown.push("GPU temperature");
  if (has("gpu_power")) shown.push("GPU power");
  if (has("gpu_core_clock")) shown.push("GPU core clock");
  if (has("cpu_temp")) shown.push("CPU load", "CPU temperature");
  if (has("vram")) shown.push("VRAM");
  if (has("ram")) shown.push("RAM");
  if (has("fps") || has("frametime")) shown.push("FPS");
  if (has("frametime")) shown.push("frametime");
  return shown;
}

function joinList(items: readonly string[]): string {
  if (items.length < 2) return items.join("");
  return `${items.slice(0, -1).join(", ")} and ${items[items.length - 1]}`;
}

/** What these settings put on screen in a game. */
export function overlayDescription(settings: OverlaySettings): string {
  const figures = joinList(shownFigures(chosenMetrics(settings)));
  if (settings.preset === "off") return `Hidden when a game starts. ${settings.toggle_key} shows ${figures}.`;
  return `Shows ${figures}.`;
}

/**
 * `settings` with one figure of Advanced switched. The list starts from what is chosen now. The
 * last figure stays: an empty list means the preset's figures, not none. A list equal to the
 * preset's is stored empty, so that the next preset chosen applies.
 */
export function toggleMetric(settings: OverlaySettings, metric: OverlayMetric): OverlaySettings {
  const chosen = chosenMetrics(settings);
  const next = OVERLAY_METRICS.map((m) => m.value).filter((m) => (m === metric ? !chosen.includes(m) : chosen.includes(m)));
  if (next.length === 0) return settings;
  const preset = PRESET_METRICS[settings.preset];
  const samePreset = next.length === preset.length && next.every((m) => preset.includes(m));
  return { ...settings, metrics: samePreset ? [] : next };
}

/** Whether `metric` is the only one chosen, and so cannot be switched off. */
export function isLastMetric(settings: OverlaySettings, metric: OverlayMetric): boolean {
  const chosen = new Set(chosenMetrics(settings));
  return chosen.size === 1 && chosen.has(metric);
}

/** What a successful `set_overlay` did, which depends on who owns the file. */
export function savedNote(config: OverlayConfig): string {
  switch (config) {
    case "user_config":
      return "Saved in Redline. Your MangoHud.conf was not changed.";
    case "managed":
      return "Saved to MangoHud.conf.";
    case "absent":
      return "Saved in Redline.";
  }
}
