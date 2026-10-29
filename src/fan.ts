// The fan block of Performance → Tuning: the curve model and the pure helpers behind the editor.
// The rules are `FanStatus.curve_rules`, the same ones `set_fan` checks (src-tauri/src/tuning/
// fan.rs): the editor never builds a curve the backend would refuse. Tested in fan.test.ts.

import { formatValue } from "./format.ts";
import type {
  CurvePoint,
  CurveRules,
  FanSetting,
  FanSettings,
  FanStatus,
  PmfwSettings,
  PmfwStatus,
  PmfwValue,
  TuningStatus,
} from "./tuning.ts";

/** Arrow keys move a point by one; with Shift by this much. */
export const BIG_STEP = 5;

/** The number of points a new curve has when the GPU does not fix it. */
export const DEFAULT_POINTS = 5;

export const FAN_SETTINGS: readonly { value: FanSetting; label: string }[] = [
  { value: "auto", label: "Automatic" },
  { value: "static", label: "Fixed" },
  { value: "curve", label: "Curve" },
];

export function settingLabel(setting: FanSetting): string {
  return FAN_SETTINGS.find((s) => s.value === setting)?.label ?? setting;
}

/** What LACT does with the fan now; null for a mode this Redline does not know. */
export function currentSetting(fan: FanStatus): FanSetting | null {
  if (!fan.control_enabled) return "auto";
  return fan.mode;
}

/**
 * Whether a fixed speed or a curve can be set, and if not, why:
 * - `ready`: `curve_rules` is there;
 * - `overdrive`: LACT cannot drive the fan and overdrive is off: recent GPUs need it;
 * - `reboot`: overdrive is set for the next boot;
 * - `not-controllable`: LACT cannot drive this fan, overdrive or not;
 * - `no-rules`: LACT could drive it, but Redline cannot read the limits of its firmware curve.
 */
export type FanAvailability = "ready" | "overdrive" | "reboot" | "not-controllable" | "no-rules";

export function fanAvailability(fan: FanStatus, status: Pick<TuningStatus, "overdrive_enabled" | "reboot_pending">): FanAvailability {
  if (fan.curve_rules !== null) return "ready";
  if (fan.configurable) return "no-rules";
  if (status.reboot_pending) return "reboot";
  if (status.overdrive_enabled === false) return "overdrive";
  return "not-controllable";
}

/** The sentence the block shows when a fixed speed or a curve cannot be set. */
export function availabilitySentence(availability: FanAvailability): string | null {
  switch (availability) {
    case "ready":
      return null;
    case "overdrive":
      return (
        "LACT cannot drive this fan now. On recent GPUs the fan is driven by the firmware, which accepts a fixed " +
        "speed, a curve or fan options only with overdrive enabled: see Overdrive below, then reboot."
      );
    case "reboot":
      return "Overdrive is set for the next boot: after a reboot the fan can take a fixed speed or a curve.";
    case "not-controllable":
      return "LACT cannot control this GPU's fan: it stays on the automatic control.";
    case "no-rules":
      return (
        "Redline cannot read the limits of this GPU's firmware fan curve, so it offers only the automatic " +
        "control."
      );
  }
}

/** The settings the mode selector offers: automatic always, the others only with rules. */
export function offeredSettings(rules: CurveRules | null): FanSetting[] {
  return rules === null ? ["auto"] : ["auto", "static", "curve"];
}

/** A speed as the backend accepts it: inside the range, whole on a firmware curve, else to 0.1 %. */
export function snapSpeed(value: number, rules: CurveRules): number {
  const rounded = rules.whole_percent ? Math.round(value) : Math.round(value * 10) / 10;
  return Math.min(rules.speed.max_pct, Math.max(rules.speed.min_pct, rounded));
}

/** A temperature as the backend accepts it: whole degrees inside the range. */
export function snapTemperature(value: number, rules: CurveRules): number {
  return Math.min(rules.temperature.max_c, Math.max(rules.temperature.min_c, Math.round(value)));
}

