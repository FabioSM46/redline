import { invoke } from "@tauri-apps/api/core";
import { useState } from "react";
import { useGpu } from "../../../GpuContext.tsx";
import { FanCurveChart } from "../../../charts/FanCurveChart.tsx";
import {
  addPoint,
  availabilitySentence,
  canEditPointCount,
  clampPmfw,
  curveSensor,
  describeFan,
  draftChanged,
  type FanDraft,
  fanAvailability,
  fanDetail,
  fanSettings,
  hasPmfw,
  initialCurve,
  initialDraft,
  movePoint,
  offeredSettings,
  PMFW_NUMBERS,
  pmfwApplies,
  pmfwRange,
  removePoint,
  settingLabel,
  snapSpeed,
} from "../../../fan.ts";
import { formatValue } from "../../../format.ts";
import type { CurvePoint, CurveRules, FanStatus, PendingChange } from "../../../tuning.ts";
import { OVERDRIVE_ANCHOR, type TuningBlockProps } from "./block.ts";

/**
 * The fan: automatic, a fixed speed or a curve, and the firmware options behind "Advanced".
 * Everything is a draft until Apply, which sends one `set_fan` through the confirmation panel.
 * Remount it (key) when the fan in force changes, so the draft starts from the new state.
 */
export function FanBlock({ gpuId, status, fan, locked, apply }: TuningBlockProps & { fan: FanStatus }) {
  const { reading } = useGpu();
  const [draft, setDraft] = useState<FanDraft>(() => initialDraft(fan));
  const [error, setError] = useState<string | null>(null);
  const rules = fan.curve_rules;
  const availability = fanAvailability(fan, status);
  const sentence = availabilitySentence(availability);
  const offered = offeredSettings(rules);
  const snapshot = reading.status === "ok" ? reading.snapshot : null;
  const rpm = snapshot?.fan_rpm ?? null;
  const showPmfw = hasPmfw(fan.pmfw);
  // Something to send: a mode other than automatic, firmware options, or LACT to stop driving the fan.
  const canSend = rules !== null || showPmfw || fan.control_enabled;

  const result = fanSettings(draft, fan);
  const changed = draftChanged(draft, fan);
  const problem = "problem" in result ? result.problem : null;

  const send = () => {
    if (!("settings" in result)) return;
    const settings = result.settings;
    setError(null);
    apply(() => invoke<PendingChange>("set_fan", { gpuId, settings }), fanDetail(fan, settings)).then(setError);
  };

  const update = (change: Partial<FanDraft>) => setDraft((d) => ({ ...d, ...change }));

  return (
    <div className="tuning-block">
      <h2 className="record-title">Fan</h2>
      <div className="setting setting-wide">
        <div className="setting-text">
          <span className="tuning-figure tuning-number">{describeFan(fan)}</span>
          <span className="setting-description tuning-number">Now {formatValue(rpm, "RPM")}</span>
          {sentence !== null && <span className="setting-description">{sentence}</span>}
          {(availability === "overdrive" || availability === "reboot") && (
            <div className="tuning-actions">
              <button
                type="button"
                className="button"
                onClick={() => {
                  const target = document.getElementById(OVERDRIVE_ANCHOR);
                  target?.scrollIntoView({ behavior: "smooth", block: "start" });
                  target?.focus({ preventScroll: true });
                }}
              >
                About overdrive
              </button>
            </div>
          )}
          {error !== null && (
            <span className="setting-error" role="alert">
              Not applied: {error}
            </span>
          )}
        </div>
        {canSend && (
          <div className="setting-control fan-controls">
            {offered.length > 1 && (
              <div className="segmented" role="radiogroup" aria-label="Fan control">
                {offered.map((setting) => (
                  <label className="segment" key={setting}>
                    <input
                      type="radio"
                      name={`fan-${gpuId}`}
                      value={setting}
                      checked={draft.setting === setting}
                      disabled={locked}
                      onChange={() => update({ setting })}
                    />
                    <span>{settingLabel(setting)}</span>
                  </label>
                ))}
              </div>
            )}
            {draft.setting === "static" && rules !== null && draft.staticSpeed !== null && (
              <FixedSpeed
                rules={rules}
                speed={draft.staticSpeed}
                locked={locked}
                onChange={(staticSpeed) => update({ staticSpeed })}
              />
            )}
            {draft.setting === "curve" && rules !== null && draft.curve !== null && (
              <CurveEditor
                rules={rules}
                curve={draft.curve}
                stored={fan.curve}
                locked={locked}
                temperature={snapshot?.[curveSensor(rules)] ?? null}
                rpm={rpm}
                onChange={(curve) => update({ curve })}
              />
            )}
            {showPmfw && <Advanced fan={fan} draft={draft} locked={locked} onChange={(pmfw) => update({ pmfw })} />}
            {problem !== null && changed && (
              <span className="setting-error" role="alert">
                {problem}
              </span>
            )}
            <div className="tuning-actions">
              <button
                type="button"
                className="button"
                disabled={locked || !changed || problem !== null}
                onClick={send}
              >
                {offered.length > 1 ? "Apply" : "Back to automatic"}
              </button>
              {offered.length > 1 && (
              <button
                type="button"
                className="button"
                disabled={locked || !changed}
                onClick={() => {
                  setDraft(initialDraft(fan));
                  setError(null);
                }}
              >
                Discard
              </button>
              )}
            </div>
          </div>
        )}
      </div>
    </div>
  );
}

