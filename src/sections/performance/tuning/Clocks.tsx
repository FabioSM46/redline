import { invoke } from "@tauri-apps/api/core";
import { useState } from "react";
import {
  canApplyClocks,
  CLOCK_LABELS,
  type ClockDraft,
  type ClockKey,
  clockLimits,
  clockRequest,
  clocksView,
  clockUnit,
  clockValue,
  firmwareRange,
  hasFloorOnly,
  listedControls,
  VOLTAGE_FLOOR_MV,
} from "../../../clocks.ts";
import type { ClockControl, ClocksStatus, PendingChange } from "../../../tuning.ts";
import { OVERDRIVE_ANCHOR, type TuningBlockProps } from "./block.ts";

/**
 * Clocks and voltage (D13): the controls `tuning_status.clocks` lists, drafts until Apply, which
 * sends one `set_clocks` through the confirmation panel. The reset sits outside every state on
 * purpose: a confirmed value comes back at every start of lactd, and the reset needs neither
 * overdrive nor a table. Remount it (key) when the values in force change.
 */
export function ClocksBlock({ gpuId, status, locked, apply }: TuningBlockProps) {
  const view = clocksView(status);
  const [error, setError] = useState<string | null>(null);

  const send = (command: () => Promise<PendingChange>, detail: string) => {
    setError(null);
    apply(command, detail).then(setError);
  };

  return (
    <div className="tuning-block tuning-clocks">
      <h2 className="record-title">Clocks and voltage</h2>
      {view.kind === "controls" && <Controls gpuId={gpuId} clocks={view.clocks} locked={locked} send={send} />}
      {view.kind === "overdrive" && (
        <div className="settings-notice tuning-unavailable" role="status">
          <p>{view.reason}</p>
          <div className="tuning-actions">
            <button type="button" className="button" onClick={showOverdrive}>
              About overdrive
            </button>
          </div>
        </div>
      )}
      {view.kind === "reboot" && (
        <p className="settings-notice" role="status">
          An overdrive change is waiting for a reboot. Redline reads the controls of this GPU again after it.
        </p>
      )}
      {view.kind === "unsupported" && (
        <p className="settings-notice" role="status">
          {view.reason}
        </p>
      )}
      {view.kind === "unreported" && (
        <p className="settings-notice" role="status">
          LACT did not report the clock and voltage controls of this GPU.
        </p>
      )}
      {error !== null && (
        <p className="setting-error" role="alert">
          Not applied: {error}
        </p>
      )}
      <Reset
        locked={locked}
        onReset={() => send(() => invoke<PendingChange>("reset_clocks", { gpuId }), "back to factory values")}
      />
    </div>
  );
}

/** The same way to the Overdrive section the fan block gives. */
function showOverdrive() {
  const target = document.getElementById(OVERDRIVE_ANCHOR);
  target?.scrollIntoView({ behavior: "smooth", block: "start" });
  target?.focus({ preventScroll: true });
}

function Controls({
  gpuId,
  clocks,
  locked,
  send,
}: {
  gpuId: string;
  clocks: ClocksStatus;
  locked: boolean;
  send: (command: () => Promise<PendingChange>, detail: string) => void;
}) {
  const [draft, setDraft] = useState<ClockDraft>({});
  const request = clockRequest(clocks, draft);
  const controls = listedControls(clocks);
  const editable = controls.some(({ key, control }) => clockLimits(key, control) !== null);

  return (
    <>
      {editable && (
        <p className="settings-notice" role="note">
          An unstable value can freeze or crash the machine. A value you keep comes back after every reboot, until you
          reset it here.
        </p>
      )}
      {controls.map(({ key, control }) => (
        <ClockRow
          key={key}
          name={key}
          control={control}
          text={draft[key] ?? String(control.current)}
          error={request.errors[key] ?? null}
          locked={locked}
          onChange={(text) => setDraft((d) => ({ ...d, [key]: text }))}
        />
      ))}
      {editable && (
        <div className="tuning-actions">
          <button
            type="button"
            className="button"
            disabled={locked || !canApplyClocks(request)}
            onClick={() =>
              send(() => invoke<PendingChange>("set_clocks", { gpuId, values: request.values }), request.detail)
            }
          >
            Apply
          </button>
        </div>
      )}
    </>
  );
}

function ClockRow({
  name,
  control,
  text,
  error,
  locked,
  onChange,
}: {
  name: ClockKey;
  control: ClockControl;
  text: string;
  error: string | null;
  locked: boolean;
  onChange: (text: string) => void;
}) {
  const limits = clockLimits(name, control);
  const range = firmwareRange(control);
  const label = CLOCK_LABELS[name];
  return (
    <div className="setting">
      <div className="setting-text">
        <span className="setting-title">{label}</span>
        <span className="tuning-figure tuning-number">{clockValue(name, control.current)}</span>
        <span className="setting-description">
          {hasFloorOnly(name, control) ? (
            <>
              The firmware gives no range for this value. Redline refuses anything above{" "}
              <span className="tuning-number">0 mV</span> or below{" "}
              <span className="tuning-number">{clockValue(name, VOLTAGE_FLOOR_MV)}</span>: that only stops a typo, it
              is not a safe value.
            </>
          ) : limits === null || range === null ? (
            "This GPU gives no range for this value, so it cannot be changed here."
          ) : (
            <>
              Range <span className="tuning-number">{clockValue(name, limits.min)}</span> to{" "}
              <span className="tuning-number">{clockValue(name, limits.max)}</span>
            </>
          )}
        </span>
        {error !== null && (
          <span className="setting-error" role="alert">
            {error}
          </span>
        )}
      </div>
      {limits !== null && (
        <div className="setting-control">
          <label className="tuning-watts">
            <input
              type="number"
              className="setting-input tuning-number"
              aria-label={label}
              min={limits.min}
              max={limits.max}
              step={1}
              value={text}
              disabled={locked}
              aria-invalid={error !== null || undefined}
              onChange={(e) => onChange(e.target.value)}
            />
            <span>{clockUnit(name)}</span>
          </label>
        </div>
      )}
    </div>
  );
}

/** Reset to factory values, confirmed in the page; then it is pending like any other change. */
function Reset({ locked, onReset }: { locked: boolean; onReset: () => void }) {
  const [asking, setAsking] = useState(false);

  if (!asking) {
    return (
      <div className="tuning-actions">
        <button type="button" className="button" disabled={locked} onClick={() => setAsking(true)}>
          Reset to factory values…
        </button>
      </div>
    );
  }
  return (
    <div className="tuning-plan" role="group" aria-label="Reset clocks and voltage">
      <p className="setting-note">
        Return every clock and voltage of this GPU to the firmware's values and clear the values LACT stores for it?
        You still confirm the result.
      </p>
      <div className="tuning-actions">
        <button
          type="button"
          className="button button-primary"
          disabled={locked}
          autoFocus
          onClick={() => {
            setAsking(false);
            onReset();
          }}
        >
          Reset
        </button>
        <button type="button" className="button" onClick={() => setAsking(false)}>
          Cancel
        </button>
      </div>
    </div>
  );
}
