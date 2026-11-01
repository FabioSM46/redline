import { useGpu } from "../GpuContext.tsx";
import { statusBarItems } from "../gpu.ts";
import { replayIndicator, replayIsOn, unavailableNotice } from "../recorder.ts";
import { useRecorder } from "../RecorderContext.tsx";
import { useUpdates } from "../UpdateContext.tsx";
import { indicatorLabel, indicatorVersion } from "../updates.ts";

/**
 * Always visible: GPU name, temperature and power of the selected GPU, an update if there is one,
 * then the replay (D8).
 */
export function StatusBar() {
  const { gpu, reading } = useGpu();
  const [name, ...values] = statusBarItems(gpu.name, reading);
  return (
    <footer className="statusbar">
      <span className="statusbar-name">{name}</span>
      {values.map((value, i) => (
        <span className="statusbar-value" key={i}>
          {value}
        </span>
      ))}
      {reading.status === "error" && (
        <span className="statusbar-error" title={reading.message}>
          Sensors unreadable
        </span>
      )}
      <UpdateIndicator />
      <RecorderIndicators />
    </footer>
  );
}

/**
 * A newer version: one click opens its release page, the other sets it aside. Information, not an
 * alarm, so it stays muted, and a failed check says nothing here: that is for Settings → General.
 */
function UpdateIndicator() {
  const { update, busy, actionError, openRelease, dismiss } = useUpdates();
  const version = indicatorVersion(update);
  if (version === null) return null;
  return (
    <span className="statusbar-update">
      <button
        type="button"
        className="statusbar-update-open"
        aria-disabled={busy}
        title="Open the release page"
        onClick={openRelease}
      >
        {indicatorLabel(version)}
      </button>
      <button
        type="button"
        className="statusbar-update-dismiss"
        aria-disabled={busy}
        aria-label={`Dismiss update ${version}`}
        title="Dismiss this version"
        onClick={dismiss}
      >
        ×
      </button>
      {actionError !== null && (
        <span className="statusbar-error" title={actionError}>
          Update action failed
        </span>
      )}
    </span>
  );
}

/** The replay indicator, which is also its switch (D8), and the recording indicator. */
function RecorderIndicators() {
  const { recorder, busy, actionError, toggleReplay } = useRecorder();
  if (recorder.status === "loading") return null;
  if (recorder.status === "error") {
    return (
      <span className="statusbar-recorder">
        <span className="statusbar-error" title={recorder.message}>
          Recorder unreadable
        </span>
      </span>
    );
  }

  const status = recorder.value;
  const notice = unavailableNotice(status);
  const on = replayIsOn(status);
  const error = actionError ?? (status.replay === "error" ? status.error : null);
  return (
    <span className="statusbar-recorder">
      {status.recording && <span className="statusbar-rec">⏺ REC</span>}
      {error !== null && (
        <span className="statusbar-error" title={error}>
          Replay failed
        </span>
      )}
      <button
        type="button"
        role="switch"
        className="statusbar-replay"
        data-state={status.replay}
        aria-checked={on}
        aria-disabled={busy || notice !== null}
        title={notice ?? (on ? "Turn the instant replay off" : "Turn the instant replay on")}
        onClick={() => {
          if (!busy && notice === null) toggleReplay();
        }}
      >
        {replayIndicator(status)}
      </button>
    </span>
  );
}