function FixedSpeed({
  rules,
  speed,
  locked,
  onChange,
}: {
  rules: CurveRules;
  speed: number;
  locked: boolean;
  onChange: (speed: number) => void;
}) {
  return (
    <div className="tuning-power">
      <input
        type="range"
        className="tuning-slider"
        aria-label="Fixed fan speed in percent"
        min={rules.speed.min_pct}
        max={rules.speed.max_pct}
        step={1}
        value={speed}
        disabled={locked}
        onChange={(e) => onChange(snapSpeed(Number(e.target.value), rules))}
      />
      <NumberField
        label="Fixed fan speed"
        unit="%"
        value={speed}
        min={rules.speed.min_pct}
        max={rules.speed.max_pct}
        disabled={locked}
        onCommit={(value) => onChange(snapSpeed(value, rules))}
      />
    </div>
  );
}

function CurveEditor({
  rules,
  curve,
  stored,
  locked,
  temperature,
  rpm,
  onChange,
}: {
  rules: CurveRules;
  curve: CurvePoint[];
  stored: CurvePoint[] | null;
  locked: boolean;
  temperature: number | null;
  rpm: number | null;
  onChange: (curve: CurvePoint[]) => void;
}) {
  const editCount = canEditPointCount(rules);
  const added = editCount ? addPoint(curve, rules) : null;
  const { min_c, max_c } = rules.temperature;
  const { min_pct, max_pct } = rules.speed;
  return (
    <div className="fan-curve-editor">
      <FanCurveChart
        curve={curve}
        rules={rules}
        disabled={locked}
        onChange={onChange}
        temperature={temperature}
        rpm={rpm}
      />
      <p className="setting-description">
        {rules.points !== null ? `${rules.points} points, ` : ""}
        <span className="tuning-number">
          {min_c} °C to {max_c} °C, {min_pct} % to {max_pct} %
        </span>
        {rules.whole_percent ? " in whole percents" : ""}. Drag a point or focus it and use the arrow keys (Shift for
        steps of 5); the fan never slows down as the temperature rises.
      </p>
      <table className="fan-points">
        <thead>
          <tr>
            <th scope="col">Point</th>
            <th scope="col">Temperature</th>
            <th scope="col">Speed</th>
            {editCount && <th scope="col"><span className="visually-hidden">Remove</span></th>}
          </tr>
        </thead>
        <tbody>
          {curve.map((point, i) => (
            // The order of the points never changes while editing: the index is stable.
            <tr key={i}>
              <td className="tuning-number">{i + 1}</td>
              <td>
                <NumberField
                  label={`Point ${i + 1} temperature`}
                  unit="°C"
                  value={point.temperature_c}
                  min={min_c}
                  max={max_c}
                  disabled={locked}
                  onCommit={(value) => onChange(movePoint(curve, i, { ...point, temperature_c: value }, rules))}
                />
              </td>
              <td>
                <NumberField
                  label={`Point ${i + 1} speed`}
                  unit="%"
                  value={point.speed_pct}
                  min={min_pct}
                  max={max_pct}
                  disabled={locked}
                  onCommit={(value) => onChange(movePoint(curve, i, { ...point, speed_pct: value }, rules))}
                />
              </td>
              {editCount && (
                <td>
                  <button
                    type="button"
                    className="button"
                    aria-label={`Remove point ${i + 1}`}
                    disabled={locked || curve.length <= 1}
                    onClick={() => {
                      const next = removePoint(curve, i, rules);
                      if (next !== null) onChange(next);
                    }}
                  >
                    Remove
                  </button>
                </td>
              )}
            </tr>
          ))}
        </tbody>
      </table>
      <div className="tuning-actions">
        {editCount && (
          <button type="button" className="button" disabled={locked || added === null} onClick={() => added && onChange(added)}>
            Add point
          </button>
        )}
        {stored !== null && (
          <button type="button" className="button" disabled={locked} onClick={() => onChange(initialCurve(stored, rules))}>
            Stored curve
          </button>
        )}
      </div>
    </div>
  );
}