function sorted(curve: readonly CurvePoint[]): CurvePoint[] {
  return [...curve].sort((a, b) => a.temperature_c - b.temperature_c);
}

/**
 * Why `set_fan` would refuse `curve`, or null. The same checks, in the same order, as
 * `check_curve` in src-tauri/src/tuning/fan.rs.
 */
export function curveProblem(curve: readonly CurvePoint[], rules: CurveRules): string | null {
  if (curve.length === 0) return "A fan curve needs at least one point.";
  if (rules.points !== null && curve.length !== rules.points) {
    return `This GPU's firmware fan curve has exactly ${rules.points} points; this curve has ${curve.length}.`;
  }
  const points = sorted(curve);
  const { min_c, max_c } = rules.temperature;
  const { min_pct, max_pct } = rules.speed;
  for (const point of points) {
    if (!Number.isInteger(point.temperature_c) || point.temperature_c < min_c || point.temperature_c > max_c) {
      return `Curve temperatures must be whole degrees between ${min_c} °C and ${max_c} °C.`;
    }
    if (!Number.isFinite(point.speed_pct) || point.speed_pct < min_pct || point.speed_pct > max_pct) {
      return `The speed at ${point.temperature_c} °C must be between ${min_pct} % and ${max_pct} %.`;
    }
    if (rules.whole_percent && !Number.isInteger(point.speed_pct)) {
      return `The speed at ${point.temperature_c} °C must be a whole percent on this GPU.`;
    }
  }
  for (let i = 1; i < points.length; i++) {
    const [low, high] = [points[i - 1], points[i]];
    if (low.temperature_c === high.temperature_c) return `The curve has two points at ${low.temperature_c} °C.`;
    if (high.speed_pct < low.speed_pct) {
      return `The fan must not slow down as the temperature rises: ${high.temperature_c} °C is below ${low.temperature_c} °C.`;
    }
  }
  return null;
}

/** A rising curve across the whole range: what the editor starts from without a usable stored one. */
export function defaultCurve(rules: CurveRules): CurvePoint[] {
  const { min_c, max_c } = rules.temperature;
  const count = Math.max(1, Math.min(rules.points ?? DEFAULT_POINTS, max_c - min_c + 1));
  if (count === 1) return [{ temperature_c: min_c, speed_pct: snapSpeed(rules.speed.max_pct, rules) }];
  return Array.from({ length: count }, (_, i) => {
    const share = i / (count - 1);
    return {
      temperature_c: Math.round(min_c + (max_c - min_c) * share),
      speed_pct: snapSpeed(rules.speed.min_pct + (rules.speed.max_pct - rules.speed.min_pct) * share, rules),
    };
  });
}

/**
 * The curve the editor starts from: LACT's stored one, sorted and brought inside the rules (a
 * speed stored with a fraction on a whole-percent GPU), when it has the right number of points
 * and stays valid; otherwise `defaultCurve`.
 */
export function initialCurve(stored: readonly CurvePoint[] | null, rules: CurveRules): CurvePoint[] {
  if (stored !== null && stored.length > 0 && (rules.points === null || stored.length === rules.points)) {
    const fitted = sorted(stored).map((p) => ({
      temperature_c: snapTemperature(p.temperature_c, rules),
      speed_pct: snapSpeed(p.speed_pct, rules),
    }));
    if (curveProblem(fitted, rules) === null) return fitted;
  }
  return defaultCurve(rules);
}

/**
 * `curve` with point `index` moved to `target`, kept valid: the temperature stays whole, inside
 * the range and strictly between its neighbours (the order of the points never changes), the
 * speed inside the range; a speed raised above a later point lifts it along, one lowered below
 * an earlier point lowers it, so the fan never slows down as the temperature rises.
 */
