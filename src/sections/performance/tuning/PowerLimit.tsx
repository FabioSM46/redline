import { invoke } from "@tauri-apps/api/core";
import { useState } from "react";
import {
  canApply,
  canReset,
  clampWatts,
  type PendingChange,
  type PowerLimit as Power,
  POWER_STEP_W,
  powerRange,
  watts,
} from "../../../tuning.ts";
import type { TuningBlockProps } from "./block.ts";

/**
 * The power cap: slider and number between LACT's min and max, Apply and Default. Every change
 * goes through the confirmation panel. Remount it (key) when the limit in force changes, so the
 * draft starts from the new value.
 */
export function PowerLimitBlock({ gpuId, power, locked, apply }: TuningBlockProps & { power: Power }) {
  const range = powerRange(power);
  const [draft, setDraft] = useState<number | null>(power.current_w ?? power.default_w ?? null);
  const [text, setText] = useState(draft === null ? "" : String(draft));
  const [error, setError] = useState<string | null>(null);

  const send = (value: number | null, detail: string) => {
    setError(null);
    apply(() => invoke<PendingChange>("set_power_limit", { gpuId, watts: value }), detail).then(setError);
  };

  const pick = (value: number) => {
    if (range === null) return;
    const clamped = clampWatts(value, range);
    setDraft(clamped);
    setText(clamped === null ? "" : String(clamped));
  };

  return (
    <div className="tuning-block">
      <h2 className="record-title">Power limit</h2>
      <div className="setting setting-wide">
        <div className="setting-text">
          <span className="tuning-figure tuning-number">{watts(power.current_w)}</span>
          <span className="setting-description">
            {range === null ? (
              "This GPU does not report the range of its power limit: it can only be reset to its default."
            ) : (
              <>
                Range <span className="tuning-number">{watts(range.min)}</span> to{" "}
                <span className="tuning-number">{watts(range.max)}</span>, default{" "}
                <span className="tuning-number">{watts(power.default_w)}</span>
              </>
            )}
          </span>
          {error !== null && (
            <span className="setting-error" role="alert">
              Not applied: {error}
            </span>
          )}
        </div>
        <div className="setting-control tuning-power">
          {range !== null && (
            <>
              <input
                type="range"
                className="tuning-slider"
                aria-label="Power limit in watts"
                min={range.min}
                max={range.max}
                step={POWER_STEP_W}
                value={draft ?? range.min}
                disabled={locked}
                onChange={(e) => pick(Number(e.target.value))}
              />
              <label className="tuning-watts">
                <input
                  type="number"
                  className="setting-input tuning-number"
                  aria-label="Power limit"
                  min={range.min}
                  max={range.max}
                  step={POWER_STEP_W}
                  value={text}
                  disabled={locked}
                  aria-invalid={draft === null || undefined}
                  onChange={(e) => {
                    setText(e.target.value);
                    const value = e.target.value.trim() === "" ? Number.NaN : Number(e.target.value);
                    setDraft(Number.isFinite(value) && value >= range.min && value <= range.max ? value : null);
                  }}
                  onBlur={() => draft === null && text.trim() !== "" && pick(Number(text))}
                />
                <span>W</span>
              </label>
            </>
          )}
          <div className="tuning-actions">
            {range !== null && (
              <button
                type="button"
                className="button"
                disabled={locked || !canApply(draft, power)}
                onClick={() => draft !== null && send(draft, `${watts(power.current_w)} → ${watts(draft)}`)}
              >
                Apply
              </button>
            )}
            <button
              type="button"
              className="button"
              disabled={locked || !canReset(power)}
              onClick={() => send(null, `${watts(power.current_w)} → default (${watts(power.default_w)})`)}
            >
              Default
            </button>
          </div>
        </div>
      </div>
    </div>
  );
}
