// The recorder contracts (src-tauri/src/recorder.rs, gallery.rs, thumbnails.rs) and the pure
// helpers the status bar, Home and the Record tab share. Tested in recorder.test.ts.

export type ReplayState = "off" | "starting" | "on" | "error";

/** `recorder_status`, and the payload of the `recorder-state` event. */
export type RecorderStatus = {
  /** False until the first detection has run, and when GPU Screen Recorder is missing. */
  available: boolean;
  /** When not available: what to install. */
  unavailable_reason: string | null;
  replay: ReplayState;
  recording: boolean;
  replay_seconds: number;
  /** Why the replay stopped, or why the last action failed. */
  error: string | null;
  /** A notice that blocks nothing, such as an unverified GPU Screen Recorder version. */
  warning: string | null;
};

/** One entry of `list_recordings`. `id` is opaque: it is only passed back to the backend. */
export type Recording = {
  id: string;
  kind: "replay" | "recording";
  file_name: string;
  saved_at_ms: number;
  size_bytes: number;
  duration_s: number | null;
};

/** How many clips Home shows. */
export const LATEST_CLIPS = 5;

/** The replay is running or about to: the switch shows on, and turning it off stops it. */
export function replayIsOn(status: RecorderStatus): boolean {
  return status.replay === "on" || status.replay === "starting";
}

/** The status bar's replay indicator: "⏺ Replay 60 s · ON". */
export function replayIndicator(status: RecorderStatus): string {
  return `⏺ Replay ${status.replay_seconds} s · ${replayWord(status.replay)}`;
}

function replayWord(replay: ReplayState): string {
  switch (replay) {
    case "on":
      return "ON";
    case "starting":
      return "STARTING";
    case "off":
    case "error":
      return "OFF";
  }
}

/**
 * The sentence shown instead of the switch when GPU Screen Recorder is not usable, or null when
 * it is. Before the first detection there is no reason yet: that is a wait, not a missing tool.
 */
export function unavailableNotice(status: RecorderStatus): string | null {
  if (status.available) return null;
  return status.unavailable_reason ?? "Looking for GPU Screen Recorder…";
}

/** The newest `limit` clips, newest first (ties by file name, as `list_recordings` sorts). */
export function latestClips(clips: readonly Recording[], limit = LATEST_CLIPS): Recording[] {
  return [...clips]
    .sort((a, b) => b.saved_at_ms - a.saved_at_ms || a.file_name.localeCompare(b.file_name))
    .slice(0, Math.max(0, limit));
}

/** When a clip was saved: the time for today's clips, the date and time otherwise (local). */
export function formatClipTime(savedAtMs: number, nowMs: number): string {
  const saved = new Date(savedAtMs);
  const now = new Date(nowMs);
  const time = `${pad(saved.getHours())}:${pad(saved.getMinutes())}`;
  const sameDay =
    saved.getFullYear() === now.getFullYear() &&
    saved.getMonth() === now.getMonth() &&
    saved.getDate() === now.getDate();
  if (sameDay) return `Today ${time}`;
  return `${saved.getFullYear()}-${pad(saved.getMonth() + 1)}-${pad(saved.getDate())} ${time}`;
}

/** A clip's length as m:ss (h:mm:ss past an hour), or null when the backend does not know it. */
export function formatDuration(seconds: number | null): string | null {
  if (seconds === null || !Number.isFinite(seconds) || seconds < 0) return null;
  const total = Math.round(seconds);
  const h = Math.floor(total / 3600);
  const m = Math.floor((total % 3600) / 60);
  const s = total % 60;
  return h > 0 ? `${h}:${pad(m)}:${pad(s)}` : `${m}:${pad(s)}`;
}

function pad(n: number): string {
  return String(n).padStart(2, "0");
}

/** After this long a "no preview" answer is asked again: null can mean "the queue was full". */
export const THUMBNAIL_RETRY_MS = 60_000;

type ThumbnailEntry =
  | { status: "pending"; promise: Promise<string | null> }
  | { status: "done"; url: string | null; at: number };

export type ThumbnailCache = {
  /** The cached preview: a data URL, null for "no preview", undefined when not known yet. */
  peek(id: string): string | null | undefined;
  /** The preview of `id`, fetched once; parallel callers share one request. */
  get(id: string): Promise<string | null>;
  /** Forgets every clip not in `ids` (deleted clips, or clips that left the list). */
  retain(ids: readonly string[]): void;
};

/**
 * An in-memory cache of `recording_thumbnail` answers, by clip id. A data URL is kept for the
 * session; a null (or a failed request) is kept for THUMBNAIL_RETRY_MS, then asked again.
 */
export function createThumbnailCache(
  fetch: (id: string) => Promise<string | null>,
  now: () => number = Date.now,
): ThumbnailCache {
  const entries = new Map<string, ThumbnailEntry>();

  function fresh(entry: ThumbnailEntry | undefined): entry is ThumbnailEntry {
    if (entry === undefined) return false;
    if (entry.status === "pending" || entry.url !== null) return true;
    return now() - entry.at < THUMBNAIL_RETRY_MS;
  }

  return {
    peek(id) {
      const entry = entries.get(id);
      return entry?.status === "done" ? entry.url : undefined;
    },
    get(id) {
      const entry = entries.get(id);
      if (fresh(entry)) {
        return entry.status === "pending" ? entry.promise : Promise.resolve(entry.url);
      }
      const pending: ThumbnailEntry = {
        status: "pending",
        promise: fetch(id)
          .catch(() => null)
          .then((url) => {
            // Dropped by `retain` (or replaced) meanwhile: the answer is no longer this entry's.
            if (entries.get(id) === pending) entries.set(id, { status: "done", url, at: now() });
            return url;
          }),
      };
      entries.set(id, pending);
      return pending.promise;
    },
    retain(ids) {
      const keep = new Set(ids);
      for (const id of [...entries.keys()]) {
        if (!keep.has(id)) entries.delete(id);
      }
    },
  };
}
