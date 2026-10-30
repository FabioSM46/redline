// The pure helpers of the Clocks and voltage block (Performance → Tuning, D13). Tested in
// clocks.test.ts, which also checks the mirrored constants against src-tauri/src/tuning/clocks.rs.
// The backend checks every value again: these checks only say why before anything is sent.

import { formatValue } from "./format.ts";
import type { ClockControl, ClocksStatus, TuningStatus } from "./tuning.ts";

/** The keys of `set_clocks`, in the order the block shows them. */
export const CLOCK_KEYS = ["max_core_clock", "core_clock_offset", "max_vram_clock", "voltage_offset"] as const;
export type ClockKey = (typeof CLOCK_KEYS)[number];

export const CLOCK_LABELS: { [Key in ClockKey]: string } = {
  max_core_clock: "Maximum core clock",
  core_clock_offset: "Core clock offset",
  max_vram_clock: "Maximum VRAM clock",
  voltage_offset: "Voltage offset",
};

/** Mirrors `tuning::clocks::NEEDS_OVERDRIVE`: the one reason that points to the Overdrive section. */
export const NEEDS_OVERDRIVE =
  "Clock and voltage controls need overdrive. Enable overdrive (the `amdgpu.ppfeaturemask` kernel parameter) and reboot.";

/** Mirrors `tuning::clocks::UNDERVOLT_ONLY`. */
export const UNDERVOLT_ONLY = "Redline only undervolts: the voltage offset must be 0 mV or lower.";

/** Mirrors `tuning::clocks::VOLTAGE_FLOOR_MV`. A limit against typos, never a safe value (D13). */
export const VOLTAGE_FLOOR_MV = -450;

/** What the block shows, from the status. */
export type ClocksView =
  /** The controls this GPU lists. */
  | { kind: "controls"; clocks: ClocksStatus }
  /** Overdrive is off: `reason`, and a way to the Overdrive section. */
  | { kind: "overdrive"; reason: string }
  /** An overdrive change waits for a reboot: `unavailable_reason` would ask for what is done. */
  | { kind: "reboot" }
  /** An unsupported family: `reason`. */
  | { kind: "unsupported"; reason: string }
  /** LACT answered nothing about the controls. */
  | { kind: "unreported" };

function hasControls(clocks: ClocksStatus): boolean {
  return CLOCK_KEYS.some((key) => clocks[key] !== null);
}

export function clocksView(status: Pick<TuningStatus, "clocks" | "reboot_pending">): ClocksView {
  const { clocks, reboot_pending: rebootPending } = status;
  const reason = clocks?.unavailable_reason ?? null;
  if (reason !== null && reason !== NEEDS_OVERDRIVE) return { kind: "unsupported", reason };
  if (clocks !== null && reason === null && hasControls(clocks)) return { kind: "controls", clocks };
  if (rebootPending) return { kind: "reboot" };
  return reason === null ? { kind: "unreported" } : { kind: "overdrive", reason };
}

/** The controls a GPU lists, in block order. */
export function listedControls(clocks: ClocksStatus): { key: ClockKey; control: ClockControl }[] {
  return CLOCK_KEYS.flatMap((key) => {
    const control = clocks[key];
    return control === null ? [] : [{ key, control }];
  });
}

/** The firmware's range, or null when it gives none (or one that cannot be used). */
export function firmwareRange(control: ClockControl): { min: number; max: number } | null {
  if (control.min === null || control.max === null || control.min > control.max) return null;
  return { min: control.min, max: control.max };
}

/**
 * The voltage offset is the one control the backend accepts without a firmware range.
 * `ClockControl` reports both bounds or neither, so one missing bound means there is no range.
 */
export function hasFloorOnly(key: ClockKey, control: ClockControl): boolean {
  return key === "voltage_offset" && (control.min === null || control.max === null);
}

/** What the backend accepts for this control, or null when it refuses every value (read-only). */
export function clockLimits(key: ClockKey, control: ClockControl): { min: number; max: number } | null {
  if (hasFloorOnly(key, control)) return { min: VOLTAGE_FLOOR_MV, max: 0 };
  const range = firmwareRange(control);
  if (range === null || key !== "voltage_offset") return range;
  return range.min > 0 ? null : { min: range.min, max: Math.min(range.max, 0) };
}

export function clockUnit(key: ClockKey): "MHz" | "mV" {
  return key === "voltage_offset" ? "mV" : "MHz";
}

/** One value for display, `—` when unknown, and always with its unit. */
export function clockValue(key: ClockKey, value: number | null | undefined): string {
  return formatValue(value, clockUnit(key));
}

/** What the user typed, by control. A control not edited has no entry. */
export type ClockDraft = { [Key in ClockKey]?: string };

export type ClockRequest = {
  /** Only what changes: a key left out stays as it is, whatever its value. */
  values: { [Key in ClockKey]?: number };
  /** Why a typed value cannot be sent, as a sentence per control. */
  errors: { [Key in ClockKey]?: string };
  /** What the confirmation panel says changed. */
  detail: string;
};

function refusal(key: ClockKey, text: string, limits: { min: number; max: number }): string | null {
  const unit = clockUnit(key);
  const value = text.trim() === "" ? Number.NaN : Number(text);
  if (!Number.isInteger(value)) return `Enter a whole number of ${unit}.`;
  if (key === "voltage_offset" && value > 0) return UNDERVOLT_ONLY;
  if (value < limits.min || value > limits.max) return `Must be between ${limits.min} ${unit} and ${limits.max} ${unit}.`;
  return null;
}

/**
 * The `set_clocks` request a draft stands for. A value equal to the one in force is left out, even
 * if it is outside the range today: nothing clamps a typed value, because a number other than the
 * one the user wrote must never reach the GPU.
 */
export function clockRequest(clocks: ClocksStatus, draft: ClockDraft): ClockRequest {
  const request: ClockRequest = { values: {}, errors: {}, detail: "" };
  const lines: string[] = [];
  for (const { key, control } of listedControls(clocks)) {
    const text = draft[key];
    const limits = clockLimits(key, control);
    if (text === undefined || limits === null || (text.trim() !== "" && Number(text) === control.current)) continue;
    const error = refusal(key, text, limits);
    if (error !== null) {
      request.errors[key] = error;
      continue;
    }
    request.values[key] = Number(text);
    lines.push(`${CLOCK_LABELS[key]} ${clockValue(key, control.current)} → ${clockValue(key, Number(text))}`);
  }
  request.detail = lines.join(", ");
  return request;
}

/** There is something to send, and everything typed can be sent. */
export function canApplyClocks(request: ClockRequest): boolean {
  return Object.keys(request.values).length > 0 && Object.keys(request.errors).length === 0;
}
