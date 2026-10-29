import { useGpu } from "../GpuContext.tsx";
import { type Figure, homeFigures } from "../home.ts";
import {
  formatClipTime,
  formatDuration,
  latestClips,
  type Recording,
  replayIsOn,
  unavailableNotice,
} from "../recorder.ts";
import { useRecorder, useThumbnail } from "../RecorderContext.tsx";
import type { SectionProps } from "../tabs.ts";

/** The GPU state at a glance: five large figures, then the secondary ones (D8), then the replay. */
export function Home({ navigate }: SectionProps) {
  return (
    <section className="section">
      <GpuFigures />
      <Replay />
      <LatestClips onShowAll={() => navigate("record")} />
    </section>
  );
}

function GpuFigures() {
  const { reading } = useGpu();

  if (reading.status === "error") {
    return <p className="state">Cannot read the GPU: {reading.message}</p>;
  }
  if (reading.status === "loading") {
    return <p className="state">Reading sensors…</p>;
  }

  const { primary, secondary } = homeFigures(reading.snapshot);
  return (
    <>
      <Figures figures={primary} className="figures figures-primary" label="Main figures" />
      <Figures figures={secondary} className="figures figures-secondary" label="Secondary figures" />
    </>
  );
}

function Figures({ figures, className, label }: { figures: Figure[]; className: string; label: string }) {
  return (
    <dl className={className} aria-label={label}>
      {figures.map((f) => (
        <div className="figure" key={f.label}>
          <dt>{f.label}</dt>
          <dd className={f.high ? "figure-value figure-high" : "figure-value"}>
            {f.value}
            {f.high && <span className="visually-hidden"> (high)</span>}
          </dd>
          {f.detail !== undefined && <dd className="figure-detail">{f.detail}</dd>}
        </div>
      ))}
    </dl>
  );
}

/** The same replay switch as the status bar, with room for what to install and why it failed. */
function Replay() {
  const { recorder, busy, actionError, toggleReplay } = useRecorder();

  if (recorder.status === "loading") {
    return <p className="home-panel state">Reading the recorder…</p>;
  }
  if (recorder.status === "error") {
    return <p className="home-panel setting-error">Cannot read the recorder: {recorder.message}</p>;
  }

  const status = recorder.value;
  const notice = unavailableNotice(status);
  const on = replayIsOn(status);
  const error = actionError ?? (status.replay === "error" ? status.error : null);
  return (
    <div className="home-panel setting">
      <div className="setting-text">
        <span>Instant replay</span>
        <span className="setting-description">
          {status.replay === "starting"
            ? "Starting…"
            : on
              ? `Keeping the last ${status.replay_seconds} s.`
              : `Off. When on, it keeps the last ${status.replay_seconds} s.`}
          {status.recording && <span className="replay-rec"> ⏺ Recording</span>}
        </span>
        {notice !== null && <span className="setting-note">{notice}</span>}
        {error !== null && (
          <span className="setting-error" role="alert">
            {error}
          </span>
        )}
        {status.warning !== null && <span className="setting-description">{status.warning}</span>}
      </div>
      <div className="setting-control">
        <input
          type="checkbox"
          role="switch"
          className="switch"
          aria-label="Instant replay"
          checked={on}
          disabled={notice !== null}
          aria-disabled={busy || notice !== null}
          onChange={() => {
            if (!busy) toggleReplay();
          }}
        />
      </div>
    </div>
  );
}

function LatestClips({ onShowAll }: { onShowAll: () => void }) {
  const { clips } = useRecorder();
  return (
    <div className="home-panel">
      <div className="clips-header">
        <h2 className="clips-title">Latest clips</h2>
        <button type="button" className="link-button" onClick={onShowAll}>
          Show all
        </button>
      </div>
      <ClipList clips={clips} />
    </div>
  );
}

function ClipList({ clips }: { clips: ReturnType<typeof useRecorder>["clips"] }) {
  if (clips.status === "loading") return <p className="state">Reading clips…</p>;
  if (clips.status === "error") return <p className="setting-error">Cannot list the clips: {clips.message}</p>;
  const latest = latestClips(clips.value);
  if (latest.length === 0) {
    return <p className="state">No clips yet. Saved replays and recordings appear here.</p>;
  }
  const now = Date.now();
  return (
    <ul className="clips">
      {latest.map((clip) => (
        <Clip key={clip.id} clip={clip} now={now} />
      ))}
    </ul>
  );
}

function Clip({ clip, now }: { clip: Recording; now: number }) {
  const thumbnail = useThumbnail(clip.id);
  const duration = formatDuration(clip.duration_s);
  return (
    <li className="clip">
      {thumbnail ? (
        <img className="clip-thumb" src={thumbnail} alt="" />
      ) : (
        <div className="clip-thumb clip-thumb-empty" aria-hidden="true" />
      )}
      <span className="clip-name" title={clip.file_name}>
        {clip.file_name}
      </span>
      <span className="clip-meta">
        {formatClipTime(clip.saved_at_ms, now)}
        {duration !== null && ` · ${duration}`}
      </span>
    </li>
  );
}
