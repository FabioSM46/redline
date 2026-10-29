// Tuning types returned by the backend and the pure helpers of Performance → Tuning.
// Tested in tuning.test.ts, which also checks these mirrors against src-tauri/src/tuning.rs.

import { formatValue } from "./format.ts";

/** Mirrors `tuning::PowerLimit`. A value LACT does not report is null, never 0. */
export type PowerLimit = {
  current_w: number | null;
  min_w: number | null;
  max_w: number | null;
  default_w: number | null;
};

/**
 * What a pending change touches. Mirrors `tuning::TuningChange`; `fan` is the fan setter of #55,
 * named ahead so the panel already has a label for it.
 */
export const TUNING_CHANGES = ["power_limit", "fan"] as const;
export type TuningChange = (typeof TUNING_CHANGES)[number];

/** Mirrors `tuning::PendingChange`: the payload of `tuning-pending` and `TuningStatus.pending`. */
export type PendingChange = {
  gpu_id: string;
  what: TuningChange;
  /** When lactd reverts the change by itself, in milliseconds since the Unix epoch. */
  deadline_ms: number;
};

/** Mirrors `tuning::Outcome`. */
export type Outcome = "confirmed" | "reverted" | "expired";

/** Mirrors `tuning::Settled`: the payload of `tuning-settled`. */
export type Settled = { gpu_id: string; outcome: Outcome };

/**
 * Mirrors `tuning::TuningStatus` (`tuning_status`). `fan` is typed loosely on purpose: the fan
 * editor (#60) owns its shape.
 */
export type TuningStatus = {
  available: boolean;
  unavailable_reason: string | null;
  warning: string | null;
  lact_version: string | null;
  overdrive_enabled: boolean | null;
  reboot_pending: boolean;
  power: PowerLimit | null;
  fan: unknown;
  profile: string | null;
  pending: PendingChange | null;
};

/** Mirrors `tuning::overdrive::OverdrivePlan` (`prepare_enable_overdrive`). */
export type OverdrivePlan = {
  action: "enable" | "disable";
  steps: string[];
  reboot_required: boolean;
  token: string;
  expires_ms: number;
};

/** Mirrors `tuning::overdrive::OverdriveResult` (`enable_overdrive`). */
export type OverdriveResult = { reboot_required: boolean };

export const PENDING_EVENT = "tuning-pending";
export const SETTLED_EVENT = "tuning-settled";

/** The power limit slider moves in whole watts. */
export const POWER_STEP_W = 1;

/** The range the power limit may take, or null when LACT does not report both ends. */
export function powerRange(power: PowerLimit | null): { min: number; max: number } | null {
  if (power === null || power.min_w === null || power.max_w === null) return null;
  if (!Number.isFinite(power.min_w) || !Number.isFinite(power.max_w) || power.min_w > power.max_w) return null;
  return { min: power.min_w, max: power.max_w };
}

/**
 * A typed or dragged value as watts the backend accepts: rounded to the step and kept inside
 * `[min, max]`. Null for something that is not a number (an empty field).
 */
export function clampWatts(value: number, range: { min: number; max: number }): number | null {
  if (!Number.isFinite(value)) return null;
  const rounded = Math.round(value / POWER_STEP_W) * POWER_STEP_W;
  return Math.min(range.max, Math.max(range.min, rounded));
}

/** The draft differs from the limit in force: there is something to apply. */
export function canApply(draft: number | null, power: PowerLimit | null): boolean {
  const range = powerRange(power);
  if (draft === null || range === null) return false;
  if (draft < range.min || draft > range.max) return false;
  return power?.current_w == null || Math.abs(draft - power.current_w) >= POWER_STEP_W / 2;
}

/** "Default" makes sense when the default is unknown (LACT resets it) or differs from now. */
export function canReset(power: PowerLimit | null): boolean {
  if (power === null) return false;
  if (power.default_w === null || power.current_w === null) return true;
  return Math.abs(power.default_w - power.current_w) >= POWER_STEP_W / 2;
}

/** Whole seconds left before `deadlineMs`, rounded up; 0 once it has passed. */
export function secondsLeft(deadlineMs: number, nowMs: number): number {
  return Math.max(0, Math.ceil((deadlineMs - nowMs) / 1000));
}

/** "4 s" for the countdown of the confirmation panel. */
export function formatCountdown(seconds: number): string {
  return `${seconds} s`;
}

const CHANGE_LABELS: { [What in TuningChange]: string } = {
  power_limit: "Power limit",
  fan: "Fan",
};