export function movePoint(
  curve: readonly CurvePoint[],
  index: number,
  target: CurvePoint,
  rules: CurveRules,
): CurvePoint[] {
  if (index < 0 || index >= curve.length) return [...curve];
  const points = sorted(curve);
  let temperature = snapTemperature(target.temperature_c, rules);
  if (index > 0) temperature = Math.max(temperature, points[index - 1].temperature_c + 1);
  if (index < points.length - 1) temperature = Math.min(temperature, points[index + 1].temperature_c - 1);
  const speed = snapSpeed(target.speed_pct, rules);
  return points.map((point, i) => {
    if (i === index) return { temperature_c: temperature, speed_pct: speed };
    if (i > index) return { ...point, speed_pct: Math.max(point.speed_pct, speed) };
    return { ...point, speed_pct: Math.min(point.speed_pct, speed) };
  });
}

/** Whether points can be added and removed: only when the GPU does not fix their number. */
export function canEditPointCount(rules: CurveRules): boolean {
  return rules.points === null;
}

/**
 * `curve` with a new point in the middle of its widest temperature gap (the range ends count),
 * on the line between its neighbours; null when the count is fixed or no whole degree is free.
 */
export function addPoint(curve: readonly CurvePoint[], rules: CurveRules): CurvePoint[] | null {
  if (!canEditPointCount(rules)) return null;
  const points = sorted(curve);
  const { min_c, max_c } = rules.temperature;
  if (points.length === 0) {
    return [{ temperature_c: Math.round((min_c + max_c) / 2), speed_pct: snapSpeed(rules.speed.max_pct, rules) }];
  }
  const edges = [min_c - 1, ...points.map((p) => p.temperature_c), max_c + 1];
  let best = -1;
  let width = 1;
  for (let i = 1; i < edges.length; i++) {
    if (edges[i] - edges[i - 1] > width) {
      width = edges[i] - edges[i - 1];
      best = i;
    }
  }
  if (best < 0) return null;
  const temperature = Math.floor((edges[best - 1] + edges[best]) / 2);
  const speed = snapSpeed(speedAt(points, temperature) ?? rules.speed.min_pct, rules);
  return sorted([...points, { temperature_c: temperature, speed_pct: speed }]);
}

/** `curve` without point `index`; null when the count is fixed or it is the last point. */
export function removePoint(curve: readonly CurvePoint[], index: number, rules: CurveRules): CurvePoint[] | null {
  if (!canEditPointCount(rules) || curve.length <= 1 || index < 0 || index >= curve.length) return null;
  return sorted(curve).filter((_, i) => i !== index);
}

/** The speed the curve asks for at `temperature`: linear between points, flat past the ends. */
export function speedAt(curve: readonly CurvePoint[], temperature: number): number | null {
  if (curve.length === 0 || !Number.isFinite(temperature)) return null;
  const points = sorted(curve);
  if (temperature <= points[0].temperature_c) return points[0].speed_pct;
  const last = points[points.length - 1];
  if (temperature >= last.temperature_c) return last.speed_pct;
  const i = points.findIndex((p) => p.temperature_c >= temperature);
  const [low, high] = [points[i - 1], points[i]];
  const share = (temperature - low.temperature_c) / (high.temperature_c - low.temperature_c);
  return low.speed_pct + (high.speed_pct - low.speed_pct) * share;
}

export function sameCurve(a: readonly CurvePoint[] | null, b: readonly CurvePoint[] | null): boolean {
  if (a === null || b === null) return a === b;
  const [x, y] = [sorted(a), sorted(b)];
  return x.length === y.length && x.every((p, i) => p.temperature_c === y[i].temperature_c && p.speed_pct === y[i].speed_pct);
}

