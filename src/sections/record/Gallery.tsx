import { invoke } from "@tauri-apps/api/core";
import { type RefObject, useEffect, useRef, useState } from "react";
import { formatSize, kindLabel } from "../../record.ts";
import { formatClipTime, formatDuration, type Recording } from "../../recorder.ts";
import { useRecorder, useThumbnail } from "../../RecorderContext.tsx";

/** `thumbnail_support` (src-tauri/src/thumbnails.rs). */
type ThumbnailSupport = { generator: string | null; hint: string | null };

/**
 * Every saved replay and recording, newest first, from `list_recordings` (re-read on
 * `file-saved` by the recorder context). Works without GPU Screen Recorder: files already saved
 * are still listed and can be opened.
 */
export function Gallery() {
  const { clips, refreshClips } = useRecorder();
  const [hint, setHint] = useState<string | null>(null);

  // The tab may have been closed while files were added or deleted outside the app.
  useEffect(() => {
    refreshClips();
    let active = true;
    invoke<ThumbnailSupport>("thumbnail_support").then(
      (support) => active && setHint(support.generator === null ? support.hint : null),
      () => {},
    );
    return () => {
      active = false;
    };
  }, [refreshClips]);

  const count = clips.status === "ready" ? clips.value.length : null;
  return (
    <div className="record-block">
      <div className="clips-header">
        <h2 className="record-title">
          Clips
          {count !== null && count > 0 && <span className="record-count"> {count}</span>}
        </h2>
        <button type="button" className="link-button" onClick={refreshClips}>
          Refresh
        </button>
      </div>
      {hint !== null && <p className="setting-description gallery-hint">{hint}</p>}
      <ClipGrid clips={clips} />
    </div>
  );
}

function ClipGrid({ clips }: { clips: ReturnType<typeof useRecorder>["clips"] }) {
  if (clips.status === "loading") return <p className="state">Reading clips…</p>;
  if (clips.status === "error") {
    return (
      <p className="setting-error" role="alert">
        Cannot list the clips: {clips.message}
      </p>
    );
  }
  if (clips.value.length === 0) {
    return <p className="state">No clips yet. Saved replays and recordings appear here.</p>;
  }
  const now = Date.now();
  return (
    <ul className="clips">
      {clips.value.map((clip) => (
        <Clip key={clip.id} clip={clip} now={now} />
      ))}
    </ul>
  );
}

type Action = "open_recording" | "show_in_folder";

function Clip({ clip, now }: { clip: Recording; now: number }) {
  const ref = useRef<HTMLLIElement>(null);
  const visible = useSeen(ref);
  const [pending, setPending] = useState<Action | null>(null);
  const [error, setError] = useState<string | null>(null);
  const duration = formatDuration(clip.duration_s);

  function run(action: Action) {
    if (pending !== null) return;
    setPending(action);
    setError(null);
    invoke(action, { id: clip.id })
      .catch((e) => setError(String(e)))
      .finally(() => setPending(null));
  }

  return (
    <li className="clip" ref={ref}>
      {visible ? <Thumbnail id={clip.id} /> : <div className="clip-thumb clip-thumb-empty" aria-hidden="true" />}
      <span className="clip-name" title={clip.file_name}>
        {clip.file_name}
      </span>
      <span className="clip-meta">
        {kindLabel(clip.kind)} · {formatClipTime(clip.saved_at_ms, now)}
      </span>
      <span className="clip-meta">
        {formatSize(clip.size_bytes)}
        {duration !== null && ` · ${duration}`}
      </span>
      <span className="clip-actions">
        <button
          type="button"
          className="link-button"
          aria-label={`Open ${clip.file_name}`}
          aria-disabled={pending !== null}
          onClick={() => run("open_recording")}
        >
          Open
        </button>
        <button
          type="button"
          className="link-button"
          aria-label={`Show ${clip.file_name} in its folder`}
          aria-disabled={pending !== null}
          onClick={() => run("show_in_folder")}
        >
          Show in folder
        </button>
      </span>
      {error !== null && (
        <span className="setting-error" role="alert">
          {error}
        </span>
      )}
    </li>
  );
}

function Thumbnail({ id }: { id: string }) {
  const thumbnail = useThumbnail(id);
  return thumbnail ? (
    <img className="clip-thumb" src={thumbnail} alt="" />
  ) : (
    <div className="clip-thumb clip-thumb-empty" aria-hidden="true" />
  );
}

/**
 * True once the element has come near the visible area, and from then on: previews are asked
 * only for the clips the user scrolls to, not for the whole folder at once.
 */
function useSeen(ref: RefObject<HTMLElement | null>): boolean {
  const [seen, setSeen] = useState(false);
  useEffect(() => {
    const element = ref.current;
    if (seen || element === null) return;
    if (typeof IntersectionObserver === "undefined") {
      setSeen(true);
      return;
    }
    const observer = new IntersectionObserver(
      (entries) => {
        if (entries.some((entry) => entry.isIntersecting)) setSeen(true);
      },
      { rootMargin: "200px" },
    );
    observer.observe(element);
    return () => observer.disconnect();
  }, [ref, seen]);
  return seen;
}
