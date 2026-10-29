import { useCallback, useEffect, useState } from "react";
import { useGpu } from "../../GpuContext.tsx";
import type { Navigate } from "../../tabs.ts";
import { changeLabel, lactUnreachable, outcomeSentence, type TuningStatus } from "../../tuning.ts";
import type { TuningBlockProps } from "./tuning/block.ts";
import { FanBlock } from "./tuning/Fan.tsx";
import { ConfirmationPanel } from "./tuning/ConfirmationPanel.tsx";
import { OverdriveBlock } from "./tuning/Overdrive.tsx";
import { PowerLimitBlock } from "./tuning/PowerLimit.tsx";
import { usePendingChange, useTuningStatus } from "./tuning/useTuning.ts";

/** What this page asked for, kept next to the change it became, for the confirmation panel. */
type Detail = { gpuId: string; deadlineMs: number; text: string };

/**
 * Performance → Tuning: what LACT can change on the selected GPU, through `lactd`, every change
 * confirmed by the user in the panel at the top before lactd's deadline.
 *
 * Layout, top to bottom: confirmation panel or last outcome, LACT notices, then one block per
 * thing to tune. Blocks get `TuningBlockProps` (`./tuning/block.ts`) and change the GPU only
 * through `apply`; the fan editor (#60) and profiles (#61) are added as blocks in `blocks()`.
 */
export function Tuning({ navigate }: { navigate: Navigate }) {
  const { gpu, gpus } = useGpu();
  const { state, reload } = useTuningStatus(gpu.id);
  const [pending, dispatch, listenError] = usePendingChange(reload);
  const [sending, setSending] = useState(false);
  const [overdriveBusy, setOverdriveBusy] = useState(false);
  const [detail, setDetail] = useState<Detail | null>(null);

  const status = state.status === "ready" ? state.value : null;
  // A change already pending when the page opens (or read before its event) shows the panel too.
  const statusPending = status?.pending ?? null;
  useEffect(() => {
    if (statusPending !== null) dispatch({ type: "status", change: statusPending, nowMs: Date.now() });
  }, [statusPending, dispatch]);

  const locked = pending.pending !== null || sending || overdriveBusy;

  const apply = useCallback<TuningBlockProps["apply"]>(
    (send, text) => {
      setSending(true);
      return send()
        .then(
          (change) => {
            setDetail({ gpuId: change.gpu_id, deadlineMs: change.deadline_ms, text });
            dispatch({ type: "pending", change });
            return null;
          },
          (e) => String(e),
        )
        .finally(() => setSending(false));
    },
    [dispatch],
  );

  const change = pending.pending;
  const panelDetail =
    change !== null && detail?.gpuId === change.gpu_id && detail.deadlineMs === change.deadline_ms ? detail.text : null;
  const settled = pending.settled;
  const settledName =
    settled !== null && gpus.length > 1 ? (gpus.find((g) => g.id === settled.change.gpu_id)?.name ?? null) : null;

  return (
    <div className="tuning">
      {change !== null && (
        <ConfirmationPanel
          key={`${change.gpu_id}:${change.deadline_ms}`}
          change={change}
          detail={panelDetail}
          dispatch={dispatch}
        />
      )}
      {change === null && settled !== null && (
        <p className="settings-notice" role="status">
          {changeLabel(settled.change.what)}
          {settledName !== null && ` on ${settledName}`}: {outcomeSentence(settled.result)}
        </p>
      )}
      {listenError !== null && (
        <p className="setting-error" role="alert">
          Redline cannot follow tuning changes, so a change made now may not show its confirmation: {listenError}
        </p>
      )}
      {state.status === "loading" && <p className="state">Reading what LACT can change…</p>}
      {state.status === "error" && (
        <p className="setting-error" role="alert">
          Cannot read the tuning state: {state.message}
        </p>
      )}
      {status !== null && (
        <TuningBody
          status={status}
          gpuId={gpu.id}
          locked={locked}
          apply={apply}
          navigate={navigate}
          reload={reload}
          onOverdriveBusy={setOverdriveBusy}
        />
      )}
    </div>
  );
}

function TuningBody({
  navigate,
  reload,
  onOverdriveBusy,
  ...block
}: TuningBlockProps & {
  navigate: Navigate;
  reload: () => void;
  onOverdriveBusy: (busy: boolean) => void;
}) {
  const { status } = block;
  return (
    <>
      {status.warning !== null && (
        <p className="settings-notice" role="status">
          {status.warning}
        </p>
      )}
      {!status.available && <Unavailable status={status} navigate={navigate} reload={reload} />}
      {status.available && <Blocks {...block} />}
      {/* Overdrive is system-wide: it needs LACT running, not a GPU LACT manages. */}
      {status.lact_version !== null && (
        <OverdriveBlock status={status} locked={block.locked} onBusy={onOverdriveBusy} onChanged={reload} />
      )}
    </>
  );
}

/**
 * The blocks of a GPU LACT manages, in page order. Extension point: the fan editor (#60) and the
 * profiles block (#61) go here, each taking `TuningBlockProps`.
 */
function Blocks(block: TuningBlockProps) {
  const { power, fan } = block.status;
  return (
    <>
      {power !== null && <PowerLimitBlock key={`${block.gpuId}:${power.current_w}`} {...block} power={power} />}
      {fan !== null && <FanBlock key={`${block.gpuId}:${JSON.stringify(fan)}`} {...block} fan={fan} />}
    </>
  );
}

/** Tuning is off for this GPU: why, what to do, and where the tools are installed. */
function Unavailable({
  status,
  navigate,
  reload,
}: {
  status: TuningStatus;
  navigate: Navigate;
  reload: () => void;
}) {
  const unreachable = lactUnreachable(status);
  return (
    <div className="settings-notice tuning-unavailable" role="status">
      <p>{status.unavailable_reason ?? "Tuning is not available for this GPU."}</p>
      {unreachable && <p>Settings → Dependencies shows what is installed and can install what is missing.</p>}
      <div className="tuning-actions">
        {unreachable && (
          <button type="button" className="button" onClick={() => navigate("settings", "dependencies")}>
            Open Dependencies
          </button>
        )}
        <button type="button" className="button" onClick={reload}>
          Check again
        </button>
      </div>
    </div>
  );
}