/** The numeric firmware options, in the order the block lists them. */
export const PMFW_NUMBERS = [
  { key: "target_temperature", label: "Target temperature", unit: "°C", autoOnly: true },
  { key: "acoustic_limit", label: "Acoustic limit", unit: "RPM", autoOnly: true },
  { key: "acoustic_target", label: "Acoustic target", unit: "RPM", autoOnly: true },
  { key: "minimum_pwm", label: "Minimum fan speed", unit: "%", autoOnly: true },
  { key: "zero_rpm_threshold", label: "Zero RPM below", unit: "°C", autoOnly: false },
] as const satisfies readonly { key: keyof PmfwStatus; label: string; unit: string; autoOnly: boolean }[];

export type PmfwNumberKey = (typeof PMFW_NUMBERS)[number]["key"];

/** The firmware options a draft can change; a key left out keeps the value in force. */
export type PmfwDraft = { [Key in PmfwNumberKey]?: number } & { zero_rpm?: boolean };

/**
 * Whether firmware option `key` takes effect in `setting`, as `set_fan` enforces it: the four
 * limits only in automatic, zero RPM everywhere except a fixed speed.
 */
export function pmfwApplies(key: PmfwNumberKey | "zero_rpm", setting: FanSetting): boolean {
  if (key === "zero_rpm" || key === "zero_rpm_threshold") return setting !== "static";
  return setting === "auto";
}

/** A firmware value the user can change: exposed, with a range. */
export function pmfwRange(value: PmfwValue | null): { min: number; max: number } | null {
  if (value === null || value.min === null || value.max === null || value.min > value.max) return null;
  return { min: value.min, max: value.max };
}

/** Whether the GPU exposes any firmware option at all (the "Advanced" section has something). */
export function hasPmfw(pmfw: PmfwStatus): boolean {
  return pmfw.zero_rpm !== null || PMFW_NUMBERS.some(({ key }) => pmfw[key] !== null);
}

/** A typed firmware value as the backend takes it: a whole number inside the range; null if not a number. */
export function clampPmfw(value: number, range: { min: number; max: number }): number | null {
  if (!Number.isFinite(value)) return null;
  return Math.min(range.max, Math.max(range.min, Math.round(value)));
}

/** What the user is about to apply. */
export type FanDraft = {
  setting: FanSetting;
  staticSpeed: number | null;
  curve: CurvePoint[] | null;
  pmfw: PmfwDraft;
};

/** The draft the block starts from: what LACT has now, automatic when its mode is unknown. */
export function initialDraft(fan: FanStatus): FanDraft {
  const rules = fan.curve_rules;
  return {
    setting: currentSetting(fan) ?? "auto",
    staticSpeed:
      rules === null ? null : snapSpeed(fan.static_speed ?? (rules.speed.min_pct + rules.speed.max_pct) / 2, rules),
    curve: rules === null ? null : initialCurve(fan.curve, rules),
    pmfw: {},
  };
}

/** The firmware options of `draft` that differ from the GPU's and take effect in its mode. */
function pmfwChanges(draft: FanDraft, pmfw: PmfwStatus): PmfwSettings {
  const changes: PmfwSettings = {};
  if (draft.pmfw.zero_rpm !== undefined && pmfw.zero_rpm !== null && draft.pmfw.zero_rpm !== pmfw.zero_rpm) {
    if (pmfwApplies("zero_rpm", draft.setting)) changes.zero_rpm = draft.pmfw.zero_rpm;
  }
  for (const { key } of PMFW_NUMBERS) {
    const value = draft.pmfw[key];
    const now = pmfw[key];
    if (value === undefined || now === null || pmfwRange(now) === null || value === now.current) continue;
    if (pmfwApplies(key, draft.setting)) changes[key] = value;
  }
  return changes;
}