/** The firmware fan options the GPU exposes; each disabled, with the reason, in a mode that ignores it. */
function Advanced({
  fan,
  draft,
  locked,
  onChange,
}: {
  fan: FanStatus;
  draft: FanDraft;
  locked: boolean;
  onChange: (pmfw: FanDraft["pmfw"]) => void;
}) {
  const { pmfw } = fan;
  const mode = settingLabel(draft.setting).toLowerCase();
  return (
    <details className="settings-advanced fan-advanced">
      <summary>Advanced</summary>
      <div className="fan-options">
        {pmfw.zero_rpm !== null && (
          <label className="fan-option">
            <span className="fan-option-text">
              <span>Zero RPM</span>
              <span className="setting-description">
                {pmfwApplies("zero_rpm", draft.setting)
                  ? "The fan stops while the GPU is cool."
                  : `Does not apply to a ${mode} speed: LACT turns it off.`}
              </span>
            </span>
            <input
              type="checkbox"
              role="switch"
              className="switch"
              checked={draft.pmfw.zero_rpm ?? pmfw.zero_rpm}
              disabled={locked || !pmfwApplies("zero_rpm", draft.setting)}
              onChange={(e) => onChange({ ...draft.pmfw, zero_rpm: e.target.checked })}
            />
          </label>
        )}
        {PMFW_NUMBERS.map(({ key, label, unit, autoOnly }) => {
          const value = pmfw[key];
          if (value === null) return null;
          const range = pmfwRange(value);
          const applies = pmfwApplies(key, draft.setting);
          return (
            <div className="fan-option" key={key}>
              <span className="fan-option-text">
                <span>{label}</span>
                <span className="setting-description tuning-number">
                  {range === null
                    ? `${formatValue(value.current, unit)}, fixed by the firmware.`
                    : !applies
                      ? autoOnly
                        ? "Applies only to the automatic control."
                        : `Does not apply to a ${mode} speed.`
                      : `${formatValue(range.min, unit)} to ${formatValue(range.max, unit)}`}
                </span>
              </span>
              {range !== null && (
                <NumberField
                  label={label}
                  unit={unit}
                  value={draft.pmfw[key] ?? value.current}
                  min={range.min}
                  max={range.max}
                  disabled={locked || !applies}
                  onCommit={(typed) => {
                    const clamped = clampPmfw(typed, range);
                    if (clamped !== null) onChange({ ...draft.pmfw, [key]: clamped });
                  }}
                />
              )}
            </div>
          );
        })}
      </div>
    </details>
  );
}

/**
 * A number typed freely and committed on Enter or when the field loses focus, so an
 * intermediate value ("4" on the way to "45") is never clamped while typing.
 */
function NumberField({
  label,
  unit,
  value,
  min,
  max,
  disabled,
  onCommit,
}: {
  label: string;
  unit: string;
  value: number;
  min: number;
  max: number;
  disabled: boolean;
  onCommit: (value: number) => void;
}) {
  const [text, setText] = useState<string | null>(null);
  const commit = () => {
    if (text === null) return;
    const typed = text.trim() === "" ? Number.NaN : Number(text);
    if (Number.isFinite(typed)) onCommit(typed);
    setText(null);
  };
  const shown = text ?? String(value);
  const typed = Number(shown);
  const invalid = shown.trim() === "" || !Number.isFinite(typed) || typed < min || typed > max;
  return (
    <label className="tuning-watts">
      <input
        type="number"
        className="setting-input tuning-number"
        aria-label={label}
        min={min}
        max={max}
        step={1}
        value={shown}
        disabled={disabled}
        aria-invalid={invalid || undefined}
        onChange={(e) => setText(e.target.value)}
        onBlur={commit}
        onKeyDown={(e) => {
          if (e.key === "Enter") commit();
          if (e.key === "Escape") setText(null);
        }}
      />
      <span>{unit}</span>
    </label>
  );
}
