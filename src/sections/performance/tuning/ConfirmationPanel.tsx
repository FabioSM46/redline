import { invoke } from "@tauri-apps/api/core";
import { type Dispatch, useState } from "react";
import { useGpu } from "../../../GpuContext.tsx";
import {
  changeLabel,
  formatCountdown,
  type Outcome,
  type PendingAction,
  type PendingChange,
  secondsLeft,
  watts,
} from "../../../tuning.ts";
import { useNow } from "./useTuning.ts";

/**
 * The change lactd applied and will revert by itself at the deadline, unless the user keeps it.
 * In the page, never a browser dialog, and nothing here answers on the user's behalf: with no
 * answer the deadline passes and lactd restores the previous values.
 */
export function ConfirmationPanel({
  change,
  detail,
  dispatch,
}: {
  change: PendingChange;
  /** What the page asked for ("250 W → 200 W"), when this page made the change. */
  detail: string | null;
  dispatch: Dispatch<PendingAction>;
}) {
  const { gpus, gpu, reading } = useGpu();
  const now = useNow(true);
  const [answering, setAnswering] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const left = secondsLeft(change.deadline_ms, now);
  const name = gpus.find((g) => g.id === change.gpu_id)?.name ?? change.gpu_id;
  const otherGpu = change.gpu_id !== gpu.id;
  // The result, read from the sensors: only for the GPU the shell is polling.
  const snapshot = !otherGpu && reading.status === "ok" ? reading.snapshot : null;

  const answer = (command: "tuning_confirm" | "tuning_revert") => {
    setAnswering(true);
    setError(null);
    invoke<Outcome>(command, { gpuId: change.gpu_id }).then(
      (outcome) => dispatch({ type: "answered", change, result: { gpu_id: change.gpu_id, outcome } }),
      (e) => {
        setAnswering(false);
        setError(String(e));
      },
    );
  };

  return (
    <section className="tuning-confirm" aria-labelledby="tuning-confirm-title">
      <h2 className="tuning-confirm-title" id="tuning-confirm-title">
        Keep this change?
      </h2>
      <p className="tuning-confirm-change">
        {changeLabel(change.what)}
        {detail !== null && <span className="tuning-confirm-detail">: {detail}</span>}
        {(otherGpu || gpus.length > 1) && <span className="tuning-confirm-gpu"> on {name}</span>}
      </p>
      {snapshot !== null && change.what === "power_limit" && (
        <p className="tuning-confirm-result">
          Now reads: power cap <span className="tuning-number">{watts(snapshot.power_cap_w)}</span>, drawing{" "}
          <span className="tuning-number">{watts(snapshot.power_w)}</span>
        </p>
      )}
      <p className="tuning-confirm-countdown" aria-live="off">
        {left > 0 ? (
          <>
            Reverts by itself in <span className="tuning-number">{formatCountdown(left)}</span> unless you keep it.
          </>
        ) : (
          "Reverting…"
        )}
      </p>
      {error !== null && (
        <p className="setting-error" role="alert">
          {error}
        </p>
      )}
      <div className="tuning-confirm-actions">
        <button
          type="button"
          className="button button-primary"
          disabled={left === 0}
          aria-disabled={answering || undefined}
          onClick={() => !answering && answer("tuning_confirm")}
        >
          Keep
        </button>
        <button
          type="button"
          className="button"
          disabled={left === 0}
          aria-disabled={answering || undefined}
          onClick={() => !answering && answer("tuning_revert")}
        >
          Revert
        </button>
      </div>
    </section>
  );
}