/** The name of a change, as the confirmation panel says it. A kind a newer backend adds is generic. */
export function changeLabel(what: TuningChange): string {
  return CHANGE_LABELS[what] ?? "Tuning change";
}

/** What the page says once a change has settled. */
export function outcomeSentence(settled: Settled): string {
  switch (settled.outcome) {
    case "confirmed":
      return "The change was kept and saved.";
    case "reverted":
      return "The change was reverted: the previous values are back.";
    case "expired":
      return "Nobody confirmed the change in time, so LACT reverted it: the previous values are back.";
  }
}

/** One watts value for display, `—` when unknown. */
export function watts(value: number | null | undefined): string {
  return formatValue(value, "W");
}

/** The pending change the page knows about, and the last one that settled. */
export type PendingState = {
  pending: PendingChange | null;
  /** The last change that settled while the page watched it, with the change itself. */
  settled: { change: PendingChange; result: Settled } | null;
};

export const NO_PENDING: PendingState = { pending: null, settled: null };

export type PendingAction =
  /** `tuning-pending`, or the reply of a setter: lactd holds this change now. */
  | { type: "pending"; change: PendingChange }
  /** `tuning-settled`. */
  | { type: "settled"; result: Settled }
  /** A `tuning_status` reply: a change pending on that GPU, read now. */
  | { type: "status"; change: PendingChange; nowMs: number }
  /** A late confirm or revert reply that says how it ended, when the event was missed. */
  | { type: "answered"; change: PendingChange; result: Settled }
  /**
   * A `tuning_status` read after the deadline no longer holds `change`: its `tuning-settled` was
   * missed. Past the deadline lactd has reverted it by itself, so it ended as `expired`.
   */
  | { type: "gone"; change: PendingChange; nowMs: number };

function sameChange(a: PendingChange, b: PendingChange): boolean {
  return a.gpu_id === b.gpu_id && a.what === b.what && a.deadline_ms === b.deadline_ms;
}

/**
 * Keeps the one change lactd holds (one for the whole daemon, any GPU). Events and replies can
 * arrive in any order and more than once: each change settles once, and a status reply read
 * before the settle never brings a settled change back.
 */
export function pendingReducer(state: PendingState, action: PendingAction): PendingState {
  switch (action.type) {
    case "pending":
      if (state.pending !== null && sameChange(state.pending, action.change)) return state;
      if (state.settled !== null && sameChange(state.settled.change, action.change)) return state;
      return { pending: action.change, settled: null };
    case "status":
      if (state.pending !== null) return state;
      if (action.change.deadline_ms <= action.nowMs) return state;
      if (state.settled !== null && sameChange(state.settled.change, action.change)) return state;
      return { pending: action.change, settled: null };
    case "settled":
      if (state.pending === null || state.pending.gpu_id !== action.result.gpu_id) return state;
      return { pending: null, settled: { change: state.pending, result: action.result } };
    case "answered":
      if (state.pending === null || !sameChange(state.pending, action.change)) return state;
      return { pending: null, settled: { change: action.change, result: action.result } };
    case "gone":
      if (state.pending === null || !sameChange(state.pending, action.change)) return state;
      if (action.nowMs < action.change.deadline_ms) return state;
      return {
        pending: null,
        settled: { change: action.change, result: { gpu_id: action.change.gpu_id, outcome: "expired" } },
      };
  }
}

/** How long after the deadline the panel waits for `tuning-settled` before asking the backend. */
export const SETTLE_GRACE_MS = 2000;

/** Whether `status` (read for `change.gpu_id`) still holds `change`. */
export function stillPending(status: { pending: PendingChange | null }, change: PendingChange): boolean {
  return status.pending !== null && sameChange(status.pending, change);
}

/** LACT cannot be reached at all (not installed, not running, no permission): see Dependencies. */
export function lactUnreachable(status: TuningStatus): boolean {
  return !status.available && status.lact_version === null;
}

/** What the overdrive block says, from the state LACT reports. */
export type OverdriveView = "enabled" | "reboot" | "disabled" | "unknown";

export function overdriveView(status: TuningStatus): OverdriveView {
  if (status.reboot_pending) return "reboot";
  if (status.overdrive_enabled === true) return "enabled";
  if (status.overdrive_enabled === false) return "disabled";
  return "unknown";
}

/** An overdrive plan's token is accepted until `expires_ms`. */
export function planExpired(plan: OverdrivePlan, nowMs: number): boolean {
  return nowMs >= plan.expires_ms;
}