/** The `set_fan` settings for `draft`, or the sentence that says why it cannot be applied. */
export function fanSettings(draft: FanDraft, fan: FanStatus): { settings: FanSettings } | { problem: string } {
  const rules = fan.curve_rules;
  const pmfw = pmfwChanges(draft, fan.pmfw);
  const withPmfw = Object.keys(pmfw).length > 0 ? { pmfw } : {};
  switch (draft.setting) {
    case "auto":
      return { settings: { mode: "auto", ...withPmfw } };
    case "static": {
      if (rules === null || draft.staticSpeed === null) return { problem: "This GPU cannot hold a fixed fan speed." };
      const { min_pct, max_pct } = rules.speed;
      if (draft.staticSpeed < min_pct || draft.staticSpeed > max_pct) {
        return { problem: `The fan speed must be between ${min_pct} % and ${max_pct} %.` };
      }
      return { settings: { mode: "static", static_speed: draft.staticSpeed, ...withPmfw } };
    }
    case "curve": {
      if (rules === null || draft.curve === null) return { problem: "This GPU cannot follow a fan curve." };
      const problem = curveProblem(draft.curve, rules);
      if (problem !== null) return { problem };
      return { settings: { mode: "curve", curve: sorted(draft.curve), ...withPmfw } };
    }
  }
}

/** The draft would change something: another mode, speed or curve, or a firmware option. */
export function draftChanged(draft: FanDraft, fan: FanStatus): boolean {
  if (Object.keys(pmfwChanges(draft, fan.pmfw)).length > 0) return true;
  const now = currentSetting(fan);
  if (draft.setting !== now) return true;
  if (draft.setting === "static") return draft.staticSpeed !== fan.static_speed;
  if (draft.setting === "curve") return !sameCurve(draft.curve, fan.curve);
  return false;
}

export function percent(value: number | null | undefined): string {
  return formatValue(value, "%", value != null && !Number.isInteger(value) ? 1 : 0);
}

/** What the fan does now, as the block's headline: "Automatic", "Fixed 40 %", "Curve". */
export function describeFan(fan: FanStatus): string {
  const setting = currentSetting(fan);
  if (setting === null) return "Controlled by LACT";
  if (setting === "static") return `Fixed ${percent(fan.static_speed)}`;
  return settingLabel(setting);
}

function describeSettings(settings: FanSettings): string {
  if (settings.mode === "static") return `Fixed ${percent(settings.static_speed)}`;
  if (settings.mode === "curve") return `Curve (${settings.curve?.length ?? 0} points)`;
  return "Automatic";
}

/** What the confirmation panel says changed: "Automatic → Fixed 40 %; zero RPM off". */
export function fanDetail(fan: FanStatus, settings: FanSettings): string {
  const parts = [`${describeFan(fan)} → ${describeSettings(settings)}`];
  const pmfw = settings.pmfw ?? {};
  if (pmfw.zero_rpm !== undefined) parts.push(`zero RPM ${pmfw.zero_rpm ? "on" : "off"}`);
  for (const { key, label, unit } of PMFW_NUMBERS) {
    const value = pmfw[key];
    if (value !== undefined) parts.push(`${label.toLowerCase()} ${formatValue(fan.pmfw[key]?.current, unit)} → ${formatValue(value, unit)}`);
  }
  return parts.join("; ");
}

/**
 * Which temperature the curve follows: the hotspot (junction) for a firmware curve, whose
 * `fan_curve` is keyed on it; the edge sensor for LACT's own curve, its default key.
 */
export function curveSensor(rules: CurveRules): "temp_junction_c" | "temp_edge_c" {
  return rules.whole_percent ? "temp_junction_c" : "temp_edge_c";
}

/** Evenly spaced round ticks covering `[min, max]`, about `count` of them. */
export function ticks(min: number, max: number, count: number): number[] {
  if (!(max > min) || count < 1) return [min];
  const raw = (max - min) / count;
  const magnitude = 10 ** Math.floor(Math.log10(raw));
  const step = [1, 2, 2.5, 5, 10].map((m) => m * magnitude).find((s) => s >= raw) ?? raw;
  const out: number[] = [];
  for (let v = Math.ceil(min / step) * step; v <= max + 1e-9; v += step) out.push(Math.round(v * 1e6) / 1e6);
  return out;
}
